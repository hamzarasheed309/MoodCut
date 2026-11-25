//! Layer masks (rect / ellipse / polygon / star / path / gradient in normalized layer
//! coordinates, plus the pixel-driven `hsl`, `rgb`, `luma` and `3d` qualifiers) with
//! feather (per axis or per vertex, smooth or linear), refine edge, motion
//! blur, expansion, opacity, invert, placement and After Effects mask modes.

use super::effects::{luma, rgb_to_hsl, smoothstep};
use super::eval::Ev;
use super::pixel::{apply_alpha_mask, blur_mask, LayerBuf};
use super::shapes;
use crate::model::project::{Effect, Mask};
use crate::model::schema;
use rayon::prelude::*;
use resvg::tiny_skia::{FillRule, Mask as SkMask, Path, PathSegment, Point, Rect, Transform};
use std::collections::VecDeque;

fn morph_line(src: &[u8], r: usize, grow: bool) -> Vec<u8> {
    if r == 0 || src.is_empty() { return src.to_vec(); }
    let mut out = vec![0u8; src.len()];
    let mut q = VecDeque::new();
    let mut right = 0;
    for i in 0..src.len() {
        let hi = (i + r).min(src.len() - 1);
        while right <= hi {
            while let Some(&last) = q.back() {
                let dominated = if grow { src[last] <= src[right] } else { src[last] >= src[right] };
                if dominated { q.pop_back(); } else { break; }
            }
            q.push_back(right);
            right += 1;
        }
        let lo = i.saturating_sub(r);
        while q.front().is_some_and(|&first| first < lo) { q.pop_front(); }
        out[i] = src[*q.front().unwrap()];
    }
    out
}

fn morph_alpha(alpha: &[u8], w: usize, h: usize, r: usize, grow: bool) -> Vec<u8> {
    if r == 0 || w == 0 || h == 0 { return alpha.to_vec(); }
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        let row = morph_line(&alpha[y * w..(y + 1) * w], r, grow);
        tmp[y * w..(y + 1) * w].copy_from_slice(&row);
    }
    let mut out = vec![0u8; w * h];
    let mut col = vec![0u8; h];
    for x in 0..w {
        for y in 0..h { col[y] = tmp[y * w + x]; }
        let result = morph_line(&col, r, grow);
        for y in 0..h { out[y * w + x] = result[y]; }
    }
    out
}

pub(crate) fn expand_alpha(alpha: &mut Vec<u8>, w: usize, h: usize, amount: f32) {
    let grow = amount >= 0.0;
    let amount = amount.abs();
    if amount < 0.01 || w == 0 || h == 0 { return; }
    let lo = amount.floor() as usize;
    let frac = amount - lo as f32;
    let low = morph_alpha(alpha, w, h, lo, grow);
    if frac <= 0.001 {
        *alpha = low;
        return;
    }
    let high = morph_alpha(alpha, w, h, lo + 1, grow);
    *alpha = low.iter().zip(high).map(|(&a, b)| (a as f32 + (b as f32 - a as f32) * frac).round().clamp(0.0, 255.0) as u8).collect();
}

