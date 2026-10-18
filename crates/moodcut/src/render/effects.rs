//! Native (compositor) effects. All work on premultiplied RGBA layer buffers in
//! output space; spatial parameters are in comp pixels and scaled to output.

use super::eval::Ev;
use super::lut;
use super::pixel::{self, blur_pixmap, map_lut, map_rgb, new_pixmap, sample, src_over, LayerBuf};
use crate::model::project::Effect;
use crate::model::schema::{self, Engine};
use crate::model::value::Color;
use rayon::prelude::*;
use resvg::tiny_skia::Pixmap;

pub struct FxCtx<'a> {
    pub ev: &'a Ev<'a>,
    /// comp px -> output px
    pub scale: f32,
    pub out_w: i32,
    pub out_h: i32,
    pub frame: i64,
    pub seed: u64,
}

fn params_ev<'a>(cx: &FxCtx<'a>, def: &'static schema::EffectDef) -> Ev<'a> {
    cx.ev.with_lists(&[def.params])
}

/// Fraction of the needed resolution a layer's source can be rasterized at
/// when its first effect is a strong, constant blur (e.g. a blurred background
/// fill): the blur hides the lost detail. Powers of two keep decode sizes stable.
pub fn blur_source_scale(fx: &[Effect], ev: &Ev, scale: f32) -> f32 {
    let Some(e) = fx.iter().find(|e| e.on) else { return 1.0 };
    if e.kind != "blur" || e.props.get("radius").is_some_and(|a| a.is_animated()) || e.props.get("dir").is_some_and(|d| d.v.str() != "both") {
        return 1.0;
    }
    let Some(def) = schema::effect("blur") else { return 1.0 };
    let r = ev.with_lists(&[def.params]).f(&e.props, "radius") * scale;
    match r {
        r if r >= 64.0 => 0.125,
        r if r >= 32.0 => 0.25,
        r if r >= 16.0 => 0.5,
        _ => 1.0,
    }
}

/// Extra pixels (output space) the effect may draw outside the layer's content.
pub fn margin(e: &Effect, ev: &Ev, scale: f32) -> f32 {
    let Some(def) = schema::effect(&e.kind) else { return 0.0 };
    if !e.on || def.engine != Engine::Native {
        return 0.0;
    }
    let pe = ev.with_lists(&[def.params]);
    let p = &e.props;
    let m = match def.name {
        "blur" => pe.f(p, "radius") * 1.6,
        "glow" => pe.f(p, "radius") * 1.6,
        "shadow" => pe.f(p, "distance") + pe.f(p, "blur") * 1.6,
        "stroke" => pe.f(p, "width") + 2.0,
        "dirblur" => pe.f(p, "length"),
        "rgbsplit" => pe.f(p, "amount"),
        "glitch" => 40.0 * pe.f(p, "amount"),
        "wave" => pe.f(p, "amp"),
        _ => 0.0,
    };
    m.max(0.0) * scale
}

fn lut1(f: impl Fn(f32) -> f32) -> [u8; 256] {
    let mut l = [0u8; 256];
    for (i, v) in l.iter_mut().enumerate() {
        *v = (f(i as f32 / 255.0).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    l
}

#[inline]
fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let l = (mx + mn) / 2.0;
    if (mx - mn).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = mx - mn;
    let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
    let h = if mx == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s <= 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}

/// Parses "x,y x,y ..." curve points and returns a 256 LUT (monotone cubic).
fn curve_lut(spec: &str) -> Option<[u8; 256]> {
    let mut pts: Vec<(f32, f32)> = spec
        .split_whitespace()
        .filter_map(|p| {
            let (x, y) = p.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect();
    if pts.len() < 2 {
        return None;
    }
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let n = pts.len();
    // Fritsch-Carlson monotone cubic
    let mut d = vec![0f32; n - 1];
    for i in 0..n - 1 {
        d[i] = (pts[i + 1].1 - pts[i].1) / (pts[i + 1].0 - pts[i].0).max(1e-6);
    }
    let mut m = vec![0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 {
        m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
    }
    Some(lut1(|x| {
        if x <= pts[0].0 {
            return pts[0].1;
        }
        if x >= pts[n - 1].0 {
            return pts[n - 1].1;
        }
        let i = pts.windows(2).position(|w| x >= w[0].0 && x <= w[1].0).unwrap_or(0);
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[i + 1];
        let h = (x1 - x0).max(1e-6);
        let t = (x - x0) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * h * m[i + 1]
    }))
}

/// Remaps pixels: f maps destination pixel center -> source position (buffer coords).
fn remap(pm: &mut Pixmap, f: impl Fn(f32, f32) -> (f32, f32) + Sync) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let src = pm.data().to_vec();
    pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w as usize {
            let (sx, sy) = f(x as f32 + 0.5, y as f32 + 0.5);
            let s = sample(&src, w, h, sx, sy);
            let o = &mut row[x * 4..x * 4 + 4];
            for k in 0..4 {
                o[k] = s[k].round().clamp(0.0, 255.0) as u8;
            }
            o[0] = o[0].min(o[3]);
            o[1] = o[1].min(o[3]);
            o[2] = o[2].min(o[3]);
        }
    });
}

fn shift(pm: &Pixmap, dx: i32, dy: i32) -> Pixmap {
    let mut out = new_pixmap(pm.width(), pm.height());
    pixel::blit(&mut out, pm, dx, dy);
    out
}

fn dilate_alpha(alpha: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    if r == 0 {
        return alpha.to_vec();
    }
    let mut tmp = vec![0u8; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &alpha[y * w..(y + 1) * w];
        for x in 0..w {
            let a = x.saturating_sub(r);
            let b = (x + r).min(w - 1);
            row[x] = *src[a..=b].iter().max().unwrap_or(&0);
        }
    });
    let mut out = vec![0u8; w * h];
    let cols: Vec<Vec<u8>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let col: Vec<u8> = (0..h).map(|y| tmp[y * w + x]).collect();
            (0..h).map(|y| *col[y.saturating_sub(r)..=(y + r).min(h - 1)].iter().max().unwrap_or(&0)).collect()
        })
        .collect();
    for (x, col) in cols.iter().enumerate() {
        for y in 0..h {
            out[y * w + x] = col[y];
        }
    }
    out
}

