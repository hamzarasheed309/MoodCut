//! Lumetri-style scopes for a rendered frame: luma waveform, RGB parade,
//! vectorscope (with the skin-tone line) and RGB histogram, plus a numeric
//! summary (levels, clipping, saturation, color cast) that an AI can read
//! instead of judging exposure by eye.

use anyhow::{bail, Result};
use rayon::prelude::*;
use resvg::tiny_skia::{Color as SkColor, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub const KINDS: &[&str] = &["waveform", "parade", "vectorscope", "histogram"];

/// Straight 0..1 RGB of every visible pixel (premultiplied data, alpha-weighted onto black).
fn pixels(frame: &Pixmap, step: usize) -> Vec<[f32; 3]> {
    frame.data().chunks_exact(4).step_by(step.max(1)).map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect()
}

#[inline]
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// BT.709 chroma (Cb, Cr), each about -0.5..0.5.
#[inline]
fn chroma(c: [f32; 3]) -> (f32, f32) {
    let y = luma(c);
    ((c[2] - y) / 1.8556, (c[0] - y) / 1.5748)
}

fn stroke(pm: &mut Pixmap, pts: &[(f32, f32)], color: [u8; 4], width: f32, close: bool) {
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(*x, *y);
        } else {
            pb.line_to(*x, *y);
        }
    }
    if close {
        pb.close();
    }
    let Some(path) = pb.finish() else { return };
    let mut paint = Paint { anti_alias: true, ..Default::default() };
    paint.set_color(SkColor::from_rgba8(color[0], color[1], color[2], color[3]));
    pm.stroke_path(&path, &paint, &Stroke { width, ..Default::default() }, Transform::identity(), None);
}

fn background(w: u32, h: u32) -> Pixmap {
    let mut pm = Pixmap::new(w.max(16), h.max(16)).unwrap();
    pm.fill(SkColor::from_rgba8(12, 12, 16, 255));
    pm
}

/// Accumulated hit counts -> brightness (log-ish so faint traces stay visible).
fn glow(count: u32, peak: u32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    ((count as f32).ln_1p() / (peak.max(1) as f32).ln_1p()).powf(0.6).clamp(0.12, 1.0)
}

/// Waveform of one channel set into a column band of `pm` (x0..x0+bw).
fn waveform_band(pm: &mut Pixmap, px: &[[f32; 3]], fw: usize, x0: u32, bw: u32, chan: Option<usize>, tint: [f32; 3]) {
    let h = pm.height();
    let mut grid = vec![0u32; (bw * h) as usize];
    for (i, c) in px.iter().enumerate() {
        let v = match chan {
            Some(k) => c[k],
            None => luma(*c),
        };
        let sx = ((i % fw) as u64 * bw as u64 / fw as u64) as u32;
        let sy = ((1.0 - v.clamp(0.0, 1.0)) * (h - 1) as f32).round() as u32;
        grid[(sy * bw + sx.min(bw - 1)) as usize] += 1;
    }
    let peak = *grid.iter().max().unwrap_or(&1);
    let w = pm.width();
    let d = pm.data_mut();
    for y in 0..h {
        for x in 0..bw {
            let g = glow(grid[(y * bw + x) as usize], peak);
            if g <= 0.0 {
                continue;
            }
            let i = ((y * w + x0 + x) * 4) as usize;
            for k in 0..3 {
                d[i + k] = (d[i + k] as f32 + tint[k] * g * 255.0).min(255.0) as u8;
            }
        }
    }
}

fn graticule_levels(pm: &mut Pixmap, x0: f32, x1: f32) {
    let h = pm.height() as f32;
    for lv in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
        let y = (1.0 - lv) * (h - 1.0) + 0.5;
        stroke(pm, &[(x0, y), (x1, y)], [255, 255, 255, if lv == 0.0 || lv == 1.0 { 70 } else { 34 }], 1.0, false);
    }
    // broadcast-legal limits (16 / 235)
    for lv in [16.0f32 / 255.0, 235.0 / 255.0] {
        let y = (1.0 - lv) * (h - 1.0) + 0.5;
        stroke(pm, &[(x0, y), (x1, y)], [255, 190, 60, 60], 1.0, false);
    }
}