/// Layer bounds in layer px (x, y, w, h).
#[derive(Clone, Copy, Debug)]
pub struct LBounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The mask outline in layer px, after rotation and placement (offset,
/// scale, trot). None for pixel-driven masks and gradients.
pub fn mask_path(m: &Mask, ev: &Ev, b: LBounds) -> Option<Path> {
    let (path, t) = shape_and_transform(m, ev, b)?;
    if t.is_identity() {
        Some(path)
    } else {
        path.transform(t)
    }
}

/// The path a `path` mask draws: its `d`, or with `smooth=1` (RotoBezier)
/// a curve through the same vertices with automatic tangents.
pub fn path_d(m: &Mask, ev: &Ev) -> String {
    let d = ev.str(&m.props, "d");
    if ev.bool(&m.props, "smooth") {
        shapes::rotobezier(&d, ev.f(&m.props, "tension") as f64).unwrap_or(d)
    } else {
        d
    }
}

/// Path-mask vertices in layer px, placed as the mask is drawn (numbered
/// from 1 in the overlay view, as `mask … pt <n>` counts them).
pub fn path_vertices_px(m: &Mask, ev: &Ev, b: LBounds) -> Vec<(f32, f32)> {
    if m.kind != "path" {
        return vec![];
    }
    let Some((_, t)) = shape_and_transform(m, ev, b) else { return vec![] };
    shapes::path_vertices(&ev.str(&m.props, "d"))
        .into_iter()
        .map(|(x, y)| {
            let mut pt = Point::from_xy(b.x + x as f32 * b.w, b.y + y as f32 * b.h);
            t.map_point(&mut pt);
            (pt.x, pt.y)
        })
        .collect()
}

/// The untransformed shape and the rotation + placement that positions it.
fn shape_and_transform(m: &Mask, ev: &Ev, b: LBounds) -> Option<(Path, Transform)> {
    let p = &m.props;
    let cx = b.x + ev.f(p, "x") * b.w;
    let cy = b.y + ev.f(p, "y") * b.h;
    let ex = ev.f(p, "expand") * 2.0;
    let w = (ev.f(p, "w") * b.w + ex).max(0.01);
    let h = (ev.f(p, "h") * b.h + ex).max(0.01);
    let rot = ev.f(p, "rot");
    let path = match m.kind.as_str() {
        "rect" => shapes::rounded_rect_path(cx, cy, w, h, ev.f(p, "radius")),
        "ellipse" => shapes::ellipse_path(cx, cy, w, h),
        "polygon" => shapes::polygon_path(cx, cy, ev.num(p, "points").round() as usize, w.min(h) / 2.0),
        "star" => shapes::star_path(cx, cy, ev.num(p, "points").round() as usize, w.min(h) / 2.0, w.min(h) / 4.0),
        "path" => shapes::parse_svg_path(&path_d(m, ev), b.w, b.h, b.x, b.y),
        _ => None,
    }?;
    let rt = if rot.abs() > 1e-6 { Transform::from_rotate_at(rot, cx, cy) } else { Transform::identity() };
    // placement (tracking) on top of the shape (roto)
    let (ox, oy) = ev.vec(p, "offset");
    let sc = ev.f(p, "scale").max(0.0001);
    let tr = ev.f(p, "trot");
    if ox.abs() < 1e-9 && oy.abs() < 1e-9 && (sc - 1.0).abs() < 1e-6 && tr.abs() < 1e-6 {
        return Some((path, rt));
    }
    let (px, py) = if m.kind == "path" {
        let rotated = path.clone().transform(rt)?;
        pivot(m, &rotated, cx, cy)
    } else {
        (cx, cy)
    };
    let t = Transform::from_translate(ox as f32 * b.w, oy as f32 * b.h)
        .pre_concat(Transform::from_rotate_at(tr, px, py))
        .pre_concat(Transform::from_translate(px, py).pre_scale(sc, sc).pre_translate(-px, -py));
    Some((path, t.pre_concat(rt)))
}

/// Point placement scales and rotates about: the shape center, or the
/// bounding-box center of a path.
fn pivot(m: &Mask, path: &Path, cx: f32, cy: f32) -> (f32, f32) {
    if m.kind == "path" {
        let r = path.bounds();
        ((r.left() + r.right()) / 2.0, (r.top() + r.bottom()) / 2.0)
    } else {
        (cx, cy)
    }
}

/// Qualifier alpha from the layer's own pixels: the Resolve-style HSL, RGB,
/// luma and 3D qualifiers.
fn pixel_alpha(lb: &LayerBuf, mk: &Mask, ev: &Ev) -> Vec<u8> {
    let p = &mk.props;
    let kind = mk.kind.as_str();
    let hue = ev.f(p, "hue").rem_euclid(360.0) / 360.0;
    let half = (ev.f(p, "hueRange") / 720.0).clamp(0.0, 0.5);
    let (slo, shi) = (ev.f(p, "satLo"), ev.f(p, "satHi"));
    let (llo, lhi) = (ev.f(p, "lumLo"), ev.f(p, "lumHi"));
    let rgb_lo = [ev.f(p, "rLo"), ev.f(p, "gLo"), ev.f(p, "bLo")];
    let rgb_hi = [ev.f(p, "rHi"), ev.f(p, "gHi"), ev.f(p, "bHi")];
    let key = ev.color(p, "color");
    let tol = ev.f(p, "tolerance").max(0.0);
    let soft = ev.f(p, "qsoft").clamp(0.001, 1.0);
    let band = |x: f32, lo: f32, hi: f32, s: f32| -> f32 {
        let a = if lo <= 0.0 { 1.0 } else { smoothstep(lo - s, lo + 1e-4, x) };
        let b = if hi >= 1.0 { 1.0 } else { 1.0 - smoothstep(hi - 1e-4, hi + s, x) };
        a * b
    };
    // 3D qualifier: distance from the key color in a luma / chroma space,
    // with luma weighted down so shading on the keyed surface stays in
    let yuv = |r: f32, g: f32, b: f32| {
        let y = luma(r, g, b);
        (y, (b - y) * 0.565, (r - y) * 0.713)
    };
    let (ky, ku, kv) = yuv(key.r, key.g, key.b);
    lb.pm
        .get()
        .data()
        .par_chunks_exact(4)
        .map(|px| {
            let a = px[3];
            if a == 0 {
                return 0u8;
            }
            let k = 1.0 / a as f32;
            let (r, g, bl) = (px[0] as f32 * k, px[1] as f32 * k, px[2] as f32 * k);
            let v = match kind {
                "hsl" => {
                    let (h, s, _) = rgb_to_hsl(r, g, bl);
                    let d = (h - hue).rem_euclid(1.0);
                    let d = d.min(1.0 - d);
                    let hw = if half >= 0.5 { 1.0 } else { 1.0 - smoothstep(half, half + soft * 0.25, d) };
                    hw * band(s, slo, shi, soft * 0.5) * band(luma(r, g, bl), llo, lhi, soft * 0.5)
                }
                "rgb" => band(r, rgb_lo[0], rgb_hi[0], soft * 0.5) * band(g, rgb_lo[1], rgb_hi[1], soft * 0.5) * band(bl, rgb_lo[2], rgb_hi[2], soft * 0.5),
                "3d" => {
                    let (y, u, vv) = yuv(r, g, bl);
                    let d = (((y - ky) * 0.5).powi(2) + (u - ku).powi(2) + (vv - kv).powi(2)).sqrt();
                    1.0 - smoothstep(tol, tol + soft * 0.5, d)
                }
                _ => band(luma(r, g, bl), llo, lhi, soft * 0.5),
            };
            (v * a as f32 + 0.5).clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// A gradient window: opaque on the side `rot` points to (0 = up), fading
/// across a band `h` (fraction of the layer height) wide, centered on x,y.
/// Feather widens the band; expand moves the line toward the clear side.
fn gradient_alpha(mk: &Mask, ev: &Ev, b: LBounds, to_buf: Transform, w: usize, h: usize, feather: f32, scale: f32) -> Vec<u8> {
    let p = &mk.props;
    let (ox, oy) = ev.vec(p, "offset");
    let cx = b.x + (ev.f(p, "x") + ox as f32) * b.w;
    let cy = b.y + (ev.f(p, "y") + oy as f32) * b.h;
    let ang = (ev.f(p, "rot") + ev.f(p, "trot")).to_radians();
    let (nx, ny) = (ang.sin(), -ang.cos());
    let width = (ev.f(p, "h") * b.h * ev.f(p, "scale").max(0.0001) + feather / scale.max(1e-6)).max(1.0);
    let shift = ev.f(p, "expand");
    let mut out = vec![0u8; w * h];
    let Some(inv) = to_buf.invert() else { return out };
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut pt = Point::from_xy(x as f32 + 0.5, y as f32 + 0.5);
            inv.map_point(&mut pt);
            let s = ((pt.x - cx) * nx + (pt.y - cy) * ny + shift) / width;
            *o = (smoothstep(-0.5, 0.5, s) * 255.0 + 0.5) as u8;
        }
    });
    out
}

/// True when the matte depends on the layer's pixels (qualifiers, or any
/// mask with edge refinement), so it can't be cached by its props alone.
pub fn uses_pixels(mk: &Mask, mev: &Ev) -> bool {
    schema::is_pixel_mask(&mk.kind) || mev.f(&mk.props, "refine") >= 0.5
}

/// True when the mask is animated and asks for motion blur.
pub fn motion_blurred(mk: &Mask, mev: &Ev) -> bool {
    mev.bool(&mk.props, "mblur") && mk.props.values().any(|a| a.is_animated())
}

/// Rasterizes one layer mask into the supplied output-space layer buffer.
/// The returned bytes include path expansion, feather, refinement, opacity
/// and invert. Use [`super::matte::mask`] instead to share the result
/// between effects.
pub fn mask_alpha(lb: &LayerBuf, mk: &Mask, ev: &Ev, b: LBounds, m: Transform, scale: f32) -> Option<Vec<u8>> {
    if !mk.on {
        return Some(vec![0; (lb.w() * lb.h()) as usize]);
    }
    let mev = ev.with_lists(&[schema::MASK]);
    // mask motion blur: the matte averaged over the shutter (AE's mask
    // motion blur), eight samples
    if motion_blurred(mk, &mev) {
        let fps = ev.x.fps.max(1.0);
        let span = mev.f(&mk.props, "shutter").clamp(0.0, 720.0) as f64 / 360.0 / fps;
        if span > 1e-6 {
            const N: usize = 8;
            let mut acc = vec![0u32; (lb.w() * lb.h()) as usize];
            for k in 0..N {
                let dt = (k as f64 / (N - 1) as f64 - 0.5) * span;
                let a = mask_alpha_at(lb, mk, &mev.with_t(ev.t + dt), b, m, scale)?;
                acc.iter_mut().zip(a).for_each(|(s, v)| *s += v as u32);
            }
            return Some(acc.into_iter().map(|s| ((s + N as u32 / 2) / N as u32) as u8).collect());
        }
    }
    mask_alpha_at(lb, mk, &mev, b, m, scale)
}

fn mask_alpha_at(lb: &LayerBuf, mk: &Mask, mev: &Ev, b: LBounds, m: Transform, scale: f32) -> Option<Vec<u8>> {
    let p = &mk.props;
    let (w, h) = (lb.w() as usize, lb.h() as usize);
    let to_buf = Transform::from_translate(-lb.x as f32, -lb.y as f32).pre_concat(m);
    let feather = mev.f(p, "feather").max(0.0) * scale;
    let fy = mev.f(p, "featherY");
    let feather_y = if fy < 0.0 { feather } else { fy * scale };
    let linear = mev.str(p, "falloff") == "linear";
    let mut feathered = false;
    let mut data = if schema::is_pixel_mask(&mk.kind) {
        let mut d = pixel_alpha(lb, mk, mev);
        let clean = mev.f(p, "clean") * scale;
        if clean > 0.3 {
            expand_alpha(&mut d, w, h, -clean * 0.5);
            blur_mask(&mut d, w, h, clean * 0.5);
        }
        d
    } else if mk.kind == "gradient" {
        feathered = true;
        gradient_alpha(mk, mev, b, to_buf, w, h, feather.max(feather_y), scale)
    } else {
        let path = mask_path(mk, mev, b)?;
        let mut sm = SkMask::new(w as u32, h as u32)?;
        sm.fill_path(&path, FillRule::Winding, true, to_buf);
        let mut d = sm.take();
        if mk.kind == "path" {
            expand_alpha(&mut d, w, h, mev.f(p, "expand") * scale);
            let spec = parse_vfeather(&mev.str(p, "vfeather"));
            if !spec.is_empty() {
                if let Some(bp) = path.transform(to_buf) {
                    vertex_feather(&mut d, w, h, &bp, &spec, feather, scale, linear);
                    feathered = true;
                }
            }
        }
        d
    };
    // matte finesse: clean black / clean white
    let (cb, cw) = (mev.f(p, "cleanBlack").clamp(0.0, 1.0), mev.f(p, "cleanWhite").clamp(0.0, 1.0));
    if cb > 0.001 || cw < 0.999 {
        let (lo, hi) = (cb * 255.0, (cw * 255.0).max(cb * 255.0 + 1.0));
        let lut: Vec<u8> = (0..256).map(|v| (((v as f32 - lo) / (hi - lo)).clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        data.iter_mut().for_each(|v| *v = lut[*v as usize]);
    }
    // matte finesse: morphology (Resolve's shrink / grow / open / close)
    let mr = mev.f(p, "morphR").max(0.0) * scale;
    if mr >= 0.5 {
        match mev.str(p, "morph").as_str() {
            "shrink" => expand_alpha(&mut data, w, h, -mr),
            "grow" => expand_alpha(&mut data, w, h, mr),
            "open" => {
                expand_alpha(&mut data, w, h, -mr);
                expand_alpha(&mut data, w, h, mr);
            }
            "close" => {
                expand_alpha(&mut data, w, h, mr);
                expand_alpha(&mut data, w, h, -mr);
            }
            _ => {}
        }
    }
    // qualifiers grow or shrink with expand as shapes do
    if schema::is_pixel_mask(&mk.kind) {
        let ex = mev.f(p, "expand") * scale;
        if ex.abs() >= 0.5 {
            expand_alpha(&mut data, w, h, ex);
        }
    }
    let mb = mev.f(p, "matteBlur").max(0.0) * scale;
    if mb > 0.3 {
        blur_mask(&mut data, w, h, mb / 2.0);
    }
    if !feathered {
        let f = (feather + feather_y) * 0.5;
        let bias = mev.f(p, "featherBias").clamp(-1.0, 1.0);
        if f > 0.3 && bias.abs() > 0.01 {
            expand_alpha(&mut data, w, h, bias * f * 0.5);
        }
        feather_xy(&mut data, w, h, feather, feather_y, linear);
    }
    let refine = mev.f(p, "refine") * scale;
    if refine >= 0.5 {
        refine_edge(&mut data, lb, w, h, refine, mev.f(p, "refineSoft"));
    }
    let op = mev.f(p, "opacity").clamp(0.0, 1.0);
    let inv = mev.bool(p, "invert");
    if op >= 0.9999 && !inv {
        return Some(data);
    }
    let opi = (op * 256.0).round() as u32;
    for v in data.iter_mut() {
        let x = ((*v as u32 * opi + 128) >> 8).min(255) as u8;
        *v = if inv { 255 - x } else { x };
    }
    Some(data)
}

// ---------------------------------------------------------------- feather

/// Box sizes approximating a gaussian with `n` passes.
fn gauss_boxes(sigma: f32, n: usize) -> Vec<usize> {
    let w_ideal = ((12.0 * sigma * sigma / n as f32) + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - (n as i32 * wl * wl) as f32 - 4.0 * n as f32 * wl as f32 - 3.0 * n as f32) / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round() as i32;
    (0..n as i32).map(|i| if i < m { wl.max(1) as usize } else { wu.max(1) as usize }).collect()
}

/// Box blur of every row of a single-channel buffer (edges clamp).
fn box_rows(data: &mut [u8], w: usize, r: usize) {
    if r == 0 || w == 0 {
        return;
    }
    data.par_chunks_mut(w).for_each(|row| {
        let src = row.to_vec();
        let div = (2 * r + 1) as u32;
        let get = |i: isize| src[i.clamp(0, w as isize - 1) as usize] as u32;
        let mut acc: u32 = (-(r as isize)..=r as isize).map(get).sum();
        for x in 0..w {
            row[x] = ((acc + div / 2) / div) as u8;
            acc += get(x as isize + r as isize + 1);
            acc -= get(x as isize - r as isize);
        }
    });
}

fn transpose(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    if w == 0 || h == 0 {
        return out;
    }
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, o) in col.iter_mut().enumerate() {
            *o = src[y * w + x];
        }
    });
    out
}

