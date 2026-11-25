//! Planar tracking (as Mocha's planar tracker and After Effects' mask
//! tracker): the features inside a region are followed with Lucas-Kanade,
//! and every frame gets a transform from the reference frame fitted with
//! RANSAC: translation, rotation, similarity, affine or full perspective
//! (a homography). Features found later inside the moving region get their
//! reference positions through the current transform, so the track doesn't
//! drift as features come and go.

use super::klt::FeatureTracker;
use super::linalg::{inv, null_vector, M3};
use super::track::Gray;

/// Motion model, from the fewest degrees of freedom to the most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// Position only.
    Translate,
    /// Position and rotation.
    Rigid,
    /// Position, rotation and uniform scale.
    Similarity,
    /// Position, rotation, scale and skew.
    Affine,
    /// Perspective (corner pin, screens, signs, walls, floors).
    Perspective,
}

impl Model {
    pub fn parse(s: &str) -> Option<Model> {
        Some(match s.to_ascii_lowercase().as_str() {
            "pos" | "position" | "translate" | "translation" => Model::Translate,
            "rotation" | "rot" | "rigid" | "posrot" => Model::Rigid,
            "similarity" | "sim" | "scale" | "posscalerot" => Model::Similarity,
            "affine" | "skew" => Model::Affine,
            "perspective" | "planar" | "homography" | "persp" | "3d" => Model::Perspective,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Model::Translate => "pos",
            Model::Rigid => "rotation",
            Model::Similarity => "similarity",
            Model::Affine => "affine",
            Model::Perspective => "perspective",
        }
    }

    fn min_pts(&self) -> usize {
        match self {
            Model::Translate => 1,
            Model::Rigid | Model::Similarity => 2,
            Model::Affine => 3,
            Model::Perspective => 4,
        }
    }
}

/// Maps a point through a 3x3 transform (homogeneous).
#[inline]
pub fn apply(h: &M3, x: f64, y: f64) -> (f64, f64) {
    let w = h[2][0] * x + h[2][1] * y + h[2][2];
    let w = if w.abs() < 1e-12 { 1e-12 } else { w };
    ((h[0][0] * x + h[0][1] * y + h[0][2]) / w, (h[1][0] * x + h[1][1] * y + h[1][2]) / w)
}

/// Normalizing transform (centroid to the origin, mean distance sqrt 2).
fn normalizer(p: &[(f64, f64)]) -> M3 {
    let n = p.len().max(1) as f64;
    let (mx, my) = p.iter().fold((0.0, 0.0), |a, q| (a.0 + q.0 / n, a.1 + q.1 / n));
    let d = p.iter().map(|q| ((q.0 - mx).powi(2) + (q.1 - my).powi(2)).sqrt()).sum::<f64>() / n;
    let s = if d > 1e-12 { std::f64::consts::SQRT_2 / d } else { 1.0 };
    [[s, 0.0, -s * mx], [0.0, s, -s * my], [0.0, 0.0, 1.0]]
}

/// Homography from >= 4 correspondences (normalized DLT).
pub fn homography(src: &[(f64, f64)], dst: &[(f64, f64)]) -> Option<M3> {
    if src.len() < 4 || src.len() != dst.len() {
        return None;
    }
    let (t1, t2) = (normalizer(src), normalizer(dst));
    let rows: Vec<Vec<f64>> = src
        .iter()
        .zip(dst)
        .flat_map(|(&(x, y), &(u, v))| {
            let (x, y) = apply(&t1, x, y);
            let (u, v) = apply(&t2, u, v);
            [vec![-x, -y, -1.0, 0.0, 0.0, 0.0, u * x, u * y, u], vec![0.0, 0.0, 0.0, -x, -y, -1.0, v * x, v * y, v]]
        })
        .collect();
    let h = null_vector(&rows, 9);
    let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], h[8]]];
    let t2i = inv(&t2)?;
    let m = super::linalg::mul(&super::linalg::mul(&t2i, &hn), &t1);
    let k = m[2][2];
    if k.abs() < 1e-12 || !m.iter().flatten().all(|v| v.is_finite()) {
        return None;
    }
    Some(m.map(|r| r.map(|v| v / k)))
}