/// Renders one scope of `frame` at w x h.
pub fn render(frame: &Pixmap, kind: &str, w: u32, h: u32) -> Result<Pixmap> {
    let fw = frame.width() as usize;
    let px = pixels(frame, 1);
    let mut pm = background(w, h);
    match kind {
        "waveform" | "luma" => {
            waveform_band(&mut pm, &px, fw, 0, w, None, [0.55, 1.0, 0.6]);
            graticule_levels(&mut pm, 0.0, w as f32);
        }
        "parade" | "rgb" => {
            let bw = w / 3;
            for (k, tint) in [[1.0, 0.25, 0.25], [0.3, 1.0, 0.3], [0.35, 0.5, 1.0]].iter().enumerate() {
                waveform_band(&mut pm, &px, fw, bw * k as u32, bw, Some(k), *tint);
            }
            graticule_levels(&mut pm, 0.0, w as f32);
            for k in 1..3 {
                let x = (bw * k) as f32 + 0.5;
                stroke(&mut pm, &[(x, 0.0), (x, h as f32)], [255, 255, 255, 50], 1.0, false);
            }
        }
        "vectorscope" | "vector" => {
            let s = w.min(h) as f32;
            let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
            let r = s * 0.46;
            // hits, colored by the average color landing there
            let mut acc = vec![[0f32; 4]; (w * h) as usize];
            for c in &px {
                let (cb, cr) = chroma(*c);
                let x = (cx + cb * 2.0 * r).round() as i32;
                let y = (cy - cr * 2.0 * r).round() as i32;
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let a = &mut acc[(y as u32 * w + x as u32) as usize];
                a[0] += c[0];
                a[1] += c[1];
                a[2] += c[2];
                a[3] += 1.0;
            }
            let peak = acc.iter().map(|a| a[3] as u32).max().unwrap_or(1);
            stroke(&mut pm, &(0..=64).map(|i| {
                let a = i as f32 / 64.0 * std::f32::consts::TAU;
                (cx + r * a.cos(), cy + r * a.sin())
            }).collect::<Vec<_>>(), [255, 255, 255, 40], 1.0, true);
            stroke(&mut pm, &[(cx - r, cy), (cx + r, cy)], [255, 255, 255, 28], 1.0, false);
            stroke(&mut pm, &[(cx, cy - r), (cx, cy + r)], [255, 255, 255, 28], 1.0, false);
            // 75% color targets
            for (c, label) in [([0.75, 0.0, 0.0], 0), ([0.75, 0.75, 0.0], 1), ([0.0, 0.75, 0.0], 2), ([0.0, 0.75, 0.75], 3), ([0.0, 0.0, 0.75], 4), ([0.75, 0.0, 0.75], 5)] {
                let _ = label;
                let (cb, cr) = chroma(c);
                let (x, y) = (cx + cb * 2.0 * r, cy - cr * 2.0 * r);
                let b = s * 0.025;
                stroke(&mut pm, &[(x - b, y - b), (x + b, y - b), (x + b, y + b), (x - b, y + b)], [(c[0] * 340.0) as u8, (c[1] * 340.0) as u8, (c[2] * 340.0) as u8, 150], 1.0, true);
            }
            // skin-tone line (I line, ~123 degrees from the +Cb axis)
            let a = 123f32.to_radians();
            stroke(&mut pm, &[(cx, cy), (cx + r * a.cos(), cy - r * a.sin())], [255, 200, 150, 90], 1.0, false);
            let d = pm.data_mut();
            for (i, a) in acc.iter().enumerate() {
                if a[3] <= 0.0 {
                    continue;
                }
                let g = glow(a[3] as u32, peak);
                for k in 0..3 {
                    let v = (a[k] / a[3] * 0.6 + 0.4) * g * 255.0;
                    d[i * 4 + k] = (d[i * 4 + k] as f32 + v).min(255.0) as u8;
                }
            }
        }
        "histogram" | "hist" => {
            let mut hist = [[0u32; 256]; 4];
            for c in &px {
                for k in 0..3 {
                    hist[k][(c[k].clamp(0.0, 1.0) * 255.0) as usize] += 1;
                }
                hist[3][(luma(*c).clamp(0.0, 1.0) * 255.0) as usize] += 1;
            }
            // ignore the extreme bins for scaling (pure black/white bars would flatten everything)
            let peak = hist.iter().flat_map(|hh| hh[1..255].iter()).copied().max().unwrap_or(1).max(1) as f32;
            let d = pm.data_mut();
            for x in 0..w {
                let bin = (x as u64 * 256 / w as u64) as usize;
                for k in 0..3 {
                    let top = ((hist[k][bin] as f32 / peak).min(1.0) * (h - 1) as f32) as u32;
                    for y in (h - top)..h {
                        let i = ((y * w + x) * 4) as usize + k;
                        d[i] = (d[i] as u32 + 150).min(255) as u8;
                    }
                }
            }
            let line: Vec<(f32, f32)> = (0..w).map(|x| {
                let bin = (x as u64 * 256 / w as u64) as usize;
                (x as f32 + 0.5, (h - 1) as f32 - (hist[3][bin] as f32 / peak).min(1.0) * (h - 1) as f32)
            }).collect();
            stroke(&mut pm, &line, [255, 255, 255, 200], 1.0, false);
            for q in [0.25f32, 0.5, 0.75] {
                let x = q * w as f32;
                stroke(&mut pm, &[(x, 0.0), (x, h as f32)], [255, 255, 255, 26], 1.0, false);
            }
        }
        other => bail!("unknown scope '{other}' ({})", KINDS.join(" ")),
    }
    Ok(pm)
}