/// Separable box blur with its own radius per axis.
fn box_xy(data: &mut [u8], w: usize, h: usize, rx: usize, ry: usize) {
    box_rows(data, w, rx);
    if ry > 0 && h > 0 && w > 0 {
        let mut t = transpose(data, w, h);
        box_rows(&mut t, h, ry);
        data.copy_from_slice(&transpose(&t, h, w));
    }
}

/// Feather with separate horizontal and vertical widths (AE's two-value
/// mask feather) and a smooth (gaussian) or linear falloff.
fn feather_xy(data: &mut [u8], w: usize, h: usize, fx: f32, fy: f32, linear: bool) {
    if (fx <= 0.3 && fy <= 0.3) || w == 0 || h == 0 {
        return;
    }
    if linear {
        // one box pass is a linear ramp as wide as the box
        box_xy(data, w, h, (fx * 0.5).round() as usize, (fy * 0.5).round() as usize);
        return;
    }
    if (fx - fy).abs() < 0.01 {
        feather_alpha(data, w, h, fx / 2.0);
        return;
    }
    let bx = if fx > 0.3 { gauss_boxes(fx / 2.0, 3) } else { vec![1, 1, 1] };
    let by = if fy > 0.3 { gauss_boxes(fy / 2.0, 3) } else { vec![1, 1, 1] };
    for (a, b) in bx.iter().zip(&by) {
        box_xy(data, w, h, (a - 1) / 2, (b - 1) / 2);
    }
}

