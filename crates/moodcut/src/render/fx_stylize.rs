//! Stylize, generate, matte and transition effects: find edges, emboss,
//! halftone, strobe, bevel, inner shadow, outer glow, four-color gradient,
//! checkerboard, grid, fractal noise, lens flare, matte choker, spill
//! suppressor, and the After Effects transition effects (linear / radial /
//! iris wipe, block dissolve, venetian blinds, gradient wipe) whose completion
//! is a keyframable parameter.

use super::effects::{dilate_alpha, hash2, luma, smoothstep, FxCtx};
use super::eval::Ev;
use super::noise::{fbm, Kind};
use super::pixel::{self, blur_mask, new_pixmap, sample, src_over, LayerBuf};
use crate::model::project::Props;
use crate::model::value::Color;
use rayon::prelude::*;
use resvg::tiny_skia::Pixmap;

/// Unpremultiplied 0..1 color of a pixel.
#[inline]
fn straight(px: &[u8]) -> (f32, f32, f32, f32) {
    let a = px[3] as f32 / 255.0;
    if a <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (px[0] as f32 / 255.0 / a, px[1] as f32 / 255.0 / a, px[2] as f32 / 255.0 / a, a)
}

#[inline]
fn put(px: &mut [u8], r: f32, g: f32, b: f32, a: f32) {
    let a = a.clamp(0.0, 1.0);
    px[0] = (r.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
    px[1] = (g.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
    px[2] = (b.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
    px[3] = (a * 255.0 + 0.5) as u8;
}

/// Mixes a generated color into the layer inside its alpha (like a fill), in parallel.
fn paint(pm: &mut Pixmap, ox: i32, oy: i32, f: impl Fn(f32, f32) -> Option<(Color, f32)> + Sync) {
    let w = pm.width() as usize;
    pm.data_mut().par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let gy = (y as i32 + oy) as f32 + 0.5;
        for x in 0..w {
            let px = &mut row[x * 4..x * 4 + 4];
            if px[3] == 0 {
                continue;
            }
            let Some((c, k)) = f((x as i32 + ox) as f32 + 0.5, gy) else { continue };
            let k = (k * c.a).clamp(0.0, 1.0);
            if k <= 0.0 {
                continue;
            }
            let (r, g, b, a) = straight(px);
            put(px, r + (c.r - r) * k, g + (c.g - g) * k, b + (c.b - b) * k, a);
        }
    });
}

/// Multiplies the layer by a 0..1 matte computed per output pixel (frame coords).
fn matte(pm: &mut Pixmap, ox: i32, oy: i32, m: impl Fn(f32, f32, &[u8]) -> f32 + Sync) {
    let w = pm.width() as usize;
    pm.data_mut().par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let gy = (y as i32 + oy) as f32 + 0.5;
        for x in 0..w {
            let px = &mut row[x * 4..x * 4 + 4];
            if px[3] == 0 {
                continue;
            }
            let k = m((x as i32 + ox) as f32 + 0.5, gy, px).clamp(0.0, 1.0);
            if k < 1.0 {
                for c in px.iter_mut() {
                    *c = (*c as f32 * k + 0.5) as u8;
                }
            }
        }
    });
}

/// Feathered step for wipes: 0 below `edge`, 1 above, over `f`.
#[inline]
fn soft_step(edge: f32, f: f32, v: f32) -> f32 {
    if f <= 1e-4 {
        if v >= edge {
            1.0
        } else {
            0.0
        }
    } else {
        smoothstep(edge - f / 2.0, edge + f / 2.0, v)
    }
}

fn alpha_of(pm: &Pixmap) -> Vec<u8> {
    pm.data().chunks_exact(4).map(|p| p[3]).collect()
}

