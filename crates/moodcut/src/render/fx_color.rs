//! Color effects: Lumetri color wheels, HSL secondary and hue/saturation
//! curves, channel mixer, leave / change color, photo filter, tritone, auto
//! levels, the video limiter, and the Resolve-style primaries, log wheels, HDR
//! zones, color warper, color slice and color space transform. Per-pixel color functions are baked into
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
        "softclip" => {
            let (k, st) = (ev.f(p, "knee").clamp(0.3, 0.999), ev.f(p, "strength").clamp(0.0, 1.0));
            if st <= 0.0 {
                return true;
            }
            // above the knee, values approach (not reach) the old white point
            let l = lut1(move |x| {
                let y = if x <= k { x } else { k + (1.0 - k) * ((x - k) / (1.0 - k) * 1.3).tanh() / 1.3f32.tanh() * 0.97 };
                x + (y - x) * st
            });
            map_lut(pm, &l, &l, &l);
        }
        "filmcurve" => {
            let (c, toe, sh, piv) = (ev.f(p, "contrast"), ev.f(p, "toe"), ev.f(p, "shoulder"), ev.f(p, "pivot").clamp(0.1, 0.9));
            let g = 4.0 + 10.0 * c;
            let sig = move |x: f32| 1.0 / (1.0 + (-g * (x - piv)).exp());
            let (lo, hi) = (sig(0.0), sig(1.0));
            let l = lut1(move |x| {
                let s = (sig(x) - lo) / (hi - lo).max(1e-6);
                let y = x + (s - x) * c;
                let y = y + toe * 0.25 * (1.0 - y).powi(4);
                y - sh * 0.25 * y.powi(4)
            });
            map_lut(pm, &l, &l, &l);
        }
        "colorwheels" => {
            let (s, m, h, o) = (wheel(ev.color(p, "shadows")), wheel(ev.color(p, "midtones")), wheel(ev.color(p, "highlights")), wheel(ev.color(p, "offset")));
            let (sl, ml, hl, ol) = (ev.f(p, "shadowsL") / 100.0, ev.f(p, "midtonesL") / 100.0, ev.f(p, "highlightsL") / 100.0, ev.f(p, "offsetL") / 100.0);
            let sat = ev.f(p, "sat") / 100.0;
            let (sp, hp) = (ev.f(p, "shadowPivot").clamp(0.0, 1.0), ev.f(p, "highlightPivot").clamp(0.0, 1.0));
            let wheels = !(s == [0.0; 3] && m == [0.0; 3] && h == [0.0; 3] && o == [0.0; 3] && sl == 0.0 && ml == 0.0 && hl == 0.0 && ol == 0.0);
            if !wheels && (sat - 1.0).abs() < 1e-4 {
                return true;
            }
            // Lift / gamma / gain / offset per channel (Resolve primary controls),
            // with optional ranges: lift fades out by the shadow pivot, gain
            // fades in above the highlight pivot.
            if wheels {
                let ch = |c: usize| {
                    let lift = s[c] * 0.6 + sl * 0.25;
                    let gamma = (1.0 + m[c] * 1.6 + ml * 0.6).clamp(0.2, 5.0);
                    let gain = (1.0 + h[c] * 1.2 + hl * 0.5).max(0.0);
                    let offset = o[c] * 0.6 + ol * 0.25;
                    lut1(move |x| {
                        let lw = if sp > 0.001 { (1.0 - x / sp).max(0.0) } else { 1.0 - x };
                        let v = x + lift * lw;
                        let v = if hp < 0.999 { v + (gain * v - v) * smoothstep(hp, 1.0, x) } else { gain * v };
                        (v + offset).max(0.0).powf(1.0 / gamma)
                    })
                };
                map_lut(pm, &ch(0), &ch(1), &ch(2));
            }
            if (sat - 1.0).abs() >= 1e-4 {
                map_rgb(pm, move |r, g, b| {
                    let l = luma(r, g, b);
                    (l + (r - l) * sat, l + (g - l) * sat, l + (b - l) * sat)
                });
            }
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
            let specs = ["hueSat", "hueHue", "hueLuma", "lumaSat", "satSat", "satLuma"].map(|k| ev.str(p, k));
            if specs.iter().all(|s| s.trim().is_empty()) {
                return true;
            }
            let hs = bump_table(&specs[0], true, 0.08);
            let hh = bump_table(&specs[1], true, 0.08);
            let hl = bump_table(&specs[2], true, 0.08);
            let ls = bump_table(&specs[3], false, 0.15);
            let sat_sat = bump_table(&specs[4], false, 0.15);
            let sat_luma = bump_table(&specs[5], false, 0.15);
            baked(pm, format!("huecurves:{specs:?}"), move |r, g, b| {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let s0 = s;
                let mut s = s * (1.0 + at(&hs, h)) * (1.0 + at(&ls, l)) * (1.0 + at(&sat_sat, s0));
                s = s.clamp(0.0, 1.0);
                let v = at(&hl, h) * smoothstep(0.0, 0.15, s0) + at(&sat_luma, s0);
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
        "primaries" => {
            let k = PRIMARIES.map(|n| ev.f(p, n));
            let md = ev.f(p, "midDetail");
            let neutral = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, k[17], 50.0, 50.0, k[20], 0.0, 0.0, 0.0, 0.0, 0.0];
            if k != neutral {
                baked(pm, format!("primaries:{k:?}"), primaries_fn(k));
            }
            if md.abs() > 0.01 {
                mid_detail(pm, md);
            }
        }
        "logwheels" => {
            let (s, m, h, o) = (wheel(ev.color(p, "shadow")), wheel(ev.color(p, "midtone")), wheel(ev.color(p, "highlight")), wheel(ev.color(p, "offset")));
            let lv = ["shadowL", "midtoneL", "highlightL", "offsetL"].map(|n| ev.f(p, n) / 100.0);
            let (lo, hi) = (ev.f(p, "low").clamp(0.0, 1.0), ev.f(p, "high").clamp(0.0, 1.0));
            let (ct, pv) = (ev.f(p, "contrast"), ev.f(p, "pivot"));
            if [s, m, h, o].iter().all(|w| *w == [0.0; 3]) && lv == [0.0; 4] && (ct - 1.0).abs() < 1e-6 {
                return true;
            }
            baked(pm, format!("logwheels:{s:?}{m:?}{h:?}{o:?}{lv:?}:{lo}:{hi}:{ct}:{pv}"), move |r, g, b| {
                let l = luma(r, g, b);
                // narrow, overlapping zones (Resolve's log wheels)
                let ws = 1.0 - smoothstep(lo - 0.12, lo + 0.04, l);
                let wh = smoothstep(hi - 0.04, hi + 0.12, l);
                let wm = (1.0 - ws - wh).max(0.0);
                let mut c = [r, g, b];
                for (i, v) in c.iter_mut().enumerate() {
                    *v += (s[i] * 0.3 + lv[0] * 0.2) * ws + (m[i] * 0.3 + lv[1] * 0.2) * wm + (h[i] * 0.3 + lv[2] * 0.2) * wh + o[i] * 0.3 + lv[3] * 0.2;
                    *v = (*v - pv) * ct + pv;
                }
                (c[0], c[1], c[2])
            });
        }
        "hdrwheels" => {
            const Z: [&str; 6] = ["black", "dark", "shadow", "light", "highlight", "specular"];
            let exps = Z.map(|z| ev.f(p, &format!("{z}Exp")));
            let sats = Z.map(|z| ev.f(p, &format!("{z}Sat")));
            let tints = Z.map(|z| wheel(ev.color(p, &format!("{z}Tint"))));
            let ats = Z.map(|z| ev.f(p, &format!("{z}At")));
            let (gexp, gsat, gtint, fall) = (ev.f(p, "exposure"), ev.f(p, "sat"), wheel(ev.color(p, "tint")), ev.f(p, "falloff").max(0.1));
            if exps == [0.0; 6] && sats == [1.0; 6] && tints.iter().all(|t| *t == [0.0; 3]) && gexp == 0.0 && (gsat - 1.0).abs() < 1e-6 && gtint == [0.0; 3] {
                return true;
            }
            baked(pm, format!("hdrwheels:{exps:?}{sats:?}{tints:?}{ats:?}:{gexp}:{gsat}:{gtint:?}:{fall}"), move |r, g, b| {
                // zones in stops from mid gray, on display-gamma-2.4 linear light
                let lin = [r, g, b].map(|v| v.max(0.0).powf(2.4));
                let y = (0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]).max(1e-6);
                let stops = (y / 0.18).log2();
                let (mut e, mut sat, mut tint) = (gexp, gsat, gtint);
                for i in 0..6 {
                    let wz = if i < 3 { 1.0 - smoothstep(ats[i] - fall * 0.5, ats[i] + fall * 0.5, stops) } else { smoothstep(ats[i] - fall * 0.5, ats[i] + fall * 0.5, stops) };
                    if wz <= 0.0 {
                        continue;
                    }
                    e += exps[i] * wz;
                    sat *= 1.0 + (sats[i] - 1.0) * wz;
                    for (t, z) in tint.iter_mut().zip(tints[i]) {
                        *t += z * wz;
                    }
                }
                let k = 2f32.powf(e);
                let yk = y * k;
                let out: Vec<f32> = (0..3).map(|c| ((yk + (lin[c] * k - yk) * sat) * (1.0 + tint[c] * 0.6)).max(0.0).powf(1.0 / 2.4)).collect();
                (out[0], out[1], out[2])
            });
        }
        "colorwarper" => {
            let pins = parse_pins(&ev.str(p, "pins"));
            let lp = parse_luma_pins(&ev.str(p, "luma"));
            let amt = ev.f(p, "amount").clamp(0.0, 1.0);
            if (pins.is_empty() && lp.is_empty()) || amt <= 0.0 {
                return true;
            }
            baked(pm, format!("warper:{pins:?}:{lp:?}:{amt}"), move |r, g, b| {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let (a0, b0) = polar(h, s);
                let (mut da, mut db, mut wsum) = (0.0f32, 0.0f32, 0.0f32);
                for pin in &pins {
                    let (pa, pb) = polar(pin[0], pin[1]);
                    let w = (-((a0 - pa).powi(2) + (b0 - pb).powi(2)) / (pin[4] * pin[4])).exp();
                    let (qa, qb) = polar(pin[2], pin[3]);
                    da += w * (qa - pa);
                    db += w * (qb - pb);
                    wsum += w;
                }
                if wsum > 1.0 {
                    da /= wsum;
                    db /= wsum;
                }
                let mut dl = 0.0;
                for pin in &lp {
                    let (pa, pb) = polar(pin[0], pin[1]);
                    dl += pin[2] * (-((a0 - pa).powi(2) + (b0 - pb).powi(2)) / (pin[3] * pin[3])).exp();
                }
                if da == 0.0 && db == 0.0 && dl == 0.0 {
                    return (r, g, b);
                }
                let (na, nb) = (a0 + da * amt, b0 + db * amt);
                let ns = (na * na + nb * nb).sqrt().clamp(0.0, 1.0);
                let nh = if ns > 1e-5 { (nb.atan2(na) / std::f32::consts::TAU).rem_euclid(1.0) } else { h };
                hsl_to_rgb(nh, ns, (l + dl * amt).clamp(0.0, 1.0))
            });
        }
        "colorslice" => {
            // vectors: name, hue center and half width (turns)
            const V: [(&str, f32, f32); 7] = [("red", 0.0, 0.085), ("skin", 0.07, 0.04), ("yellow", 0.165, 0.085), ("green", 0.333, 0.11), ("cyan", 0.5, 0.085), ("blue", 0.667, 0.11), ("magenta", 0.833, 0.085)];
            let vals: Vec<[f32; 5]> = V.iter().map(|(n, c, w)| [*c, *w, ev.f(p, &format!("{n}Hue")) / 360.0, ev.f(p, &format!("{n}Sat")) / 100.0, ev.f(p, &format!("{n}Den")) / 100.0]).collect();
            let (gden, gsat) = (ev.f(p, "density") / 100.0, ev.f(p, "sat") / 100.0);
            if gden == 0.0 && gsat == 0.0 && vals.iter().all(|v| v[2] == 0.0 && v[3] == 0.0 && v[4] == 0.0) {
                return true;
            }
            baked(pm, format!("colorslice:{vals:?}:{gden}:{gsat}"), move |r, g, b| {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let chroma = smoothstep(0.03, 0.18, s);
                let (mut dh, mut sm, mut den) = (0.0f32, 1.0 + gsat, gden);
                for v in &vals {
                    let d = hue_dist(h, v[0]);
                    if d >= v[1] {
                        continue;
                    }
                    let k = 0.5 * (1.0 + (std::f32::consts::PI * d / v[1]).cos()) * chroma;
                    dh += v[2] * k;
                    sm *= 1.0 + v[3] * k;
                    den += v[4] * k;
                }
                let s2 = (s * sm).clamp(0.0, 1.0);
                // density is subtractive: saturated colors get darker and richer
                let l2 = (l * (1.0 - den * 0.45 * s2)).clamp(0.0, 1.0);
                hsl_to_rgb((h + dh).rem_euclid(1.0), s2, l2)
            });
        }
        "cst" => {
            let (from, to) = (ev.str(p, "from"), ev.str(p, "to"));
            let (tm, gm, ex) = (ev.str(p, "tonemap"), ev.str(p, "gamut"), ev.f(p, "exposure"));
            if from == to && ex.abs() < 1e-6 {
                return true;
            }
            baked(pm, format!("cst:{from}:{to}:{tm}:{gm}:{ex}"), move |r, g, b| {
                let o = super::colorspace::cst(&from, &to, [r as f64, g as f64, b as f64], &tm, &gm, ex as f64);
                (o[0] as f32, o[1] as f32, o[2] as f32)
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

/// `primaries` parameters in the order [`primaries_fn`] reads them.
const PRIMARIES: [&str; 26] = [
    "liftY", "liftR", "liftG", "liftB", "gammaY", "gammaR", "gammaG", "gammaB", "gainY", "gainR", "gainG", "gainB", "offsetY", "offsetR", "offsetG", "offsetB", "contrast", "pivot", "sat", "hue", "lumMix", "temp", "tint", "boost", "shadows", "highlights",
];

/// One Resolve primary stage on a channel: lift (black point, white held),
/// gain, gamma (+ brightens), offset.
fn primary_stage(x: f32, lift: f32, gamma: f32, gain: f32, offset: f32) -> f32 {
    let v = (lift + x * (1.0 - lift)) * gain;
    let v = if gamma.abs() > 1e-6 { v.max(0.0).powf(2f32.powf(-gamma * 1.4)) } else { v };
    v + offset
}

/// The per-pixel part of `primaries` (midtone detail is spatial and runs
/// after the LUT).
fn primaries_fn(k: [f32; 26]) -> impl Fn(f32, f32, f32) -> (f32, f32, f32) + Sync {
    move |r, g, b| {
        let (ly, gy, ny, oy) = (k[0], k[4], k[8], k[12]);
        let mut c = [r, g, b];
        // master (Y) controls: per channel at lum mix 0, on luma only (color
        // kept) at lum mix 100
        if ly != 0.0 || gy != 0.0 || ny != 1.0 || oy != 0.0 {
            let mix = k[20] / 100.0;
            let l = luma(r, g, b);
            let dl = primary_stage(l, ly, gy, ny, oy) - l;
            for v in c.iter_mut() {
                let per = primary_stage(*v, ly, gy, ny, oy);
                *v = per + (*v + dl - per) * mix;
            }
        }
        for (i, v) in c.iter_mut().enumerate() {
            *v = primary_stage(*v, k[1 + i], k[5 + i], k[9 + i], k[13 + i]);
        }
        let (ct, pv) = (k[16], k[17]);
        if (ct - 1.0).abs() > 1e-6 {
            for v in c.iter_mut() {
                *v = (*v - pv) * ct + pv;
            }
        }
        let (tp, tn) = (k[21] / 4000.0, k[22] / 100.0);
        if tp != 0.0 || tn != 0.0 {
            c[0] *= 1.0 + 0.25 * tp;
            c[2] *= 1.0 - 0.25 * tp;
            c[1] *= 1.0 - 0.12 * tn;
        }
        let (sh, hi) = (k[24] / 100.0, k[25] / 100.0);
        if sh != 0.0 || hi != 0.0 {
            let l = luma(c[0], c[1], c[2]).clamp(0.0, 1.0);
            let d = sh * 0.3 * (1.0 - smoothstep(0.0, 0.55, l)).powi(2) + hi * 0.3 * smoothstep(0.45, 1.0, l).powi(2);
            for v in c.iter_mut() {
                *v += d;
            }
        }
        // hue, saturation and color boost in the B-Y / R-Y plane (luma kept)
        let (sat, hue, boost) = (k[18] / 50.0, (k[19] - 50.0) / 50.0 * std::f32::consts::PI, k[23] / 100.0);
        if (sat - 1.0).abs() > 1e-6 || hue.abs() > 1e-6 || boost != 0.0 {
            let l = luma(c[0], c[1], c[2]);
            let (mut u, mut w) = (c[2] - l, c[0] - l);
            if hue.abs() > 1e-6 {
                let (cs, sn) = (hue.cos(), hue.sin());
                (u, w) = (u * cs - w * sn, u * sn + w * cs);
            }
            let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
            let s0 = if mx > 1e-4 { ((mx - mn) / mx).clamp(0.0, 1.0) } else { 0.0 };
            let f = sat * (1.0 + boost * (1.0 - s0).powi(2));
            let (rr, bb) = (l + w * f, l + u * f);
            c = [rr, (l - 0.2126 * rr - 0.0722 * bb) / 0.7152, bb];
        }
        (c[0], c[1], c[2])
    }
}

/// Midtone detail: local contrast (the picture minus a blur of itself),
/// weighted to the midtones. Negative values soften toward the blur.
fn mid_detail(pm: &mut Pixmap, amount: f32) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let sigma = (w.max(h) as f32 / 160.0).max(1.5);
    let mut blur = pm.clone();
    super::pixel::blur_pixmap(&mut blur, sigma, 0);
    let k = amount / 100.0 * 1.5;
    pm.data_mut().par_chunks_exact_mut(4).zip(blur.data().par_chunks_exact(4)).for_each(|(px, bl)| {
        let a = px[3];
        if a == 0 {
            return;
        }
        let l = luma(px[0] as f32, px[1] as f32, px[2] as f32) / a as f32;
        let lb = luma(bl[0] as f32, bl[1] as f32, bl[2] as f32) / bl[3].max(1) as f32;
        let wm = 1.0 - (2.0 * l - 1.0).powi(2);
        let d = (l - lb) * k * wm.max(0.0) * a as f32;
        for v in px.iter_mut().take(3) {
            *v = (*v as f32 + d).round().clamp(0.0, a as f32) as u8;
        }
    });
}

/// Hue (turns) and saturation as a point in the chroma plane.
#[inline]
fn polar(h: f32, s: f32) -> (f32, f32) {
    let a = h * std::f32::consts::TAU;
    (s * a.cos(), s * a.sin())
}

/// Color warper pins 'h,s>h,s[,r]' as [h, s, h2, s2, r] (hues in turns).
fn parse_pins(spec: &str) -> Vec<[f32; 5]> {
    spec.split_whitespace()
        .filter_map(|t| {
            let (a, b) = t.split_once('>')?;
            let f: Vec<f32> = a.split(',').chain(b.split(',')).filter_map(|x| x.trim().parse().ok()).collect();
            (f.len() >= 4).then(|| [f[0].rem_euclid(360.0) / 360.0, f[1].clamp(0.0, 1.0), f[2].rem_euclid(360.0) / 360.0, f[3].clamp(0.0, 1.0), f.get(4).copied().unwrap_or(0.25).max(0.02)])
        })
        .collect()
}

/// Chroma-luma pins 'h,s>dl[,r]' as [h, s, dl, r].
fn parse_luma_pins(spec: &str) -> Vec<[f32; 4]> {
    spec.split_whitespace()
        .filter_map(|t| {
            let (a, b) = t.split_once('>')?;
            let f: Vec<f32> = a.split(',').chain(b.split(',')).filter_map(|x| x.trim().parse().ok()).collect();
            (f.len() >= 3).then(|| [f[0].rem_euclid(360.0) / 360.0, f[1].clamp(0.0, 1.0), f[2].clamp(-1.0, 1.0), f.get(3).copied().unwrap_or(0.25).max(0.02)])
        })
        .collect()
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