/// `vfeather='3:40 5:-10'`: per-vertex feather widths (vertex numbers from 1).
pub fn parse_vfeather(s: &str) -> Vec<(usize, f32)> {
    s.split([' ', ';'])
        .filter_map(|t| {
            let (n, v) = t.split_once([':', '='])?;
            Some((n.trim().parse::<usize>().ok()?.checked_sub(1)?, v.trim().trim_end_matches("px").parse::<f32>().ok()?))
        })
        .collect()
}

/// Variable-width feather, as AE's mask feather points: each vertex has its
/// own width (positive fades outward from the edge, negative inward;
/// unlisted vertices use `def`), interpolated along the outline. Alpha comes
/// from the distance to the outline, computed only in the band it reaches.
fn vertex_feather(data: &mut [u8], w: usize, _h: usize, path: &Path, spec: &[(usize, f32)], def: f32, scale: f32, linear: bool) {
    let fw = |id: usize| spec.iter().find(|(n, _)| *n == id).map(|(_, v)| v * scale).unwrap_or(def);
    // outline as short lines with a feather width at each end
    let mut lines: Vec<[f32; 6]> = vec![];
    fn push_curve(pts: &[Point], f0: f32, f1: f32, lines: &mut Vec<[f32; 6]>) {
        let n = pts.len() - 1;
        for i in 0..n {
            let (a, b) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            lines.push([pts[i].x, pts[i].y, pts[i + 1].x, pts[i + 1].y, f0 + (f1 - f0) * a, f0 + (f1 - f0) * b]);
        }
    }
    /// Vertex number of a segment's end point.
    fn end_id(last_drawing: bool, e: Point, start: Point, v0: usize, next_id: &mut usize) -> usize {
        if last_drawing && (e.x - start.x).abs() < 0.01 && (e.y - start.y).abs() < 0.01 {
            v0
        } else {
            *next_id += 1;
            *next_id - 1
        }
    }
    let segs: Vec<PathSegment> = path.segments().collect();
    let mut cur = Point::from_xy(0.0, 0.0);
    let mut start = cur;
    let (mut v0, mut vcur, mut next_id) = (0usize, 0usize, 0usize);
    for (i, seg) in segs.iter().enumerate() {
        // the last drawing segment of a subpath that lands on its start is
        // not a vertex of its own (as `path_vertices` counts them)
        let last_drawing = !matches!(segs.get(i + 1), Some(PathSegment::LineTo(_) | PathSegment::QuadTo(..) | PathSegment::CubicTo(..)));
        match *seg {
            PathSegment::MoveTo(pt) => {
                cur = pt;
                start = pt;
                v0 = next_id;
                vcur = v0;
                next_id += 1;
            }
            PathSegment::LineTo(pt) => {
                let id = end_id(last_drawing, pt, start, v0, &mut next_id);
                push_curve(&[cur, pt], fw(vcur), fw(id), &mut lines);
                cur = pt;
                vcur = id;
            }
            PathSegment::QuadTo(c, pt) => {
                let id = end_id(last_drawing, pt, start, v0, &mut next_id);
                let q: Vec<Point> = (0..=10)
                    .map(|k| {
                        let t = k as f32 / 10.0;
                        let mt = 1.0 - t;
                        Point::from_xy(mt * mt * cur.x + 2.0 * mt * t * c.x + t * t * pt.x, mt * mt * cur.y + 2.0 * mt * t * c.y + t * t * pt.y)
                    })
                    .collect();
                push_curve(&q, fw(vcur), fw(id), &mut lines);
                cur = pt;
                vcur = id;
            }
            PathSegment::CubicTo(c1, c2, pt) => {
                let id = end_id(last_drawing, pt, start, v0, &mut next_id);
                let q: Vec<Point> = (0..=16)
                    .map(|k| {
                        let t = k as f32 / 16.0;
                        let mt = 1.0 - t;
                        Point::from_xy(
                            mt * mt * mt * cur.x + 3.0 * mt * mt * t * c1.x + 3.0 * mt * t * t * c2.x + t * t * t * pt.x,
                            mt * mt * mt * cur.y + 3.0 * mt * mt * t * c1.y + 3.0 * mt * t * t * c2.y + t * t * t * pt.y,
                        )
                    })
                    .collect();
                push_curve(&q, fw(vcur), fw(id), &mut lines);
                cur = pt;
                vcur = id;
            }
            PathSegment::Close => {
                if (cur.x - start.x).abs() > 0.01 || (cur.y - start.y).abs() > 0.01 {
                    push_curve(&[cur, start], fw(vcur), fw(v0), &mut lines);
                }
                cur = start;
                vcur = v0;
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    let reach = |l: &[f32; 6]| l[4].abs().max(l[5].abs()) + 1.5;
    let fall = |x: f32| if linear { x.clamp(0.0, 1.0) } else { smoothstep(0.0, 1.0, x) };
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        let near: Vec<&[f32; 6]> = lines.iter().filter(|l| py >= l[1].min(l[3]) - reach(l) && py <= l[1].max(l[3]) + reach(l)).collect();
        if near.is_empty() {
            return;
        }
        for (x, o) in row.iter_mut().enumerate() {
            let px = x as f32 + 0.5;
            let (mut best, mut bw) = (f32::INFINITY, 0.0f32);
            for l in &near {
                let r = reach(l);
                if px < l[0].min(l[2]) - r || px > l[0].max(l[2]) + r {
                    continue;
                }
                let (dx, dy) = (l[2] - l[0], l[3] - l[1]);
                let len2 = dx * dx + dy * dy;
                let u = if len2 > 1e-9 { (((px - l[0]) * dx + (py - l[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let d = ((px - l[0] - dx * u).powi(2) + (py - l[1] - dy * u).powi(2)).sqrt();
                if d < best {
                    best = d;
                    bw = l[4] + (l[5] - l[4]) * u;
                }
            }
            if !best.is_finite() {
                continue;
            }
            let inside = *o >= 128;
            if !inside && bw > 0.5 && best < bw {
                *o = (*o).max((fall(1.0 - best / bw) * 255.0 + 0.5) as u8);
            } else if inside && bw < -0.5 && best < -bw {
                *o = (*o).min((fall(best / -bw) * 255.0 + 0.5) as u8);
            }
        }
    });
}

/// Gaussian feather. Large radii are blurred at reduced resolution and
/// upsampled, so the cost stays flat as the radius grows.
fn feather_alpha(data: &mut [u8], w: usize, h: usize, sigma: f32) {
    let k: usize = if sigma >= 48.0 { 4 } else if sigma >= 16.0 { 2 } else { 1 };
    if k == 1 || w < k * 4 || h < k * 4 {
        blur_mask(data, w, h, sigma);
        return;
    }
    let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
    let mut small = vec![0u8; sw * sh];
    for sy in 0..sh {
        for sx in 0..sw {
            let mut acc = 0u32;
            let mut n = 0u32;
            for y in sy * k..((sy + 1) * k).min(h) {
                for x in sx * k..((sx + 1) * k).min(w) {
                    acc += data[y * w + x] as u32;
                    n += 1;
                }
            }
            small[sy * sw + sx] = (acc / n.max(1)) as u8;
        }
    }
    blur_mask(&mut small, sw, sh, sigma / k as f32);
    let kf = k as f32;
    data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let fy = ((y as f32 + 0.5) / kf - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        for (x, o) in row.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) / kf - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            let a = small[y0 * sw + x0] as f32 * (1.0 - tx) + small[y0 * sw + x1] as f32 * tx;
            let b = small[y1 * sw + x0] as f32 * (1.0 - tx) + small[y1 * sw + x1] as f32 * tx;
            *o = (a * (1.0 - ty) + b * ty + 0.5) as u8;
        }
    });
}

// ---------------------------------------------------------------- refine edge

/// Box mean over a (2r+1)^2 window of an f32 image (edges clamp).
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        let get = |i: isize| s[i.clamp(0, w as isize - 1) as usize] as f64;
        let mut acc: f64 = (-(r as isize)..=r as isize).map(get).sum();
        let div = (2 * r + 1) as f64;
        for (x, o) in row.iter_mut().enumerate() {
            *o = (acc / div) as f32;
            acc += get(x as isize + r as isize + 1) - get(x as isize - r as isize);
        }
    });
    let mut out = vec![0f32; w * h];
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let get = |i: isize| tmp[i.clamp(0, h as isize - 1) as usize * w + x] as f64;
            let mut acc: f64 = (-(r as isize)..=r as isize).map(get).sum();
            let div = (2 * r + 1) as f64;
            (0..h)
                .map(|y| {
                    let v = (acc / div) as f32;
                    acc += get(y as isize + r as isize + 1) - get(y as isize - r as isize);
                    v
                })
                .collect()
        })
        .collect();
    for (x, col) in cols.into_iter().enumerate() {
        for (y, v) in col.into_iter().enumerate() {
            out[y * w + x] = v;
        }
    }
    out
}

