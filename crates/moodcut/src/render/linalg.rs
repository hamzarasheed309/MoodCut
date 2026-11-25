//! Small dense linear algebra for the trackers and the camera solver:
//! 3-vectors and 3x3 matrices, rotations (Rodrigues, and Euler angles in the
//! X-then-Y-then-Z order `warp::rotate3` uses), a Jacobi eigen solver for
//! symmetric matrices, a 3x3 SVD and Cholesky solves. No dependencies.

pub type V3 = [f64; 3];
pub type M3 = [[f64; 3]; 3];

pub const I3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

#[inline]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
#[inline]
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
#[inline]
pub fn unit(a: V3) -> V3 {
    let n = norm(a);
    if n < 1e-300 {
        [0.0, 0.0, 0.0]
    } else {
        scale(a, 1.0 / n)
    }
}

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    o
}

#[inline]
pub fn mulv(a: &M3, v: V3) -> V3 {
    [dot(a[0], v), dot(a[1], v), dot(a[2], v)]
}

pub fn tr(a: &M3) -> M3 {
    [[a[0][0], a[1][0], a[2][0]], [a[0][1], a[1][1], a[2][1]], [a[0][2], a[1][2], a[2][2]]]
}

pub fn det(a: &M3) -> f64 {
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
}

pub fn inv(a: &M3) -> Option<M3> {
    let d = det(a);
    if d.abs() < 1e-300 {
        return None;
    }
    let k = 1.0 / d;
    Some([
        [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) * k, (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * k, (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * k],
        [(a[1][2] * a[2][0] - a[1][0] * a[2][2]) * k, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * k, (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * k],
        [(a[1][0] * a[2][1] - a[1][1] * a[2][0]) * k, (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * k, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * k],
    ])
}

pub fn skew(v: V3) -> M3 {
    [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]]
}

/// Rotation matrix of a rotation vector (axis times angle in radians).
pub fn rodrigues(w: V3) -> M3 {
    let th = norm(w);
    if th < 1e-12 {
        // first order: I + [w]x
        let s = skew(w);
        return [[1.0, s[0][1], s[0][2]], [s[1][0], 1.0, s[1][2]], [s[2][0], s[2][1], 1.0]];
    }
    let k = scale(w, 1.0 / th);
    let (s, c) = th.sin_cos();
    let kx = skew(k);
    let kx2 = mul(&kx, &kx);
    let mut r = I3;
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] += s * kx[i][j] + (1.0 - c) * kx2[i][j];
        }
    }
    r
}

/// Rotation vector of a rotation matrix (the inverse of [`rodrigues`]).
pub fn log_rot(r: &M3) -> V3 {
    let c = ((r[0][0] + r[1][1] + r[2][2] - 1.0) / 2.0).clamp(-1.0, 1.0);
    let th = c.acos();
    let v = [r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]];
    if th < 1e-9 {
        return scale(v, 0.5);
    }
    if std::f64::consts::PI - th < 1e-6 {
        // near 180 degrees: the axis from the diagonal
        let x = ((r[0][0] + 1.0) / 2.0).max(0.0).sqrt();
        let y = ((r[1][1] + 1.0) / 2.0).max(0.0).sqrt();
        let z = ((r[2][2] + 1.0) / 2.0).max(0.0).sqrt();
        let mut a = [x, y, z];
        if a[0] > 1e-6 {
            a[1] = a[1].copysign(r[0][1]);
            a[2] = a[2].copysign(r[0][2]);
        } else if a[1] > 1e-6 {
            a[2] = a[2].copysign(r[1][2]);
        }
        return scale(unit(a), th);
    }
    scale(v, th / (2.0 * th.sin()))
}

/// The rotation nearest to `m` (polar decomposition by SVD).
pub fn orthonormalize(m: &M3) -> M3 {
    let (u, _, v) = svd3(m);
    let mut r = mul(&u, &tr(&v));
    if det(&r) < 0.0 {
        let mut u2 = u;
        for row in u2.iter_mut() {
            row[2] = -row[2];
        }
        r = mul(&u2, &tr(&v));
    }
    r
}

/// R = Rz(rz) * Ry(ry) * Rx(rx), angles in degrees: the rotation
/// `warp::rotate3(v, rx, ry, rz)` applies to a vector.
pub fn from_euler(rx: f64, ry: f64, rz: f64) -> M3 {
    let (sx, cx) = rx.to_radians().sin_cos();
    let (sy, cy) = ry.to_radians().sin_cos();
    let (sz, cz) = rz.to_radians().sin_cos();
    let rxm = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let rym = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rzm = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mul(&rzm, &mul(&rym, &rxm))
}