/// Weighted least-squares fit of the model (all points).
pub fn fit(model: Model, src: &[(f64, f64)], dst: &[(f64, f64)]) -> Option<M3> {
    let n = src.len();
    if n < model.min_pts() {
        return None;
    }
    let nf = n as f64;
    let (sx, sy) = src.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / nf, a.1 + p.1 / nf));
    let (dx, dy) = dst.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / nf, a.1 + p.1 / nf));
    let lin = |a: f64, b: f64, c: f64, d: f64| -> M3 { [[a, b, dx - (a * sx + b * sy)], [c, d, dy - (c * sx + d * sy)], [0.0, 0.0, 1.0]] };
    match model {
        Model::Translate => Some(lin(1.0, 0.0, 0.0, 1.0)),
        Model::Rigid | Model::Similarity => {
            let (mut a, mut b, mut den) = (0.0, 0.0, 0.0);
            for (p, q) in src.iter().zip(dst) {
                let (x, y, u, v) = (p.0 - sx, p.1 - sy, q.0 - dx, q.1 - dy);
                a += x * u + y * v;
                b += x * v - y * u;
                den += x * x + y * y;
            }
            if den < 1e-12 {
                return Some(lin(1.0, 0.0, 0.0, 1.0));
            }
            let (a, b) = if model == Model::Rigid {
                let th = b.atan2(a);
                (th.cos(), th.sin())
            } else {
                (a / den, b / den)
            };
            Some(lin(a, -b, b, a))
        }
        Model::Affine => {
            let pairs: Vec<((f64, f64), (f64, f64), f64)> = src.iter().zip(dst).map(|(p, q)| (*p, *q, 1.0)).collect();
            let m = super::track::fit_affine(&pairs);
            Some([[m[0], m[1], m[2]], [m[3], m[4], m[5]], [0.0, 0.0, 1.0]])
        }
        Model::Perspective => homography(src, dst),
    }
}

/// Deterministic sampler.
struct Rng(u64);
impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n.max(1) as u64) as usize
    }
}

/// RANSAC fit: the model and which correspondences are inliers (within
/// `thr` px). None when too few points agree.
pub fn ransac(model: Model, src: &[(f64, f64)], dst: &[(f64, f64)], thr: f64) -> Option<(M3, Vec<bool>)> {
    let n = src.len();
    let k = model.min_pts();
    if n < k {
        return None;
    }
    let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ n as u64);
    let t2 = thr * thr;
    let count = |h: &M3| -> usize {
        src.iter()
            .zip(dst)
            .filter(|(p, q)| {
                let (u, v) = apply(h, p.0, p.1);
                (u - q.0).powi(2) + (v - q.1).powi(2) < t2
            })
            .count()
    };
    let mut best: Option<(usize, M3)> = None;
    let iters = if k == 1 { 60 } else { 400 };
    for _ in 0..iters {
        let mut idx: Vec<usize> = vec![];
        while idx.len() < k {
            let i = rng.below(n);
            if !idx.contains(&i) {
                idx.push(i);
            }
        }
        let s: Vec<(f64, f64)> = idx.iter().map(|&i| src[i]).collect();
        let d: Vec<(f64, f64)> = idx.iter().map(|&i| dst[i]).collect();
        let Some(h) = fit(model, &s, &d) else { continue };
        let c = count(&h);
        if best.as_ref().is_none_or(|b| c > b.0) {
            best = Some((c, h));
            if c * 10 >= n * 9 {
                break;
            }
        }
    }
    let (_, mut h) = best?;
    let mut inl: Vec<bool> = vec![];
    for _ in 0..3 {
        inl = src
            .iter()
            .zip(dst)
            .map(|(p, q)| {
                let (u, v) = apply(&h, p.0, p.1);
                (u - q.0).powi(2) + (v - q.1).powi(2) < t2
            })
            .collect();
        let (s, d): (Vec<(f64, f64)>, Vec<(f64, f64)>) = src.iter().zip(dst).zip(&inl).filter(|(_, ok)| **ok).map(|((p, q), _)| (*p, *q)).unzip();
        if s.len() < k {
            return None;
        }
        match fit(model, &s, &d) {
            Some(h2) => h = h2,
            None => break,
        }
    }
    (inl.iter().filter(|x| **x).count() >= k).then_some((h, inl))
}