fn hash2(x: u32, y: u32, s: u64) -> u32 {
    let mut h = (x as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ (y as u64).wrapping_mul(0xC2B2AE3D27D4EB4F) ^ s;
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58476D1CE4E5B9);
    h ^= h >> 29;
    h as u32
}

pub fn lumetri_fn(p: [f32; 11], stint: Color, htint: Color) -> impl Fn(f32, f32, f32) -> (f32, f32, f32) + Sync {
    let [exposure, contrast, highlights, shadows, whites, blacks, temp, tint, sat, vibrance, fade] = p;
    move |r, g, b| {
        let ex = 2f32.powf(exposure);
        let (mut r, mut g, mut b) = (r * ex, g * ex, b * ex);
        // white balance
        let k = temp / 100.0;
        r *= 1.0 + 0.22 * k;
        b *= 1.0 - 0.22 * k;
        g *= 1.0 - 0.12 * (tint / 100.0);
        let tone = |v: f32| -> f32 {
            let mut v = v;
            v += shadows / 100.0 * 0.35 * (1.0 - v).max(0.0).powi(3);
            v += highlights / 100.0 * 0.35 * v.clamp(0.0, 1.0).powi(3);
            v += whites / 100.0 * 0.2 * v.clamp(0.0, 1.0).powi(6);
            v += blacks / 100.0 * 0.2 * (1.0 - v).max(0.0).powi(6);
            let c = 1.0 + contrast / 100.0 * 0.8;
            let pivot = 0.45;
            v = pivot + (v - pivot) * c;
            let f = fade / 100.0 * 0.18;
            f + v * (1.0 - f)
        };
        r = tone(r);
        g = tone(g);
        b = tone(b);
        let l = luma(r, g, b);
        // saturation & vibrance
        let s = sat / 100.0;
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        let cur_sat = (mx - mn).clamp(0.0, 1.0);
        let vib = 1.0 + vibrance / 100.0 * (1.0 - cur_sat);
        let f = s * vib;
        r = l + (r - l) * f;
        g = l + (g - l) * f;
        b = l + (b - l) * f;
        // split toning
        let l2 = luma(r, g, b).clamp(0.0, 1.0);
        if stint.a > 0.0 {
            let w = (1.0 - l2).powi(2) * stint.a * 0.6;
            r += (stint.r - 0.5) * w;
            g += (stint.g - 0.5) * w;
            b += (stint.b - 0.5) * w;
        }
        if htint.a > 0.0 {
            let w = l2.powi(2) * htint.a * 0.6;
            r += (htint.r - 0.5) * w;
            g += (htint.g - 0.5) * w;
            b += (htint.b - 0.5) * w;
        }
        (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
    }
}

/// Applies one effect to a layer buffer.
pub fn apply(e: &Effect, lb: &mut LayerBuf, cx: &FxCtx) {
    if !e.on {
        return;
    }
    let Some(def) = schema::effect(&e.kind) else { return };
    if def.engine != Engine::Native {
        return;
    }
    let ev = params_ev(cx, def);
    let p = &e.props;
    let s = cx.scale;
    let (ox, oy) = (lb.x, lb.y);
    let (ow, oh) = (cx.out_w as f32, cx.out_h as f32);
    let pm = lb.pm.make_mut();
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    match def.name {
        "lumetri" => {
            let vals = ["exposure", "contrast", "highlights", "shadows", "whites", "blacks", "temp", "tint", "sat", "vibrance", "fade"].map(|k| ev.f(p, k));
            let st = ev.color(p, "stint");
            let ht = ev.color(p, "htint");
            if vals == [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0] && st.a == 0.0 && ht.a == 0.0 {
                return;
            }
            let key = format!("lumetri:{:?}:{}:{}", vals, st.to_hex(), ht.to_hex());
            let l = lut::generated(&key, lumetri_fn(vals, st, ht));
            l.apply(pm, 1.0);
        }
        "bc" => {
            let br = ev.f(p, "brightness") / 100.0;
            let ct = ev.f(p, "contrast") / 100.0;
            let l = lut1(|v| (v + br * 0.5 - 0.5) * (1.0 + ct) + 0.5);
            map_lut(pm, &l, &l, &l);
        }
        "exposure" => {
            let st = ev.f(p, "stops");
            let off = ev.f(p, "offset");
            let g = ev.f(p, "gamma").max(0.01);
            let l = lut1(|v| (v * 2f32.powf(st) + off).max(0.0).powf(1.0 / g));
            map_lut(pm, &l, &l, &l);
        }
        "levels" => {
            let (ib, iw, g, obk, owt) = (ev.f(p, "inBlack"), ev.f(p, "inWhite"), ev.f(p, "gamma").max(0.01), ev.f(p, "outBlack"), ev.f(p, "outWhite"));
            let l = lut1(|v| {
                let x = ((v - ib) / (iw - ib).max(1e-4)).clamp(0.0, 1.0).powf(1.0 / g);
                obk + x * (owt - obk)
            });
            map_lut(pm, &l, &l, &l);
        }
        "curves" => {
            let id = lut1(|v| v);
            let m = curve_lut(&ev.str(p, "master"));
            let comp = |c: Option<[u8; 256]>| -> [u8; 256] {
                let base = c.unwrap_or(id);
                match &m {
                    Some(mm) => {
                        let mut o = [0u8; 256];
                        for i in 0..256 {
                            o[i] = mm[base[i] as usize];
                        }
                        o
                    }
                    None => base,
                }
            };
            let lr = comp(curve_lut(&ev.str(p, "r")));
            let lg = comp(curve_lut(&ev.str(p, "g")));
            let lb_ = comp(curve_lut(&ev.str(p, "b")));
            map_lut(pm, &lr, &lg, &lb_);
        }
        "hue" => {
            let sh = ev.f(p, "shift") / 360.0;
            let sa = ev.f(p, "sat");
            let li = ev.f(p, "light");
            map_rgb(pm, |r, g, b| {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let l2 = if li >= 0.0 { l + (1.0 - l) * li } else { l * (1.0 + li) };
                hsl_to_rgb((h + sh).rem_euclid(1.0), (s * sa).clamp(0.0, 1.0), l2)
            });
        }
        "saturation" => {
            let a = ev.f(p, "amount");
            map_rgb(pm, |r, g, b| {
                let l = luma(r, g, b);
                (l + (r - l) * a, l + (g - l) * a, l + (b - l) * a)
            });
        }
        "vibrance" => {
            let a = ev.f(p, "amount");
            map_rgb(pm, |r, g, b| {
                let l = luma(r, g, b);
                let s = r.max(g).max(b) - r.min(g).min(b);
                let f = 1.0 + a * (1.0 - s);
                (l + (r - l) * f, l + (g - l) * f, l + (b - l) * f)
            });
        }
        "temperature" => {
            let k = ev.f(p, "temp") / 100.0;
            let t = ev.f(p, "tint") / 100.0;
            map_rgb(pm, |r, g, b| (r * (1.0 + 0.22 * k), g * (1.0 - 0.12 * t), b * (1.0 - 0.22 * k)));
        }
        "tint" | "duotone" => {
            let (c0, c1) = if def.name == "tint" { (ev.color(p, "black"), ev.color(p, "white")) } else { (ev.color(p, "dark"), ev.color(p, "light")) };
            let a = ev.f(p, "amount");
            map_rgb(pm, |r, g, b| {
                let l = luma(r, g, b);
                let t = c0.lerp(&c1, l);
                (r + (t.r - r) * a, g + (t.g - g) * a, b + (t.b - b) * a)
            });
        }
        "fill" => {
            let c = ev.color(p, "color");
            let a = ev.f(p, "amount");
            map_rgb(pm, |r, g, b| (r + (c.r - r) * a, g + (c.g - g) * a, b + (c.b - b) * a));
        }
        "grayscale" => {
            let a = ev.f(p, "amount");
            map_rgb(pm, |r, g, b| {
                let l = luma(r, g, b);
                (r + (l - r) * a, g + (l - g) * a, b + (l - b) * a)
            });
        }
        "sepia" => {
            let a = ev.f(p, "amount");
            map_rgb(pm, |r, g, b| {
                let (sr, sg, sb) = (0.393 * r + 0.769 * g + 0.189 * b, 0.349 * r + 0.686 * g + 0.168 * b, 0.272 * r + 0.534 * g + 0.131 * b);
                (r + (sr - r) * a, g + (sg - g) * a, b + (sb - b) * a)
            });
        }
        "invert" => {
            let a = ev.f(p, "amount");
            let l = lut1(|v| v + (1.0 - 2.0 * v) * a);
            map_lut(pm, &l, &l, &l);
        }
        "lut" => {
            let path = ev.str(p, "file");
            if let Ok(l) = lut::load_cube(&path) {
                l.apply(pm, ev.f(p, "amount"));
            }
        }
        "posterize" => {
            let n = ev.f(p, "levels").max(2.0);
            let l = lut1(|v| (v * (n - 1.0)).round() / (n - 1.0));
            map_lut(pm, &l, &l, &l);
        }
        "threshold" => {
            let t = ev.f(p, "level");
            map_rgb(pm, |r, g, b| {
                let v = if luma(r, g, b) >= t { 1.0 } else { 0.0 };
                (v, v, v)
            });
        }
        "blur" => {
            let r = ev.f(p, "radius") * s;
            let dir = match ev.str(p, "dir").as_str() {
                "h" => 1,
                "v" => 2,
                _ => 0,
            };
            blur_pixmap(pm, r / 2.0, dir);
        }
        "dirblur" => {
            let ang = ev.f(p, "angle").to_radians();
            let len = ev.f(p, "length") * s;
            if len < 0.5 {
                return;
            }
            let n = (len / 1.5).clamp(3.0, 48.0) as i32;
            let (dx, dy) = (ang.cos() * len, ang.sin() * len);
            let src = pm.data().to_vec();
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w as usize {
                    let mut acc = [0f32; 4];
                    for k in 0..n {
                        let t = k as f32 / (n - 1) as f32 - 0.5;
                        let sm = sample(&src, w, h, x as f32 + 0.5 + dx * t, y as f32 + 0.5 + dy * t);
                        for c in 0..4 {
                            acc[c] += sm[c];
                        }
                    }
                    for c in 0..4 {
                        row[x * 4 + c] = (acc[c] / n as f32).round().clamp(0.0, 255.0) as u8;
                    }
                }
            });
        }
        "zoomblur" => {
            let amt = ev.f(p, "amount").clamp(0.0, 1.0);
            if amt <= 0.0 {
                return;
            }
            let (ccx, ccy) = (ev.f(p, "cx") * ow - ox as f32, ev.f(p, "cy") * oh - oy as f32);
            let n = 24;
            let src = pm.data().to_vec();
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w as usize {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let mut acc = [0f32; 4];
                    for k in 0..n {
                        let t = 1.0 - amt * k as f32 / (n - 1) as f32;
                        let sm = sample(&src, w, h, ccx + (px - ccx) * t, ccy + (py - ccy) * t);
                        for c in 0..4 {
                            acc[c] += sm[c];
                        }
                    }
                    for c in 0..4 {
                        row[x * 4 + c] = (acc[c] / n as f32).round().clamp(0.0, 255.0) as u8;
                    }
                }
            });
        }
        "sharpen" => {
            let amt = ev.f(p, "amount");
            let mut bl = pm.clone();
            blur_pixmap(&mut bl, (ev.f(p, "radius") * s).max(0.5), 0);
            let bd = bl.data();
            pm.data_mut().par_chunks_mut(4).zip(bd.par_chunks(4)).for_each(|(o, b)| {
                let a = o[3] as f32;
                for c in 0..3 {
                    let v = o[c] as f32 + amt * (o[c] as f32 - b[c] as f32);
                    o[c] = v.round().clamp(0.0, a) as u8;
                }
            });
        }
        "glow" => {
            let thr = ev.f(p, "threshold");
            let inten = ev.f(p, "intensity");
            let tintc = ev.color(p, "color");
            let mut bright = pm.clone();
            bright.data_mut().par_chunks_mut(4).for_each(|px| {
                let a = px[3] as f32 / 255.0;
                if a <= 0.0 {
                    return;
                }
                let (r, g, b) = (px[0] as f32 / 255.0 / a, px[1] as f32 / 255.0 / a, px[2] as f32 / 255.0 / a);
                let k = smoothstep(thr - 0.1, thr + 0.1, luma(r, g, b));
                let (r, g, b) = if tintc.a > 0.0 { (tintc.r, tintc.g, tintc.b) } else { (r, g, b) };
                let na = a * k;
                px[0] = (r * na * 255.0) as u8;
                px[1] = (g * na * 255.0) as u8;
                px[2] = (b * na * 255.0) as u8;
                px[3] = (na * 255.0) as u8;
            });
            blur_pixmap(&mut bright, ev.f(p, "radius") * s / 2.0, 0);
            let bd = bright.data();
            pm.data_mut().par_chunks_mut(4).zip(bd.par_chunks(4)).for_each(|(o, g)| {
                let na = (o[3] as f32 + g[3] as f32 * inten * (1.0 - o[3] as f32 / 255.0)).min(255.0);
                for c in 0..3 {
                    o[c] = (o[c] as f32 + g[c] as f32 * inten).min(na) as u8;
                }
                o[3] = na as u8;
            });
        }
        "shadow" => {
            let c = ev.color(p, "color");
            let op = ev.f(p, "opacity");
            let ang = ev.f(p, "angle").to_radians();
            let dist = ev.f(p, "distance") * s;
            drop_shadow(pm, c.with_alpha(c.a * op), ang.sin() * dist, -ang.cos() * dist, ev.f(p, "blur") * s / 2.0);
        }
        "stroke" => {
            let c = ev.color(p, "color");
            let r = (ev.f(p, "width") * s).round().max(0.0) as usize;
            if r == 0 {
                return;
            }
            let alpha: Vec<u8> = pm.data().chunks(4).map(|p| p[3]).collect();
            let mut dil = dilate_alpha(&alpha, w as usize, h as usize, r);
            pixel::blur_mask(&mut dil, w as usize, h as usize, 0.7);
            let [cr, cg, cb, ca] = c.to_u8();
            let mut st = new_pixmap(w as u32, h as u32);
            st.data_mut().par_chunks_mut(4).zip(dil.par_iter()).for_each(|(d, &a)| {
                let a = a as u32 * ca as u32 / 255;
                d[0] = (cr as u32 * a / 255) as u8;
                d[1] = (cg as u32 * a / 255) as u8;
                d[2] = (cb as u32 * a / 255) as u8;
                d[3] = a as u8;
            });
            src_over(&mut st, pm, 0, 0, 1.0);
            *pm = st;
        }
        "vignette" => {
            let amt = ev.f(p, "amount");
            let size = ev.f(p, "size");
            let soft = ev.f(p, "soft").max(0.01);
            let c = ev.color(p, "color");
            if amt <= 0.0 {
                return;
            }
            let key = format!("{}x{}+{}+{}|{}x{}|{:.4}|{:.4}|{:.4}", w, h, ox, oy, ow, oh, amt, size, soft);
            let mask = vignette_mask(&key, w, h, ox, oy, ow, oh, amt, size, soft);
            let [cr, cg, cb, _] = c.to_u8();
            pm.data_mut().par_chunks_mut(w as usize * 4).zip(mask.par_chunks(w as usize)).for_each(|(row, mrow)| {
                for (px, &v) in row.chunks_exact_mut(4).zip(mrow.iter()) {
                    if v == 0 {
                        continue;
                    }
                    let a = px[3] as u32;
                    let keep = 256 - v as u32;
                    let add = |cc: u8| (cc as u32 * a / 255) * v as u32;
                    px[0] = ((px[0] as u32 * keep + add(cr)) >> 8) as u8;
                    px[1] = ((px[1] as u32 * keep + add(cg)) >> 8) as u8;
                    px[2] = ((px[2] as u32 * keep + add(cb)) >> 8) as u8;
                }
            });
        }
        "noise" => {
            let amt = ev.f(p, "amount") * 255.0;
            let mono = ev.bool(p, "mono");
            let seed = if ev.bool(p, "animated") { cx.seed ^ (cx.frame as u64).wrapping_mul(7919) } else { cx.seed };
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w as usize {
                    let px = &mut row[x * 4..x * 4 + 4];
                    let a = px[3] as f32;
                    if a == 0.0 {
                        continue;
                    }
                    let gx = (x as i32 + ox) as u32;
                    let gy = (y as i32 + oy) as u32;
                    let n0 = hash2(gx, gy, seed);
                    for k in 0..3 {
                        let nv = if mono { n0 } else { hash2(gx, gy, seed.wrapping_add(k as u64 * 101)) };
                        let r = ((nv & 0xffff) as f32 / 65535.0 - 0.5) * amt * (a / 255.0);
                        px[k] = (px[k] as f32 + r).round().clamp(0.0, a) as u8;
                    }
                }
            });
        }
        "pixelate" => {
            let bs = (ev.f(p, "size") * s).round().max(1.0) as i32;
            if bs <= 1 {
                return;
            }
            let src = pm.data().to_vec();
            let wu = w as usize;
            pm.data_mut().par_chunks_mut(wu * 4 * bs as usize).enumerate().for_each(|(by, band)| {
                let y0 = by as i32 * bs;
                let rows = band.len() / (wu * 4);
                for bx in (0..w).step_by(bs as usize) {
                    let mut acc = [0u32; 4];
                    let mut n = 0u32;
                    for yy in 0..rows {
                        for xx in bx..(bx + bs).min(w) {
                            let i = (((y0 as usize + yy) * wu) + xx as usize) * 4;
                            for c in 0..4 {
                                acc[c] += src[i + c] as u32;
                            }
                            n += 1;
                        }
                    }
                    let avg = acc.map(|v| (v / n.max(1)) as u8);
                    for yy in 0..rows {
                        for xx in bx..(bx + bs).min(w) {
                            let i = (yy * wu + xx as usize) * 4;
                            band[i..i + 4].copy_from_slice(&avg);
                        }
                    }
                }
            });
        }
        "rgbsplit" => {
            let amt = ev.f(p, "amount") * s;
            let ang = ev.f(p, "angle").to_radians();
            rgb_split(pm, ang.cos() * amt, ang.sin() * amt);
        }
        "glitch" => {
            let amt = ev.f(p, "amount");
            if amt <= 0.0 {
                return;
            }
            let slices = ev.num(p, "slices").max(1.0) as u32;
            let speed = ev.f(p, "speed").max(0.0);
            let step = (ev.t * speed as f64).floor() as u64;
            let seed = cx.seed ^ step.wrapping_mul(0x9E37);
            let src = pm.data().to_vec();
            let wu = w as usize;
            // displaced slices
            let mut y = 0;
            let mut k = 0u32;
            while y < h {
                let r = hash2(k, 1, seed);
                let sh_ = ((r % 1000) as f32 / 1000.0 * (h as f32 / slices as f32) * 2.0).max(2.0) as i32;
                let active = (hash2(k, 2, seed) % 100) as f32 / 100.0 < amt * 0.6;
                if active {
                    let off = (((hash2(k, 3, seed) % 2001) as f32 / 1000.0 - 1.0) * amt * 60.0 * s) as i32;
                    for yy in y..(y + sh_).min(h) {
                        let row = &mut pm.data_mut()[yy as usize * wu * 4..(yy as usize + 1) * wu * 4];
                        for x in 0..w {
                            let sx = x - off;
                            let o = &mut row[x as usize * 4..x as usize * 4 + 4];
                            if sx >= 0 && sx < w {
                                let i = (yy as usize * wu + sx as usize) * 4;
                                o.copy_from_slice(&src[i..i + 4]);
                            } else {
                                o.copy_from_slice(&[0, 0, 0, 0]);
                            }
                        }
                    }
                }
                y += sh_;
                k += 1;
            }
            let rs = amt * 12.0 * s * if (hash2(9, 9, seed) % 2) == 0 { 1.0 } else { -1.0 };
            rgb_split(pm, rs, 0.0);
        }
        "scanlines" => {
            let sp = (ev.f(p, "spacing") * s).max(2.0);
            let op = ev.f(p, "opacity");
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                let gy = (y as i32 + oy) as f32;
                if (gy % sp) < sp / 2.0 {
                    for v in row.iter_mut().enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, v)| v) {
                        *v = (*v as f32 * (1.0 - op)) as u8;
                    }
                }
            });
        }
        "letterbox" => {
            let ratio = ev.f(p, "ratio").max(0.1);
            let bar = ((oh - ow / ratio) / 2.0).max(0.0);
            if bar <= 0.0 {
                return;
            }
            let c = ev.color(p, "color").to_premul_u8();
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                let gy = (y as i32 + oy) as f32 + 0.5;
                if gy < bar || gy > oh - bar {
                    for px in row.chunks_exact_mut(4) {
                        px.copy_from_slice(&c);
                    }
                }
            });
        }
        "gradient" => {
            let c1 = ev.color(p, "c1");
            let c2 = ev.color(p, "c2");
            let radial = ev.str(p, "type") == "radial";
            let ang = ev.f(p, "angle").to_radians();
            let amt = ev.f(p, "amount");
            let (dx, dy) = (ang.cos(), ang.sin());
            let (wf, hf) = (w as f32, h as f32);
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w as usize {
                    let (u, v) = ((x as f32 + 0.5) / wf - 0.5, (y as f32 + 0.5) / hf - 0.5);
                    let t = if radial { ((u * u + v * v).sqrt() * 2.0).min(1.0) } else { ((u * dx + v * dy) / (dx.abs() * 0.5 + dy.abs() * 0.5).max(1e-3) * 0.5 + 0.5).clamp(0.0, 1.0) };
                    let g = c1.lerp(&c2, t);
                    let px = &mut row[x * 4..x * 4 + 4];
                    let a = px[3] as f32;
                    let k = amt * g.a;
                    for i in 0..3 {
                        let target = [g.r, g.g, g.b][i] * a;
                        px[i] = (px[i] as f32 + (target - px[i] as f32) * k).round().clamp(0.0, a) as u8;
                    }
                }
            });
        }
        "lightleak" => {
            let c = ev.color(p, "color");
            let amt = ev.f(p, "amount");
            let sp = ev.f(p, "speed") as f64;
            let t = ev.t * sp;
            let blobs: Vec<(f32, f32, f32, f32)> = (0..3)
                .map(|i| {
                    let fi = i as f64;
                    let x = 0.5 + 0.45 * (t * (0.7 + fi * 0.3) + fi * 2.1).sin();
                    let y = 0.5 + 0.4 * (t * (0.5 + fi * 0.2) + fi * 1.3).cos();
                    let r = 0.35 + 0.15 * (t * 0.9 + fi).sin();
                    let k = (0.6 + 0.4 * (t * 1.3 + fi * 0.7).sin()) as f32;
                    (x as f32 * ow, y as f32 * oh, r as f32 * ow.max(oh), k)
                })
                .collect();
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                let gy = (y as i32 + oy) as f32;
                for x in 0..w as usize {
                    let gx = (x as i32 + ox) as f32;
                    let mut l = 0f32;
                    for (bx, by, br, bk) in &blobs {
                        let d2 = ((gx - bx).powi(2) + (gy - by).powi(2)) / (br * br);
                        l += bk * (-d2 * 2.5).exp();
                    }
                    let l = (l * amt).min(1.0);
                    if l <= 0.002 {
                        continue;
                    }
                    let px = &mut row[x * 4..x * 4 + 4];
                    let na = (px[3] as f32 + (255.0 - px[3] as f32) * l).min(255.0);
                    for i in 0..3 {
                        let lc = [c.r, c.g, c.b][i] * 255.0 * l;
                        let v = px[i] as f32;
                        px[i] = (v + lc - v * lc / 255.0).min(na) as u8;
                    }
                    px[3] = na as u8;
                }
            });
        }
        "wave" => {
            let amp = ev.f(p, "amp") * s;
            let len = (ev.f(p, "length") * s).max(1.0);
            let ph = (ev.t * ev.num(p, "speed") * std::f64::consts::TAU) as f32;
            let horiz = ev.str(p, "dir") != "v";
            remap(pm, |x, y| {
                if horiz {
                    (x + amp * ((y + oy as f32) / len * std::f32::consts::TAU + ph).sin(), y)
                } else {
                    (x, y + amp * ((x + ox as f32) / len * std::f32::consts::TAU + ph).sin())
                }
            });
        }
        "twirl" | "bulge" => {
            let (ccx, ccy) = (ev.f(p, "cx") * ow - ox as f32, ev.f(p, "cy") * oh - oy as f32);
            let rad = (ev.f(p, "radius") * oh).max(1.0);
            if def.name == "twirl" {
                let ang = ev.f(p, "angle").to_radians();
                remap(pm, |x, y| {
                    let (dx, dy) = (x - ccx, y - ccy);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d >= rad {
                        return (x, y);
                    }
                    let f = (1.0 - d / rad).powi(2) * ang;
                    let (sn, cs) = f.sin_cos();
                    (ccx + dx * cs - dy * sn, ccy + dx * sn + dy * cs)
                });
            } else {
                let amt = ev.f(p, "amount");
                remap(pm, |x, y| {
                    let (dx, dy) = (x - ccx, y - ccy);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d >= rad || d < 1e-3 {
                        return (x, y);
                    }
                    let r = d / rad;
                    let k = 1.0 - amt * (1.0 - r * r) * 0.5;
                    (ccx + dx * k, ccy + dy * k)
                });
            }
        }
        "mirror" => {
            let axis = ev.str(p, "axis");
            let (wf, hf) = (w as f32, h as f32);
            remap(pm, |x, y| {
                let nx = if (axis == "h" || axis == "both") && x > wf / 2.0 { wf - x } else { x };
                let ny = if (axis == "v" || axis == "both") && y > hf / 2.0 { hf - y } else { y };
                (nx, ny)
            });
        }
        "kaleido" => {
            let n = ev.f(p, "segments").max(2.0);
            let rot = ev.f(p, "angle").to_radians();
            let (ccx, ccy) = (w as f32 / 2.0, h as f32 / 2.0);
            let seg = std::f32::consts::TAU / n;
            remap(pm, |x, y| {
                let (dx, dy) = (x - ccx, y - ccy);
                let r = (dx * dx + dy * dy).sqrt();
                let mut a = dy.atan2(dx) - rot;
                a = a.rem_euclid(seg);
                if a > seg / 2.0 {
                    a = seg - a;
                }
                let a = a + rot;
                (ccx + r * a.cos(), ccy + r * a.sin())
            });
        }
        "flip" => {
            let fh = ev.bool(p, "h");
            let fv = ev.bool(p, "v");
            let (wf, hf) = (w as f32, h as f32);
            remap(pm, |x, y| (if fh { wf - x } else { x }, if fv { hf - y } else { y }));
        }
        "chromakey" => {
            let kc = ev.color(p, "color");
            let sim = ev.f(p, "similarity");
            let smooth = ev.f(p, "smooth").max(0.001);
            let spill = ev.f(p, "spill");
            let ycc = |r: f32, g: f32, b: f32| (-0.168736 * r - 0.331264 * g + 0.5 * b, 0.5 * r - 0.418688 * g - 0.081312 * b);
            let (kb, kr) = ycc(kc.r, kc.g, kc.b);
            let dom = if kc.g >= kc.r && kc.g >= kc.b { 1 } else if kc.b >= kc.r { 2 } else { 0 };
            pm.data_mut().par_chunks_mut(4).for_each(|px| {
                let a = px[3] as f32 / 255.0;
                if a <= 0.0 {
                    return;
                }
                let mut c = [px[0] as f32 / 255.0 / a, px[1] as f32 / 255.0 / a, px[2] as f32 / 255.0 / a];
                let (cb, cr) = ycc(c[0], c[1], c[2]);
                let d = ((cb - kb).powi(2) + (cr - kr).powi(2)).sqrt() * 2.0;
                let k = smoothstep(sim * 0.6, sim * 0.6 + smooth * 0.6, d);
                // spill suppression on the dominant key channel
                let others = match dom {
                    0 => c[1].max(c[2]),
                    1 => c[0].max(c[2]),
                    _ => c[0].max(c[1]),
                };
                if c[dom] > others {
                    c[dom] -= (c[dom] - others) * spill;
                }
                let na = a * k;
                for i in 0..3 {
                    px[i] = (c[i].clamp(0.0, 1.0) * na * 255.0) as u8;
                }
                px[3] = (na * 255.0) as u8;
            });
        }
        "lumakey" => {
            let thr = ev.f(p, "threshold");
            let sm = ev.f(p, "smooth").max(0.001);
            let inv = ev.bool(p, "invert");
            pm.data_mut().par_chunks_mut(4).for_each(|px| {
                let a = px[3] as f32 / 255.0;
                if a <= 0.0 {
                    return;
                }
                let l = luma(px[0] as f32 / 255.0 / a, px[1] as f32 / 255.0 / a, px[2] as f32 / 255.0 / a);
                let mut k = smoothstep(thr, thr + sm, l);
                if inv {
                    k = 1.0 - smoothstep(1.0 - thr - sm, 1.0 - thr, l);
                }
                for i in 0..4 {
                    px[i] = (px[i] as f32 * k) as u8;
                }
            });
        }
        // geometric effects handled by the compositor (crop -> mask, cornerpin -> warp)
        _ => {}
    }
}

