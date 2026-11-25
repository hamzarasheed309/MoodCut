//! 3D camera tracking (structure from motion), the solver behind `camtrack`:
//! from 2D feature tracks to a camera path (rotation, position and one focal
//! length) and a cloud of 3D track points.
//!
//! Keyframes are picked as the picture moves. Two keyframes with enough
//! parallax start the reconstruction (essential matrix by RANSAC, pose,
//! triangulation); the other keyframes are added one at a time (pose from the
//! solved points, then new points triangulated), with local bundle
//! adjustment on the way and a global one at the end. The focal length is
//! found by solving with several fields of view and refining the best one in
//! the bundle adjustment. Shots without parallax (tripod pans and tilts) are
//! solved as a camera that only rotates. Finally every frame is posed from
//! the solved points.
//!
//! Conventions: image px with x right and y down; a camera maps a world
//! point X to `R X + t` (x right, y down, z forward), as MoodCut's comp
//! cameras do. The result is in "solve space": the reference frame's camera
//! sits at the origin looking down +z, and the median depth of the points
//! seen from it is 1.

use super::klt::Track;
use super::linalg::*;
use rayon::prelude::*;

/// A camera pose: x_cam = r * x_world + t.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub r: M3,
    pub t: V3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose { r: I3, t: [0.0; 3] };

    pub fn center(&self) -> V3 {
        scale(mulv(&tr(&self.r), self.t), -1.0)
    }
    pub fn from_center(r: M3, c: V3) -> Pose {
        Pose { r, t: scale(mulv(&r, c), -1.0) }
    }
    #[inline]
    pub fn cam(&self, x: V3) -> V3 {
        add(mulv(&self.r, x), self.t)
    }
    /// Interpolates rotation (geodesic) and center (linear).
    pub fn lerp(&self, o: &Pose, k: f64) -> Pose {
        let (c0, c1) = (self.center(), o.center());
        Pose::from_center(slerp(&self.r, &o.r, k), add(c0, scale(sub(c1, c0), k)))
    }
}

/// Image size, principal point (the center) and focal length in px.
#[derive(Clone, Copy, Debug)]
pub struct Cam {
    pub f: f64,
    pub cx: f64,
    pub cy: f64,
}

impl Cam {
    #[inline]
    pub fn project(&self, xc: V3) -> Option<(f64, f64)> {
        if xc[2] <= 1e-9 {
            return None;
        }
        Some((self.cx + self.f * xc[0] / xc[2], self.cy + self.f * xc[1] / xc[2]))
    }
    #[inline]
    fn norm(&self, u: f64, v: f64) -> (f64, f64) {
        ((u - self.cx) / self.f, (v - self.cy) / self.f)
    }
}

/// Focal length (px) for a horizontal field of view (degrees) at width w.
pub fn focal_for_fov(fov: f64, w: f64) -> f64 {
    (w / 2.0) / (fov.clamp(1.0, 170.0).to_radians() / 2.0).tan()
}

/// Horizontal field of view (degrees) of a focal length at width w.
pub fn fov_of(f: f64, w: f64) -> f64 {
    2.0 * ((w / 2.0) / f).atan().to_degrees()
}

/// Solver settings.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Analysis image size (px).
    pub w: f64,
    pub h: f64,
    /// Known horizontal field of view in degrees (None: solve for it).
    pub fov: Option<f64>,
    /// Frame whose camera defines solve space.
    pub reference: usize,
    /// Force a rotation-only (tripod) solve.
    pub nodal: bool,
}

/// A solved 3D point.
#[derive(Clone, Debug)]
pub struct Point3 {
    /// Index of the feature track it came from.
    pub track: usize,
    pub x: V3,
    /// RMS reprojection error over all frames that see it (px).
    pub err: f64,
    /// Frames it was seen in.
    pub views: usize,
}

/// The solved camera path and points.
#[derive(Clone, Debug)]
pub struct Solution {
    pub f: f64,
    /// Per frame (None where the camera couldn't be solved).
    pub poses: Vec<Option<Pose>>,
    pub points: Vec<Point3>,
    /// RMS reprojection error over every frame (px).
    pub rms: f64,
    /// Rotation-only (tripod) solve.
    pub nodal: bool,
    pub keyframes: usize,
}

// ---------------------------------------------------------------- observations

/// Position of track `ti` in frame `f`, if it was seen there.
#[inline]
fn obs_at(tracks: &[Track], ti: usize, f: usize) -> Option<(f64, f64)> {
    let t = &tracks[ti];
    let first = t.first()?.0 as usize;
    if f < first {
        return None;
    }
    t.get(f - first).filter(|p| p.0 as usize == f).map(|p| (p.1 as f64, p.2 as f64))
}

/// Tracks seen in frame `f`.
fn seen_in(by_frame: &[Vec<usize>], f: usize) -> &[usize] {
    by_frame.get(f).map(|v| v.as_slice()).unwrap_or(&[])
}

/// Keyframes: the first usable frame, then one whenever the picture has
/// moved enough or many tracks have ended, and the last frame.
fn keyframes(tracks: &[Track], by_frame: &[Vec<usize>], w: f64) -> Vec<usize> {
    let n = by_frame.len();
    let Some(first) = (0..n).find(|&f| by_frame[f].len() >= 20) else { return vec![] };
    let mut kfs = vec![first];
    let mut last = first;
    for f in first + 1..n {
        if by_frame[f].len() < 12 {
            continue;
        }
        let mut d: Vec<f64> = vec![];
        for &ti in &by_frame[last] {
            if let (Some(a), Some(b)) = (obs_at(tracks, ti, last), obs_at(tracks, ti, f)) {
                d.push((a.0 - b.0).hypot(a.1 - b.1));
            }
        }
        let shared = d.len() as f64 / by_frame[last].len().max(1) as f64;
        d.sort_by(f64::total_cmp);
        let med = d.get(d.len() / 2).copied().unwrap_or(0.0);
        if (med > 0.04 * w && f - last >= 2) || shared < 0.65 || f - last >= 30 {
            kfs.push(f);
            last = f;
        }
    }
    if let Some(end) = (0..n).rev().find(|&f| by_frame[f].len() >= 12) {
        if end > last + 1 {
            kfs.push(end);
        } else if end == last + 1 {
            *kfs.last_mut().unwrap() = end;
        }
    }
    kfs.dedup();
    kfs
}

// ---------------------------------------------------------------- two-view geometry

/// Deterministic pseudo random numbers (xorshift), so solves repeat exactly.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn sample(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::with_capacity(k);
        while out.len() < k.min(n) {
            let i = self.below(n);
            if !out.contains(&i) {
                out.push(i);
            }
        }
        out
    }
}

/// Essential matrix from >= 8 normalized correspondences (x1 -> x2) with the
/// two-equal-singular-values constraint enforced.
fn essential_8pt(p1: &[(f64, f64)], p2: &[(f64, f64)]) -> Option<M3> {
    if p1.len() < 8 {
        return None;
    }
    let rows: Vec<Vec<f64>> = p1.iter().zip(p2).map(|(&(x1, y1), &(x2, y2))| vec![x2 * x1, x2 * y1, x2, y2 * x1, y2 * y1, y2, x1, y1, 1.0]).collect();
    let e = null_vector(&rows, 9);
    let m = [[e[0], e[1], e[2]], [e[3], e[4], e[5]], [e[6], e[7], e[8]]];
    let (u, s, v) = svd3(&m);
    let k = (s[0] + s[1]) / 2.0;
    if k < 1e-12 {
        return None;
    }
    let d = [[k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, 0.0]];
    Some(mul(&mul(&u, &d), &tr(&v)))
}

