//! Blur and distortion effects: tilt-shift, surface (skin) blur, lens blur,
//! the Transform effect, offset, polar coordinates, ripple, lens distortion,
//! turbulent displace and motion tile. Remaps run in parallel over rows and
//! sample bilinearly; spatial parameters are comp px scaled to the output.

use super::effects::{luma, remap, smoothstep, FxCtx};
use super::eval::Ev;
use super::noise::{fbm, Kind};
use super::pixel::{self, blur_pixmap, lerp_into, IRect, LayerBuf};
use crate::model::project::Props;
use rayon::prelude::*;
use resvg::tiny_skia::Transform;

/// Luma (0..1) of a premultiplied pixel, alpha included.
#[inline]
fn pluma(px: &[u8]) -> f32 {
    luma(px[0] as f32, px[1] as f32, px[2] as f32) / 255.0
}

/// Bilinear sample with wrap-around.
fn sample_wrap(src: &[u8], w: i32, h: i32, x: f32, y: f32) -> [f32; 4] {
    let (x, y) = (x - 0.5, y - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let px = |xx: i32, yy: i32| {
        let i = ((yy.rem_euclid(h) * w + xx.rem_euclid(w)) * 4) as usize;
        [src[i] as f32, src[i + 1] as f32, src[i + 2] as f32, src[i + 3] as f32]
    };
    let (xi, yi) = (x0 as i32, y0 as i32);
    let (a, b, c, d) = (px(xi, yi), px(xi + 1, yi), px(xi, yi + 1), px(xi + 1, yi + 1));
    let mut o = [0f32; 4];
    for k in 0..4 {
        o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
    }
    o
}

/// Grows the layer buffer to cover the whole frame (effects that move content outside its box).
fn to_frame(lb: &mut LayerBuf, cx: &FxCtx) {
    let frame = IRect { x: 0, y: 0, w: cx.out_w, h: cx.out_h };
    lb.expand_to(frame.union(&lb.rect().intersect(&frame)));
}

/// Applies a blur/distort effect. Returns false when `name` isn't one of this module's.
pub fn apply(name: &str, ev: &Ev, p: &Props, lb: &mut LayerBuf, cx: &FxCtx) -> bool {
    let s = cx.scale;
    let (ow, oh) = (cx.out_w as f32, cx.out_h as f32);
    match name {
        "tiltshift" => {
            let (ox, oy) = (lb.x as f32, lb.y as f32);
            let pm = lb.pm.make_mut();
            let mut bl = pm.clone();
            blur_pixmap(&mut bl, ev.f(p, "radius") * s / 2.0, 0);
            let (c, w, soft) = (ev.f(p, "center") * oh, ev.f(p, "width") * oh, ev.f(p, "soft") * oh);
            let (sn, cs) = ev.f(p, "angle").to_radians().sin_cos();
            lerp_into(pm, &bl, |x, y, _| {
                let (dx, dy) = (x as f32 + ox + 0.5 - ow / 2.0, y as f32 + oy + 0.5 - c);
                smoothstep(w, w + soft, (-dx * sn + dy * cs).abs())
            });
        }
        "smooth" => {
            let pm = lb.pm.make_mut();
            let mut bl = pm.clone();
            blur_pixmap(&mut bl, ev.f(p, "radius") * s / 2.0, 0);
            let thr = ev.f(p, "threshold").max(1e-3);
            let amt = ev.f(p, "amount");
            let src = pm.data().to_vec();
            let w = pm.width() as usize;
            lerp_into(pm, &bl, |x, y, bp| {
                let i = (y as usize * w + x as usize) * 4;
                let d = pluma(&src[i..i + 4]) - pluma(bp);
                amt * (-(d * d) / (2.0 * thr * thr)).exp()
            });
        }
        "lensblur" => {
            let pm = lb.pm.make_mut();
            let r = ev.f(p, "radius") * s / 2.0;
            let (bright, thr) = (ev.f(p, "bright"), ev.f(p, "threshold"));
            let mut hl = pm.clone();
            hl.data_mut().par_chunks_mut(4).for_each(|px| {
                let k = smoothstep(thr, 1.0, pluma(px) / (px[3].max(1) as f32 / 255.0)).powi(2);
                for c in px.iter_mut() {
                    *c = (*c as f32 * k) as u8;
                }
            });
            blur_pixmap(pm, r, 0);
            blur_pixmap(&mut hl, r * 0.8, 0);
            let hd = hl.data();
            pm.data_mut().par_chunks_mut(4).zip(hd.par_chunks(4)).for_each(|(o, h)| {
                for c in 0..3 {
                    o[c] = (o[c] as f32 + h[c] as f32 * bright).min(o[3] as f32) as u8;
                }
            });
        }
        "transform" => {
            let (w0, h0) = (lb.w() as f32, lb.h() as f32);
            let an = ev.vec(p, "anchor");
            let (ax, ay) = (lb.x as f32 + an.0 as f32 * w0, lb.y as f32 + an.1 as f32 * h0);
            to_frame(lb, cx);
            let (bx, by) = (lb.x as f32, lb.y as f32);
            let off = ev.vec(p, "offset");
            let sc = ev.vec(p, "scale");
            let m = Transform::from_translate(ax - bx + off.0 as f32 * s, ay - by + off.1 as f32 * s)
                .pre_rotate(ev.f(p, "rot"))
                .pre_concat(Transform::from_skew(ev.f(p, "skew").to_radians().tan(), 0.0))
                .pre_scale(sc.0 as f32, sc.1 as f32)
                .pre_translate(-(ax - bx), -(ay - by));
            let pm = lb.pm.make_mut();
            let src = pm.clone();
            pm.fill(resvg::tiny_skia::Color::TRANSPARENT);
            pixel::draw_affine(pm, &src, m, false);
            let op = ev.f(p, "opacity").clamp(0.0, 1.0);
            if op < 0.999 {
                let n = (pm.width() * pm.height()) as usize;
                pixel::apply_alpha_mask(pm, &vec![(op * 255.0) as u8; n]);
            }
        }
        "offset" => {
            let pm = lb.pm.make_mut();
            let (w, h) = (pm.width() as i32, pm.height() as i32);
            let (dx, dy) = (ev.f(p, "x") * s, ev.f(p, "y") * s);
            let blend = ev.f(p, "blend");
            let src = pm.data().to_vec();
            pm.data_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
                for x in 0..w as usize {
                    let v = sample_wrap(&src, w, h, x as f32 + 0.5 - dx, y as f32 + 0.5 - dy);
                    let o = &mut row[x * 4..x * 4 + 4];
                    for k in 0..4 {
                        o[k] = (v[k] + (o[k] as f32 - v[k]) * blend).round().clamp(0.0, 255.0) as u8;
                    }
                    pixel::fix_premul(o);
                }
            });
        }
        "polar" => {
            let pm = lb.pm.make_mut();
            let (w, h) = (pm.width() as f32, pm.height() as f32);
            let amt = ev.f(p, "amount");
            let to_polar = ev.str(p, "mode") != "torect";
            let (cxp, cyp) = (w / 2.0, h / 2.0);
            let rmax = w.min(h) / 2.0;
            remap(pm, |x, y| {
                let (sx, sy) = if to_polar {
                    let (dx, dy) = (x - cxp, y - cyp);
                    let a = (dx.atan2(dy) / std::f32::consts::TAU + 0.5).rem_euclid(1.0);
                    (a * w, (dx * dx + dy * dy).sqrt() / rmax * h)
                } else {
                    let a = (x / w - 0.5) * std::f32::consts::TAU;
                    let r = y / h * rmax;
                    (cxp + r * a.sin(), cyp + r * a.cos())
                };
                (x + (sx - x) * amt, y + (sy - y) * amt)
            });
        }
        "ripple" => {
            let (ox, oy) = (lb.x as f32, lb.y as f32);
            let pm = lb.pm.make_mut();
            let (ccx, ccy) = (ev.f(p, "cx") * ow - ox, ev.f(p, "cy") * oh - oy);
            let amp = ev.f(p, "amp") * s;
            let len = (ev.f(p, "length") * s).max(1.0);
            let rad = (ev.f(p, "radius") * oh).max(1.0);
            let ph = (ev.t * ev.num(p, "speed")) as f32 * std::f32::consts::TAU;
            remap(pm, |x, y| {
                let (dx, dy) = (x - ccx, y - ccy);
                let d = (dx * dx + dy * dy).sqrt();
                if d < 1e-3 {
                    return (x, y);
                }
                let f = (1.0 - d / rad).max(0.0);
                let o = amp * f * (d / len * std::f32::consts::TAU - ph).sin();
                (x + dx / d * o, y + dy / d * o)
            });
        }
        "lens" => {
            let (ox, oy) = (lb.x as f32, lb.y as f32);
            let pm = lb.pm.make_mut();
            let (ccx, ccy) = (ev.f(p, "cx") * ow - ox, ev.f(p, "cy") * oh - oy);
            let k = ev.f(p, "amount");
            let z = ev.f(p, "zoom").max(0.1);
            let norm = ((ow * ow + oh * oh).sqrt() / 2.0).max(1.0);
            remap(pm, |x, y| {
                let (dx, dy) = ((x - ccx) / norm, (y - ccy) / norm);
                let f = (1.0 + k * (dx * dx + dy * dy)) / z;
                (ccx + dx * f * norm, ccy + dy * f * norm)
            });
        }
        "turbulence" => {
            let (ox, oy) = (lb.x as f32, lb.y as f32);
            let pm = lb.pm.make_mut();
            let amt = ev.f(p, "amount") * s;
            let size = (ev.f(p, "size") * s).max(1.0);
            let oct = ev.f(p, "complexity");
            let z = (ev.t * ev.num(p, "speed")) as f32;
            let seed = cx.seed as u32;
            // displacement on a coarse grid, interpolated per pixel
            let step = (size / 8.0).clamp(1.0, 8.0);
            let (w, h) = (pm.width() as f32, pm.height() as f32);
            let (gw, gh) = ((w / step).ceil() as usize + 2, (h / step).ceil() as usize + 2);
            let grid: Vec<(f32, f32)> = (0..gw * gh)
                .into_par_iter()
                .map(|i| {
                    let (gx, gy) = ((i % gw) as f32 * step + ox, (i / gw) as f32 * step + oy);
                    let n = |sd: u32| fbm(gx / size, gy / size, z, oct, Kind::Smooth, sd) * 2.0 - 1.0;
                    (n(seed) * amt, n(seed ^ 0x5bd1e995) * amt)
                })
                .collect();
            remap(pm, |x, y| {
                let (fx, fy) = (x / step, y / step);
                let (ix, iy) = ((fx as usize).min(gw - 2), (fy as usize).min(gh - 2));
                let (tx, ty) = (fx - ix as f32, fy - iy as f32);
                let g = |a: usize, b: usize| grid[b * gw + a];
                let lerp2 = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let d = lerp2(lerp2(g(ix, iy), g(ix + 1, iy), tx), lerp2(g(ix, iy + 1), g(ix + 1, iy + 1), tx), ty);
                (x + d.0, y + d.1)
            });
        }
        "tile" => {
            let (w0, h0) = (lb.w() as f32, lb.h() as f32);
            let (x0, y0) = (lb.x as f32, lb.y as f32);
            let (ccx, ccy) = (x0 + w0 / 2.0, y0 + h0 / 2.0);
            let (tw, th) = ((ev.f(p, "tileW") * w0).max(1.0), (ev.f(p, "tileH") * h0).max(1.0));
            let (outw, outh) = (ev.f(p, "outW") * w0, ev.f(p, "outH") * h0);
            let mirror = ev.bool(p, "mirror");
            let ph = ev.vec(p, "phase");
            let src = lb.pm.get().clone();
            let target = IRect::from_ltrb((ccx - outw / 2.0).floor() as i32, (ccy - outh / 2.0).floor() as i32, (ccx + outw / 2.0).ceil() as i32, (ccy + outh / 2.0).ceil() as i32);
            let frame = IRect { x: 0, y: 0, w: cx.out_w, h: cx.out_h };
            lb.expand_to(target.intersect(&frame).union(&lb.rect().intersect(&frame)));
            let (bx, by) = (lb.x as f32, lb.y as f32);
            let (sw, sh) = (src.width() as i32, src.height() as i32);
            let sd = src.data();
            let pm = lb.pm.make_mut();
            let w = pm.width() as usize;
            let (tl, tt) = (ccx - tw / 2.0 + ph.0 as f32 * s, ccy - th / 2.0 + ph.1 as f32 * s);
            let (tr_, tb) = (target.x as f32, target.y as f32);
            let (tre, tbe) = (target.r() as f32, target.b() as f32);
            pm.data_mut().par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                let gy = by + y as f32 + 0.5;
                for x in 0..w {
                    let gx = bx + x as f32 + 0.5;
                    let o = &mut row[x * 4..x * 4 + 4];
                    if gx < tr_ || gx > tre || gy < tb || gy > tbe {
                        o.copy_from_slice(&[0, 0, 0, 0]);
                        continue;
                    }
                    let (u, v) = ((gx - tl) / tw, (gy - tt) / th);
                    let (iu, iv) = (u.floor(), v.floor());
                    let (mut fu, mut fv) = (u - iu, v - iv);
                    if mirror {
                        if (iu as i64).rem_euclid(2) == 1 {
                            fu = 1.0 - fu;
                        }
                        if (iv as i64).rem_euclid(2) == 1 {
                            fv = 1.0 - fv;
                        }
                    }
                    let (sx, sy) = (ccx - tw / 2.0 + fu * tw - x0, ccy - th / 2.0 + fv * th - y0);
                    let c = pixel::sample_clamp(sd, sw, sh, sx, sy);
                    for k in 0..4 {
                        o[k] = c[k].round().clamp(0.0, 255.0) as u8;
                    }
                    pixel::fix_premul(o);
                }
            });
        }
        _ => return false,
    }
    true
}