/// Point in polygon (even-odd).
pub fn inside(poly: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1 + 1e-300) + a.0 {
            c = !c;
        }
    }
    c
}

/// The polygon pulled toward its centroid by `k` (0.1 = 10 percent).
fn shrink(poly: &[(f64, f64)], k: f64) -> Vec<(f64, f64)> {
    let n = poly.len().max(1) as f64;
    let (cx, cy) = poly.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n));
    poly.iter().map(|p| (p.0 + (cx - p.0) * k, p.1 + (cy - p.1) * k)).collect()
}

/// One tracked frame: the transform from the reference frame's px to this
/// frame's, and a confidence 0..1.
pub type Fit = Option<(M3, f64)>;

/// Tracks the plane inside `poly` (px in the reference frame `start`) through
/// `frames`, forward and backward from `start`.
pub fn track_region(frames: &[Gray], start: usize, poly: &[(f64, f64)], model: Model) -> Vec<Fit> {
    let n = frames.len();
    let mut out: Vec<Fit> = vec![None; n];
    if n == 0 || start >= n || poly.len() < 3 {
        return out;
    }
    let fwd: Vec<usize> = (start..n).collect();
    let back: Vec<usize> = (0..=start).rev().collect();
    for order in [fwd, back] {
        for (fi, fit) in run(frames, &order, poly, model) {
            if out[fi].is_none() || fi != start {
                out[fi] = fit;
            }
        }
    }
    out
}