/// Sampson distance of a normalized correspondence to an essential matrix.
fn sampson(e: &M3, a: (f64, f64), b: (f64, f64)) -> f64 {
    let x1 = [a.0, a.1, 1.0];
    let x2 = [b.0, b.1, 1.0];
    let ex1 = mulv(e, x1);
    let etx2 = mulv(&tr(e), x2);
    let num = dot(x2, ex1);
    let den = ex1[0] * ex1[0] + ex1[1] * ex1[1] + etx2[0] * etx2[0] + etx2[1] * etx2[1];
    if den < 1e-300 {
        return f64::INFINITY;
    }
    num * num / den
}

/// RANSAC essential matrix; `thr` is the inlier distance in normalized units.
fn ransac_essential(p1: &[(f64, f64)], p2: &[(f64, f64)], thr: f64, rng: &mut Rng) -> Option<(M3, Vec<bool>)> {
    let n = p1.len();
    if n < 12 {
        return None;
    }
    let t2 = thr * thr;
    let mut best: Option<(usize, M3)> = None;
    for _ in 0..400 {
        let idx = rng.sample(n, 8);
        let a: Vec<(f64, f64)> = idx.iter().map(|&i| p1[i]).collect();
        let b: Vec<(f64, f64)> = idx.iter().map(|&i| p2[i]).collect();
        let Some(e) = essential_8pt(&a, &b) else { continue };
        let cnt = (0..n).filter(|&i| sampson(&e, p1[i], p2[i]) < t2).count();
        if best.as_ref().is_none_or(|b| cnt > b.0) {
            best = Some((cnt, e));
            if cnt as f64 > 0.95 * n as f64 {
                break;
            }
        }
    }
    let (_, e) = best?;
    // refit on the inliers, twice
    let mut e = e;
    let mut inl: Vec<bool> = (0..n).map(|i| sampson(&e, p1[i], p2[i]) < t2).collect();
    for _ in 0..2 {
        let a: Vec<(f64, f64)> = (0..n).filter(|&i| inl[i]).map(|i| p1[i]).collect();
        let b: Vec<(f64, f64)> = (0..n).filter(|&i| inl[i]).map(|i| p2[i]).collect();
        if let Some(e2) = essential_8pt(&a, &b) {
            let inl2: Vec<bool> = (0..n).map(|i| sampson(&e2, p1[i], p2[i]) < t2).collect();
            if inl2.iter().filter(|x| **x).count() >= inl.iter().filter(|x| **x).count() {
                e = e2;
                inl = inl2;
            }
        }
    }
    Some((e, inl))
}

/// Linear triangulation from normalized observations in two or more views.
fn triangulate(views: &[(Pose, (f64, f64))]) -> Option<V3> {
    let mut rows = vec![];
    for (p, (x, y)) in views {
        let pr = |i: usize| [p.r[i][0], p.r[i][1], p.r[i][2], p.t[i]];
        let (r0, r1, r2) = (pr(0), pr(1), pr(2));
        rows.push((0..4).map(|k| x * r2[k] - r0[k]).collect::<Vec<f64>>());
        rows.push((0..4).map(|k| y * r2[k] - r1[k]).collect::<Vec<f64>>());
    }
    let h = null_vector(&rows, 4);
    if h[3].abs() < 1e-12 {
        return None;
    }
    let x = [h[0] / h[3], h[1] / h[3], h[2] / h[3]];
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Angle (degrees) between the rays from two camera centers to X.
fn parallax(c1: V3, c2: V3, x: V3) -> f64 {
    let (a, b) = (unit(sub(x, c1)), unit(sub(x, c2)));
    dot(a, b).clamp(-1.0, 1.0).acos().to_degrees()
}

/// The pose of view 2 (view 1 at the origin) encoded by E: the one of four
/// that puts the most points in front of both cameras.
fn pose_from_essential(e: &M3, p1: &[(f64, f64)], p2: &[(f64, f64)]) -> Option<Pose> {
    let (mut u, _, mut v) = svd3(e);
    if det(&u) < 0.0 {
        u = mul(&u, &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]]);
    }
    if det(&v) < 0.0 {
        v = mul(&v, &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]]);
    }
    let w = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let r1 = mul(&mul(&u, &w), &tr(&v));
    let r2 = mul(&mul(&u, &tr(&w)), &tr(&v));
    let t = [u[0][2], u[1][2], u[2][2]];
    let mut best: Option<(usize, Pose)> = None;
    let step = (p1.len() / 60).max(1);
    for r in [r1, r2] {
        for tt in [t, scale(t, -1.0)] {
            let pose = Pose { r, t: tt };
            let mut front = 0;
            for i in (0..p1.len()).step_by(step) {
                if let Some(x) = triangulate(&[(Pose::IDENTITY, p1[i]), (pose, p2[i])]) {
                    if x[2] > 0.0 && pose.cam(x)[2] > 0.0 {
                        front += 1;
                    }
                }
            }
            if best.as_ref().is_none_or(|b| front > b.0) {
                best = Some((front, pose));
            }
        }
    }
    best.map(|b| b.1)
}

// ---------------------------------------------------------------- pose from points

/// Jacobian of the projection of a camera-space point.
#[inline]
fn proj_jac(c: &Cam, xc: V3) -> [[f64; 3]; 2] {
    let iz = 1.0 / xc[2];
    let iz2 = iz * iz;
    [[c.f * iz, 0.0, -c.f * xc[0] * iz2], [0.0, c.f * iz, -c.f * xc[1] * iz2]]
}

#[inline]
fn huber_w(r2: f64, d: f64) -> f64 {
    let r = r2.sqrt();
    if r <= d {
        1.0
    } else {
        d / r
    }
}