/// Angles (rx, ry, rz) in degrees with `from_euler(rx, ry, rz) == r`. Of the
/// two solutions (and their 360-degree wraps) the one closest to `near` is
/// returned, so keyframed angles don't jump between frames.
pub fn to_euler(r: &M3, near: Option<V3>) -> V3 {
    let sy = (-r[2][0]).clamp(-1.0, 1.0);
    let b = sy.asin();
    let (a, g) = if b.cos().abs() > 1e-7 {
        (r[2][1].atan2(r[2][2]), r[1][0].atan2(r[0][0]))
    } else {
        // gimbal lock: only rx - rz (or rx + rz) is defined; keep rz
        let g = near.map(|n| n[2].to_radians()).unwrap_or(0.0);
        let a = if sy > 0.0 { r[0][1].atan2(r[1][1]) + g } else { (-r[0][1]).atan2(r[1][1]) - g };
        (a, g)
    };
    let deg = |v: f64| v.to_degrees();
    let s1 = [deg(a), deg(b), deg(g)];
    let s2 = [deg(a + std::f64::consts::PI), deg(std::f64::consts::PI - b), deg(g + std::f64::consts::PI)];
    let Some(n) = near else {
        return s1.map(wrap180);
    };
    let fit = |s: V3| -> V3 { [unwrap_near(s[0], n[0]), unwrap_near(s[1], n[1]), unwrap_near(s[2], n[2])] };
    let (f1, f2) = (fit(s1), fit(s2));
    let d = |s: V3| (s[0] - n[0]).abs() + (s[1] - n[1]).abs() + (s[2] - n[2]).abs();
    if d(f2) < d(f1) {
        f2
    } else {
        f1
    }
}

fn wrap180(v: f64) -> f64 {
    let w = (v + 180.0).rem_euclid(360.0) - 180.0;
    if w <= -180.0 {
        w + 360.0
    } else {
        w
    }
}

/// `v` plus a multiple of 360 that lands closest to `near`.
pub fn unwrap_near(v: f64, near: f64) -> f64 {
    v + ((near - v) / 360.0).round() * 360.0
}