fn run(frames: &[Gray], order: &[usize], poly: &[(f64, f64)], model: Model) -> Vec<(usize, Fit)> {
    let (w, h) = (frames[0].w, frames[0].h);
    let area = {
        let mut a = 0.0;
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            a += p.0 * q.1 - q.0 * p.1;
        }
        (a / 2.0).abs()
    };
    // a feature every ~14 px of the region, between 60 and 400
    let target = ((area / 196.0) as usize).clamp(60, 400);
    let mut ft = FeatureTracker::new(w, h, target);
    ft.min_dist = ((area / target as f64 / 2.0).sqrt() as f32).clamp(5.0, 30.0);
    let mut refs: Vec<Option<(f64, f64)>> = vec![];
    let mut cur: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut out = vec![];
    for (step, &fi) in order.iter().enumerate() {
        let region: Vec<(f64, f64)> = shrink(&poly.iter().map(|p| apply(&cur, p.0, p.1)).collect::<Vec<_>>(), 0.04);
        let keep = move |x: f32, y: f32| inside(&region, x as f64, y as f64);
        ft.push(&frames[fi].d, Some(&keep));
        refs.resize(ft.tracks.len(), None);
        let act = ft.active();
        if step == 0 {
            for &(i, x, y) in &act {
                refs[i] = Some((x as f64, y as f64));
            }
            out.push((fi, Some((cur, 1.0))));
            continue;
        }
        let (mut src, mut dst, mut ids) = (vec![], vec![], vec![]);
        for &(i, x, y) in &act {
            if let Some(r) = refs[i] {
                src.push(r);
                dst.push((x as f64, y as f64));
                ids.push(i);
            }
        }
        let thr = (w.max(h) as f64 / 400.0).clamp(1.5, 3.0);
        match ransac(model, &src, &dst, thr) {
            Some((m, inl)) if inl.iter().filter(|x| **x).count() >= model.min_pts().max(4).min(src.len()) => {
                let good = inl.iter().filter(|x| **x).count();
                for (k, ok) in inl.iter().enumerate() {
                    if !ok {
                        ft.drop_track(ids[k]);
                    }
                }
                // confidence: share of agreeing features and enough of them
                let conf = (good as f64 / src.len().max(1) as f64) * (good as f64 / 12.0).min(1.0);
                cur = m;
                out.push((fi, Some((cur, conf))));
            }
            _ => {
                // nothing to fit: hold the last transform, flag it as lost
                out.push((fi, Some((cur, 0.0))));
            }
        }
        // new features get reference positions through the inverse
        if let Some(back) = inv(&cur) {
            for &(i, x, y) in &ft.active() {
                if refs[i].is_none() {
                    refs[i] = Some(apply(&back, x as f64, y as f64));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::klt::tests::texture;

    #[test]
    fn homography_fit_is_exact() {
        let h: M3 = [[1.1, 0.05, 12.0], [-0.03, 0.95, -4.0], [0.0004, -0.0002, 1.0]];
        let src: Vec<(f64, f64)> = vec![(10.0, 10.0), (300.0, 20.0), (280.0, 220.0), (30.0, 200.0), (150.0, 120.0), (90.0, 60.0)];
        let dst: Vec<(f64, f64)> = src.iter().map(|p| apply(&h, p.0, p.1)).collect();
        let g = homography(&src, &dst).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                assert!((g[i][j] - h[i][j]).abs() < 1e-6 * (1.0 + h[i][j].abs()), "{g:?}");
            }
        }
        // one gross outlier is rejected
        let mut d2 = dst.clone();
        d2.push((500.0, 500.0));
        let mut s2 = src.clone();
        s2.push((100.0, 100.0));
        let (_, inl) = ransac(Model::Perspective, &s2, &d2, 1.0).unwrap();
        assert!(!inl[6] && inl[..6].iter().all(|x| *x));
        assert!(inside(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], 5.0, 5.0));
        assert!(!inside(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], 15.0, 5.0));
    }

    #[test]
    fn follows_a_plane_in_perspective() {
        let (w, h) = (320usize, 200usize);
        let tex = texture(w * 2, h * 2, 0.0, 0.0);
        // frames: the texture seen through a slowly changing homography
        let hs: Vec<M3> = (0..10)
            .map(|i| {
                let t = i as f64;
                [[1.0 + 0.01 * t, 0.02 * t / 10.0, 30.0 + 2.0 * t], [-0.01 * t / 10.0, 1.0 + 0.008 * t, 20.0 + 1.2 * t], [0.0002 * t / 10.0, 0.0001 * t / 10.0, 1.0]]
            })
            .collect();
        let frames: Vec<Gray> = hs
            .iter()
            .map(|hm| {
                let hi = inv(hm).unwrap();
                let mut d = vec![0u8; w * h];
                for y in 0..h {
                    for x in 0..w {
                        // frame px -> texture px
                        let (u, v) = apply(&hi, x as f64, y as f64);
                        let (u, v) = (u.clamp(0.0, (w * 2 - 2) as f64), v.clamp(0.0, (h * 2 - 2) as f64));
                        let (x0, y0) = (u as usize, v as usize);
                        let (fx, fy) = (u - x0 as f64, v - y0 as f64);
                        let p = |xx: usize, yy: usize| tex[yy * w * 2 + xx] as f64;
                        let a = p(x0, y0) + (p(x0 + 1, y0) - p(x0, y0)) * fx;
                        let b = p(x0, y0 + 1) + (p(x0 + 1, y0 + 1) - p(x0, y0 + 1)) * fx;
                        d[y * w + x] = (a + (b - a) * fy).round() as u8;
                    }
                }
                Gray::new(w, h, d)
            })
            .collect();
        let quad = [(80.0, 50.0), (240.0, 50.0), (240.0, 150.0), (80.0, 150.0)];
        let fits = track_region(&frames, 0, &quad, Model::Perspective);
        // compare the tracked corners with the truth: H_i * H_0^-1
        let h0i = inv(&hs[0]).unwrap();
        for i in 0..10 {
            let (m, conf) = fits[i].unwrap();
            assert!(conf > 0.5, "frame {i} conf {conf}");
            let truth = super::super::linalg::mul(&hs[i], &h0i);
            for &(x, y) in &quad {
                let (a, b) = (apply(&m, x, y), apply(&truth, x, y));
                assert!((a.0 - b.0).abs() < 0.8 && (a.1 - b.1).abs() < 0.8, "frame {i}: {a:?} vs {b:?}");
            }
        }
    }
}
