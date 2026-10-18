//! Low-level pixel utilities on premultiplied RGBA8 buffers: layer buffers,
//! fast parallel compositing, separable box/gaussian blur, sampling.

use rayon::prelude::*;
use resvg::tiny_skia::{IntSize, Pixmap};
use std::sync::Arc;

/// A layer rendered into output space: `pm` is placed at (x, y).
pub struct LayerBuf {
    pub pm: Pm,
    pub x: i32,
    pub y: i32,
}

/// Owned or shared pixmap (shared avoids copying decoded frames).
pub enum Pm {
    Owned(Pixmap),
    Shared(Arc<Pixmap>),
}

impl Pm {
    pub fn get(&self) -> &Pixmap {
        match self {
            Pm::Owned(p) => p,
            Pm::Shared(p) => p,
        }
    }
    pub fn make_mut(&mut self) -> &mut Pixmap {
        if let Pm::Shared(p) = self {
            *self = Pm::Owned((**p).clone());
        }
        match self {
            Pm::Owned(p) => p,
            _ => unreachable!(),
        }
    }
}

impl LayerBuf {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Option<LayerBuf> {
        Some(LayerBuf { pm: Pm::Owned(Pixmap::new(w.max(1), h.max(1))?), x, y })
    }
    pub fn w(&self) -> u32 {
        self.pm.get().width()
    }
    pub fn h(&self) -> u32 {
        self.pm.get().height()
    }
    pub fn rect(&self) -> IRect {
        IRect { x: self.x, y: self.y, w: self.w() as i32, h: self.h() as i32 }
    }
    /// Re-embeds the buffer into a larger rect (padding with transparency).
    pub fn expand_to(&mut self, r: IRect) {
        if r == self.rect() {
            return;
        }
        let mut np = Pixmap::new(r.w.max(1) as u32, r.h.max(1) as u32).unwrap();
        blit(&mut np, self.pm.get(), self.x - r.x, self.y - r.y);
        self.pm = Pm::Owned(np);
        self.x = r.x;
        self.y = r.y;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub fn from_ltrb(l: i32, t: i32, r: i32, b: i32) -> IRect {
        IRect { x: l, y: t, w: (r - l).max(0), h: (b - t).max(0) }
    }
    pub fn r(&self) -> i32 {
        self.x + self.w
    }
    pub fn b(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect::from_ltrb(self.x.max(o.x), self.y.max(o.y), self.r().min(o.r()), self.b().min(o.b()))
    }
    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect::from_ltrb(self.x.min(o.x), self.y.min(o.y), self.r().max(o.r()), self.b().max(o.b()))
    }
    pub fn pad(&self, m: i32) -> IRect {
        IRect::from_ltrb(self.x - m, self.y - m, self.r() + m, self.b() + m)
    }
    /// Bounding rect of float points.
    pub fn bound(pts: &[(f32, f32)]) -> IRect {
        let mut l = f32::INFINITY;
        let mut t = f32::INFINITY;
        let mut r = f32::NEG_INFINITY;
        let mut b = f32::NEG_INFINITY;
        for &(x, y) in pts {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            l = l.min(x);
            t = t.min(y);
            r = r.max(x);
            b = b.max(y);
        }
        if !l.is_finite() {
            return IRect { x: 0, y: 0, w: 0, h: 0 };
        }
        IRect::from_ltrb(l.floor() as i32 - 1, t.floor() as i32 - 1, r.ceil() as i32 + 1, b.ceil() as i32 + 1)
    }
}

/// Copies `src` into `dst` at offset (replace, no blending).
pub fn blit(dst: &mut Pixmap, src: &Pixmap, ox: i32, oy: i32) {
    let dw = dst.width() as i32;
    let dh = dst.height() as i32;
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    let x0 = ox.max(0);
    let x1 = (ox + sw).min(dw);
    if x1 <= x0 {
        return;
    }
    let sd = src.data();
    let dd = dst.data_mut();
    for y in oy.max(0)..(oy + sh).min(dh) {
        let sy = y - oy;
        let s = ((sy * sw + (x0 - ox)) * 4) as usize;
        let d = ((y * dw + x0) * 4) as usize;
        let n = ((x1 - x0) * 4) as usize;
        dd[d..d + n].copy_from_slice(&sd[s..s + n]);
    }
}

