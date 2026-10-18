//! Projective warps: 3D layer rotation (perspective camera) and corner pin.

use super::pixel::{sample, IRect, LayerBuf};
use rayon::prelude::*;
use resvg::tiny_skia::Pixmap;

/// 3x3 homography (row-major) mapping source (u,v,1) to destination.
#[derive(Clone, Copy, Debug)]
pub struct H(pub [f64; 9]);

impl H {
    /// Homography from the unit square corners (0,0),(1,0),(1,1),(0,1) to quad q.
    pub fn square_to_quad(q: [(f64, f64); 4]) -> H {
        let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = q;
        let dx1 = x1 - x2;
        let dx2 = x3 - x2;
        let dy1 = y1 - y2;
        let dy2 = y3 - y2;
        let sx = x0 - x1 + x2 - x3;
        let sy = y0 - y1 + y2 - y3;
        let (g, h) = if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            (0.0, 0.0)
        } else {
            let det = dx1 * dy2 - dx2 * dy1;
            if det.abs() < 1e-12 {
                (0.0, 0.0)
            } else {
                ((sx * dy2 - dx2 * sy) / det, (dx1 * sy - sx * dy1) / det)
            }
        };
        let a = x1 - x0 + g * x1;
        let b = x3 - x0 + h * x3;
        let c = x0;
        let d = y1 - y0 + g * y1;
        let e = y3 - y0 + h * y3;
        let f = y0;
        H([a, b, c, d, e, f, g, h, 1.0])
    }

    pub fn inverse(&self) -> Option<H> {
        let m = self.0;
        let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6]) + m[2] * (m[3] * m[7] - m[4] * m[6]);
        if det.abs() < 1e-14 {
            return None;
        }
        let id = 1.0 / det;
        Some(H([
            (m[4] * m[8] - m[5] * m[7]) * id,
            (m[2] * m[7] - m[1] * m[8]) * id,
            (m[1] * m[5] - m[2] * m[4]) * id,
            (m[5] * m[6] - m[3] * m[8]) * id,
            (m[0] * m[8] - m[2] * m[6]) * id,
            (m[2] * m[3] - m[0] * m[5]) * id,
            (m[3] * m[7] - m[4] * m[6]) * id,
            (m[1] * m[6] - m[0] * m[7]) * id,
            (m[0] * m[4] - m[1] * m[3]) * id,
        ]))
    }

    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = self.0;
        let w = m[6] * x + m[7] * y + m[8];
        let w = if w.abs() < 1e-12 { 1e-12 } else { w };
        ((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w)
    }
}

/// Warps `src` so its corners land on `quad` (output px). Returns the buffer clipped to `clip`.
pub fn warp_to_quad(src: &Pixmap, quad: [(f64, f64); 4], clip: IRect) -> Option<LayerBuf> {
    let pts: Vec<(f32, f32)> = quad.iter().map(|(x, y)| (*x as f32, *y as f32)).collect();
    let r = IRect::bound(&pts).intersect(&clip);
    if r.is_empty() {
        return None;
    }
    let hm = H::square_to_quad(quad);
    let inv = hm.inverse()?;
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let mut lb = LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32)?;
    let sd = src.data();
    let pm = lb.pm.make_mut();
    let w = r.w as usize;
    pm.data_mut().par_chunks_mut(w * 4).enumerate().for_each(|(yy, row)| {
        let y = (r.y + yy as i32) as f64 + 0.5;
        for xx in 0..w {
            let x = (r.x + xx as i32) as f64 + 0.5;
            let (u, v) = inv.apply(x, y);
            if !(-0.01..=1.01).contains(&u) || !(-0.01..=1.01).contains(&v) {
                continue;
            }
            let s = sample(sd, sw, sh, (u * sw as f64) as f32, (v * sh as f64) as f32);
            let o = &mut row[xx * 4..xx * 4 + 4];
            for k in 0..4 {
                o[k] = s[k].round().clamp(0.0, 255.0) as u8;
            }
        }
    });
    Some(lb)
}

/// Rotates a 3D point (x,y,z) by rx, ry, rz (degrees), X then Y then Z.
pub fn rotate3(p: (f64, f64, f64), rx: f64, ry: f64, rz: f64) -> (f64, f64, f64) {
    let (x, y, z) = p;
    let (s, c) = rx.to_radians().sin_cos();
    let (y, z) = (y * c - z * s, y * s + z * c);
    let (s, c) = ry.to_radians().sin_cos();
    let (x, z) = (x * c + z * s, -x * s + z * c);
    let (s, c) = rz.to_radians().sin_cos();
    (x * c - y * s, x * s + y * c, z)
}

/// AE-like default camera projection for a comp of size (cw, ch).
pub fn project(p: (f64, f64, f64), cw: f64, ch: f64) -> (f64, f64) {
    let zoom = cw * 0.926;
    let k = zoom / (p.2 + zoom).max(1.0);
    (cw / 2.0 + (p.0 - cw / 2.0) * k, ch / 2.0 + (p.1 - ch / 2.0) * k)
}
