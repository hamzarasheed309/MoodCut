//! Planar point tracking for masks, nulls and corner pins.
//!
//! Frames are small grayscale images (up to 640 px wide). Each point is a
//! template matched by normalized cross-correlation over a three-level
//! pyramid (coarse search, then refine), with a sub-pixel parabola fit and
//! a template that is refreshed only while it still resembles the first one
//! (the drift guard). Similarity and affine motion track a 3x3 grid of
//! patches inside the region and fit a transform to them by least squares.

use rayon::prelude::*;

/// An 8-bit grayscale frame.
#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub d: Vec<u8>,
}

impl Gray {
    pub fn new(w: usize, h: usize, d: Vec<u8>) -> Gray {
        Gray { w, h, d }
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> f32 {
        self.d[y * self.w + x] as f32
    }

    /// Half size (2x2 box average).
    fn half(&self) -> Gray {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut d = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let (x0, y0) = ((x * 2).min(self.w - 1), (y * 2).min(self.h - 1));
                let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
                let s = self.d[y0 * self.w + x0] as u32 + self.d[y0 * self.w + x1] as u32 + self.d[y1 * self.w + x0] as u32 + self.d[y1 * self.w + x1] as u32;
                d[y * w + x] = ((s + 2) / 4) as u8;
            }
        }
        Gray { w, h, d }
    }
}

/// A three-level pyramid (full, 1/2, 1/4).
pub struct Pyr {
    pub levels: [Gray; 3],
}

impl Pyr {
    pub fn new(g: Gray) -> Pyr {
        let l1 = g.half();
        let l2 = l1.half();
        Pyr { levels: [g, l1, l2] }
    }
}

/// A zero-mean, unit-norm template.
#[derive(Clone)]
struct Tpl {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Tpl {
    /// The patch of `g` centered at (cx, cy), or None if it leaves the frame.
    fn cut(g: &Gray, cx: f32, cy: f32, w: usize, h: usize) -> Option<Tpl> {
        let x0 = (cx - w as f32 / 2.0).round();
        let y0 = (cy - h as f32 / 2.0).round();
        if x0 < 0.0 || y0 < 0.0 || x0 as usize + w > g.w || y0 as usize + h > g.h || w < 3 || h < 3 {
            return None;
        }
        let (x0, y0) = (x0 as usize, y0 as usize);
        let mut v = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                v.push(g.at(x0 + x, y0 + y));
            }
        }
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        v.iter_mut().for_each(|p| *p -= mean);
        let norm = v.iter().map(|p| p * p).sum::<f32>().sqrt();
        if norm < 1e-3 {
            return None; // flat: nothing to track
        }
        v.iter_mut().for_each(|p| *p /= norm);
        Some(Tpl { w, h, v })
    }

    /// NCC of the template with `g` at top-left (x0, y0); -1 outside.
    fn ncc(&self, g: &Gray, x0: i32, y0: i32) -> f32 {
        if x0 < 0 || y0 < 0 || x0 as usize + self.w > g.w || y0 as usize + self.h > g.h {
            return -1.0;
        }
        let (x0, y0) = (x0 as usize, y0 as usize);
        let n = (self.w * self.h) as f32;
        let (mut s, mut s2, mut st) = (0f32, 0f32, 0f32);
        for y in 0..self.h {
            let row = &g.d[(y0 + y) * g.w + x0..(y0 + y) * g.w + x0 + self.w];
            let trow = &self.v[y * self.w..(y + 1) * self.w];
            for (p, t) in row.iter().zip(trow) {
                let p = *p as f32;
                s += p;
                s2 += p * p;
                st += p * t;
            }
        }
        let var = s2 - s * s / n;
        if var < 1e-3 {
            return 0.0;
        }
        // the template is zero-mean, so sum(t * (p - mean_p)) == sum(t * p)
        st / var.sqrt()
    }
}

/// A point's template at every pyramid level, and where the point sits
/// relative to the full-resolution template's center (templates start on
/// whole pixels, the point usually doesn't; without this offset every
/// template refresh would shift the track by up to half a pixel).
#[derive(Clone)]
struct PointTpl {
    lv: [Option<Tpl>; 3],
    off: (f32, f32),
}

impl PointTpl {
    fn cut(p: &Pyr, x: f32, y: f32, w: usize, h: usize) -> Option<PointTpl> {
        let mut lv: [Option<Tpl>; 3] = [None, None, None];
        for (l, slot) in lv.iter_mut().enumerate() {
            let k = (1 << l) as f32;
            let (tw, th) = (((w as f32 / k).round() as usize).max(5) | 1, ((h as f32 / k).round() as usize).max(5) | 1);
            *slot = Tpl::cut(&p.levels[l], x / k, y / k, tw, th);
        }
        let t0 = lv[0].as_ref()?;
        let cx0 = (x - t0.w as f32 / 2.0).round() + t0.w as f32 / 2.0;
        let cy0 = (y - t0.h as f32 / 2.0).round() + t0.h as f32 / 2.0;
        Some(PointTpl { lv, off: (x - cx0, y - cy0) })
    }