/// Refine edge: a guided filter that snaps the soft part of a matte to the
/// layer's own edges (hair, fur, motion blur), as Refine Edge does in After
/// Effects. Solid inside and empty outside stay as they were; only the
/// band around the edge (within `r` px) changes. `soft` (0..1) trades edge
/// detail for smoothness.
fn refine_edge(alpha: &mut [u8], lb: &LayerBuf, w: usize, h: usize, r: f32, soft: f32) {
    let Some(bb) = super::matte::bbox_of(alpha, w, h) else { return };
    let ri = r.round().max(1.0) as usize;
    let pad = 2 * ri as i32;
    let full = super::pixel::IRect { x: 0, y: 0, w: w as i32, h: h as i32 };
    let rg = bb.pad(pad).intersect(&full);
    if rg.is_empty() {
        return;
    }
    let (rw, rh) = (rg.w as usize, rg.h as usize);
    let src = lb.pm.get().data();
    let mut gi = vec![0f32; rw * rh];
    let mut pa = vec![0f32; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            let (gx, gy) = (rg.x as usize + x, rg.y as usize + y);
            let o = (gy * w + gx) * 4;
            gi[y * rw + x] = luma(src[o] as f32, src[o + 1] as f32, src[o + 2] as f32) / 255.0;
            pa[y * rw + x] = alpha[gy * w + gx] as f32 / 255.0;
        }
    }
    let eps = 0.0004 + soft.clamp(0.0, 1.0).powi(2) * 0.04;
    let ii: Vec<f32> = gi.iter().map(|v| v * v).collect();
    let ip: Vec<f32> = gi.iter().zip(&pa).map(|(a, b)| a * b).collect();
    let (mi, mp, mii, mip) = (box_mean(&gi, rw, rh, ri), box_mean(&pa, rw, rh, ri), box_mean(&ii, rw, rh, ri), box_mean(&ip, rw, rh, ri));
    let mut ca = vec![0f32; rw * rh];
    let mut cb = vec![0f32; rw * rh];
    for k in 0..rw * rh {
        let var = mii[k] - mi[k] * mi[k];
        let cov = mip[k] - mi[k] * mp[k];
        ca[k] = cov / (var + eps);
        cb[k] = mp[k] - ca[k] * mi[k];
    }
    let (ma, mb) = (box_mean(&ca, rw, rh, ri), box_mean(&cb, rw, rh, ri));
    for y in 0..rh {
        for x in 0..rw {
            let k = y * rw + x;
            // only the edge band: where the window holds both inside and outside
            if mp[k] <= 0.002 || mp[k] >= 0.998 {
                continue;
            }
            let q = (ma[k] * gi[k] + mb[k]).clamp(0.0, 1.0);
            alpha[(rg.y as usize + y) * w + rg.x as usize + x] = (q * 255.0 + 0.5) as u8;
        }
    }
}