#[inline]
fn mul255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    (t + (t >> 8)) >> 8
}

/// Premultiplied source-over of `src` (placed at ox, oy) onto `dst`, with opacity. Parallel.
pub fn src_over(dst: &mut Pixmap, src: &Pixmap, ox: i32, oy: i32, opacity: f32) {
    let dw = dst.width() as i32;
    let dh = dst.height() as i32;
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    let x0 = ox.max(0);
    let x1 = (ox + sw).min(dw);
    let y0 = oy.max(0);
    let y1 = (oy + sh).min(dh);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let op = (opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    if op == 0 {
        return;
    }
    let sd = src.data();
    let stride = (dw * 4) as usize;
    let rows = &mut dst.data_mut()[(y0 as usize) * stride..(y1 as usize) * stride];
    rows.par_chunks_mut(stride).enumerate().for_each(|(i, row)| {
        let y = y0 + i as i32;
        let sy = y - oy;
        let srow = &sd[((sy * sw) * 4) as usize..((sy * sw + sw) * 4) as usize];
        let n = (x1 - x0) as usize;
        let dseg = &mut row[(x0 * 4) as usize..(x0 as usize + n) * 4];
        let sseg = &srow[((x0 - ox) * 4) as usize..((x0 - ox) as usize + n) * 4];
        if op == 255 {
            for (d, s) in dseg.chunks_exact_mut(4).zip(sseg.chunks_exact(4)) {
                let sa = s[3] as u32;
                if sa == 255 {
                    d.copy_from_slice(s);
                } else if sa != 0 {
                    let inv = 255 - sa;
                    d[0] = (s[0] as u32 + mul255(d[0] as u32, inv)) as u8;
                    d[1] = (s[1] as u32 + mul255(d[1] as u32, inv)) as u8;
                    d[2] = (s[2] as u32 + mul255(d[2] as u32, inv)) as u8;
                    d[3] = (sa + mul255(d[3] as u32, inv)) as u8;
                }
            }
        } else {
            for (d, s) in dseg.chunks_exact_mut(4).zip(sseg.chunks_exact(4)) {
                let sa = mul255(s[3] as u32, op);
                if sa == 0 {
                    continue;
                }
                let inv = 255 - sa;
                d[0] = (mul255(s[0] as u32, op) + mul255(d[0] as u32, inv)).min(255) as u8;
                d[1] = (mul255(s[1] as u32, op) + mul255(d[1] as u32, inv)).min(255) as u8;
                d[2] = (mul255(s[2] as u32, op) + mul255(d[2] as u32, inv)).min(255) as u8;
                d[3] = (sa + mul255(d[3] as u32, inv)).min(255) as u8;
            }
        }
    });
}

/// Per-pixel lerp: out = a*(1-m) + b*m where both buffers share size. `m` gives 0..1 per pixel.
pub fn lerp_into(a: &mut Pixmap, b: &Pixmap, m: impl Fn(i32, i32, &[u8]) -> f32 + Sync) {
    let w = a.width() as usize;
    let stride = w * 4;
    let bd = b.data();
    a.data_mut().par_chunks_mut(stride).enumerate().for_each(|(y, row)| {
        let brow = &bd[y * stride..(y + 1) * stride];
        for x in 0..w {
            let bp = &brow[x * 4..x * 4 + 4];
            let t = m(x as i32, y as i32, bp).clamp(0.0, 1.0);
            if t <= 0.0 {
                continue;
            }
            let ap = &mut row[x * 4..x * 4 + 4];
            if t >= 1.0 {
                ap.copy_from_slice(bp);
                continue;
            }
            let ti = (t * 256.0) as u32;
            let si = 256 - ti;
            for c in 0..4 {
                ap[c] = ((ap[c] as u32 * si + bp[c] as u32 * ti) >> 8) as u8;
            }
        }
    });
}

/// Multiplies every pixel (all channels, premultiplied) by a mask value 0..255.
pub fn apply_alpha_mask(pm: &mut Pixmap, mask: &[u8]) {
    let w = pm.width() as usize;
    pm.data_mut().par_chunks_mut(w * 4).zip(mask.par_chunks(w)).for_each(|(row, mrow)| {
        for (px, &m) in row.chunks_exact_mut(4).zip(mrow.iter()) {
            if m == 255 {
                continue;
            }
            let m = m as u32;
            px[0] = mul255(px[0] as u32, m) as u8;
            px[1] = mul255(px[1] as u32, m) as u8;
            px[2] = mul255(px[2] as u32, m) as u8;
            px[3] = mul255(px[3] as u32, m) as u8;
        }
    });
}

/// Box sizes approximating a gaussian with `n` passes (Kuckir).
fn boxes_for_gauss(sigma: f32, n: usize) -> Vec<usize> {
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

/// Horizontal box blur (radius r) of an interleaved 4-channel u8 buffer, in parallel rows.
fn box_h(data: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 || w == 0 {
        return;
    }
    let _ = h;
    data.par_chunks_mut(w * 4).for_each(|row| {
        let src: Vec<u8> = row.to_vec();
        let div = (2 * r + 1) as u32;
        for c in 0..4 {
            let get = |i: isize| -> u32 { src[(i.clamp(0, w as isize - 1) as usize) * 4 + c] as u32 };
            let mut acc: u32 = 0;
            for i in -(r as isize)..=(r as isize) {
                acc += get(i);
            }
            for x in 0..w {
                row[x * 4 + c] = ((acc + div / 2) / div) as u8;
                acc += get(x as isize + r as isize + 1);
                acc -= get(x as isize - r as isize);
            }
        }
    });
}

/// Vertical box blur via column strips.
fn box_v(data: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 || h == 0 {
        return;
    }
    let src: Vec<u8> = data.to_vec();
    let div = (2 * r + 1) as u32;
    // process in strips of 16 columns for cache friendliness
    let strip = 16usize;
    let nstrips = w.div_ceil(strip);
    let results: Vec<(usize, Vec<u8>)> = (0..nstrips)
        .into_par_iter()
        .map(|si| {
            let x0 = si * strip;
            let x1 = (x0 + strip).min(w);
            let sw = x1 - x0;
            let mut out = vec![0u8; sw * h * 4];
            for xx in 0..sw {
                let x = x0 + xx;
                for c in 0..4 {
                    let get = |y: isize| -> u32 { src[((y.clamp(0, h as isize - 1) as usize) * w + x) * 4 + c] as u32 };
                    let mut acc: u32 = 0;
                    for i in -(r as isize)..=(r as isize) {
                        acc += get(i);
                    }
                    for y in 0..h {
                        out[(y * sw + xx) * 4 + c] = ((acc + div / 2) / div) as u8;
                        acc += get(y as isize + r as isize + 1);
                        acc -= get(y as isize - r as isize);
                    }
                }
            }
            (x0, out)
        })
        .collect();
    for (x0, out) in results {
        let sw = out.len() / (h * 4);
        for y in 0..h {
            let d = (y * w + x0) * 4;
            data[d..d + sw * 4].copy_from_slice(&out[y * sw * 4..(y + 1) * sw * 4]);
        }
    }
}

/// Gaussian-ish blur (3 box passes). `sigma` in pixels. `dir`: 0 both, 1 horizontal, 2 vertical.
pub fn blur_rgba(data: &mut [u8], w: usize, h: usize, sigma: f32, dir: u8) {
    if sigma < 0.3 || w == 0 || h == 0 {
        return;
    }
    // large blurs: downsample for speed
    if sigma > 24.0 && w > 64 && h > 64 && dir == 0 {
        let f = (sigma / 12.0).floor().clamp(2.0, 8.0) as usize;
        let sw = w.div_ceil(f);
        let sh = h.div_ceil(f);
        let mut small = vec![0u8; sw * sh * 4];
        for sy in 0..sh {
            for sx in 0..sw {
                let mut acc = [0u32; 4];
                let mut n = 0;
                for yy in sy * f..((sy + 1) * f).min(h) {
                    for xx in sx * f..((sx + 1) * f).min(w) {
                        let i = (yy * w + xx) * 4;
                        for c in 0..4 {
                            acc[c] += data[i + c] as u32;
                        }
                        n += 1;
                    }
                }
                let o = (sy * sw + sx) * 4;
                for c in 0..4 {
                    small[o + c] = (acc[c] / n.max(1)) as u8;
                }
            }
        }
        blur_rgba(&mut small, sw, sh, sigma / f as f32, 0);
        // bilinear upsample
        data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            let fy = ((y as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
            let y0 = fy.floor() as usize;
            let y1 = (y0 + 1).min(sh - 1);
            let ty = fy - y0 as f32;
            for x in 0..w {
                let fx = ((x as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
                let x0 = fx.floor() as usize;
                let x1 = (x0 + 1).min(sw - 1);
                let tx = fx - x0 as f32;
                for c in 0..4 {
                    let p00 = small[(y0 * sw + x0) * 4 + c] as f32;
                    let p10 = small[(y0 * sw + x1) * 4 + c] as f32;
                    let p01 = small[(y1 * sw + x0) * 4 + c] as f32;
                    let p11 = small[(y1 * sw + x1) * 4 + c] as f32;
                    let v = (p00 * (1.0 - tx) + p10 * tx) * (1.0 - ty) + (p01 * (1.0 - tx) + p11 * tx) * ty;
                    row[x * 4 + c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        });
        return;
    }
    for b in boxes_for_gauss(sigma, 3) {
        let r = (b - 1) / 2;
        if dir != 2 {
            box_h(data, w, h, r);
        }
        if dir != 1 {
            box_v(data, w, h, r);
        }
    }
}

pub fn blur_pixmap(pm: &mut Pixmap, sigma: f32, dir: u8) {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    blur_rgba(pm.data_mut(), w, h, sigma, dir);
}

/// Single-channel blur (for masks).
pub fn blur_mask(m: &mut [u8], w: usize, h: usize, sigma: f32) {
    if sigma < 0.3 {
        return;
    }
    // expand to 4 channels cheaply by processing as 1-channel via a temporary rgba buffer
    let mut tmp = vec![0u8; w * h * 4];
    for (i, &v) in m.iter().enumerate() {
        tmp[i * 4] = v;
    }
    blur_rgba(&mut tmp, w, h, sigma, 0);
    for (i, v) in m.iter_mut().enumerate() {
        *v = tmp[i * 4];
    }
}

/// Bilinear sample of a premultiplied pixmap at float coords; transparent outside.
#[inline]
pub fn sample(src: &[u8], w: i32, h: i32, x: f32, y: f32) -> [f32; 4] {
    let x = x - 0.5;
    let y = y - 0.5;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let px = |xx: i32, yy: i32| -> [f32; 4] {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            [0.0; 4]
        } else {
            let i = ((yy * w + xx) * 4) as usize;
            [src[i] as f32, src[i + 1] as f32, src[i + 2] as f32, src[i + 3] as f32]
        }
    };
    let a = px(x0, y0);
    let b = px(x0 + 1, y0);
    let c = px(x0, y0 + 1);
    let d = px(x0 + 1, y0 + 1);
    let mut o = [0f32; 4];
    for k in 0..4 {
        o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
    }
    o
}

/// Clamped bilinear sample (edge pixels extend).
#[inline]
pub fn sample_clamp(src: &[u8], w: i32, h: i32, x: f32, y: f32) -> [f32; 4] {
    sample(src, w, h, x.clamp(0.5, w as f32 - 0.5), y.clamp(0.5, h as f32 - 0.5))
}

/// Ensures color channels never exceed alpha (valid premultiplied data).
pub fn fix_premul(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        let a = px[3];
        px[0] = px[0].min(a);
        px[1] = px[1].min(a);
        px[2] = px[2].min(a);
    }
}

pub fn new_pixmap(w: u32, h: u32) -> Pixmap {
    Pixmap::new(w.max(1), h.max(1)).unwrap()
}

pub fn pixmap_from_vec(data: Vec<u8>, w: u32, h: u32) -> Pixmap {
    Pixmap::from_vec(data, IntSize::from_wh(w.max(1), h.max(1)).unwrap()).unwrap()
}

/// Maps every pixel through f(r,g,b) on straight (unpremultiplied) 0..1 floats, in parallel.
pub fn map_rgb(pm: &mut Pixmap, f: impl Fn(f32, f32, f32) -> (f32, f32, f32) + Sync) {
    let w = pm.width() as usize;
    pm.data_mut().par_chunks_mut(w * 4).for_each(|row| {
        for px in row.chunks_exact_mut(4) {
            let a = px[3];
            if a == 0 {
                continue;
            }
            let af = a as f32 / 255.0;
            let inv = 1.0 / (af * 255.0);
            let (r, g, b) = f(px[0] as f32 * inv, px[1] as f32 * inv, px[2] as f32 * inv);
            px[0] = (r.clamp(0.0, 1.0) * af * 255.0 + 0.5) as u8;
            px[1] = (g.clamp(0.0, 1.0) * af * 255.0 + 0.5) as u8;
            px[2] = (b.clamp(0.0, 1.0) * af * 255.0 + 0.5) as u8;
        }
    });
}

/// Applies per-channel 8-bit LUTs (on unpremultiplied values).
pub fn map_lut(pm: &mut Pixmap, lr: &[u8; 256], lg: &[u8; 256], lb: &[u8; 256]) {
    let w = pm.width() as usize;
    pm.data_mut().par_chunks_mut(w * 4).for_each(|row| {
        for px in row.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a == 0 {
                continue;
            }
            if a == 255 {
                px[0] = lr[px[0] as usize];
                px[1] = lg[px[1] as usize];
                px[2] = lb[px[2] as usize];
            } else {
                let un = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as usize;
                px[0] = mul255(lr[un(px[0])] as u32, a) as u8;
                px[1] = mul255(lg[un(px[1])] as u32, a) as u8;
                px[2] = mul255(lb[un(px[2])] as u32, a) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn src_over_opaque_copy() {
        let mut dst = new_pixmap(4, 4);
        let mut src = new_pixmap(2, 2);
        src.data_mut().copy_from_slice(&[255, 0, 0, 255].repeat(4));
        src_over(&mut dst, &src, 1, 1, 1.0);
        assert_eq!(&dst.data()[(4 + 1) * 4..(4 + 1) * 4 + 4], &[255, 0, 0, 255]);
        assert_eq!(&dst.data()[0..4], &[0, 0, 0, 0]);
        src_over(&mut dst, &src, 1, 1, 0.5);
        let a = dst.data()[(4 + 1) * 4 + 3];
        assert_eq!(a, 255);
    }

    #[test]
    fn blur_preserves_energy_roughly() {
        let w = 32;
        let mut d = vec![0u8; w * w * 4];
        let c = (16 * w + 16) * 4;
        d[c..c + 4].copy_from_slice(&[255, 255, 255, 255]);
        blur_rgba(&mut d, w, w, 2.0, 0);
        let sum: u32 = d.chunks(4).map(|p| p[3] as u32).sum();
        assert!(sum > 150 && sum < 400, "{sum}");
    }
}

/// Parallel affine draw: `m` maps src px -> dst px. Writes (replaces) into a
/// transparent `dst`. Bilinear (fixed point) or Catmull-Rom bicubic.
pub fn draw_affine(dst: &mut Pixmap, src: &Pixmap, m: resvg::tiny_skia::Transform, bicubic: bool) {
    let Some(inv) = m.invert() else { return };
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let dw = dst.width() as usize;
    let sd = src.data();
    // inverse mapping: sx = a*x + c*y + e ; sy = b*x + d*y + f
    let (a, b, c, d, e, f) = (inv.sx, inv.ky, inv.kx, inv.sy, inv.tx, inv.ty);
    dst.data_mut().par_chunks_mut(dw * 4).enumerate().for_each(|(y, row)| {
        let yc = y as f32 + 0.5;
        // row-wise valid x range (where the source sample is inside [-1, w] x [-1, h])
        let (mut x0, mut x1) = (0f32, dw as f32);
        let clip = |p: f32, q: f32, lo: f32, hi: f32, x0: &mut f32, x1: &mut f32| {
            // lo <= p + q*x <= hi
            if q.abs() < 1e-9 {
                if p < lo || p > hi {
                    *x1 = -1.0;
                }
            } else {
                let (u, v) = ((lo - p) / q, (hi - p) / q);
                *x0 = x0.max(u.min(v));
                *x1 = x1.min(u.max(v));
            }
        };
        clip(c * yc + e - a * 0.0 + a * 0.5, a, -1.0, sw as f32 + 1.0, &mut x0, &mut x1);
        clip(d * yc + f + b * 0.5, b, -1.0, sh as f32 + 1.0, &mut x0, &mut x1);
        let xs = (x0.floor().max(0.0)) as usize;
        let xe = (x1.ceil().min(dw as f32)).max(0.0) as usize;
        for x in xs..xe {
            let xc = x as f32 + 0.5;
            let sx = a * xc + c * yc + e;
            let sy = b * xc + d * yc + f;
            let o = &mut row[x * 4..x * 4 + 4];
            if bicubic {
                let v = sample_bicubic(sd, sw, sh, sx, sy);
                o.copy_from_slice(&v);
            } else {
                let fx = sx - 0.5;
                let fy = sy - 0.5;
                let ix = fx.floor() as i32;
                let iy = fy.floor() as i32;
                let tx = ((fx - ix as f32) * 256.0) as u32;
                let ty = ((fy - iy as f32) * 256.0) as u32;
                let px = |xx: i32, yy: i32| -> [u32; 4] {
                    if xx < 0 || yy < 0 || xx >= sw || yy >= sh {
                        [0; 4]
                    } else {
                        let i = ((yy * sw + xx) * 4) as usize;
                        [sd[i] as u32, sd[i + 1] as u32, sd[i + 2] as u32, sd[i + 3] as u32]
                    }
                };
                let (p00, p10, p01, p11) = (px(ix, iy), px(ix + 1, iy), px(ix, iy + 1), px(ix + 1, iy + 1));
                for k in 0..4 {
                    let top = p00[k] * (256 - tx) + p10[k] * tx;
                    let bot = p01[k] * (256 - tx) + p11[k] * tx;
                    o[k] = ((top * (256 - ty) + bot * ty + 32768) >> 16) as u8;
                }
            }
        }
    });
}

#[inline]
fn cubic_w(t: f32) -> [f32; 4] {
    // Catmull-Rom
    let t2 = t * t;
    let t3 = t2 * t;
    [(-t3 + 2.0 * t2 - t) * 0.5, (3.0 * t3 - 5.0 * t2 + 2.0) * 0.5, (-3.0 * t3 + 4.0 * t2 + t) * 0.5, (t3 - t2) * 0.5]
}

#[inline]
fn sample_bicubic(sd: &[u8], sw: i32, sh: i32, sx: f32, sy: f32) -> [u8; 4] {
    let fx = sx - 0.5;
    let fy = sy - 0.5;
    let ix = fx.floor() as i32;
    let iy = fy.floor() as i32;
    let wx = cubic_w(fx - ix as f32);
    let wy = cubic_w(fy - iy as f32);
    let mut acc = [0f32; 4];
    for (j, wyj) in wy.iter().enumerate() {
        let yy = iy - 1 + j as i32;
        if yy < 0 || yy >= sh {
            continue;
        }
        for (i, wxi) in wx.iter().enumerate() {
            let xx = ix - 1 + i as i32;
            if xx < 0 || xx >= sw {
                continue;
            }
            let w = wxi * wyj;
            let p = ((yy * sw + xx) * 4) as usize;
            for k in 0..4 {
                acc[k] += sd[p + k] as f32 * w;
            }
        }
    }
    let a = acc[3].round().clamp(0.0, 255.0);
    [acc[0].round().clamp(0.0, a) as u8, acc[1].round().clamp(0.0, a) as u8, acc[2].round().clamp(0.0, a) as u8, a as u8]
}