/// Exposure and color numbers for a frame, as one compact line plus hints.
pub fn summary(frame: &Pixmap) -> String {
    let px = pixels(frame, (frame.width() as usize * frame.height() as usize / 200_000).max(1));
    if px.is_empty() {
        return "empty frame".into();
    }
    let n = px.len() as f32;
    let mut ls: Vec<f32> = px.par_iter().map(|c| luma(*c)).collect();
    ls.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |q: f32| ls[((ls.len() - 1) as f32 * q) as usize];
    let mean = ls.iter().sum::<f32>() / n;
    let clipped = px.iter().filter(|c| c.iter().any(|v| *v >= 0.995)).count() as f32 / n * 100.0;
    let crushed = ls.iter().filter(|l| **l <= 0.01).count() as f32 / n * 100.0;
    let illegal = ls.iter().filter(|l| **l > 235.0 / 255.0 || **l < 16.0 / 255.0).count() as f32 / n * 100.0;
    let (mut cb, mut cr, mut sat) = (0.0, 0.0, 0.0);
    for c in &px {
        let (b, r) = chroma(*c);
        cb += b;
        cr += r;
        sat += (b * b + r * r).sqrt();
    }
    let (cb, cr, sat) = (cb / n, cr / n, sat / n);
    let warm = (cr - cb) * 100.0;
    let green = -(cr + cb) * 100.0;
    let cast = if warm.abs() < 1.5 && green.abs() < 1.5 {
        "neutral".to_string()
    } else {
        let mut v = vec![];
        if warm.abs() >= 1.5 {
            v.push(format!("{} {:.0}", if warm > 0.0 { "warm" } else { "cool" }, warm.abs()));
        }
        if green.abs() >= 1.5 {
            v.push(format!("{} {:.0}", if green > 0.0 { "green" } else { "magenta" }, green.abs()));
        }
        v.join(", ")
    };
    let mut s = format!(
        "luma {:.0}-{:.0}% (1st-99th pct), median {:.0}%, mean {:.0}% | clipped {:.1}% | crushed {:.1}% | outside 16-235 {:.1}% | saturation {:.0}% | cast {cast}",
        pct(0.01) * 100.0,
        pct(0.99) * 100.0,
        pct(0.5) * 100.0,
        mean * 100.0,
        clipped,
        crushed,
        illegal,
        sat * 200.0
    );
    let mut hints = vec![];
    if clipped > 2.0 {
        hints.push("highlights clip: lower exposure or whites");
    }
    if crushed > 8.0 {
        hints.push("shadows crushed: raise blacks or shadows");
    }
    if pct(0.99) < 0.75 {
        hints.push("no bright highlights: raise exposure/whites (or autolevels)");
    }
    if pct(0.01) > 0.12 {
        hints.push("milky blacks: lower blacks or add contrast");
    }
    if mean < 0.22 {
        hints.push("dark overall");
    } else if mean > 0.7 {
        hints.push("bright overall");
    }
    if warm.abs() > 6.0 || green.abs() > 6.0 {
        hints.push("strong color cast: balance with temp/tint (or autolevels perchannel=1)");
    }
    if sat * 200.0 < 6.0 {
        hints.push("almost no color");
    }
    if !hints.is_empty() {
        s.push_str("\nhints: ");
        s.push_str(&hints.join("; "));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_and_summary() {
        let mut f = Pixmap::new(64, 32).unwrap();
        for (i, p) in f.data_mut().chunks_exact_mut(4).enumerate() {
            let v = ((i % 64) * 4) as u8;
            p.copy_from_slice(&[v, v / 2, 40, 255]);
        }
        for k in KINDS {
            let pm = render(&f, k, 256, 128).unwrap();
            assert!(pm.data().chunks_exact(4).any(|p| p[1] > 60), "{k} drew nothing");
        }
        assert!(render(&f, "nope", 64, 64).is_err());
        let s = summary(&f);
        assert!(s.contains("cast warm"), "{s}");
        let mut white = Pixmap::new(16, 16).unwrap();
        white.fill(SkColor::WHITE);
        assert!(summary(&white).contains("clipped 100.0%"));
    }
}
