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

type V3 = (f64, f64, f64);

fn sub(a: V3, b: V3) -> V3 {
    (a.0 - b.0, a.1 - b.1, a.2 - b.2)
}
fn dot(a: V3, b: V3) -> f64 {
    a.0 * b.0 + a.1 * b.1 + a.2 * b.2
}
fn cross(a: V3, b: V3) -> V3 {
    (a.1 * b.2 - a.2 * b.1, a.2 * b.0 - a.0 * b.2, a.0 * b.1 - a.1 * b.0)
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-12);
    (a.0 / l, a.1 / l, a.2 / l)
}

/// A comp camera (x right, y down, z into the screen, like After Effects):
/// position, orthonormal basis (right, down, forward) and focal length in px.
#[derive(Clone, Copy, Debug)]
pub struct Cam {
    pub pos: V3,
    pub right: V3,
    pub down: V3,
    pub fwd: V3,
    pub zoom: f64,
    pub cw: f64,
    pub ch: f64,
}

impl Cam {
    /// The default camera: centered, `zoom` px in front of z = 0, looking straight in.
    pub fn default_for(cw: f64, ch: f64) -> Cam {
        let zoom = cw * 0.926;
        Cam { pos: (cw / 2.0, ch / 2.0, -zoom), right: (1.0, 0.0, 0.0), down: (0.0, 1.0, 0.0), fwd: (0.0, 0.0, 1.0), zoom, cw, ch }
    }

    /// A camera at `pos` aimed at `poi` (two-node), or looking down +z when
    /// `poi` is None (one-node), then turned by its own rx / ry / rz.
    pub fn new(pos: V3, poi: Option<V3>, rx: f64, ry: f64, rz: f64, zoom: f64, cw: f64, ch: f64) -> Cam {
        let (mut right, mut down, mut fwd) = ((1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0));
        if let Some(t) = poi {
            let f = sub(t, pos);
            if dot(f, f) > 1e-9 {
                fwd = norm(f);
                let r = cross((0.0, 1.0, 0.0), fwd);
                // looking straight up or down: keep the previous right vector
                right = if dot(r, r) > 1e-9 { norm(r) } else { (1.0, 0.0, 0.0) };
                down = cross(fwd, right);
            }
        }
        if rx.abs() > 1e-9 || ry.abs() > 1e-9 || rz.abs() > 1e-9 {
            // the camera's own orientation, applied in its local frame
            let rot = |v: V3| {
                let l = rotate3(v, rx, ry, rz);
                (right.0 * l.0 + down.0 * l.1 + fwd.0 * l.2, right.1 * l.0 + down.1 * l.1 + fwd.1 * l.2, right.2 * l.0 + down.2 * l.1 + fwd.2 * l.2)
            };
            let (r, d, f) = (rot((1.0, 0.0, 0.0)), rot((0.0, 1.0, 0.0)), rot((0.0, 0.0, 1.0)));
            (right, down, fwd) = (r, d, f);
        }
        Cam { pos, right, down, fwd, zoom: zoom.max(1.0), cw, ch }
    }

    /// Distance along the view axis (bigger = farther away).
    pub fn depth(&self, p: V3) -> f64 {
        dot(sub(p, self.pos), self.fwd)
    }

    /// Screen position of a world point, or None behind (or at) the camera.
    pub fn project(&self, p: V3) -> Option<(f64, f64)> {
        let v = sub(p, self.pos);
        let z = dot(v, self.fwd);
        if z < 1.0 {
            return None;
        }
        let k = self.zoom / z;
        Some((self.cw / 2.0 + dot(v, self.right) * k, self.ch / 2.0 + dot(v, self.down) * k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_camera_matches_the_fixed_projection() {
        let c = Cam::default_for(1920.0, 1080.0);
        for p in [(100.0, 200.0, 0.0), (960.0, 540.0, 500.0), (1500.0, 90.0, -300.0)] {
            let a = project(p, 1920.0, 1080.0);
            let b = c.project(p).unwrap();
            assert!((a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6, "{a:?} {b:?}");
        }
        // a two-node camera looking at the comp center from the default spot is the same camera
        let t = Cam::new((960.0, 540.0, -1777.92), Some((960.0, 540.0, 0.0)), 0.0, 0.0, 0.0, 1777.92, 1920.0, 1080.0);
        let b = t.project((100.0, 200.0, 0.0)).unwrap();
        let a = project((100.0, 200.0, 0.0), 1920.0, 1080.0);
        assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3);
        assert!(c.project((0.0, 0.0, -2000.0)).is_none(), "behind the camera");
    }
}