    /// Best match near (px, py) (full-res point position), searching `radius` px.
    fn find(&self, p: &Pyr, px: f32, py: f32, radius: f32) -> (f32, f32, f32) {
        // work with template centers; the point is the center plus `off`
        let (mut cx, mut cy) = (px - self.off.0, py - self.off.1);
        for l in (0..3).rev() {
            let Some(t) = &self.lv[l] else { continue };
            let k = (1 << l) as f32;
            let r = if l == 2 || self.lv[l + 1..].iter().all(|x| x.is_none()) { (radius / k).ceil() as i32 + 1 } else { 2 };
            let g = &p.levels[l];
            let bx = (cx / k - t.w as f32 / 2.0).round() as i32;
            let by = (cy / k - t.h as f32 / 2.0).round() as i32;
            let mut best = (-2.0f32, bx, by);
            for dy in -r..=r {
                for dx in -r..=r {
                    let v = t.ncc(g, bx + dx, by + dy);
                    if v > best.0 {
                        best = (v, bx + dx, by + dy);
                    }
                }
            }
            let (score, x0, y0) = best;
            let (mut fx, mut fy) = (x0 as f32, y0 as f32);
            if l == 0 {
                // sub-pixel: parabola through the peak and its neighbors
                let para = |a: f32, b: f32, c: f32| {
                    let den = a - 2.0 * b + c;
                    if den.abs() < 1e-6 { 0.0 } else { (0.5 * (a - c) / den).clamp(-0.5, 0.5) }
                };
                fx += para(t.ncc(g, x0 - 1, y0), score, t.ncc(g, x0 + 1, y0));
                fy += para(t.ncc(g, x0, y0 - 1), score, t.ncc(g, x0, y0 + 1));
                return ((fx + t.w as f32 / 2.0) * k + self.off.0, (fy + t.h as f32 / 2.0) * k + self.off.1, score.max(0.0));
            }
            cx = (fx + t.w as f32 / 2.0) * k;
            cy = (fy + t.h as f32 / 2.0) * k;
        }
        (cx + self.off.0, cy + self.off.1, 0.0)
    }
}

/// Tracks one point (full-res px) through `frames` from `start`, both ways.
/// Returns (x, y, confidence) per frame.
pub fn track_point(frames: &[Pyr], start: usize, x: f32, y: f32, pw: usize, ph: usize) -> Vec<(f32, f32, f32)> {
    let n = frames.len();
    let mut out = vec![(x, y, 0.0f32); n];
    if n == 0 || start >= n {
        return out;
    }
    let Some(first) = PointTpl::cut(&frames[start], x, y, pw, ph) else { return out };
    out[start] = (x, y, 1.0);
    let radius = (pw.max(ph) as f32 * 0.75).max(12.0);
    let run = |order: Vec<usize>, out: &mut Vec<(f32, f32, f32)>| {
        let mut cur = first.clone();
        let (mut lx, mut ly) = (x, y);
        let (mut vx, mut vy) = (0.0f32, 0.0f32);
        for i in order {
            let (nx, ny, conf) = cur.find(&frames[i], lx + vx, ly + vy, radius);
            out[i] = (nx, ny, conf);
            vx = nx - lx;
            vy = ny - ly;
            lx = nx;
            ly = ny;
            // drift guard: refresh only while the new patch still looks like the first
            if conf > 0.85 {
                if let Some(np) = PointTpl::cut(&frames[i], nx, ny, pw, ph) {
                    let same = first.lv[0].as_ref().zip(np.lv[0].as_ref()).map(|(a, b)| a.v.iter().zip(&b.v).map(|(p, q)| p * q).sum::<f32>()).unwrap_or(0.0);
                    if same > 0.6 {
                        cur = np;
                    }
                }
            }
        }
    };
    run(((start + 1)..n).collect(), &mut out);
    run((0..start).rev().collect(), &mut out);
    out
}

/// Motion model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Pos,
    Similarity,
    Affine,
}

/// One tracked frame: a 2x3 matrix mapping the start frame's px to this
/// frame's ([a, b, c, d, e, f]: x' = a x + b y + c, y' = d x + e y + f).
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub m: [f64; 6],
    pub conf: f64,
}