/// Drop shadow under the existing content of `pm` (in place).
pub fn drop_shadow(pm: &mut Pixmap, c: Color, dx: f32, dy: f32, sigma: f32) {
    let (w, h) = (pm.width(), pm.height());
    let [cr, cg, cb, _] = c.to_u8();
    let ca = c.a.clamp(0.0, 1.0);
    let mut sh = new_pixmap(w, h);
    sh.data_mut().par_chunks_mut(4).zip(pm.data().par_chunks(4)).for_each(|(d, sp)| {
        let a = sp[3] as f32 * ca;
        d[0] = (cr as f32 * a / 255.0) as u8;
        d[1] = (cg as f32 * a / 255.0) as u8;
        d[2] = (cb as f32 * a / 255.0) as u8;
        d[3] = a as u8;
    });
    let mut sh = shift(&sh, dx.round() as i32, dy.round() as i32);
    blur_pixmap(&mut sh, sigma, 0);
    src_over(&mut sh, pm, 0, 0, 1.0);
    *pm = sh;
}

/// Cached vignette strength mask (0..256 per pixel) for a buffer placement.
#[allow(clippy::too_many_arguments)]
fn vignette_mask(key: &str, w: i32, h: i32, ox: i32, oy: i32, ow: f32, oh: f32, amt: f32, size: f32, soft: f32) -> std::sync::Arc<Vec<u16>> {
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<u16>>>>> = OnceLock::new();
    let c = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(m) = c.lock().get(key) {
        return m.clone();
    }
    let (hw, hh) = (ow / 2.0, oh / 2.0);
    let mut m = vec![0u16; (w * h) as usize];
    m.par_chunks_mut(w as usize).enumerate().for_each(|(y, row)| {
        let gy = (y as i32 + oy) as f32 + 0.5;
        for (x, v) in row.iter_mut().enumerate() {
            let gx = (x as i32 + ox) as f32 + 0.5;
            let dx = (gx - hw) / hw;
            let dy = (gy - hh) / hh;
            let r = (dx * dx + dy * dy).sqrt();
            *v = (smoothstep(size, size + soft, r) * amt * 256.0).round().clamp(0.0, 256.0) as u16;
        }
    });
    let m = Arc::new(m);
    let mut g = c.lock();
    if g.len() > 32 {
        g.clear();
    }
    g.insert(key.to_string(), m.clone());
    m
}

