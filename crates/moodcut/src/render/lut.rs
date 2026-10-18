//! 3D LUTs: .cube parsing, generated LUTs for complex color transforms, and
//! fast parallel tetrahedral application on premultiplied pixels.

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use rayon::prelude::*;
use resvg::tiny_skia::Pixmap;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

pub struct Lut3 {
    pub n: usize,
    /// r fastest, then g, then b
    pub data: Vec<[f32; 3]>,
    /// fixed-point copy (0..=65280 = 255*256) for the fast path
    fixed: OnceLock<Vec<[i32; 3]>>,
}

impl Lut3 {
    pub fn parse_cube(text: &str) -> Result<Lut3> {
        let mut n = 0usize;
        let mut dmin = [0f32; 3];
        let mut dmax = [1f32; 3];
        let mut data = Vec::new();
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let up = l.to_ascii_uppercase();
            if up.starts_with("TITLE") {
                continue;
            }
            if let Some(r) = up.strip_prefix("LUT_3D_SIZE") {
                n = r.trim().parse().map_err(|_| anyhow!("bad LUT_3D_SIZE"))?;
                continue;
            }
            if up.starts_with("LUT_1D_SIZE") {
                bail!("1D .cube LUTs are not supported (use a 3D LUT)");
            }
            if let Some(r) = up.strip_prefix("DOMAIN_MIN") {
                let v: Vec<f32> = r.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if v.len() == 3 {
                    dmin = [v[0], v[1], v[2]];
                }
                continue;
            }
            if let Some(r) = up.strip_prefix("DOMAIN_MAX") {
                let v: Vec<f32> = r.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                if v.len() == 3 {
                    dmax = [v[0], v[1], v[2]];
                }
                continue;
            }
            let v: Vec<f32> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() == 3 {
                let norm = |i: usize| (v[i] - dmin[i]) / (dmax[i] - dmin[i]).max(1e-6);
                data.push([norm(0), norm(1), norm(2)]);
            }
        }
        if n < 2 || data.len() != n * n * n {
            bail!("invalid .cube: expected {}^3 entries, got {}", n, data.len());
        }
        Ok(Lut3 { n, data, fixed: OnceLock::new() })
    }

    pub fn generate(n: usize, f: impl Fn(f32, f32, f32) -> (f32, f32, f32) + Sync) -> Lut3 {
        let data: Vec<[f32; 3]> = (0..n * n * n)
            .into_par_iter()
            .map(|i| {
                let r = (i % n) as f32 / (n - 1) as f32;
                let g = ((i / n) % n) as f32 / (n - 1) as f32;
                let b = (i / (n * n)) as f32 / (n - 1) as f32;
                let (a, bb, c) = f(r, g, b);
                [a, bb, c]
            })
            .collect();
        Lut3 { n, data, fixed: OnceLock::new() }
    }

    #[inline]
    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[r + g * self.n + b * self.n * self.n]
    }

    /// Tetrahedral interpolation, inputs 0..1.
    #[inline]
    pub fn lookup(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        let m = (self.n - 1) as f32;
        let (fr, fg, fb) = (r.clamp(0.0, 1.0) * m, g.clamp(0.0, 1.0) * m, b.clamp(0.0, 1.0) * m);
        let (r0, g0, b0) = (fr.floor() as usize, fg.floor() as usize, fb.floor() as usize);
        let (r1, g1, b1) = ((r0 + 1).min(self.n - 1), (g0 + 1).min(self.n - 1), (b0 + 1).min(self.n - 1));
        let (dr, dg, db) = (fr - r0 as f32, fg - g0 as f32, fb - b0 as f32);
        let c000 = self.at(r0, g0, b0);
        let c111 = self.at(r1, g1, b1);
        let mut o = [0f32; 3];
        let mix = |o: &mut [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3], wa: f32, wb: f32, wc: f32| {
            for k in 0..3 {
                o[k] = c000[k] + wa * (a[k] - c000[k]) + wb * (b[k] - a[k]) + wc * (c111[k] - b[k]);
                let _ = c;
            }
        };
        if dr > dg {
            if dg > db {
                mix(&mut o, self.at(r1, g0, b0), self.at(r1, g1, b0), c111, dr, dg, db);
            } else if dr > db {
                mix(&mut o, self.at(r1, g0, b0), self.at(r1, g0, b1), c111, dr, db, dg);
            } else {
                mix(&mut o, self.at(r0, g0, b1), self.at(r1, g0, b1), c111, db, dr, dg);
            }
        } else if db > dg {
            mix(&mut o, self.at(r0, g0, b1), self.at(r0, g1, b1), c111, db, dg, dr);
        } else if db > dr {
            mix(&mut o, self.at(r0, g1, b0), self.at(r0, g1, b1), c111, dg, db, dr);
        } else {
            mix(&mut o, self.at(r0, g1, b0), self.at(r1, g1, b0), c111, dg, dr, db);
        }
        o
    }

    /// Applies the LUT with `amount` blend to a premultiplied pixmap (fixed-point tetrahedral).
    pub fn apply(&self, pm: &mut Pixmap, amount: f32) {
        let n = self.n;
        let fx = self.fixed.get_or_init(|| self.data.iter().map(|c| [(c[0].clamp(0.0, 1.0) * 65280.0) as i32, (c[1].clamp(0.0, 1.0) * 65280.0) as i32, (c[2].clamp(0.0, 1.0) * 65280.0) as i32]).collect());
        // per-input-value cell index and fraction (0..256)
        let mut idx = [0usize; 256];
        let mut frac = [0i32; 256];
        for v in 0..256 {
            let f = v as f32 / 255.0 * (n - 1) as f32;
            let i = (f.floor() as usize).min(n - 2);
            idx[v] = i;
            frac[v] = ((f - i as f32) * 256.0).round() as i32;
        }
        let amt = (amount.clamp(0.0, 1.0) * 256.0) as i32;
        let (n1, n2) = (n, n * n);
        let w = pm.width() as usize;
        pm.data_mut().par_chunks_mut(w * 4).for_each(|row| {
            for px in row.chunks_exact_mut(4) {
                let a = px[3] as i32;
                if a == 0 {
                    continue;
                }
                let (r, g, b) = if a == 255 {
                    (px[0] as usize, px[1] as usize, px[2] as usize)
                } else {
                    let un = |v: u8| ((v as i32 * 255 + a / 2) / a).min(255) as usize;
                    (un(px[0]), un(px[1]), un(px[2]))
                };
                let (ri, gi, bi) = (idx[r], idx[g], idx[b]);
                let (dr, dg, db) = (frac[r], frac[g], frac[b]);
                let base = ri + gi * n1 + bi * n2;
                let c000 = fx[base];
                let c111 = fx[base + 1 + n1 + n2];
                let (v1, v2, w1, w2, w3) = if dr > dg {
                    if dg > db {
                        (fx[base + 1], fx[base + 1 + n1], dr, dg, db)
                    } else if dr > db {
                        (fx[base + 1], fx[base + 1 + n2], dr, db, dg)
                    } else {
                        (fx[base + n2], fx[base + 1 + n2], db, dr, dg)
                    }
                } else if db > dg {
                    (fx[base + n2], fx[base + n1 + n2], db, dg, dr)
                } else if db > dr {
                    (fx[base + n1], fx[base + n1 + n2], dg, db, dr)
                } else {
                    (fx[base + n1], fx[base + 1 + n1], dg, dr, db)
                };
                for k in 0..3 {
                    // value in 0..65280 (=255*256)
                    let o = c000[k] + ((w1 * (v1[k] - c000[k]) + w2 * (v2[k] - v1[k]) + w3 * (c111[k] - v2[k])) >> 8);
                    let src = [r, g, b][k] as i32 * 256;
                    let mixed = src + (((o - src) * amt) >> 8);
                    let v = (mixed.clamp(0, 65280) + 128) >> 8;
                    px[k] = if a == 255 { v as u8 } else { ((v * a + 127) / 255) as u8 };
                }
            }
        });
    }

    /// Reference float implementation (kept for tests).
    pub fn apply_f32(&self, pm: &mut Pixmap, amount: f32) {
        let w = pm.width() as usize;
        let amount = amount.clamp(0.0, 1.0);
        pm.data_mut().par_chunks_mut(w * 4).for_each(|row| {
            for px in row.chunks_exact_mut(4) {
                let a = px[3];
                if a == 0 {
                    continue;
                }
                let af = a as f32 / 255.0;
                let inv = 1.0 / (af * 255.0);
                let (r, g, b) = (px[0] as f32 * inv, px[1] as f32 * inv, px[2] as f32 * inv);
                let o = self.lookup(r, g, b);
                let mixc = |src: f32, dst: f32| src + (dst - src) * amount;
                px[0] = (mixc(r, o[0]).clamp(0.0, 1.0) * af * 255.0 + 0.5) as u8;
                px[1] = (mixc(g, o[1]).clamp(0.0, 1.0) * af * 255.0 + 0.5) as u8;
                px[2] = (mixc(b, o[2]).clamp(0.0, 1.0) * af * 255.0 + 0.5) as u8;
            }
        });
    }
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Lut3>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Lut3>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn load_cube(path: &str) -> Result<Arc<Lut3>> {
    let key = format!("file:{path}");
    if let Some(l) = cache().lock().get(&key) {
        return Ok(l.clone());
    }
    let text = std::fs::read_to_string(path).map_err(|e| anyhow!("can't read LUT {path}: {e}"))?;
    let l = Arc::new(Lut3::parse_cube(&text)?);
    cache().lock().insert(key, l.clone());
    Ok(l)
}