impl Motion {
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
    }
    /// Uniform scale and rotation (degrees) of the linear part.
    pub fn scale_rot(&self) -> (f64, f64) {
        let (a, d) = (self.m[0], self.m[3]);
        ((a * a + d * d).sqrt(), d.atan2(a).to_degrees())
    }
}

/// Tracks a region (center cx, cy and size w, h in full-res px).
pub fn track_region(frames: &[Pyr], start: usize, cx: f32, cy: f32, w: f32, h: f32, mode: Mode) -> Vec<Motion> {
    if mode == Mode::Pos {
        let tr = track_point(frames, start, cx, cy, w.max(9.0) as usize, h.max(9.0) as usize);
        return tr.iter().map(|&(x, y, c)| Motion { m: [1.0, 0.0, (x - cx) as f64, 0.0, 1.0, (y - cy) as f64], conf: c as f64 }).collect();
    }
    // a 3x3 grid of patches inside the region
    let (pw, ph) = ((w / 3.0).max(9.0) as usize, (h / 3.0).max(9.0) as usize);
    let pts: Vec<(f32, f32)> = (0..9).map(|i| (cx + ((i % 3) as f32 - 1.0) * w / 3.0, cy + ((i / 3) as f32 - 1.0) * h / 3.0)).collect();
    let tracks: Vec<Vec<(f32, f32, f32)>> = pts.par_iter().map(|&(x, y)| track_point(frames, start, x, y, pw, ph)).collect();
    (0..frames.len())
        .map(|f| {
            let pairs: Vec<((f64, f64), (f64, f64), f64)> = pts.iter().zip(&tracks).map(|(&(x, y), t)| ((x as f64, y as f64), (t[f].0 as f64, t[f].1 as f64), t[f].2 as f64)).filter(|p| p.2 > 0.3).collect();
            let conf = if pairs.is_empty() { 0.0 } else { pairs.iter().map(|p| p.2).sum::<f64>() / 9.0 };
            let m = match mode {
                Mode::Affine if pairs.len() >= 3 => fit_affine(&pairs),
                _ if pairs.len() >= 2 => fit_similarity(&pairs),
                _ => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            };
            Motion { m, conf }
        })
        .collect()
}

/// Weighted least-squares similarity (scale, rotation, translation).
pub fn fit_similarity(p: &[((f64, f64), (f64, f64), f64)]) -> [f64; 6] {
    let wsum: f64 = p.iter().map(|x| x.2).sum::<f64>().max(1e-9);
    let (mut sx, mut sy, mut dx, mut dy) = (0.0, 0.0, 0.0, 0.0);
    for ((x, y), (u, v), w) in p {
        sx += x * w;
        sy += y * w;
        dx += u * w;
        dy += v * w;
    }
    let (sx, sy, dx, dy) = (sx / wsum, sy / wsum, dx / wsum, dy / wsum);
    let (mut a, mut b, mut den) = (0.0, 0.0, 0.0);
    for ((x, y), (u, v), w) in p {
        let (x, y, u, v) = (x - sx, y - sy, u - dx, v - dy);
        a += w * (x * u + y * v);
        b += w * (x * v - y * u);
        den += w * (x * x + y * y);
    }
    if den < 1e-9 {
        return [1.0, 0.0, dx - sx, 0.0, 1.0, dy - sy];
    }
    let (a, b) = (a / den, b / den);
    // x' = a x - b y + tx ; y' = b x + a y + ty
    [a, -b, dx - (a * sx - b * sy), b, a, dy - (b * sx + a * sy)]
}

/// Weighted least-squares affine transform.
pub fn fit_affine(p: &[((f64, f64), (f64, f64), f64)]) -> [f64; 6] {
    // normal equations for [a b c] (x') and [d e f] (y') with design rows [x y 1]
    let mut ata = [[0.0f64; 3]; 3];
    let (mut atu, mut atv) = ([0.0f64; 3], [0.0f64; 3]);
    for ((x, y), (u, v), w) in p {
        let r = [*x, *y, 1.0];
        for i in 0..3 {
            for j in 0..3 {
                ata[i][j] += w * r[i] * r[j];
            }
            atu[i] += w * r[i] * u;
            atv[i] += w * r[i] * v;
        }
    }
    match (solve3(ata, atu), solve3(ata, atv)) {
        (Some(a), Some(b)) => [a[0], a[1], a[2], b[0], b[1], b[2]],
        _ => fit_similarity(p),
    }
}

fn solve3(m: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(&m);
    if d.abs() < 1e-9 {
        return None;
    }
    let mut out = [0.0; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let mut mc = m;
        for r in 0..3 {
            mc[r][c] = b[r];
        }
        *o = det(&mc) / d;
    }
    Some(out)
}