/// Crop effect -> rect mask in normalized coordinates.
fn crop_mask(e: &Effect, ev: &Ev, b: LBounds) -> Option<(Path, f32)> {
    let p = &e.props;
    let (l, t, r, bt) = (ev.f(p, "l"), ev.f(p, "t"), ev.f(p, "r"), ev.f(p, "b"));
    if l <= 0.0 && t <= 0.0 && r <= 0.0 && bt <= 0.0 {
        return None;
    }
    let rect = Rect::from_ltrb(b.x + l * b.w, b.y + t * b.h, b.x + (1.0 - r) * b.w, b.y + (1.0 - bt) * b.h)?;
    Some((resvg::tiny_skia::PathBuilder::from_rect(rect), ev.f(p, "soft")))
}

#[inline]
pub(crate) fn mul255(a: u8, b: u8) -> u8 {
    let x = a as u32 * b as u32 + 128;
    ((x + (x >> 8)) >> 8) as u8
}

/// Combines one mask into the accumulated matte with an After Effects mask
/// mode. `first` is true for the first mask that takes part.
pub(crate) fn combine(acc: &mut [u8], s: &[u8], mode: &str, first: bool) {
    match mode {
        "subtract" => acc.iter_mut().zip(s).for_each(|(d, &v)| *d = mul255(*d, 255 - v)),
        "intersect" | "darken" if first => acc.copy_from_slice(s),
        "intersect" => acc.iter_mut().zip(s).for_each(|(d, &v)| *d = mul255(*d, v)),
        "darken" => acc.iter_mut().zip(s).for_each(|(d, &v)| *d = (*d).min(v)),
        "lighten" => acc.iter_mut().zip(s).for_each(|(d, &v)| *d = (*d).max(v)),
        "difference" => acc.iter_mut().zip(s).for_each(|(d, &v)| *d = d.abs_diff(v)),
        _ => acc.iter_mut().zip(s).for_each(|(d, &v)| *d = d.saturating_add(v)),
    }
}