/// Applies a stylize/generate/matte/transition effect. Returns false when
/// `name` isn't one of this module's.
pub fn apply(name: &str, ev: &Ev, p: &Props, lb: &mut LayerBuf, cx: &FxCtx) -> bool {
    let s = cx.scale;
    let (ox, oy) = (lb.x, lb.y);
    let (ow, oh) = (cx.out_w as f32, cx.out_h as f32);
    let pm = lb.pm.make_mut();
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let wu = w as usize;
    match name {
        "findedges" | "emboss" => {
            let src = pm.data().to_vec();
            let lum = |x: i32, y: i32| -> f32 {
                let i = ((y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) * 4) as usize;
                luma(src[i] as f32, src[i + 1] as f32, src[i + 2] as f32) / 255.0
            };
            let amt = ev.f(p, "amount");
            let edges = name == "findedges";
            let (inv, strength) = (ev.bool(p, "invert"), ev.f(p, "strength"));
            let rel = ev.f(p, "relief") * s;
            let (dx, dy) = {
                let a = ev.f(p, "angle").to_radians();
                (a.sin() * rel.max(0.5), -a.cos() * rel.max(0.5))
            };
            let keep = ev.bool(p, "color");
            pm.data_mut().par_chunks_mut(wu * 4).enumerate().for_each(|(y, row)| {
                let y = y as i32;
                for x in 0..w {
                    let o = &mut row[x as usize * 4..x as usize * 4 + 4];
                    if o[3] == 0 {
                        continue;
                    }
                    let (r, g, b, a) = straight(o);
                    let (nr, ng, nb) = if edges {
                        let gx = lum(x + 1, y - 1) + 2.0 * lum(x + 1, y) + lum(x + 1, y + 1) - lum(x - 1, y - 1) - 2.0 * lum(x - 1, y) - lum(x - 1, y + 1);
                        let gy = lum(x - 1, y + 1) + 2.0 * lum(x, y + 1) + lum(x + 1, y + 1) - lum(x - 1, y - 1) - 2.0 * lum(x, y - 1) - lum(x + 1, y - 1);
                        let m = ((gx * gx + gy * gy).sqrt() * strength).min(1.0);
                        let v = if inv { 1.0 - m } else { m };
                        (v, v, v)
                    } else {
                        let sx = |k: f32| (x as f32 + dx * k).round() as i32;
                        let sy = |k: f32| (y as f32 + dy * k).round() as i32;
                        let d = (lum(sx(-1.0), sy(-1.0)) - lum(sx(1.0), sy(1.0))) * 2.0;
                        if keep {
                            (r + d, g + d, b + d)
                        } else {
                            (0.5 + d, 0.5 + d, 0.5 + d)
                        }
                    };
                    put(o, r + (nr - r) * amt, g + (ng - g) * amt, b + (nb - b) * amt, a);
                }
            });
        }
        "halftone" => {
            let cs = (ev.f(p, "size") * s).max(2.0);
            let ang = ev.f(p, "angle").to_radians();
            let mono = ev.bool(p, "mono");
            let amt = ev.f(p, "amount");
            let src = pm.data().to_vec();
            // ink coverage of one screen at pixel (gx, gy) for a channel value
            let dot = move |gx: f32, gy: f32, a: f32, val: &dyn Fn(f32, f32) -> f32| -> f32 {
                let (sn, c) = a.sin_cos();
                let (u, v) = (gx * c + gy * sn, -gx * sn + gy * c);
                let (cu, cv) = (((u / cs).floor() + 0.5) * cs, ((v / cs).floor() + 0.5) * cs);
                let (px, py) = (cu * c - cv * sn, cu * sn + cv * c);
                let r = cs * 0.5 * 1.42 * val(px, py).clamp(0.0, 1.0).sqrt();
                let d = ((u - cu).powi(2) + (v - cv).powi(2)).sqrt();
                1.0 - smoothstep(r - 0.75, r + 0.75, d)
            };
            pm.data_mut().par_chunks_mut(wu * 4).enumerate().for_each(|(y, row)| {
                for x in 0..wu {
                    let o = &mut row[x * 4..x * 4 + 4];
                    if o[3] == 0 {
                        continue;
                    }
                    let (r, g, b, a) = straight(o);
                    let (gx, gy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let at = |px: f32, py: f32, ch: usize| -> f32 {
                        let v = sample(&src, w, h, px, py);
                        if v[3] <= 0.0 {
                            return 0.0;
                        }
                        if ch == 3 {
                            luma(v[0], v[1], v[2]) / v[3]
                        } else {
                            v[ch] / v[3]
                        }
                    };
                    let (nr, ng, nb) = if mono {
                        let ink = dot(gx, gy, ang, &|px, py| 1.0 - at(px, py, 3));
                        (1.0 - ink, 1.0 - ink, 1.0 - ink)
                    } else {
                        (dot(gx, gy, ang + 0.26, &|px, py| at(px, py, 0)), dot(gx, gy, ang + 1.31, &|px, py| at(px, py, 1)), dot(gx, gy, ang, &|px, py| at(px, py, 2)))
                    };
                    put(o, r + (nr - r) * amt, g + (ng - g) * amt, b + (nb - b) * amt, a);
                }
            });
        }
        "strobe" => {
            let ph = (ev.t * ev.num(p, "freq")).rem_euclid(1.0) as f32;
            if ph >= ev.f(p, "duty") {
                return true;
            }
            let amt = ev.f(p, "amount");
            if ev.str(p, "mode") == "hide" {
                let n = (w * h) as usize;
                pixel::apply_alpha_mask(pm, &vec![((1.0 - amt) * 255.0) as u8; n]);
            } else {
                let c = ev.color(p, "color");
                paint(pm, ox, oy, |_, _| Some((c, amt)));
            }
        }
        "bevel" => {
            let depth = (ev.f(p, "depth") * s).max(0.5);
            let a0 = alpha_of(pm);
            let mut bl = a0.clone();
            blur_mask(&mut bl, wu, h as usize, depth / 2.0);
            let ang = ev.f(p, "angle").to_radians();
            let (lx, ly) = (-ang.sin(), ang.cos());
            let inten = ev.f(p, "intensity");
            let (lc, dc) = (ev.color(p, "light"), ev.color(p, "dark"));
            let k = depth * 1.4 / 255.0;
            pm.data_mut().par_chunks_mut(wu * 4).enumerate().for_each(|(y, row)| {
                for x in 0..wu {
                    let o = &mut row[x * 4..x * 4 + 4];
                    if o[3] == 0 {
                        continue;
                    }
                    let at = |xx: i32, yy: i32| bl[(yy.clamp(0, h - 1) * w + xx.clamp(0, w - 1)) as usize] as f32;
                    let (xi, yi) = (x as i32, y as i32);
                    let gx = (at(xi + 1, yi) - at(xi - 1, yi)) / 2.0;
                    let gy = (at(xi, yi + 1) - at(xi, yi - 1)) / 2.0;
                    let shade = (-(gx * lx + gy * ly) * k).clamp(-1.0, 1.0) * inten;
                    let (r, g, b, a) = straight(o);
                    let (c, t) = if shade >= 0.0 { (lc, shade * lc.a) } else { (dc, -shade * dc.a) };
                    put(o, r + (c.r - r) * t, g + (c.g - g) * t, b + (c.b - b) * t, a);
                }
            });
        }
        "innershadow" => {
            let c = ev.color(p, "color");
            let op = ev.f(p, "opacity") * c.a;
            let ang = ev.f(p, "angle").to_radians();
            let dist = ev.f(p, "distance") * s;
            let a0 = alpha_of(pm);
            let (dx, dy) = ((ang.sin() * dist).round() as i32, (-ang.cos() * dist).round() as i32);
            // the inverse alpha, offset and blurred, seen through the layer
            let mut m: Vec<u8> = (0..(w * h) as usize)
                .map(|i| {
                    let (x, y) = (i as i32 % w - dx, i as i32 / w - dy);
                    if x < 0 || y < 0 || x >= w || y >= h {
                        255
                    } else {
                        255 - a0[(y * w + x) as usize]
                    }
                })
                .collect();
            blur_mask(&mut m, wu, h as usize, ev.f(p, "blur") * s / 2.0);
            pm.data_mut().par_chunks_mut(4).zip(m.par_iter()).for_each(|(o, &mv)| {
                if o[3] == 0 {
                    return;
                }
                let k = mv as f32 / 255.0 * op;
                let (r, g, b, a) = straight(o);
                put(o, r + (c.r - r) * k, g + (c.g - g) * k, b + (c.b - b) * k, a);
            });
        }
        "outerglow" => {
            let c = ev.color(p, "color");
            let size = ev.f(p, "size") * s;
            let a0 = alpha_of(pm);
            let mut m = dilate_alpha(&a0, wu, h as usize, (ev.f(p, "spread") * size).round() as usize);
            blur_mask(&mut m, wu, h as usize, size / 2.0);
            let op = ev.f(p, "opacity") * c.a;
            let [cr, cg, cb, _] = c.to_u8();
            let mut glow = new_pixmap(w as u32, h as u32);
            glow.data_mut().par_chunks_mut(4).zip(m.par_iter()).for_each(|(d, &mv)| {
                let a = (mv as f32 * op).min(255.0);
                d[0] = (cr as f32 * a / 255.0) as u8;
                d[1] = (cg as f32 * a / 255.0) as u8;
                d[2] = (cb as f32 * a / 255.0) as u8;
                d[3] = a as u8;
            });
            src_over(&mut glow, pm, 0, 0, 1.0);
            *pm = glow;
        }
        "fourcolor" => {
            let (tl, tr, bl, br) = (ev.color(p, "tl"), ev.color(p, "tr"), ev.color(p, "bl"), ev.color(p, "br"));
            let amt = ev.f(p, "amount");
            let (fw, fh) = (w as f32, h as f32);
            paint(pm, ox, oy, |gx, gy| {
                let (u, v) = (((gx - ox as f32) / fw).clamp(0.0, 1.0), ((gy - oy as f32) / fh).clamp(0.0, 1.0));
                Some((tl.lerp(&tr, u).lerp(&bl.lerp(&br, u), v), amt))
            });
        }
        "checker" => {
            let size = (ev.f(p, "size") * s).max(1.0);
            let off = ev.vec(p, "offset");
            let (c1, c2) = (ev.color(p, "c1"), ev.color(p, "c2"));
            let amt = ev.f(p, "amount");
            paint(pm, ox, oy, |gx, gy| {
                let (i, j) = (((gx - off.0 as f32 * s) / size).floor() as i64, ((gy - off.1 as f32 * s) / size).floor() as i64);
                Some((if (i + j).rem_euclid(2) == 0 { c1 } else { c2 }, amt))
            });
        }
        "grid" => {
            let sp = (ev.f(p, "spacing") * s).max(2.0);
            let lw = (ev.f(p, "width") * s).max(0.0);
            let off = ev.vec(p, "offset");
            let c = ev.color(p, "color");
            paint(pm, ox, oy, |gx, gy| {
                let d = |v: f32| {
                    let m = v.rem_euclid(sp);
                    m.min(sp - m)
                };
                let e = d(gx - off.0 as f32 * s).min(d(gy - off.1 as f32 * s));
                let k = 1.0 - smoothstep(lw / 2.0 - 0.6, lw / 2.0 + 0.6, e);
                (k > 0.0).then_some((c, k))
            });
        }
        "fractal" => {
            let scale = (ev.f(p, "scale") * s).max(1.0);
            let oct = ev.f(p, "complexity");
            let (con, bri) = (ev.f(p, "contrast"), ev.f(p, "brightness"));
            let z = (ev.t * ev.num(p, "speed")) as f32;
            let kind = match ev.str(p, "type").as_str() {
                "turbulent" => Kind::Turbulent,
                "ridged" => Kind::Ridged,
                _ => Kind::Smooth,
            };
            let (c1, c2) = (ev.color(p, "c1"), ev.color(p, "c2"));
            let off = ev.vec(p, "offset");
            let amt = ev.f(p, "amount");
            let seed = cx.seed as u32;
            // evaluate on a grid as fine as the smallest octave needs, then interpolate
            let step = (scale / 2f32.powf(oct.clamp(1.0, 10.0)) / 2.0).clamp(1.0, 4.0);
            let (gw, gh) = ((w as f32 / step).ceil() as usize + 2, (h as f32 / step).ceil() as usize + 2);
            let grid: Vec<f32> = (0..gw * gh)
                .into_par_iter()
                .map(|i| {
                    let gx = (i % gw) as f32 * step + ox as f32 - off.0 as f32 * s;
                    let gy = (i / gw) as f32 * step + oy as f32 - off.1 as f32 * s;
                    ((fbm(gx / scale, gy / scale, z, oct, kind, seed) - 0.5) * con + 0.5 + bri).clamp(0.0, 1.0)
                })
                .collect();
            paint(pm, ox, oy, |gx, gy| {
                let (fx, fy) = ((gx - ox as f32 - 0.5) / step, (gy - oy as f32 - 0.5) / step);
                let (ix, iy) = ((fx.max(0.0) as usize).min(gw - 2), (fy.max(0.0) as usize).min(gh - 2));
                let (tx, ty) = ((fx - ix as f32).clamp(0.0, 1.0), (fy - iy as f32).clamp(0.0, 1.0));
                let g = |a: usize, b: usize| grid[b * gw + a];
                let v = (g(ix, iy) * (1.0 - tx) + g(ix + 1, iy) * tx) * (1.0 - ty) + (g(ix, iy + 1) * (1.0 - tx) + g(ix + 1, iy + 1) * tx) * ty;
                Some((c1.lerp(&c2, v), amt))
            });
        }
        "lensflare" => {
            let pos = ev.vec(p, "pos");
            let (fx, fy) = (pos.0 as f32 * s, pos.1 as f32 * s);
            let br = ev.f(p, "brightness");
            let tint = ev.color(p, "color");
            let rays = ev.f(p, "rays").round();
            let streak = ev.f(p, "streak");
            let ghosts = ev.f(p, "ghosts");
            let diag = (ow * ow + oh * oh).sqrt();
            let (cxf, cyf) = (ow / 2.0, oh / 2.0);
            let gh: Vec<(f32, f32, f32, f32)> = [(0.45, 0.06, 0.5), (0.8, 0.025, 0.9), (1.15, 0.09, 0.35), (1.6, 0.04, 0.6)]
                .iter()
                .map(|(f, r, k)| (cxf + (cxf - fx) * f, cyf + (cyf - fy) * f, r * diag, k * ghosts * 0.18))
                .collect();
            pm.data_mut().par_chunks_mut(wu * 4).enumerate().for_each(|(y, row)| {
                let gy = (y as i32 + oy) as f32 + 0.5;
                for x in 0..wu {
                    let o = &mut row[x * 4..x * 4 + 4];
                    if o[3] == 0 {
                        continue;
                    }
                    let gx = (x as i32 + ox) as f32 + 0.5;
                    let (dx, dy) = (gx - fx, gy - fy);
                    let d = (dx * dx + dy * dy).sqrt() / diag;
                    let mut i = (-(d * d) / (2.0 * 0.018f32.powi(2))).exp() + 0.35 * (-d / 0.12).exp();
                    if rays > 0.0 {
                        let th = dy.atan2(dx);
                        i += 0.35 * (0.5 + 0.5 * (rays * th).cos()).powi(12) * (-d / 0.22).exp();
                    }
                    i += streak * 0.5 * (-(dy.abs() / diag) / 0.0035).exp() * (-(dx.abs() / diag) / (0.35 * streak.max(0.05))).exp();
                    for &(gxx, gyy, r, k) in &gh {
                        let dd = ((gx - gxx).powi(2) + (gy - gyy).powi(2)).sqrt();
                        i += k * (1.0 - smoothstep(r * 0.6, r, dd));
                    }
                    let i = i * br;
                    if i <= 0.002 {
                        continue;
                    }
                    let (r, g, b, a) = straight(o);
                    let scr = |c: f32, t: f32| 1.0 - (1.0 - c) * (1.0 - (t * i).min(1.0));
                    put(o, scr(r, tint.r), scr(g, tint.g), scr(b, tint.b), a);
                }
            });
        }
        "choke" => {
            let amt = ev.f(p, "amount") * s;
            let r = amt.abs().round() as usize;
            let a0 = alpha_of(pm);
            let mut na = if amt >= 0.0 {
                let inv: Vec<u8> = a0.iter().map(|v| 255 - v).collect();
                dilate_alpha(&inv, wu, h as usize, r).into_iter().map(|v| 255 - v).collect()
            } else {
                dilate_alpha(&a0, wu, h as usize, r)
            };
            blur_mask(&mut na, wu, h as usize, ev.f(p, "soft") * s / 2.0);
            if amt < 0.0 && r > 0 {
                // spread: grow the colors with the matte
                let mut chans: Vec<Vec<u8>> = (0..3).map(|c| pm.data().chunks_exact(4).map(|px| px[c]).collect()).collect();
                for ch in chans.iter_mut() {
                    *ch = dilate_alpha(ch, wu, h as usize, r);
                }
                let amax = dilate_alpha(&a0, wu, h as usize, r);
                pm.data_mut().par_chunks_mut(4).enumerate().for_each(|(i, o)| {
                    let k = na[i] as f32 / amax[i].max(1) as f32;
                    for c in 0..3 {
                        o[c] = (chans[c][i] as f32 * k).min(na[i] as f32) as u8;
                    }
                    o[3] = na[i];
                });
            } else {
                pm.data_mut().par_chunks_mut(4).enumerate().for_each(|(i, o)| {
                    if o[3] == 0 {
                        return;
                    }
                    let k = na[i].min(o[3]) as f32 / o[3] as f32;
                    for c in o.iter_mut() {
                        *c = (*c as f32 * k + 0.5) as u8;
                    }
                });
            }
        }
        "spill" => {
            let kc = ev.color(p, "color");
            let amt = ev.f(p, "amount");
            let dom = if kc.g >= kc.r && kc.g >= kc.b { 1 } else if kc.b >= kc.r { 2 } else { 0 };
            pixel::map_rgb(pm, |r, g, b| {
                let mut c = [r, g, b];
                let other = match dom {
                    0 => g.max(b),
                    1 => r.max(b),
                    _ => r.max(g),
                };
                if c[dom] > other {
                    c[dom] -= (c[dom] - other) * amt;
                }
                (c[0], c[1], c[2])
            });
        }
        "linearwipe" => {
            let pr = ev.f(p, "completion") / 100.0;
            let f = ev.f(p, "feather") * s;
            let (sn, cs) = ev.f(p, "angle").to_radians().sin_cos();
            let (dx, dy) = (sn, -cs);
            let e = (dx.abs() * ow + dy.abs() * oh) / 2.0;
            let edge = -e - f / 2.0 + pr * (2.0 * e + f);
            matte(pm, ox, oy, |gx, gy, _| soft_step(edge, f, (gx - ow / 2.0) * dx + (gy - oh / 2.0) * dy));
        }
        "radialwipe" => {
            let pr = ev.f(p, "completion") / 100.0;
            let (ccx, ccy) = (ev.f(p, "cx") * ow, ev.f(p, "cy") * oh);
            let start = ev.f(p, "start") / 360.0;
            let f = ev.f(p, "feather") / 360.0;
            let dir = ev.str(p, "dir");
            matte(pm, ox, oy, |gx, gy, _| {
                let a = ((gx - ccx).atan2(-(gy - ccy)) / std::f32::consts::TAU - start).rem_euclid(1.0);
                let u = match dir.as_str() {
                    "ccw" => 1.0 - a,
                    "both" => a.min(1.0 - a) * 2.0,
                    _ => a,
                };
                soft_step(-f / 2.0 + pr * (1.0 + f), f, u)
            });
        }
        "iriswipe" => {
            let pr = ev.f(p, "completion") / 100.0;
            let (ccx, ccy) = (ev.f(p, "cx") * ow, ev.f(p, "cy") * oh);
            let f = ev.f(p, "feather") * s;
            let far = [(0.0, 0.0), (ow, 0.0), (0.0, oh), (ow, oh)].iter().map(|(x, y): &(f32, f32)| ((x - ccx).powi(2) + (y - ccy).powi(2)).sqrt()).fold(0.0, f32::max) + f;
            let inv = ev.bool(p, "invert");
            matte(pm, ox, oy, |gx, gy, _| {
                let d = ((gx - ccx).powi(2) + (gy - ccy).powi(2)).sqrt();
                if inv {
                    soft_step(-f / 2.0 + pr * (far + f), f, d)
                } else {
                    1.0 - soft_step(far * (1.0 - pr) - f / 2.0, f, d)
                }
            });
        }
        "blockdissolve" => {
            let pr = ev.f(p, "completion") / 100.0;
            let size = (ev.f(p, "size") * s).max(1.0);
            let soft = ev.f(p, "soft");
            let seed = cx.seed;
            matte(pm, ox, oy, |gx, gy, _| {
                let r = (hash2((gx / size).floor() as i32 as u32, (gy / size).floor() as i32 as u32, seed) & 0xffff) as f32 / 65536.0;
                soft_step(-soft / 2.0 + pr * (1.0 + soft), soft, r)
            });
        }
        "venetianblinds" => {
            let pr = ev.f(p, "completion") / 100.0;
            let sw = (ev.f(p, "width") * s).max(1.0);
            let f = ev.f(p, "feather") * s / sw;
            let (sn, cs) = ev.f(p, "angle").to_radians().sin_cos();
            matte(pm, ox, oy, |gx, gy, _| {
                let v = (gx * sn + gy * cs) / sw;
                soft_step(-f / 2.0 + pr * (1.0 + f), f, v.rem_euclid(1.0))
            });
        }
        "gradientwipe" => {
            let pr = ev.f(p, "completion") / 100.0;
            let soft = ev.f(p, "soft");
            let inv = ev.bool(p, "invert");
            matte(pm, ox, oy, |_, _, px| {
                let (r, g, b, _) = straight(px);
                let l = luma(r, g, b);
                soft_step(-soft / 2.0 + pr * (1.0 + soft), soft, if inv { 1.0 - l } else { l })
            });
        }
        _ => return false,
    }
    true
}