/// Ramer-Douglas-Peucker on several channels at once: the indexes to keep so
/// every channel stays within its tolerance of a linear interpolation.
pub fn simplify(ts: &[f64], chans: &[Vec<f64>], tol: &[f64]) -> Vec<usize> {
    let n = ts.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut worst = (0.0f64, 0usize);
        for i in a + 1..b {
            let f = (ts[i] - ts[a]) / (ts[b] - ts[a]).max(1e-12);
            let err = chans.iter().zip(tol).map(|(c, t)| ((c[a] + (c[b] - c[a]) * f) - c[i]).abs() / t.max(1e-12)).fold(0.0, f64::max);
            if err > worst.0 {
                worst = (err, i);
            }
        }
        if worst.0 > 1.0 {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    (0..n).filter(|i| keep[*i]).collect()
}

/// Frame ranges where confidence stays below `min` (for the reply's `!lost` lines).
pub fn lost_ranges(conf: &[f64], min: f64) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut start = None;
    for (i, c) in conf.iter().enumerate() {
        match (start, *c < min) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                out.push((s, i - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, conf.len() - 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured frame with a bright textured square at (sx, sy).
    fn scene(w: usize, h: usize, sx: f32, sy: f32) -> Gray {
        let mut d = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                // static background texture
                let bg = (((x * 7 + y * 13) % 31) as f32 * 2.0) as u8;
                let (fx, fy) = (x as f32 - sx, y as f32 - sy);
                let v = if (0.0..40.0).contains(&fx) && (0.0..40.0).contains(&fy) {
                    // the object: random 5 px blocks plus a gradient, moving with sub-pixel offsets
                    // (not a checker: a periodic pattern matches itself one cell away, for any tracker)
                    let (bx, by) = ((fx / 5.0).floor() as i64, (fy / 5.0).floor() as i64);
                    let n = ((bx * 92_837_111) ^ (by * 689_287_499)).rem_euclid(1000) as f32 / 1000.0;
                    140.0 + 90.0 * n + fx
                } else {
                    bg as f32
                };
                d[y * w + x] = v.clamp(0.0, 255.0) as u8;
            }
        }
        Gray::new(w, h, d)
    }

    #[test]
    fn recovers_known_motion() {
        let frames: Vec<Pyr> = (0..20).map(|i| Pyr::new(scene(320, 180, 60.0 + i as f32 * 4.3, 50.0 + i as f32 * 1.7))).collect();
        let tr = track_point(&frames, 0, 80.0, 70.0, 30, 30);
        for (i, (x, y, c)) in tr.iter().enumerate() {
            let (ex, ey) = (80.0 + i as f32 * 4.3, 70.0 + i as f32 * 1.7);
            assert!((x - ex).abs() < 1.2 && (y - ey).abs() < 1.2, "frame {i}: got {x},{y} expected {ex},{ey} (conf {c})");
        }
        // starting in the middle works both ways
        let tr = track_point(&frames, 10, 80.0 + 43.0, 70.0 + 17.0, 30, 30);
        assert!((tr[0].0 - 80.0).abs() < 1.5 && (tr[19].0 - (80.0 + 19.0 * 4.3)).abs() < 1.5);
    }

    #[test]
    fn similarity_fit_is_exact_on_clean_points() {
        let (s, r, tx, ty) = (1.2f64, 0.3f64, 5.0, -3.0);
        let pts: Vec<((f64, f64), (f64, f64), f64)> = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (7.0, 3.0)]
            .iter()
            .map(|&(x, y)| ((x, y), (s * (r.cos() * x - r.sin() * y) + tx, s * (r.sin() * x + r.cos() * y) + ty), 1.0))
            .collect();
        let m = fit_similarity(&pts);
        let mo = Motion { m, conf: 1.0 };
        let (sc, rot) = mo.scale_rot();
        assert!((sc - s).abs() < 1e-9 && (rot - r.to_degrees()).abs() < 1e-6);
        let a = fit_affine(&pts);
        assert!((a[2] - tx).abs() < 1e-6 && (a[5] - ty).abs() < 1e-6);
    }

    #[test]
    fn simplify_keeps_corners_only() {
        let ts: Vec<f64> = (0..11).map(|i| i as f64).collect();
        let x: Vec<f64> = ts.iter().map(|t| if *t <= 5.0 { *t } else { 10.0 - t }).collect();
        assert_eq!(simplify(&ts, &[x], &[0.01]), vec![0, 5, 10]);
        assert_eq!(lost_ranges(&[0.9, 0.2, 0.1, 0.9, 0.3], 0.5), vec![(1, 2), (4, 4)]);
    }
}