/// Eigen decomposition of a symmetric n x n matrix (row-major) by cyclic
/// Jacobi rotations. Returns eigenvalues in ascending order and the matching
/// eigenvectors as columns of a row-major n x n matrix.
pub fn eig_sym(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut m = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..60 {
        let mut off = 0.0;
        for p in 0..n {
            for q in p + 1..n {
                off += m[p * n + q] * m[p * n + q];
            }
        }
        if off < 1e-30 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = m[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let (app, aqq) = (m[p * n + p], m[q * n + q]);
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (mkp, mkq) = (m[k * n + p], m[k * n + q]);
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let (mpk, mqk) = (m[p * n + k], m[q * n + k]);
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| m[i * n + i].total_cmp(&m[j * n + j]));
    let vals: Vec<f64> = order.iter().map(|&i| m[i * n + i]).collect();
    let mut vecs = vec![0.0; n * n];
    for (c, &src) in order.iter().enumerate() {
        for r in 0..n {
            vecs[r * n + c] = v[r * n + src];
        }
    }
    (vals, vecs)
}

/// The unit vector x minimizing |A x| for an m x n matrix A given as rows
/// (the right singular vector of the smallest singular value).
pub fn null_vector(rows: &[Vec<f64>], n: usize) -> Vec<f64> {
    let mut ata = vec![0.0; n * n];
    for r in rows {
        for i in 0..n {
            if r[i] == 0.0 {
                continue;
            }
            for j in i..n {
                ata[i * n + j] += r[i] * r[j];
            }
        }
    }
    for i in 0..n {
        for j in 0..i {
            ata[i * n + j] = ata[j * n + i];
        }
    }
    let (_, vecs) = eig_sym(&ata, n);
    (0..n).map(|r| vecs[r * n]).collect()
}

/// SVD of a 3x3 matrix: (U, singular values descending, V) with A = U S V^T.
/// U and V are orthonormal (not forced to be rotations).
pub fn svd3(a: &M3) -> (M3, V3, M3) {
    let ata = mul(&tr(a), a);
    let flat: Vec<f64> = ata.iter().flatten().copied().collect();
    let (vals, vecs) = eig_sym(&flat, 3);
    // descending
    let col = |c: usize| [vecs[c], vecs[3 + c], vecs[6 + c]];
    let vcols = [col(2), col(1), col(0)];
    let s = [vals[2].max(0.0).sqrt(), vals[1].max(0.0).sqrt(), vals[0].max(0.0).sqrt()];
    let mut ucols = [[0.0; 3]; 3];
    for i in 0..2 {
        let av = mulv(a, vcols[i]);
        ucols[i] = if s[i] > 1e-12 { scale(av, 1.0 / s[i]) } else { [0.0; 3] };
    }
    // make the first two orthonormal (numerical safety), the third their cross
    if norm(ucols[0]) < 0.5 {
        ucols[0] = [1.0, 0.0, 0.0];
    }
    ucols[0] = unit(ucols[0]);
    let proj = dot(ucols[1], ucols[0]);
    ucols[1] = sub(ucols[1], scale(ucols[0], proj));
    if norm(ucols[1]) < 1e-6 {
        let t = if ucols[0][0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        ucols[1] = cross(ucols[0], t);
    }
    ucols[1] = unit(ucols[1]);
    let third = cross(ucols[0], ucols[1]);
    let av = mulv(a, vcols[2]);
    // keep the sign that matches A v3 when it is meaningful
    ucols[2] = if s[2] > 1e-9 && dot(third, av) < 0.0 { scale(third, -1.0) } else { third };
    let to_m = |c: [V3; 3]| -> M3 { [[c[0][0], c[1][0], c[2][0]], [c[0][1], c[1][1], c[2][1]], [c[0][2], c[1][2], c[2][2]]] };
    (to_m(ucols), s, to_m(vcols))
}

/// Solves (A) x = b in place for a symmetric positive definite n x n matrix
/// (row-major). Returns false if A isn't positive definite.
pub fn cholesky_solve(a: &mut [f64], n: usize, b: &mut [f64]) -> bool {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 1e-300 || !d.is_finite() {
            return false;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    // forward: L y = b
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    // back: L^T x = y
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    true
}

/// Rotation taking unit vectors `a` (rows) onto `b` in the weighted least
/// squares sense (Kabsch): argmin sum w |b - R a|^2.
pub fn kabsch(a: &[V3], b: &[V3], w: &[f64]) -> M3 {
    let mut h = [[0.0; 3]; 3];
    for ((p, q), &wi) in a.iter().zip(b).zip(w) {
        for i in 0..3 {
            for j in 0..3 {
                h[i][j] += wi * q[i] * p[j];
            }
        }
    }
    orthonormalize(&h)
}

/// Rotation interpolation: r0 at t=0, r1 at t=1 (geodesic).
pub fn slerp(r0: &M3, r1: &M3, t: f64) -> M3 {
    let d = log_rot(&mul(&tr(r0), r1));
    mul(r0, &rodrigues(scale(d, t)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &M3, b: &M3, eps: f64) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < eps))
    }

    #[test]
    fn euler_round_trip_matches_rotate3() {
        for &(x, y, z) in &[(10.0, -20.0, 30.0), (0.0, 0.0, 0.0), (-170.0, 45.0, 95.0), (5.0, 89.0, -3.0)] {
            let r = from_euler(x, y, z);
            // same as warp::rotate3 on the basis vectors
            for (k, e) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].iter().enumerate() {
                let p = crate::render::warp::rotate3((e[0], e[1], e[2]), x, y, z);
                assert!((p.0 - r[0][k]).abs() < 1e-9 && (p.1 - r[1][k]).abs() < 1e-9 && (p.2 - r[2][k]).abs() < 1e-9);
            }
            let e = to_euler(&r, Some([x, y, z]));
            assert!(close(&from_euler(e[0], e[1], e[2]), &r, 1e-7), "{x} {y} {z} -> {e:?}");
        }
        // continuity across the wrap
        let e = to_euler(&from_euler(0.0, 0.0, 181.0), Some([0.0, 0.0, 179.0]));
        assert!((e[2] - 181.0).abs() < 1e-6, "{e:?}");
    }

    #[test]
    fn rodrigues_round_trip() {
        for w in [[0.1, -0.2, 0.3], [0.0, 0.0, 0.0], [3.0, 0.1, -0.2], [1e-9, 0.0, 0.0]] {
            let r = rodrigues(w);
            assert!((det(&r) - 1.0).abs() < 1e-9);
            let back = log_rot(&r);
            assert!(close(&rodrigues(back), &r, 1e-9), "{w:?} {back:?}");
        }
    }

    #[test]
    fn eigen_and_svd() {
        let a = [4.0, 1.0, 2.0, 1.0, 3.0, 0.5, 2.0, 0.5, 5.0];
        let (vals, vecs) = eig_sym(&a, 3);
        for c in 0..3 {
            let v = [vecs[c], vecs[3 + c], vecs[6 + c]];
            let av = [a[0] * v[0] + a[1] * v[1] + a[2] * v[2], a[3] * v[0] + a[4] * v[1] + a[5] * v[2], a[6] * v[0] + a[7] * v[1] + a[8] * v[2]];
            for i in 0..3 {
                assert!((av[i] - vals[c] * v[i]).abs() < 1e-9);
            }
        }
        assert!(vals[0] <= vals[1] && vals[1] <= vals[2]);
        let m = [[1.0, 2.0, 0.5], [0.0, 1.0, -1.0], [3.0, 0.2, 1.0]];
        let (u, s, v) = svd3(&m);
        let back = mul(&mul(&u, &[[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]]), &tr(&v));
        assert!(close(&back, &m, 1e-8), "{back:?}");
        let mut spd = vec![4.0, 1.0, 1.0, 3.0];
        let mut b = vec![1.0, 2.0];
        assert!(cholesky_solve(&mut spd, 2, &mut b));
        assert!((4.0 * b[0] + b[1] - 1.0).abs() < 1e-12 && (b[0] + 3.0 * b[1] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn kabsch_recovers_rotation() {
        let r = rodrigues([0.2, -0.4, 0.1]);
        let a: Vec<V3> = vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.3, 0.4, 0.86], [-0.5, 0.2, 0.84]];
        let b: Vec<V3> = a.iter().map(|p| mulv(&r, *p)).collect();
        let k = kabsch(&a, &b, &[1.0; 4]);
        assert!(close(&k, &r, 1e-9));
        let half = slerp(&I3, &r, 0.5);
        assert!(close(&mul(&half, &half), &r, 1e-9));
    }
}