fn rgb_split(pm: &mut Pixmap, dx: f32, dy: f32) {
    if dx.abs() < 0.3 && dy.abs() < 0.3 {
        return;
    }
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let src = pm.data().to_vec();
    pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w as usize {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = sample(&src, w, h, fx - dx, fy - dy);
            let g = sample(&src, w, h, fx, fy);
            let b = sample(&src, w, h, fx + dx, fy + dy);
            let a = r[3].max(g[3]).max(b[3]);
            let o = &mut row[x * 4..x * 4 + 4];
            o[0] = r[0].round().min(a) as u8;
            o[1] = g[1].round().min(a) as u8;
            o[2] = b[2].round().min(a) as u8;
            o[3] = a.round() as u8;
        }
    });
}

/// FFmpeg decode-time filter chain for a clip's Ff-engine effects.
pub fn ff_filters(fx: &[Effect]) -> String {
    let mut v = vec![];
    for e in fx {
        if !e.on {
            continue;
        }
        let Some(def) = schema::effect(&e.kind) else { continue };
        if def.engine != Engine::Ff {
            continue;
        }
        let n = |k: &str, d: f64| e.props.get(k).map(|a| a.v.num()).unwrap_or(d);
        match def.name {
            "denoise" => {
                let s = n("strength", 4.0).clamp(0.0, 30.0);
                v.push(format!("hqdn3d={:.2}:{:.2}:{:.2}:{:.2}", s, s * 0.75, s * 1.5, s * 1.1));
            }
            "stabilize" => {
                let r = n("strength", 16.0).clamp(4.0, 64.0) as i32;
                v.push(format!("deshake=rx={r}:ry={r}"));
            }
            "deinterlace" => v.push("bwdif=mode=send_frame".into()),
            "deflicker" => v.push(format!("deflicker=size={}", n("size", 5.0).clamp(2.0, 129.0) as i32)),
            "ff" => {
                let f = e.props.get("filter").map(|a| a.v.str()).unwrap_or_default();
                if !f.trim().is_empty() {
                    v.push(f);
                }
            }
            _ => {}
        }
    }
    v.join(",")
}