/// Refines a pose from 2D-3D correspondences (Levenberg-Marquardt with a
/// Huber loss). Returns the pose and the RMS error (px) of the inliers.
pub fn refine_pose(cam: &Cam, init: Pose, corr: &[(V3, (f64, f64))], iters: usize) -> (Pose, f64) {
    let mut pose = init;
    let mut lambda = 1e-3;
    let cost = |p: &Pose| -> f64 {
        corr.iter()
            .map(|(x, (u, v))| match cam.project(p.cam(*x)) {
                Some((pu, pv)) => {
                    let r2 = (u - pu).powi(2) + (v - pv).powi(2);
                    let w = huber_w(r2, 2.0);
                    if w >= 1.0 {
                        r2
                    } else {
                        2.0 * 2.0 * r2.sqrt() - 4.0
                    }
                }
                None => 400.0,
            })
            .sum()
    };
    let mut cur = cost(&pose);
    for _ in 0..iters {
        let mut h = [0.0f64; 36];
        let mut g = [0.0f64; 6];
        for (x, (u, v)) in corr {
            let rx = mulv(&pose.r, *x);
            let xc = add(rx, pose.t);
            let Some((pu, pv)) = cam.project(xc) else { continue };
            let res = [u - pu, v - pv];
            let w = huber_w(res[0] * res[0] + res[1] * res[1], 2.0);
            let jp = proj_jac(cam, xc);
            let sk = skew(rx);
            // d xc / d omega = -[R X]x, d xc / d t = I
            let mut j = [[0.0f64; 6]; 2];
            for r in 0..2 {
                for c in 0..3 {
                    j[r][c] = -(jp[r][0] * sk[0][c] + jp[r][1] * sk[1][c] + jp[r][2] * sk[2][c]);
                    j[r][3 + c] = jp[r][c];
                }
            }
            for a in 0..6 {
                for b in 0..6 {
                    h[a * 6 + b] += w * (j[0][a] * j[0][b] + j[1][a] * j[1][b]);
                }
                g[a] += w * (j[0][a] * res[0] + j[1][a] * res[1]);
            }
        }
        let mut improved = false;
        for _ in 0..6 {
            let mut hm = h;
            for d in 0..6 {
                hm[d * 6 + d] *= 1.0 + lambda;
                hm[d * 6 + d] += 1e-9;
            }
            let mut dx = g;
            if !cholesky_solve(&mut hm, 6, &mut dx) {
                lambda *= 10.0;
                continue;
            }
            let np = Pose { r: mul(&rodrigues([dx[0], dx[1], dx[2]]), &pose.r), t: add(pose.t, [dx[3], dx[4], dx[5]]) };
            let nc = cost(&np);
            if nc < cur {
                let small = dx.iter().map(|v| v * v).sum::<f64>() < 1e-16;
                pose = np;
                cur = nc;
                lambda = (lambda * 0.3).max(1e-9);
                improved = !small;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    pose.r = orthonormalize(&pose.r);
    let errs: Vec<f64> = corr.iter().filter_map(|(x, (u, v))| cam.project(pose.cam(*x)).map(|(pu, pv)| (u - pu).powi(2) + (v - pv).powi(2))).filter(|e| *e < 16.0).collect();
    let rms = if errs.len() < corr.len() / 2 { f64::INFINITY } else { (errs.iter().sum::<f64>() / errs.len().max(1) as f64).sqrt() };
    (pose, rms)
}

/// Linear (DLT) pose from >= 6 2D-3D correspondences, for when no nearby
/// pose is a good start.
fn dlt_pose(cam: &Cam, corr: &[(V3, (f64, f64))]) -> Option<Pose> {
    if corr.len() < 6 {
        return None;
    }
    let mut rows = vec![];
    for (x, (u, v)) in corr {
        let (nx, ny) = cam.norm(*u, *v);
        let xh = [x[0], x[1], x[2], 1.0];
        let mut r1 = vec![0.0; 12];
        let mut r2 = vec![0.0; 12];
        for k in 0..4 {
            r1[k] = xh[k];
            r1[8 + k] = -nx * xh[k];
            r2[4 + k] = xh[k];
            r2[8 + k] = -ny * xh[k];
        }
        rows.push(r1);
        rows.push(r2);
    }
    let p = null_vector(&rows, 12);
    let m = [[p[0], p[1], p[2]], [p[4], p[5], p[6]], [p[8], p[9], p[10]]];
    let mut t = [p[3], p[7], p[11]];
    let (_, s, _) = svd3(&m);
    let k = (s[0] + s[1] + s[2]) / 3.0;
    if k < 1e-12 {
        return None;
    }
    let mut r = orthonormalize(&scale_m(&m, 1.0 / k));
    t = scale(t, 1.0 / k);
    if det(&m) < 0.0 {
        r = orthonormalize(&scale_m(&m, -1.0 / k));
        t = scale(t, -1.0);
    }
    Some(Pose { r, t })
}

fn scale_m(m: &M3, k: f64) -> M3 {
    let mut o = *m;
    for row in o.iter_mut() {
        for v in row.iter_mut() {
            *v *= k;
        }
    }
    o
}

// ---------------------------------------------------------------- bundle adjustment

/// One observation for the bundle adjustment: camera, point, pixel.
#[derive(Clone, Copy)]
struct BaObs {
    cam: usize,
    pt: usize,
    u: f64,
    v: f64,
}

/// Sparse bundle adjustment (Schur complement, Levenberg-Marquardt, Huber
/// loss) over the cameras not marked fixed, the points, and optionally the
/// shared focal length. Returns the RMS error (px).
fn bundle_adjust(cam: &mut Cam, poses: &mut [Pose], fixed: &[bool], pts: &mut [V3], obs: &[BaObs], refine_f: bool, iters: usize) -> f64 {
    let nf = usize::from(refine_f);
    // parameter offsets of the free cameras
    let mut off = vec![usize::MAX; poses.len()];
    let mut nc = nf;
    for (j, o) in off.iter_mut().enumerate() {
        if !fixed[j] {
            *o = nc;
            nc += 6;
        }
    }
    // observations grouped by point
    let mut by_pt: Vec<Vec<usize>> = vec![vec![]; pts.len()];
    for (k, o) in obs.iter().enumerate() {
        by_pt[o.pt].push(k);
    }
    let huber = 2.0;
    let cost_of = |cam: &Cam, poses: &[Pose], pts: &[V3]| -> f64 {
        obs.par_iter()
            .map(|o| match cam.project(poses[o.cam].cam(pts[o.pt])) {
                Some((pu, pv)) => {
                    let r2 = (o.u - pu).powi(2) + (o.v - pv).powi(2);
                    if r2 <= huber * huber {
                        r2
                    } else {
                        2.0 * huber * r2.sqrt() - huber * huber
                    }
                }
                None => 100.0,
            })
            .sum()
    };
    let mut cur = cost_of(&*cam, &*poses, &*pts);
    let mut lambda = 1e-3;
    for _ in 0..iters {
        // per observation: residual, weight, camera jacobian (2 x 7: f, w, t) and point jacobian (2 x 3)
        struct J {
            r: [f64; 2],
            w: f64,
            jc: [[f64; 7]; 2],
            jp: [[f64; 3]; 2],
            ok: bool,
        }
        let js: Vec<J> = obs
            .par_iter()
            .map(|o| {
                let p = &poses[o.cam];
                let rx = mulv(&p.r, pts[o.pt]);
                let xc = add(rx, p.t);
                if xc[2] <= 1e-6 {
                    return J { r: [0.0; 2], w: 0.0, jc: [[0.0; 7]; 2], jp: [[0.0; 3]; 2], ok: false };
                }
                let (pu, pv) = (cam.cx + cam.f * xc[0] / xc[2], cam.cy + cam.f * xc[1] / xc[2]);
                let r = [o.u - pu, o.v - pv];
                let w = huber_w(r[0] * r[0] + r[1] * r[1], huber);
                let jp_ = proj_jac(cam, xc);
                let sk = skew(rx);
                let mut jc = [[0.0; 7]; 2];
                let mut jp = [[0.0; 3]; 2];
                for row in 0..2 {
                    jc[row][0] = xc[row] / xc[2];
                    for c in 0..3 {
                        jc[row][1 + c] = -(jp_[row][0] * sk[0][c] + jp_[row][1] * sk[1][c] + jp_[row][2] * sk[2][c]);
                        jc[row][4 + c] = jp_[row][c];
                        jp[row][c] = jp_[row][0] * p.r[0][c] + jp_[row][1] * p.r[1][c] + jp_[row][2] * p.r[2][c];
                    }
                }
                J { r, w, jc, jp, ok: true }
            })
            .collect();
        // global index of local camera parameter k (0 = f, 1..7 = pose) for camera j
        let gidx = |j: usize, k: usize| -> Option<usize> {
            if k == 0 {
                refine_f.then_some(0)
            } else if off[j] == usize::MAX {
                None
            } else {
                Some(off[j] + k - 1)
            }
        };
        let mut u = vec![0.0f64; nc * nc];
        let mut ec = vec![0.0f64; nc];
        let mut v = vec![[0.0f64; 9]; pts.len()];
        let mut ep = vec![[0.0f64; 3]; pts.len()];
        let mut wblk = vec![[[0.0f64; 3]; 7]; obs.len()];
        for (k, o) in obs.iter().enumerate() {
            let j = &js[k];
            if !j.ok {
                continue;
            }
            let w = j.w;
            for a in 0..7 {
                let Some(ga) = gidx(o.cam, a) else { continue };
                ec[ga] += w * (j.jc[0][a] * j.r[0] + j.jc[1][a] * j.r[1]);
                for b in 0..7 {
                    let Some(gb) = gidx(o.cam, b) else { continue };
                    u[ga * nc + gb] += w * (j.jc[0][a] * j.jc[0][b] + j.jc[1][a] * j.jc[1][b]);
                }
                for c in 0..3 {
                    wblk[k][a][c] = w * (j.jc[0][a] * j.jp[0][c] + j.jc[1][a] * j.jp[1][c]);
                }
            }
            for a in 0..3 {
                ep[o.pt][a] += w * (j.jp[0][a] * j.r[0] + j.jp[1][a] * j.r[1]);
                for b in 0..3 {
                    v[o.pt][a * 3 + b] += w * (j.jp[0][a] * j.jp[0][b] + j.jp[1][a] * j.jp[1][b]);
                }
            }
        }
        let mut accepted = false;
        for _attempt in 0..6 {
            // damped Schur complement
            let mut s = u.clone();
            for d in 0..nc {
                s[d * nc + d] += lambda * s[d * nc + d] + 1e-9;
            }
            let mut rhs = ec.clone();
            let mut vinv = vec![[0.0f64; 9]; pts.len()];
            for (i, list) in by_pt.iter().enumerate() {
                if list.is_empty() {
                    continue;
                }
                let mut vm = [[v[i][0], v[i][1], v[i][2]], [v[i][3], v[i][4], v[i][5]], [v[i][6], v[i][7], v[i][8]]];
                for d in 0..3 {
                    vm[d][d] += lambda * vm[d][d] + 1e-9;
                }
                let Some(vi) = inv(&vm) else { continue };
                vinv[i] = [vi[0][0], vi[0][1], vi[0][2], vi[1][0], vi[1][1], vi[1][2], vi[2][0], vi[2][1], vi[2][2]];
                // y_k = W_k V^-1 (7 x 3) for each observation of the point
                let ys: Vec<(usize, [[f64; 3]; 7])> = list
                    .iter()
                    .filter(|&&k| js[k].ok)
                    .map(|&k| {
                        let mut y = [[0.0; 3]; 7];
                        for a in 0..7 {
                            for c in 0..3 {
                                y[a][c] = wblk[k][a][0] * vi[0][c] + wblk[k][a][1] * vi[1][c] + wblk[k][a][2] * vi[2][c];
                            }
                        }
                        (k, y)
                    })
                    .collect();
                for (ka, ya) in &ys {
                    let ca = obs[*ka].cam;
                    for a in 0..7 {
                        let Some(ga) = gidx(ca, a) else { continue };
                        rhs[ga] -= ya[a][0] * ep[i][0] + ya[a][1] * ep[i][1] + ya[a][2] * ep[i][2];
                        for (kb, _) in &ys {
                            let cb = obs[*kb].cam;
                            for b in 0..7 {
                                let Some(gb) = gidx(cb, b) else { continue };
                                s[ga * nc + gb] -= ya[a][0] * wblk[*kb][b][0] + ya[a][1] * wblk[*kb][b][1] + ya[a][2] * wblk[*kb][b][2];
                            }
                        }
                    }
                }
            }
            let mut dc = rhs;
            if nc > 0 && !cholesky_solve(&mut s, nc, &mut dc) {
                lambda *= 10.0;
                continue;
            }
            // back substitution for the points
            let mut dp = vec![[0.0f64; 3]; pts.len()];
            for (i, list) in by_pt.iter().enumerate() {
                if list.is_empty() {
                    continue;
                }
                let mut b = ep[i];
                for &k in list {
                    if !js[k].ok {
                        continue;
                    }
                    let c = obs[k].cam;
                    for a in 0..7 {
                        let Some(ga) = gidx(c, a) else { continue };
                        for cc in 0..3 {
                            b[cc] -= wblk[k][a][cc] * dc[ga];
                        }
                    }
                }
                let vi = &vinv[i];
                dp[i] = [vi[0] * b[0] + vi[1] * b[1] + vi[2] * b[2], vi[3] * b[0] + vi[4] * b[1] + vi[5] * b[2], vi[6] * b[0] + vi[7] * b[1] + vi[8] * b[2]];
            }
            let mut ncam = *cam;
            if refine_f {
                ncam.f = (cam.f + dc[0]).max(cam.f * 0.2);
            }
            let nposes: Vec<Pose> = poses
                .iter()
                .enumerate()
                .map(|(j, p)| {
                    if off[j] == usize::MAX {
                        *p
                    } else {
                        let o = off[j];
                        Pose { r: mul(&rodrigues([dc[o], dc[o + 1], dc[o + 2]]), &p.r), t: add(p.t, [dc[o + 3], dc[o + 4], dc[o + 5]]) }
                    }
                })
                .collect();
            let npts: Vec<V3> = pts.iter().zip(&dp).map(|(x, d)| add(*x, *d)).collect();
            let nc_ = cost_of(&ncam, &nposes[..], &npts[..]);
            if nc_ < cur {
                let rel = (cur - nc_) / cur.max(1e-12);
                *cam = ncam;
                poses.copy_from_slice(&nposes);
                pts.copy_from_slice(&npts);
                cur = nc_;
                lambda = (lambda * 0.3).max(1e-10);
                accepted = rel > 1e-7;
                break;
            }
            lambda *= 8.0;
        }
        if !accepted {
            break;
        }
    }
    for p in poses.iter_mut() {
        p.r = orthonormalize(&p.r);
    }
    rms(cam, poses, pts, obs)
}

fn rms(cam: &Cam, poses: &[Pose], pts: &[V3], obs: &[BaObs]) -> f64 {
    if obs.is_empty() {
        return 0.0;
    }
    let s: f64 = obs.iter().map(|o| cam.project(poses[o.cam].cam(pts[o.pt])).map(|(pu, pv)| ((o.u - pu).powi(2) + (o.v - pv).powi(2)).min(100.0)).unwrap_or(100.0)).sum();
    (s / obs.len() as f64).sqrt()
}

// ---------------------------------------------------------------- reconstruction on keyframes

/// A keyframe reconstruction.
struct Recon {
    cam: Cam,
    /// Pose per keyframe (index into kfs).
    poses: Vec<Option<Pose>>,
    /// 3D point per track.
    pts: Vec<Option<V3>>,
    rms: f64,
}

struct Ctx<'a> {
    tracks: &'a [Track],
    by_frame: &'a [Vec<usize>],
    kfs: &'a [usize],
}

impl<'a> Ctx<'a> {
    fn obs(&self, ti: usize, k: usize) -> Option<(f64, f64)> {
        obs_at(self.tracks, ti, self.kfs[k])
    }

    /// Tracks seen in both keyframes, with their positions.
    fn common(&self, a: usize, b: usize) -> Vec<(usize, (f64, f64), (f64, f64))> {
        seen_in(self.by_frame, self.kfs[a]).iter().filter_map(|&ti| Some((ti, self.obs(ti, a)?, self.obs(ti, b)?))).collect()
    }

    /// Picks the keyframe pair to start from and its relative pose:
    /// enough shared tracks and the most parallax (median angle >= 1 deg).
    fn init(&self, cam: &Cam, rng: &mut Rng, pair: Option<(usize, usize)>) -> Option<(usize, usize, Pose, Vec<(usize, V3)>)> {
        let mut best: Option<(f64, usize, usize, Pose, Vec<(usize, V3)>)> = None;
        let thr = 1.5 / cam.f;
        let nk = self.kfs.len();
        let firsts: Vec<usize> = match pair {
            Some((a, _)) => vec![a],
            None => (0..nk.saturating_sub(1).min(8)).collect(),
        };
        for a in firsts {
            let seconds: Vec<usize> = match pair {
                Some((_, b)) => vec![b],
                None => (a + 1..nk.min(a + 14)).collect(),
            };
            for b in seconds {
                let c = self.common(a, b);
                if c.len() < 30 {
                    break;
                }
                let p1: Vec<(f64, f64)> = c.iter().map(|x| cam.norm(x.1 .0, x.1 .1)).collect();
                let p2: Vec<(f64, f64)> = c.iter().map(|x| cam.norm(x.2 .0, x.2 .1)).collect();
                let Some((e, inl)) = ransac_essential(&p1, &p2, thr, rng) else { continue };
                let (i1, i2): (Vec<(f64, f64)>, Vec<(f64, f64)>) = (0..c.len()).filter(|&i| inl[i]).map(|i| (p1[i], p2[i])).unzip();
                if i1.len() < 25 {
                    continue;
                }
                let Some(pose) = pose_from_essential(&e, &i1, &i2) else { continue };
                let (c1, c2) = ([0.0; 3], pose.center());
                let mut pts = vec![];
                let mut angles = vec![];
                for i in (0..c.len()).filter(|&i| inl[i]) {
                    let Some(x) = triangulate(&[(Pose::IDENTITY, p1[i]), (pose, p2[i])]) else { continue };
                    let (z1, z2) = (x[2], pose.cam(x)[2]);
                    if z1 <= 0.0 || z2 <= 0.0 {
                        continue;
                    }
                    let e1 = cam.project(x).map(|(u, v)| (u - c[i].1 .0).hypot(v - c[i].1 .1)).unwrap_or(99.0);
                    let e2 = cam.project(pose.cam(x)).map(|(u, v)| (u - c[i].2 .0).hypot(v - c[i].2 .1)).unwrap_or(99.0);
                    if e1 > 3.0 || e2 > 3.0 {
                        continue;
                    }
                    angles.push(parallax(c1, c2, x));
                    pts.push((c[i].0, x));
                }
                if pts.len() < 20 {
                    continue;
                }
                angles.sort_by(f64::total_cmp);
                let med = angles[angles.len() / 2];
                if med < 1.0 {
                    continue;
                }
                // prefer many points with healthy parallax, then earlier pairs
                let score = pts.len() as f64 * med.min(5.0) / (1.0 + 0.02 * a as f64);
                if best.as_ref().is_none_or(|bb| score > bb.0) {
                    best = Some((score, a, b, pose, pts));
                }
            }
            if best.as_ref().is_some_and(|b| b.0 > 150.0) {
                break;
            }
        }
        best.map(|(_, a, b, p, pts)| (a, b, p, pts))
    }

    /// 2D-3D correspondences of keyframe k.
    fn corr(&self, k: usize, pts: &[Option<V3>]) -> Vec<(V3, (f64, f64))> {
        seen_in(self.by_frame, self.kfs[k]).iter().filter_map(|&ti| Some((pts[ti]?, self.obs(ti, k)?))).collect()
    }

    /// Bundle adjustment over the keyframes in `free` (others fixed) and the
    /// points they see.
    fn ba(&self, r: &mut Recon, free: &[usize], refine_f: bool, iters: usize) -> f64 {
        let reg: Vec<usize> = (0..self.kfs.len()).filter(|&k| r.poses[k].is_some()).collect();
        if reg.len() < 2 {
            return r.rms;
        }
        let ci: std::collections::HashMap<usize, usize> = reg.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let mut poses: Vec<Pose> = reg.iter().map(|&k| r.poses[k].unwrap()).collect();
        let fixed: Vec<bool> = reg.iter().map(|k| !free.contains(k)).collect();
        // points seen by a free camera
        let mut pid: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        let mut pts: Vec<V3> = vec![];
        let mut obs = vec![];
        for &k in free {
            if r.poses[k].is_none() {
                continue;
            }
            for &ti in seen_in(self.by_frame, self.kfs[k]) {
                if let Some(x) = r.pts[ti] {
                    pid.entry(ti).or_insert_with(|| {
                        pts.push(x);
                        pts.len() - 1
                    });
                }
            }
        }
        for (&ti, &pi) in &pid {
            for &k in &reg {
                if let Some((u, v)) = self.obs(ti, k) {
                    obs.push(BaObs { cam: ci[&k], pt: pi, u, v });
                }
            }
        }
        if obs.is_empty() {
            return r.rms;
        }
        let e = bundle_adjust(&mut r.cam, &mut poses, &fixed, &mut pts, &obs, refine_f, iters);
        for (i, &k) in reg.iter().enumerate() {
            r.poses[k] = Some(poses[i]);
        }
        for (&ti, &pi) in &pid {
            r.pts[ti] = Some(pts[pi]);
        }
        e
    }

    /// Drops observations that disagree with the solve (moving objects, bad
    /// tracks): points with a large error in any keyframe are removed.
    fn cull(&self, r: &mut Recon, max_px: f64) -> usize {
        let mut removed = 0;
        for ti in 0..r.pts.len() {
            let Some(x) = r.pts[ti] else { continue };
            let mut views = 0;
            let mut bad = false;
            for k in 0..self.kfs.len() {
                let (Some(p), Some((u, v))) = (r.poses[k], self.obs(ti, k)) else { continue };
                match r.cam.project(p.cam(x)) {
                    Some((pu, pv)) if (u - pu).hypot(v - pv) <= max_px => views += 1,
                    _ => bad = true,
                }
            }
            if bad || views < 2 {
                r.pts[ti] = None;
                removed += 1;
            }
        }
        removed
    }

    /// Triangulates tracks seen in keyframe k (and other registered ones)
    /// that have no point yet.
    fn add_points(&self, r: &mut Recon, k: usize) {
        let Some(pk) = r.poses[k] else { return };
        let ck = pk.center();
        for &ti in seen_in(self.by_frame, self.kfs[k]) {
            if r.pts[ti].is_some() {
                continue;
            }
            let mut views = vec![];
            let mut best_angle = 0.0f64;
            let ok = self.obs(ti, k).unwrap();
            for j in 0..self.kfs.len() {
                let (Some(pj), Some(o)) = (r.poses[j], self.obs(ti, j)) else { continue };
                views.push((pj, r.cam.norm(o.0, o.1), o));
                if j != k {
                    // rough angle from the two rays
                    let (a, b) = (r.cam.norm(ok.0, ok.1), r.cam.norm(o.0, o.1));
                    let ra = mulv(&tr(&pk.r), unit([a.0, a.1, 1.0]));
                    let rb = mulv(&tr(&pj.r), unit([b.0, b.1, 1.0]));
                    let _ = ck;
                    best_angle = best_angle.max(dot(ra, rb).clamp(-1.0, 1.0).acos().to_degrees());
                }
            }
            if views.len() < 2 || best_angle < 1.2 {
                continue;
            }
            let vv: Vec<(Pose, (f64, f64))> = views.iter().map(|v| (v.0, v.1)).collect();
            let Some(x) = triangulate(&vv) else { continue };
            let good = views.iter().all(|(p, _, o)| {
                let xc = p.cam(x);
                xc[2] > 0.0 && r.cam.project(xc).is_some_and(|(u, v)| (u - o.0).hypot(v - o.1) < 3.0)
            });
            if good {
                r.pts[ti] = Some(x);
            }
        }
    }

    /// Full keyframe reconstruction with focal length f.
    fn reconstruct(&self, f: f64, w: f64, h: f64, refine_f: bool, rng: &mut Rng, pair: Option<(usize, usize)>) -> Option<Recon> {
        let cam = Cam { f, cx: w / 2.0, cy: h / 2.0 };
        let (a, b, pose, init_pts) = self.init(&cam, rng, pair)?;
        let nk = self.kfs.len();
        let mut r = Recon { cam, poses: vec![None; nk], pts: vec![None; self.tracks.len()], rms: 0.0 };
        r.poses[a] = Some(Pose::IDENTITY);
        r.poses[b] = Some(pose);
        for (ti, x) in init_pts {
            r.pts[ti] = Some(x);
        }
        self.ba(&mut r, &[b], false, 15);
        self.add_points(&mut r, b);
        // keyframes between the pair, then forward, then backward
        let order: Vec<usize> = (a + 1..b).chain(b + 1..nk).chain((0..a).rev()).collect();
        let mut stop_fwd = false;
        let mut stop_back = false;
        let mut added = 0;
        for k in order {
            if (k > b && stop_fwd) || (k < a && stop_back) {
                continue;
            }
            let corr = self.corr(k, &r.pts);
            let near = (0..nk).filter(|&j| r.poses[j].is_some()).min_by_key(|&j| (j as i64 - k as i64).abs())?;
            let mut ok = false;
            if corr.len() >= 10 {
                let (p, e) = refine_pose(&r.cam, r.poses[near].unwrap(), &corr, 30);
                let (p, e) = if e > 3.0 {
                    match dlt_pose(&r.cam, &corr) {
                        Some(d) => {
                            let (p2, e2) = refine_pose(&r.cam, d, &corr, 30);
                            if e2 < e {
                                (p2, e2)
                            } else {
                                (p, e)
                            }
                        }
                        None => (p, e),
                    }
                } else {
                    (p, e)
                };
                if e <= 3.0 {
                    r.poses[k] = Some(p);
                    ok = true;
                }
            }
            if !ok {
                if k > b {
                    stop_fwd = true;
                } else if k < a {
                    stop_back = true;
                }
                continue;
            }
            self.add_points(&mut r, k);
            added += 1;
            if added % 3 == 0 {
                // local bundle adjustment: the newest keyframes around k
                let free: Vec<usize> = (k.saturating_sub(5)..(k + 6).min(nk)).filter(|&j| r.poses[j].is_some() && j != a).collect();
                self.ba(&mut r, &free, false, 6);
            }
        }
        let free: Vec<usize> = (0..nk).filter(|&j| r.poses[j].is_some() && j != a).collect();
        self.ba(&mut r, &free, false, 15);
        self.cull(&mut r, 4.0);
        r.rms = self.ba(&mut r, &free, refine_f, if refine_f { 40 } else { 15 });
        self.cull(&mut r, 3.0);
        Some(r)
    }
}

// ---------------------------------------------------------------- rotation only (tripod)

/// A camera that only rotates: rotations per keyframe from matched
/// bearings, track directions on the unit sphere.
fn nodal_keyframes(ctx: &Ctx, f: f64, w: f64, h: f64, rng: &mut Rng) -> Option<Recon> {
    let cam = Cam { f, cx: w / 2.0, cy: h / 2.0 };
    let nk = ctx.kfs.len();
    let bearing = |o: (f64, f64)| {
        let (x, y) = cam.norm(o.0, o.1);
        unit([x, y, 1.0])
    };
    let mut rots: Vec<Option<M3>> = vec![None; nk];
    rots[0] = Some(I3);
    for k in 1..nk {
        let c = ctx.common(k - 1, k);
        if c.len() < 8 {
            break;
        }
        let a: Vec<V3> = c.iter().map(|x| bearing(x.1)).collect();
        let b: Vec<V3> = c.iter().map(|x| bearing(x.2)).collect();
        // RANSAC on two-point samples
        let thr = (2.0 / f).max(1e-4);
        let mut best = (0usize, I3);
        for _ in 0..200 {
            let s = rng.sample(a.len(), 2);
            let r = kabsch(&[a[s[0]], a[s[1]]], &[b[s[0]], b[s[1]]], &[1.0, 1.0]);
            let n = a.iter().zip(&b).filter(|(p, q)| norm(sub(mulv(&r, **p), **q)) < thr).count();
            if n > best.0 {
                best = (n, r);
            }
        }
        let inl: Vec<usize> = (0..a.len()).filter(|&i| norm(sub(mulv(&best.1, a[i]), b[i])) < thr * 2.0).collect();
        if inl.len() < 6 {
            break;
        }
        let rr = kabsch(&inl.iter().map(|&i| a[i]).collect::<Vec<_>>(), &inl.iter().map(|&i| b[i]).collect::<Vec<_>>(), &vec![1.0; inl.len()]);
        rots[k] = Some(mul(&rr, &rots[k - 1]?));
    }
    // alternate: directions from rotations, rotations from directions
    let mut dirs: Vec<Option<V3>> = vec![None; ctx.tracks.len()];
    for _ in 0..4 {
        for (ti, d) in dirs.iter_mut().enumerate() {
            let mut s = [0.0; 3];
            let mut n = 0;
            for k in 0..nk {
                if let (Some(r), Some(o)) = (rots[k], ctx.obs(ti, k)) {
                    s = add(s, mulv(&tr(&r), bearing(o)));
                    n += 1;
                }
            }
            *d = (n >= 2).then(|| unit(s));
        }
        for k in 1..nk {
            if rots[k].is_none() {
                continue;
            }
            let pairs: Vec<(V3, V3)> = seen_in(ctx.by_frame, ctx.kfs[k]).iter().filter_map(|&ti| Some((dirs[ti]?, bearing(ctx.obs(ti, k)?)))).collect();
            if pairs.len() >= 6 {
                let (a, b): (Vec<V3>, Vec<V3>) = pairs.into_iter().unzip();
                rots[k] = Some(kabsch(&a, &b, &vec![1.0; a.len()]));
            }
        }
    }
    // error in px
    let mut s = 0.0;
    let mut n = 0usize;
    for k in 0..nk {
        let Some(r) = rots[k] else { continue };
        for &ti in seen_in(ctx.by_frame, ctx.kfs[k]) {
            let (Some(d), Some(o)) = (dirs[ti], ctx.obs(ti, k)) else { continue };
            if let Some((u, v)) = cam.project(mulv(&r, d)) {
                s += ((u - o.0).powi(2) + (v - o.1).powi(2)).min(100.0);
                n += 1;
            }
        }
    }
    if n < 20 {
        return None;
    }
    Some(Recon { cam, poses: rots.iter().map(|r| r.map(|r| Pose { r, t: [0.0; 3] })).collect(), pts: dirs, rms: (s / n as f64).sqrt() })
}

// ---------------------------------------------------------------- the solver

/// Solves the camera from feature tracks over `n_frames` frames.
pub fn solve(tracks: &[Track], n_frames: usize, s: &Settings) -> Result<Solution, String> {
    if n_frames < 3 {
        return Err("too few frames to solve a camera (at least 3)".into());
    }
    let mut by_frame: Vec<Vec<usize>> = vec![vec![]; n_frames];
    for (ti, t) in tracks.iter().enumerate() {
        if t.len() < 3 {
            continue;
        }
        for &(f, _, _) in t {
            if (f as usize) < n_frames {
                by_frame[f as usize].push(ti);
            }
        }
    }
    let kfs = keyframes(tracks, &by_frame, s.w);
    if kfs.len() < 2 {
        return Err("not enough detail to track (too few features survive between frames): the shot may be blurry, flat or too short".into());
    }
    let ctx = Ctx { tracks, by_frame: &by_frame, kfs: &kfs };
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let candidates: Vec<f64> = match s.fov {
        Some(fov) => vec![focal_for_fov(fov, s.w)],
        None => [24.0, 34.0, 45.0, 56.0, 68.0, 82.0].iter().map(|&fov| focal_for_fov(fov, s.w)).collect(),
    };
    let default_f = focal_for_fov(56.0, s.w);
    let mut nodal = s.nodal;
    let mut recon: Option<Recon> = None;
    // the keyframe pair to start from (chosen once, with a usual lens)
    let pair = if nodal {
        None
    } else {
        let f0 = if s.fov.is_some() { candidates[0] } else { default_f };
        ctx.init(&Cam { f: f0, cx: s.w / 2.0, cy: s.h / 2.0 }, &mut rng, None).map(|(a, b, _, _)| (a, b))
    };
    if pair.is_none() {
        nodal = true;
    }
    if !nodal {
        // try each field of view with the focal length held, keep the best
        let mut tried: Vec<(f64, Recon)> = vec![];
        for &f in &candidates {
            let mut rr = Rng(rng.next());
            if let Some(r) = ctx.reconstruct(f, s.w, s.h, false, &mut rr, pair) {
                let solved = r.poses.iter().filter(|p| p.is_some()).count();
                // unsolved keyframes count against a candidate
                let score = r.rms * (1.0 + 2.0 * (1.0 - solved as f64 / kfs.len() as f64));
                tried.push((score, r));
            }
        }
        if !tried.is_empty() {
            let best = tried.iter().map(|t| t.0).fold(f64::INFINITY, f64::min);
            // within 3 percent: an unconstrained focal length; prefer the usual lens
            let pick = tried
                .iter()
                .filter(|t| t.0 <= best * 1.03)
                .min_by(|a, b| ((a.1.cam.f / default_f).ln().abs()).total_cmp(&(b.1.cam.f / default_f).ln().abs()))
                .map(|t| t.1.cam.f)
                .unwrap_or(default_f);
            let mut rr = Rng(rng.next());
            recon = ctx.reconstruct(pick, s.w, s.h, s.fov.is_none(), &mut rr, pair);
            if let Some(r) = &recon {
                // a focal length that ran away (degenerate motion) falls back to the pick
                if !(r.cam.f > pick * 0.4 && r.cam.f < pick * 2.5) {
                    let mut rr = Rng(rng.next());
                    recon = ctx.reconstruct(pick, s.w, s.h, false, &mut rr, pair);
                }
            }
        }
        if recon.is_none() {
            nodal = true;
        }
    }
    if nodal {
        let mut best: Option<Recon> = None;
        for &f in &candidates {
            let mut rr = Rng(rng.next());
            if let Some(r) = nodal_keyframes(&ctx, f, s.w, s.h, &mut rr) {
                if best.as_ref().is_none_or(|b| r.rms < b.rms * 0.97) {
                    best = Some(r);
                }
            }
        }
        recon = best;
    }
    let Some(r) = recon else {
        return Err("the camera couldn't be solved: too little detail or motion that isn't a camera move (try a range with steady detail, or fov= if you know the lens)".into());
    };
    Ok(finish(&ctx, r, n_frames, s, nodal))
}

/// Poses every frame from the solved points, then moves everything into
/// solve space.
fn finish(ctx: &Ctx, r: Recon, n_frames: usize, s: &Settings, nodal: bool) -> Solution {
    let kfs = ctx.kfs;
    let reg: Vec<(usize, Pose)> = kfs.iter().enumerate().filter_map(|(k, &f)| r.poses[k].map(|p| (f, p))).collect();
    let cam = r.cam;
    let poses: Vec<Option<Pose>> = (0..n_frames)
        .into_par_iter()
        .map(|f| {
            if let Some(&(_, p)) = reg.iter().find(|(kf, _)| *kf == f) {
                return Some(p);
            }
            let before = reg.iter().rev().find(|(kf, _)| *kf < f);
            let after = reg.iter().find(|(kf, _)| *kf > f);
            let init = match (before, after) {
                (Some(&(fa, pa)), Some(&(fb, pb))) => pa.lerp(&pb, (f - fa) as f64 / (fb - fa) as f64),
                (Some(&(fa, pa)), None) if f - fa <= 3 => pa,
                (None, Some(&(fb, pb))) if fb - f <= 3 => pb,
                _ => return None,
            };
            let corr: Vec<(V3, (f64, f64))> = seen_in(ctx.by_frame, f).iter().filter_map(|&ti| Some((r.pts[ti]?, obs_at(ctx.tracks, ti, f)?))).collect();
            if corr.len() < 8 {
                return (before.is_some() && after.is_some()).then_some(init);
            }
            if nodal {
                let bearing = |o: (f64, f64)| {
                    let (x, y) = cam.norm(o.0, o.1);
                    unit([x, y, 1.0])
                };
                let (a, b): (Vec<V3>, Vec<V3>) = corr.iter().map(|(d, o)| (*d, bearing(*o))).unzip();
                return Some(Pose { r: kabsch(&a, &b, &vec![1.0; a.len()]), t: [0.0; 3] });
            }
            let (p, e) = refine_pose(&cam, init, &corr, 20);
            if e <= 3.0 {
                Some(p)
            } else {
                (before.is_some() && after.is_some()).then_some(init)
            }
        })
        .collect();
    // reference camera: the solved frame nearest the requested one
    let rf = (0..n_frames).filter(|&f| poses[f].is_some()).min_by_key(|&f| (f as i64 - s.reference as i64).abs()).unwrap_or(0);
    let pref = poses[rf].unwrap_or(Pose::IDENTITY);
    let (rref, cref) = (pref.r, pref.center());
    let to_ref = |x: V3| mulv(&rref, sub(x, cref));
    // median depth seen from the reference camera -> 1
    let mut depths: Vec<f64> = r.pts.iter().flatten().map(|x| to_ref(*x)[2]).filter(|z| *z > 0.0).collect();
    depths.sort_by(f64::total_cmp);
    let k = if nodal { 1.0 } else { 1.0 / depths.get(depths.len() / 2).copied().unwrap_or(1.0).max(1e-9) };
    let poses: Vec<Option<Pose>> = poses.iter().map(|p| p.map(|p| Pose::from_center(mul(&p.r, &tr(&rref)), scale(to_ref(p.center()), k)))).collect();
    // points with their error over every frame that sees them
    let mut points = vec![];
    let mut total = 0.0;
    let mut count = 0usize;
    for (ti, x) in r.pts.iter().enumerate() {
        let Some(x) = x else { continue };
        let xs = scale(to_ref(*x), k);
        let (mut e2, mut n) = (0.0, 0usize);
        for &(f, u, v) in &ctx.tracks[ti] {
            let Some(Some(p)) = poses.get(f as usize) else { continue };
            if let Some((pu, pv)) = cam.project(p.cam(xs)) {
                let d = ((u as f64 - pu).powi(2) + (v as f64 - pv).powi(2)).min(100.0);
                e2 += d;
                n += 1;
            }
        }
        if n == 0 {
            continue;
        }
        total += e2;
        count += n;
        points.push(Point3 { track: ti, x: xs, err: (e2 / n as f64).sqrt(), views: n });
    }
    Solution { f: cam.f, poses, points, rms: if count > 0 { (total / count as f64).sqrt() } else { r.rms }, nodal, keyframes: kfs.len() }
}

/// A best-fit plane through points: (centroid, unit normal).
pub fn fit_plane(pts: &[V3]) -> Option<(V3, V3)> {
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f64;
    let c = scale(pts.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / n);
    let mut cov = [0.0; 9];
    for p in pts {
        let d = sub(*p, c);
        for i in 0..3 {
            for j in 0..3 {
                cov[i * 3 + j] += d[i] * d[j];
            }
        }
    }
    let (vals, vecs) = eig_sym(&cov, 3);
    if vals[1] < 1e-12 {
        return None; // collinear
    }
    Some((c, unit([vecs[0], vecs[3], vecs[6]])))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Synthetic tracks of random points seen by a moving camera; points
    /// appear and disappear as they enter and leave the frame.
    pub fn synth(poses: &[Pose], f: f64, w: f64, h: f64, n_pts: usize, noise: f64, seed: u64) -> (Vec<Track>, Vec<V3>) {
        let mut rng = Rng(seed);
        let mut uni = move || (rng.next() % 1_000_000) as f64 / 1_000_000.0;
        let cam = Cam { f, cx: w / 2.0, cy: h / 2.0 };
        let mut tracks = vec![];
        let mut xs = vec![];
        for _ in 0..n_pts {
            // a box of points in front of the first camera, 3 to 9 units away
            let z = 3.0 + uni() * 6.0;
            let x = [(uni() - 0.5) * z * 1.6, (uni() - 0.5) * z * 1.0, z];
            let mut t: Track = vec![];
            let mut started = false;
            for (fi, p) in poses.iter().enumerate() {
                let seen = cam.project(p.cam(x)).filter(|(u, v)| *u > 12.0 && *v > 12.0 && *u < w - 12.0 && *v < h - 12.0);
                match seen {
                    Some((u, v)) => {
                        started = true;
                        let nu = u + (uni() - 0.5) * 2.0 * noise;
                        let nv = v + (uni() - 0.5) * 2.0 * noise;
                        t.push((fi as u32, nu as f32, nv as f32));
                    }
                    None if started => break,
                    None => {}
                }
            }
            if t.len() >= 3 {
                tracks.push(t);
                xs.push(x);
            }
        }
        (tracks, xs)
    }

    #[test]
    fn essential_and_triangulation_are_exact_on_clean_data() {
        let pose = Pose::from_center(rodrigues([0.02, -0.08, 0.01]), [0.6, 0.05, 0.1]);
        let mut rng = Rng(7);
        let mut uni = move || (rng.next() % 10000) as f64 / 10000.0;
        let xs: Vec<V3> = (0..60).map(|_| [(uni() - 0.5) * 4.0, (uni() - 0.5) * 3.0, 4.0 + uni() * 4.0]).collect();
        let p1: Vec<(f64, f64)> = xs.iter().map(|x| (x[0] / x[2], x[1] / x[2])).collect();
        let p2: Vec<(f64, f64)> = xs
            .iter()
            .map(|x| {
                let c = pose.cam(*x);
                (c[0] / c[2], c[1] / c[2])
            })
            .collect();
        let e = essential_8pt(&p1, &p2).unwrap();
        let got = pose_from_essential(&e, &p1, &p2).unwrap();
        let ang = norm(log_rot(&mul(&got.r, &tr(&pose.r))));
        assert!(ang < 1e-6, "rotation off by {ang}");
        let dir = dot(unit(got.t), unit(pose.t));
        assert!(dir > 0.999999, "translation direction {dir}");
        // triangulated with the true pose
        let x = triangulate(&[(Pose::IDENTITY, p1[3]), (pose, p2[3])]).unwrap();
        assert!(norm(sub(x, xs[3])) < 1e-6);
    }

    #[test]
    fn pose_refinement_converges() {
        let truth = Pose::from_center(rodrigues([0.05, 0.1, -0.02]), [0.3, -0.2, 0.4]);
        let cam = Cam { f: 800.0, cx: 480.0, cy: 270.0 };
        let mut rng = Rng(3);
        let mut uni = move || (rng.next() % 10000) as f64 / 10000.0;
        let corr: Vec<(V3, (f64, f64))> = (0..40)
            .map(|_| {
                let x = [(uni() - 0.5) * 4.0, (uni() - 0.5) * 3.0, 4.0 + uni() * 4.0];
                (x, cam.project(truth.cam(x)).unwrap())
            })
            .collect();
        let (p, e) = refine_pose(&cam, Pose::IDENTITY, &corr, 50);
        assert!(e < 1e-3, "rms {e}");
        assert!(norm(sub(p.center(), truth.center())) < 1e-4);
        let d = dlt_pose(&cam, &corr).unwrap();
        assert!(norm(sub(d.center(), truth.center())) < 1e-3);
    }

    /// Camera path recovered up to a similarity: compare after aligning the
    /// first camera and the scale of the path.
    fn path_error(sol: &Solution, truth: &[Pose]) -> (f64, f64) {
        let r0 = sol.poses[0].unwrap();
        let t0 = truth[0];
        let (mut rot_err, mut pos_err) = (0.0f64, 0.0f64);
        let (cs, ct): (Vec<V3>, Vec<V3>) = (0..truth.len()).filter_map(|i| sol.poses[i].map(|p| (p.center(), truth[i].center()))).unzip();
        // scale from the path length
        let len = |v: &[V3]| v.windows(2).map(|w| norm(sub(w[1], w[0]))).sum::<f64>();
        let k = len(&ct) / len(&cs).max(1e-12);
        for i in 0..truth.len() {
            let Some(p) = sol.poses[i] else { continue };
            // relative motion from frame 0
            let ra = mul(&p.r, &tr(&r0.r));
            let rb = mul(&truth[i].r, &tr(&t0.r));
            rot_err = rot_err.max(norm(log_rot(&mul(&ra, &tr(&rb)))).to_degrees());
            let ca = scale(mulv(&r0.r, sub(p.center(), r0.center())), k);
            let cb = mulv(&t0.r, sub(truth[i].center(), t0.center()));
            pos_err = pos_err.max(norm(sub(ca, cb)));
        }
        (rot_err, pos_err)
    }

    #[test]
    fn solves_a_dolly_with_pan_and_finds_the_focal_length() {
        let (w, h, f) = (960.0, 540.0, 900.0);
        let truth: Vec<Pose> = (0..48)
            .map(|i| {
                let t = i as f64 / 47.0;
                Pose::from_center(from_euler(2.0 * t, -12.0 * t, 1.0 * t), [1.6 * t, -0.15 * t, 0.4 * t])
            })
            .collect();
        let (tracks, _) = synth(&truth, f, w, h, 700, 0.3, 11);
        let sol = solve(&tracks, truth.len(), &Settings { w, h, fov: None, reference: 0, nodal: false }).unwrap();
        assert!(!sol.nodal);
        assert!(sol.poses.iter().all(|p| p.is_some()), "unsolved frames");
        assert!(sol.rms < 1.0, "rms {}", sol.rms);
        assert!((sol.f / f - 1.0).abs() < 0.08, "focal {} vs {f}", sol.f);
        let (re, pe) = path_error(&sol, &truth);
        assert!(re < 0.5, "rotation error {re} deg");
        assert!(pe < 0.08, "position error {pe}");
        // reference camera is the origin
        let p0 = sol.poses[0].unwrap();
        assert!(norm(p0.center()) < 1e-9 && norm(log_rot(&p0.r)) < 1e-9);
    }

    #[test]
    fn solves_a_tripod_pan_as_a_rotating_camera() {
        let (w, h, f) = (960.0, 540.0, 750.0);
        let truth: Vec<Pose> = (0..40).map(|i| Pose { r: from_euler(1.5 * i as f64 / 39.0, -20.0 * i as f64 / 39.0, 0.0), t: [0.0; 3] }).collect();
        let (tracks, _) = synth(&truth, f, w, h, 600, 0.3, 5);
        let sol = solve(&tracks, truth.len(), &Settings { w, h, fov: None, reference: 0, nodal: false }).unwrap();
        assert!(sol.poses.iter().filter(|p| p.is_some()).count() >= 38);
        let (re, _) = path_error(&sol, &truth);
        assert!(re < 1.0, "rotation error {re} deg (nodal {})", sol.nodal);
        assert!(sol.rms < 1.5, "rms {}", sol.rms);
    }

    #[test]
    fn plane_fit() {
        let pts = vec![[0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0]];
        let (c, n) = fit_plane(&pts).unwrap();
        assert!((c[1] - 1.0).abs() < 1e-12 && n[1].abs() > 0.999999);
    }
}