/// Applies masks (and crop effects) to `lb`. `m` maps layer px -> output px.
/// Masks with `cut=0` (windows) or `mode=none` only limit effects and are
/// skipped here.
pub fn apply_masks(lb: &mut LayerBuf, masks: &[Mask], fx: &[Effect], ev: &Ev, b: LBounds, m: Transform, scale: f32) {
    let mev = ev.with_lists(&[schema::MASK]);
    let crops: Vec<&Effect> = fx.iter().filter(|e| e.on && e.kind == "crop").collect();
    let active: Vec<&Mask> = masks.iter().filter(|x| x.on && mev.bool(&x.props, "cut") && mev.str(&x.props, "mode") != "none").collect();
    if active.is_empty() && crops.is_empty() {
        return;
    }
    let (w, h) = (lb.w(), lb.h());
    let to_buf = Transform::from_translate(-lb.x as f32, -lb.y as f32).pre_concat(m);
    let n = (w * h) as usize;
    // One reused u8 accumulator per render thread, no per-mask float buffers.
    // It is taken out of the cell (not borrowed) because rayon may run another
    // layer's masks on this thread while we wait inside a parallel loop.
    thread_local! {
        static ACC: std::cell::Cell<Vec<u8>> = const { std::cell::Cell::new(Vec::new()) };
    }
    let mut acc = ACC.with(|c| c.take());
    {
        let mut started = false;
        for mk in active.iter() {
            let mode = mev.str(&mk.props, "mode");
            let Some(data) = super::matte::mask(lb, mk, &mev, b, m, scale) else { continue };
            let first = !started;
            if !started {
                acc.clear();
                acc.resize(n, if mode == "subtract" { 255 } else { 0 });
                started = true;
            }
            combine(&mut acc, &data.alpha, &mode, first);
        }
        let cev = ev.with_lists(&[schema::effect("crop").map(|e| e.params).unwrap_or(&[])]);
        for c in crops {
            let Some((path, soft)) = crop_mask(c, &cev, b) else { continue };
            let Some(mut sm) = SkMask::new(w, h) else { continue };
            sm.fill_path(&path, FillRule::Winding, true, to_buf);
            let mut data = sm.take();
            if soft * scale > 0.3 {
                blur_mask(&mut data, w as usize, h as usize, soft * scale / 2.0);
            }
            if !started {
                acc.clear();
                acc.resize(n, 255);
                started = true;
            }
            acc.iter_mut().zip(data.iter()).for_each(|(d, &v)| *d = mul255(*d, v));
        }
        if started {
            apply_alpha_mask(lb.pm.make_mut(), &acc);
        }
    }
    if acc.capacity() <= 64 << 20 {
        ACC.with(|c| c.set(acc));
    }
}

