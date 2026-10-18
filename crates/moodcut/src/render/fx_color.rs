//! Color effects: Lumetri color wheels, HSL secondary and hue/saturation
//! curves, channel mixer, leave / change color, photo filter, tritone, auto
//! levels and the video limiter. Per-pixel color functions are baked into
//! cached 3D LUTs (as Lumetri is), so any grade costs one lookup per pixel.

use super::effects::{hsl_to_rgb, lut1, luma, rgb_to_hsl, smoothstep};
use super::eval::Ev;
use super::lut;
use super::pixel::{map_lut, map_rgb};
use crate::model::project::Props;
use crate::model::value::Color;
use rayon::prelude::*;
use resvg::tiny_skia::Pixmap;

/// Circular hue distance (0..0.5 turns).
fn hue_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(1.0);
    d.min(1.0 - d)
}

/// Direction a wheel color pushes in (chroma only), scaled by its alpha.
fn wheel(c: Color) -> [f32; 3] {
    if c.a <= 0.0 {
        return [0.0; 3];
    }
    let l = luma(c.r, c.g, c.b);
    [(c.r - l) * c.a, (c.g - l) * c.a, (c.b - l) * c.a]
}

/// Sum of smooth bumps 'x,y[,w] ...' sampled at 256 points (periodic for hues).
fn bump_table(spec: &str, periodic: bool, def_w: f32) -> Option<[f32; 256]> {
    let pts: Vec<(f32, f32, f32)> = spec
        .split_whitespace()
        .filter_map(|p| {
            let v: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() >= 2).then(|| (v[0], v[1], v.get(2).copied().unwrap_or(def_w).max(0.01)))
        })
        .collect();
    if pts.is_empty() {
        return None;
    }
    let mut t = [0f32; 256];
    for (i, o) in t.iter_mut().enumerate() {
        let x = i as f32 / 255.0;
        for &(px, py, w) in &pts {
            let d = if periodic { hue_dist(x, px) } else { (x - px).abs() };
            if d < w {
                *o += py * 0.5 * (1.0 + (std::f32::consts::PI * d / w).cos());
            }
        }
    }
    Some(t)
}

#[inline]
fn at(t: &Option<[f32; 256]>, x: f32) -> f32 {
    t.as_ref().map(|t| t[(x.clamp(0.0, 1.0) * 255.0).round() as usize]).unwrap_or(0.0)
}