/// Cached generated LUT for a parameter key.
pub fn generated(key: &str, f: impl Fn(f32, f32, f32) -> (f32, f32, f32) + Sync) -> Arc<Lut3> {
    if let Some(l) = cache().lock().get(key) {
        return l.clone();
    }
    let l = Arc::new(Lut3::generate(33, f));
    let mut c = cache().lock();
    if c.len() > 512 {
        c.retain(|k, _| k.starts_with("file:"));
    }
    c.insert(key.to_string(), l.clone());
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_matches_float() {
        let l = Lut3::generate(33, |r, g, b| ((r * 1.2 - 0.05).clamp(0.0, 1.0), (g * g).clamp(0.0, 1.0), (0.3 * r + 0.7 * b).clamp(0.0, 1.0)));
        let mut data = vec![];
        let mut seed = 12345u32;
        for _ in 0..4096 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let a = if seed % 3 == 0 { (seed >> 24) as u8 } else { 255 };
            let c = |s: u32| ((s & 0xff) as u32 * a as u32 / 255) as u8;
            data.extend_from_slice(&[c(seed >> 8), c(seed >> 13), c(seed >> 18), a]);
        }
        let mut p1 = resvg::tiny_skia::Pixmap::from_vec(data.clone(), resvg::tiny_skia::IntSize::from_wh(64, 64).unwrap()).unwrap();
        let mut p2 = p1.clone();
        l.apply(&mut p1, 1.0);
        l.apply_f32(&mut p2, 1.0);
        let maxd = p1.data().iter().zip(p2.data()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(maxd <= 2, "max diff {maxd}");
    }

    #[test]
    fn identity_lut() {
        let l = Lut3::generate(17, |r, g, b| (r, g, b));
        let o = l.lookup(0.3, 0.6, 0.9);
        assert!((o[0] - 0.3).abs() < 1e-4 && (o[1] - 0.6).abs() < 1e-4 && (o[2] - 0.9).abs() < 1e-4);
        let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let c = Lut3::parse_cube(cube).unwrap();
        let o = c.lookup(0.25, 0.5, 0.75);
        assert!((o[0] - 0.25).abs() < 1e-4 && (o[2] - 0.75).abs() < 1e-4);
    }
}