#[cfg(test)]
mod feather_tests {
    use super::*;

    #[test]
    fn vfeather_spec_parses() {
        assert_eq!(parse_vfeather("1:20 3:-8px"), vec![(0, 20.0), (2, -8.0)]);
        assert!(parse_vfeather("0:5 x").is_empty());
    }

    #[test]
    fn mask_modes_follow_after_effects() {
        let mut acc = vec![100u8, 200];
        combine(&mut acc, &[100, 100], "add", false);
        assert_eq!(acc, vec![200, 255]);
        let mut acc = vec![100u8, 200];
        combine(&mut acc, &[150, 150], "lighten", false);
        assert_eq!(acc, vec![150, 200]);
        let mut acc = vec![100u8, 200];
        combine(&mut acc, &[150, 150], "darken", false);
        assert_eq!(acc, vec![100, 150]);
        let mut acc = vec![255u8, 255];
        combine(&mut acc, &[255, 0], "subtract", false);
        assert_eq!(acc, vec![0, 255]);
    }

    #[test]
    fn linear_feather_is_a_ramp_and_xy_feather_is_anisotropic() {
        let (w, h) = (64usize, 64usize);
        let mut a = vec![0u8; w * h];
        for y in 0..h {
            for x in 32..w {
                a[y * w + x] = 255;
            }
        }
        let mut lin = a.clone();
        feather_xy(&mut lin, w, h, 16.0, 16.0, true);
        let row: Vec<u8> = lin[32 * w..33 * w].to_vec();
        // monotonic, roughly linear across the edge
        assert!(row.windows(2).all(|p| p[1] >= p[0]));
        assert!((row[32] as i32 - 128).abs() < 20);
        // vertical-only feather leaves a vertical edge hard
        let mut v = a.clone();
        feather_xy(&mut v, w, h, 0.0, 20.0, false);
        assert_eq!(v[32 * w + 28], 0);
        assert_eq!(v[32 * w + 36], 255);
    }

    #[test]
    fn refine_keeps_solid_regions() {
        let (w, h) = (48u32, 48u32);
        let mut lb = LayerBuf::new(0, 0, w, h).unwrap();
        lb.pm.make_mut().fill(resvg::tiny_skia::Color::from_rgba8(200, 200, 200, 255));
        let mut a = vec![0u8; (w * h) as usize];
        for y in 10..38 {
            for x in 10..38 {
                a[y * 48 + x] = 255;
            }
        }
        blur_mask(&mut a, 48, 48, 2.0);
        let before = a.clone();
        refine_edge(&mut a, &lb, 48, 48, 3.0, 0.2);
        assert_eq!(a[24 * 48 + 24], before[24 * 48 + 24]);
        assert_eq!(a[2 * 48 + 2], 0);
    }
}