fn baked(pm: &mut Pixmap, key: String, f: impl Fn(f32, f32, f32) -> (f32, f32, f32) + Sync) {
    lut::generated(&key, |r, g, b| {
        let (r, g, b) = f(r, g, b);
        (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
    })
    .apply(pm, 1.0);
}

/// White-balance shift used by the secondary correction.
fn temp_tint(r: f32, g: f32, b: f32, temp: f32, tint: f32) -> (f32, f32, f32) {
    let k = temp / 100.0;
    (r * (1.0 + 0.22 * k), g * (1.0 - 0.12 * (tint / 100.0)), b * (1.0 - 0.22 * k))
}

/// Applies a color effect. Returns false when `name` isn't one of this module's.
pub fn apply(name: &str, ev: &Ev, p: &Props, pm: &mut Pixmap) -> bool {
    match name {
        "colorwheels" => {
            let (s, m, h) = (wheel(ev.color(p, "shadows")), wheel(ev.color(p, "midtones")), wheel(ev.color(p, "highlights")));
            let (sl, ml, hl) = (ev.f(p, "shadowsL") / 100.0, ev.f(p, "midtonesL") / 100.0, ev.f(p, "highlightsL") / 100.0);
            if s == [0.0; 3] && m == [0.0; 3] && h == [0.0; 3] && sl == 0.0 && ml == 0.0 && hl == 0.0 {
                return true;
            }
            // lift / gamma / gain per channel (ASC CDL style)
            let ch = |c: usize| {
                let lift = s[c] * 0.6 + sl * 0.25;
                let gamma = (1.0 + m[c] * 1.6 + ml * 0.6).clamp(0.2, 5.0);
                let gain = (1.0 + h[c] * 1.2 + hl * 0.5).max(0.0);
                lut1(move |x| (gain * (x + lift * (1.0 - x))).max(0.0).powf(1.0 / gamma))
            };
            map_lut(pm, &ch(0), &ch(1), &ch(2));
        }
        "channelmixer" => {
            let k = |n: &str| ev.f(p, n) / 100.0;
            let m = [k("rr"), k("rg"), k("rb"), k("gr"), k("gg"), k("gb"), k("br"), k("bg"), k("bb")];
            let mono = ev.bool(p, "mono");
            if !mono && m == [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
                return true;
            }
            baked(pm, format!("mixer:{m:?}:{mono}"), move |r, g, b| {
                let rr = m[0] * r + m[1] * g + m[2] * b;
                if mono {
                    (rr, rr, rr)
                } else {
                    (rr, m[3] * r + m[4] * g + m[5] * b, m[6] * r + m[7] * g + m[8] * b)
                }
            });
        }
        "leavecolor" | "changecolor" => {
            let leave = name == "leavecolor";
            let key = if leave { ev.color(p, "color") } else { ev.color(p, "from") };
            let to = ev.color(p, "to");
            let (tol, soft, amt) = (ev.f(p, "tolerance") * 0.5, ev.f(p, "soft") * 0.5 + 1e-3, ev.f(p, "amount"));
            let hue_only = ev.str(p, "mode") != "color";
            let (h0, _, l0) = rgb_to_hsl(key.r, key.g, key.b);
            let (h1, s1, l1) = rgb_to_hsl(to.r, to.g, to.b);
            let tag = format!("{name}:{}:{}:{tol}:{soft}:{amt}:{hue_only}", key.to_hex(), to.to_hex());
            baked(pm, tag, move |r, g, b| {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let w = (1.0 - smoothstep(tol, tol + soft, hue_dist(h, h0))) * smoothstep(0.04, 0.16, s);
                if leave {
                    let k = amt * (1.0 - w);
                    let y = luma(r, g, b);
                    (r + (y - r) * k, g + (y - g) * k, b + (y - b) * k)
                } else {
                    let (nr, ng, nb) = if hue_only { hsl_to_rgb(h1, s1.max(s * 0.5), l) } else { hsl_to_rgb(h1, s1, (l1 + l - l0).clamp(0.0, 1.0)) };
                    let k = w * amt;
                    (r + (nr - r) * k, g + (ng - g) * k, b + (nb - b) * k)
                }
            });
        }
        "secondary" => {
            let hue = ev.f(p, "hue").rem_euclid(360.0) / 360.0;
            let range = ev.f(p, "hueRange") / 360.0;
            let soft = ev.f(p, "soft");
            let (smin, lmin, lmax) = (ev.f(p, "satMin"), ev.f(p, "lumMin"), ev.f(p, "lumMax"));
            let (shift, sat, light, temp, tint) = (ev.f(p, "shift") / 360.0, ev.f(p, "sat"), ev.f(p, "light"), ev.f(p, "temp"), ev.f(p, "tint"));
            let (show, inv) = (ev.bool(p, "show"), ev.bool(p, "invert"));
            let hs = soft * 0.25 + 1e-3;
            let ss = soft * 0.5 + 1e-3;
            let tag = format!("sec:{hue}:{range}:{soft}:{smin}:{lmin}:{lmax}:{shift}:{sat}:{light}:{temp}:{tint}:{show}:{inv}");
            baked(pm, tag, move |r, g, b| {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let wh = 1.0 - smoothstep(range, range + hs, hue_dist(h, hue));
                let ws = if smin <= 0.0 { 1.0 } else { smoothstep(smin - ss, smin, s) };
                let wl = smoothstep(lmin - ss, lmin, l) * (1.0 - smoothstep(lmax, lmax + ss, l));
                let mut k = wh * ws * wl;
                if inv {
                    k = 1.0 - k;
                }
                if show {
                    return (k, k, k);
                }
                if k <= 0.0 {
                    return (r, g, b);
                }
                let l2 = if light >= 0.0 { l + (1.0 - l) * light } else { l * (1.0 + light) };
                let (nr, ng, nb) = hsl_to_rgb((h + shift).rem_euclid(1.0), (s * sat).clamp(0.0, 1.0), l2);
                let (nr, ng, nb) = temp_tint(nr, ng, nb, temp, tint);
                (r + (nr - r) * k, g + (ng - g) * k, b + (nb - b) * k)
            });
        }
        "huecurves" => {
            let specs = ["hueSat", "hueHue", "hueLuma", "lumaSat", "satSat"].map(|k| ev.str(p, k));
            if specs.iter().all(|s| s.trim().is_empty()) {
                return true;
            }
            let hs = bump_table(&specs[0], true, 0.08);
            let hh = bump_table(&specs[1], true, 0.08);
            let hl = bump_table(&specs[2], true, 0.08);
            let ls = bump_table(&specs[3], false, 0.15);
            let sat_sat = bump_table(&specs[4], false, 0.15);
            baked(pm, format!("huecurves:{specs:?}"), move |r, g, b| {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let s0 = s;
                let mut s = s * (1.0 + at(&hs, h)) * (1.0 + at(&ls, l)) * (1.0 + at(&sat_sat, s0));
                s = s.clamp(0.0, 1.0);
                let v = at(&hl, h) * smoothstep(0.0, 0.15, s0);
                let l = if v >= 0.0 { l + (1.0 - l) * v * 0.8 } else { l * (1.0 + v * 0.8) };
                let h = (h + at(&hh, h)).rem_euclid(1.0);
                hsl_to_rgb(h, s, l.clamp(0.0, 1.0))
            });
        }
        "photofilter" => {
            let c = ev.color(p, "color");
            let d = ev.f(p, "density");
            let keep = ev.bool(p, "preserve");
            map_rgb(pm, |r, g, b| {
                let (fr, fg, fb) = (r * (1.0 + (c.r - 1.0) * d), g * (1.0 + (c.g - 1.0) * d), b * (1.0 + (c.b - 1.0) * d));
                if keep {
                    let k = luma(r, g, b) / luma(fr, fg, fb).max(1e-4);
                    (fr * k, fg * k, fb * k)
                } else {
                    (fr, fg, fb)
                }
            });
        }
        "tritone" => {
            let (sh, mi, hi) = (ev.color(p, "shadows"), ev.color(p, "midtones"), ev.color(p, "highlights"));
            let a = ev.f(p, "amount");
            map_rgb(pm, |r, g, b| {
                let l = luma(r, g, b);
                let t = if l < 0.5 { sh.lerp(&mi, l * 2.0) } else { mi.lerp(&hi, (l - 0.5) * 2.0) };
                (r + (t.r - r) * a, g + (t.g - g) * a, b + (t.b - b) * a)
            });
        }
        "autolevels" => auto_levels(pm, ev.f(p, "amount"), ev.f(p, "clip"), ev.bool(p, "perchannel")),
        "legalize" => {
            let (lo, hi) = (ev.f(p, "min"), ev.f(p, "max").max(ev.f(p, "min") + 0.01));
            let l = if ev.str(p, "mode") == "clip" { lut1(|v| v.clamp(lo, hi)) } else { lut1(|v| lo + v * (hi - lo)) };
            map_lut(pm, &l, &l, &l);
        }
        _ => return false,
    }
    true
}

/// Per-frame histogram stretch (alpha-weighted, unpremultiplied).
fn auto_levels(pm: &mut Pixmap, amount: f32, clip: f32, per_channel: bool) {
    if amount <= 0.0 {
        return;
    }
    let w = pm.width() as usize;
    let hist = pm
        .data()
        .par_chunks(w * 4)
        .map(|row| {
            let mut h = [[0u32; 256]; 4];
            for px in row.chunks_exact(4) {
                let a = px[3] as u32;
                if a < 16 {
                    continue;
                }
                let un = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as usize;
                let (r, g, b) = (un(px[0]), un(px[1]), un(px[2]));
                h[0][r] += 1;
                h[1][g] += 1;
                h[2][b] += 1;
                h[3][((r * 54 + g * 183 + b * 19) >> 8).min(255)] += 1;
            }
            h
        })
        .reduce(
            || [[0u32; 256]; 4],
            |mut a, b| {
                for c in 0..4 {
                    for i in 0..256 {
                        a[c][i] += b[c][i];
                    }
                }
                a
            },
        );
    let range = |h: &[u32; 256]| -> Option<(f32, f32)> {
        let n: u64 = h.iter().map(|&v| v as u64).sum();
        if n == 0 {
            return None;
        }
        let cut = (n as f64 * clip.clamp(0.0, 0.2) as f64) as u64;
        let (mut acc, mut lo) = (0u64, 0usize);
        while lo < 255 && acc + h[lo] as u64 <= cut {
            acc += h[lo] as u64;
            lo += 1;
        }
        let (mut acc, mut hi) = (0u64, 255usize);
        while hi > 0 && acc + h[hi] as u64 <= cut {
            acc += h[hi] as u64;
            hi -= 1;
        }
        (hi > lo + 8).then_some((lo as f32 / 255.0, hi as f32 / 255.0))
    };
    let lutf = |r: Option<(f32, f32)>| match r {
        Some((lo, hi)) => lut1(|v| v + (((v - lo) / (hi - lo)) - v) * amount),
        None => lut1(|v| v),
    };
    if per_channel {
        map_lut(pm, &lutf(range(&hist[0])), &lutf(range(&hist[1])), &lutf(range(&hist[2])));
    } else {
        let l = lutf(range(&hist[3]));
        map_lut(pm, &l, &l, &l);
    }
}
