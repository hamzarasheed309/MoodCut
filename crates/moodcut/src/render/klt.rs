//! Feature tracking shared by the planar (perspective) tracker and the 3D
//! camera tracker: Shi-Tomasi corners and pyramidal Lucas-Kanade (Bouguet's
//! method) with a forward-backward check, on f32 image pyramids.
//!
//! [`FeatureTracker`] takes frames one at a time (only two pyramids are held
//! in memory), keeps a target number of features alive by detecting new
//! corners where there are few, and records every track's positions.

use rayon::prelude::*;

/// One pyramid level: the image and its x / y derivatives.
pub struct Level {
    pub w: usize,
    pub h: usize,
    pub i: Vec<f32>,
    pub gx: Vec<f32>,
    pub gy: Vec<f32>,
}

/// A Gaussian image pyramid, full resolution first.
pub struct Pyramid {
    pub levels: Vec<Level>,
}

impl Pyramid {
    pub fn new(gray: &[u8], w: usize, h: usize, levels: usize) -> Pyramid {
        let mut out = vec![];
        let (mut cw, mut ch) = (w, h);
        let mut cur: Vec<f32> = gray.iter().map(|v| *v as f32).collect();
        for l in 0..levels.max(1) {
            let (gx, gy) = gradients(&cur, cw, ch);
            let next = if l + 1 < levels && cw >= 40 && ch >= 40 { Some(downsample(&cur, cw, ch)) } else { None };
            out.push(Level { w: cw, h: ch, i: std::mem::take(&mut cur), gx, gy });
            match next {
                Some((nw, nh, d)) => {
                    cur = d;
                    cw = nw;
                    ch = nh;
                }
                None => break,
            }
        }
        Pyramid { levels: out }
    }
}

/// Scharr derivatives (per-pixel units).
fn gradients(i: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let mut gx = vec![0f32; w * h];
    let mut gy = vec![0f32; w * h];
    if w < 3 || h < 3 {
        return (gx, gy);
    }
    gx.par_chunks_mut(w).zip(gy.par_chunks_mut(w)).enumerate().for_each(|(y, (rx, ry))| {
        let ym = y.saturating_sub(1);
        let yp = (y + 1).min(h - 1);
        let (a, b, c) = (&i[ym * w..ym * w + w], &i[y * w..y * w + w], &i[yp * w..yp * w + w]);
        for x in 0..w {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(w - 1);
            rx[x] = (3.0 * (a[xp] - a[xm]) + 10.0 * (b[xp] - b[xm]) + 3.0 * (c[xp] - c[xm])) / 32.0;
            ry[x] = (3.0 * (c[xm] - a[xm]) + 10.0 * (c[x] - a[x]) + 3.0 * (c[xp] - a[xp])) / 32.0;
        }
    });
    (gx, gy)
}

/// Gaussian [1 4 6 4 1] blur and decimation by two.
fn downsample(i: &[f32], w: usize, h: usize) -> (usize, usize, Vec<f32>) {
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    // horizontal pass at the kept columns
    let mut tmp = vec![0f32; nw * h];
    tmp.par_chunks_mut(nw).enumerate().for_each(|(y, row)| {
        let src = &i[y * w..y * w + w];
        for (nx, o) in row.iter_mut().enumerate() {
            let x = (nx * 2) as isize;
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                let xx = (x + k as isize - 2).clamp(0, w as isize - 1) as usize;
                s += kv * src[xx];
            }
            *o = s;
        }
    });
    let mut out = vec![0f32; nw * nh];
    out.par_chunks_mut(nw).enumerate().for_each(|(ny, row)| {
        let y = (ny * 2) as isize;
        for (nx, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (k, kv) in K.iter().enumerate() {
                let yy = (y + k as isize - 2).clamp(0, h as isize - 1) as usize;
                s += kv * tmp[yy * nw + nx];
            }
            *o = s;
        }
    });
    (nw, nh, out)
}

/// Bilinear sample (clamped to the image).
#[inline]
pub fn bil(img: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32 - 1e-3);
    let y = y.clamp(0.0, (h - 1) as f32 - 1e-3);
    let (x0, y0) = (x as usize, y as usize);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let i = y0 * w + x0;
    let a = img[i] + (img[i + 1] - img[i]) * fx;
    let b = img[i + w] + (img[i + w + 1] - img[i + w]) * fx;
    a + (b - a) * fy
}

/// Box sum of radius r along rows then columns (edges clamp).
fn box_sum(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..y * w + w];
        for (x, o) in row.iter_mut().enumerate() {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            *o = s[a..=b].iter().sum();
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for yy in a..=b {
                s += tmp[yy * w + x];
            }
            *o = s;
        }
    });
    out
}

/// Shi-Tomasi corners on one level: local maxima of the structure tensor's
/// smaller eigenvalue, strongest first, at least `min_dist` apart from each
/// other and from `existing`, where `keep(x, y)` allows. At most `max`.
pub fn corners(l: &Level, max: usize, min_dist: f32, keep: &(dyn Fn(f32, f32) -> bool + Sync), existing: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let (w, h) = (l.w, l.h);
    if w < 24 || h < 24 || max == 0 {
        return vec![];
    }
    let xx: Vec<f32> = l.gx.iter().map(|g| g * g).collect();
    let yy: Vec<f32> = l.gy.iter().map(|g| g * g).collect();
    let xy: Vec<f32> = l.gx.iter().zip(&l.gy).map(|(a, b)| a * b).collect();
    let (sxx, syy, sxy) = (box_sum(&xx, w, h, 2), box_sum(&yy, w, h, 2), box_sum(&xy, w, h, 2));
    let resp: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (a, c, b) = (sxx[k], syy[k], sxy[k]);
            (a + c) / 2.0 - (((a - c) / 2.0).powi(2) + b * b).sqrt()
        })
        .collect();
    let margin = 10usize;
    let best = resp.par_iter().cloned().reduce(|| 0.0, f32::max);
    // relative quality, and an absolute floor so flat frames give nothing
    let thr = (best * 0.01).max(25.0 * 4.0);
    let mut cand: Vec<(f32, usize, usize)> = (margin..h.saturating_sub(margin))
        .into_par_iter()
        .flat_map_iter(|y| {
            let resp = &resp;
            (margin..w.saturating_sub(margin)).filter_map(move |x| {
                let v = resp[y * w + x];
                if v < thr {
                    return None;
                }
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if (dx != 0 || dy != 0) && resp[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize] > v {
                            return None;
                        }
                    }
                }
                Some((v, x, y))
            })
        })
        .collect();
    cand.sort_by(|a, b| b.0.total_cmp(&a.0));
    // greedy spacing on a grid of min_dist cells
    let cell = min_dist.max(1.0);
    let (gw, gh) = ((w as f32 / cell).ceil() as usize + 1, (h as f32 / cell).ceil() as usize + 1);
    let mut grid: Vec<Vec<(f32, f32)>> = vec![vec![]; gw * gh];
    let put = |grid: &mut Vec<Vec<(f32, f32)>>, x: f32, y: f32| {
        let (cx, cy) = ((x / cell).floor().max(0.0) as usize, (y / cell).floor().max(0.0) as usize);
        if cx < gw && cy < gh {
            grid[cy * gw + cx].push((x, y));
        }
    };
    for &(x, y) in existing {
        put(&mut grid, x, y);
    }
    let d2 = min_dist * min_dist;
    let mut out = vec![];
    for (_, x, y) in cand {
        let (fx, fy) = (x as f32, y as f32);
        if !keep(fx, fy) {
            continue;
        }
        let (cx, cy) = ((fx / cell) as i32, (fy / cell) as i32);
        let mut free = true;
        'n: for ny in cy - 1..=cy + 1 {
            for nx in cx - 1..=cx + 1 {
                if nx < 0 || ny < 0 || nx as usize >= gw || ny as usize >= gh {
                    continue;
                }
                for &(px, py) in &grid[ny as usize * gw + nx as usize] {
                    if (px - fx).powi(2) + (py - fy).powi(2) < d2 {
                        free = false;
                        break 'n;
                    }
                }
            }
        }
        if free {
            put(&mut grid, fx, fy);
            out.push((fx, fy));
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

/// Lucas-Kanade settings.
#[derive(Clone, Copy, Debug)]
pub struct LkOpts {
    /// Window half size (7 = 15x15 px).
    pub half: i32,
    pub iters: usize,
    /// Stop when the update is below this (px at the level).
    pub eps: f32,
    /// Smallest structure-tensor eigenvalue per window pixel (texture needed).
    pub min_eig: f32,
    /// Largest mean absolute difference (gray levels) of the matched window.
    pub max_err: f32,
    /// Forward-backward check: largest round-trip distance (px), 0 = off.
    pub fb: f32,
}

impl Default for LkOpts {
    fn default() -> Self {
        LkOpts { half: 7, iters: 20, eps: 0.01, min_eig: 1.0, max_err: 18.0, fb: 1.0 }
    }
}

/// Tracks one point from pyramid `a` to `b`, starting the search at `guess`.
/// Returns the new position and the mean absolute difference of the window.
pub fn lk(a: &Pyramid, b: &Pyramid, p: (f32, f32), guess: (f32, f32), o: &LkOpts) -> Option<(f32, f32, f32)> {
    let nl = a.levels.len().min(b.levels.len());
    if nl == 0 {
        return None;
    }
    let half = o.half;
    let win = (2 * half + 1) as usize;
    let n = win * win;
    let mut tpl = vec![0f32; n];
    let mut ix = vec![0f32; n];
    let mut iy = vec![0f32; n];
    let top = (1u32 << (nl - 1)) as f32;
    let mut g = ((guess.0 - p.0) / top, (guess.1 - p.1) / top);
    for l in (0..nl).rev() {
        let (la, lb) = (&a.levels[l], &b.levels[l]);
        let s = (1u32 << l) as f32;
        let (px, py) = (p.0 / s, p.1 / s);
        let hf = half as f32;
        let inside = |x: f32, y: f32, lv: &Level| x >= hf + 1.0 && y >= hf + 1.0 && x <= lv.w as f32 - hf - 2.0 && y <= lv.h as f32 - hf - 2.0;
        if !inside(px, py, la) {
            if l == 0 {
                return None;
            }
            g = (g.0 * 2.0, g.1 * 2.0);
            continue;
        }
        let (mut gxx, mut gxy, mut gyy) = (0f32, 0f32, 0f32);
        let mut k = 0;
        for dy in -half..=half {
            for dx in -half..=half {
                let (x, y) = (px + dx as f32, py + dy as f32);
                let (t, a1, b1) = (bil(&la.i, la.w, la.h, x, y), bil(&la.gx, la.w, la.h, x, y), bil(&la.gy, la.w, la.h, x, y));
                tpl[k] = t;
                ix[k] = a1;
                iy[k] = b1;
                gxx += a1 * a1;
                gxy += a1 * b1;
                gyy += b1 * b1;
                k += 1;
            }
        }
        let det = gxx * gyy - gxy * gxy;
        let min_eig = ((gxx + gyy) - ((gxx - gyy).powi(2) + 4.0 * gxy * gxy).sqrt()) / 2.0 / n as f32;
        if min_eig < o.min_eig || det.abs() < 1e-6 {
            if l == 0 {
                return None;
            }
            g = (g.0 * 2.0, g.1 * 2.0);
            continue;
        }
        let mut v = (0f32, 0f32);
        for _ in 0..o.iters {
            let (qx, qy) = (px + g.0 + v.0, py + g.1 + v.1);
            if !inside(qx, qy, lb) {
                if l == 0 {
                    return None;
                }
                break;
            }
            let (mut bx, mut by) = (0f32, 0f32);
            let mut k = 0;
            for dy in -half..=half {
                for dx in -half..=half {
                    let d = tpl[k] - bil(&lb.i, lb.w, lb.h, qx + dx as f32, qy + dy as f32);
                    bx += d * ix[k];
                    by += d * iy[k];
                    k += 1;
                }
            }
            let dvx = (gyy * bx - gxy * by) / det;
            let dvy = (gxx * by - gxy * bx) / det;
            v = (v.0 + dvx, v.1 + dvy);
            if dvx * dvx + dvy * dvy < o.eps * o.eps {
                break;
            }
        }
        g = if l > 0 { ((g.0 + v.0) * 2.0, (g.1 + v.1) * 2.0) } else { (g.0 + v.0, g.1 + v.1) };
    }
    let q = (p.0 + g.0, p.1 + g.1);
    let l0 = &b.levels[0];
    let hf = half as f32;
    if !(q.0 >= hf && q.1 >= hf && q.0 <= l0.w as f32 - hf - 1.0 && q.1 <= l0.h as f32 - hf - 1.0) || !q.0.is_finite() || !q.1.is_finite() {
        return None;
    }
    // error at full resolution
    let la = &a.levels[0];
    let mut err = 0f32;
    for dy in -half..=half {
        for dx in -half..=half {
            err += (bil(&la.i, la.w, la.h, p.0 + dx as f32, p.1 + dy as f32) - bil(&l0.i, l0.w, l0.h, q.0 + dx as f32, q.1 + dy as f32)).abs();
        }
    }
    Some((q.0, q.1, err / n as f32))
}

/// Tracks many points from `a` to `b` in parallel; None where a point was
/// lost (no texture, left the frame, too different, or failed the
/// forward-backward check).
pub fn track(a: &Pyramid, b: &Pyramid, pts: &[(f32, f32)], guesses: &[(f32, f32)], o: &LkOpts) -> Vec<Option<(f32, f32)>> {
    pts.par_iter()
        .zip(guesses.par_iter())
        .map(|(&p, &g)| {
            let (x, y, err) = lk(a, b, p, g, o)?;
            if err > o.max_err {
                return None;
            }
            if o.fb > 0.0 {
                let (bx, by, _) = lk(b, a, (x, y), p, o)?;
                if (bx - p.0).hypot(by - p.1) > o.fb {
                    return None;
                }
            }
            Some((x, y))
        })
        .collect()
}

/// A track: positions (frame index, x, y) in consecutive frames.
pub type Track = Vec<(u32, f32, f32)>;

/// Tracks features through frames given one at a time.
pub struct FeatureTracker {
    pub tracks: Vec<Track>,
    active: Vec<usize>,
    prev: Option<Pyramid>,
    frame: u32,
    w: usize,
    h: usize,
    levels: usize,
    /// Features kept alive.
    pub target: usize,
    /// Minimum spacing of new features (px).
    pub min_dist: f32,
    pub opts: LkOpts,
}

impl FeatureTracker {
    pub fn new(w: usize, h: usize, target: usize) -> FeatureTracker {
        let min_dist = ((w * h) as f32 / (target.max(1) as f32 * 2.5)).sqrt().clamp(6.0, 40.0);
        FeatureTracker { tracks: vec![], active: vec![], prev: None, frame: 0, w, h, levels: 4, target, min_dist, opts: LkOpts::default() }
    }

    /// Number of frames pushed so far.
    pub fn frames(&self) -> u32 {
        self.frame
    }

    /// Positions of the features alive in the last frame: (track index, x, y).
    pub fn active(&self) -> Vec<(usize, f32, f32)> {
        self.active.iter().filter_map(|&i| self.tracks[i].last().map(|&(_, x, y)| (i, x, y))).collect()
    }

    /// Ends a track early (an outlier found by the caller).
    pub fn drop_track(&mut self, i: usize) {
        self.active.retain(|&a| a != i);
    }

    /// Adds the next frame (8-bit gray, w x h). New features are detected
    /// where `detect_in(x, y)` allows (everywhere when None).
    pub fn push(&mut self, gray: &[u8], detect_in: Option<&(dyn Fn(f32, f32) -> bool + Sync)>) {
        let pyr = Pyramid::new(gray, self.w, self.h, self.levels);
        let f = self.frame;
        if let Some(prev) = &self.prev {
            let pts: Vec<(f32, f32)> = self.active.iter().map(|&i| self.tracks[i].last().map(|p| (p.1, p.2)).unwrap_or((0.0, 0.0))).collect();
            // constant-velocity prediction
            let guesses: Vec<(f32, f32)> = self
                .active
                .iter()
                .zip(&pts)
                .map(|(&i, &p)| {
                    let t = &self.tracks[i];
                    if t.len() >= 2 {
                        let a = t[t.len() - 2];
                        (p.0 + (p.0 - a.1), p.1 + (p.1 - a.2))
                    } else {
                        p
                    }
                })
                .collect();
            let res = track(prev, &pyr, &pts, &guesses, &self.opts);
            // a bad prediction (sudden stop) can fail where no prediction works
            let retry: Vec<usize> = res.iter().enumerate().filter(|(k, r)| r.is_none() && guesses[*k] != pts[*k]).map(|(k, _)| k).collect();
            let mut res = res;
            if !retry.is_empty() {
                let rp: Vec<(f32, f32)> = retry.iter().map(|&k| pts[k]).collect();
                let again = track(prev, &pyr, &rp, &rp, &self.opts);
                for (k, r) in retry.into_iter().zip(again) {
                    res[k] = r;
                }
            }
            let mut alive = Vec::with_capacity(self.active.len());
            for (&i, r) in self.active.iter().zip(res) {
                if let Some((x, y)) = r {
                    self.tracks[i].push((f, x, y));
                    alive.push(i);
                }
            }
            self.active = alive;
        }
        // replenish where features are sparse
        if self.active.len() < self.target * 9 / 10 {
            let existing: Vec<(f32, f32)> = self.active.iter().filter_map(|&i| self.tracks[i].last().map(|p| (p.1, p.2))).collect();
            let all = |_: f32, _: f32| true;
            let keep: &(dyn Fn(f32, f32) -> bool + Sync) = detect_in.unwrap_or(&all);
            let want = self.target - self.active.len();
            for (x, y) in corners(&pyr.levels[0], want, self.min_dist, keep, &existing) {
                self.active.push(self.tracks.len());
                self.tracks.push(vec![(f, x, y)]);
            }
        }
        self.prev = Some(pyr);
        self.frame += 1;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A smooth random texture sampled at a sub-pixel offset.
    pub fn texture(w: usize, h: usize, ox: f32, oy: f32) -> Vec<u8> {
        let f = |x: f32, y: f32| -> f32 {
            let mut v = 0.0;
            for (k, (a, b, c)) in [(0.11f32, 0.07f32, 0.0f32), (0.05, -0.13, 1.3), (0.21, 0.17, 2.1), (-0.09, 0.23, 0.7), (0.31, -0.05, 4.0)].iter().enumerate() {
                v += ((x * a + y * b) + c).sin() * (1.0 / (k as f32 + 1.0));
            }
            v
        };
        let mut d = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x as f32 - ox, y as f32 - oy);
                d[y * w + x] = (128.0 + 50.0 * f(sx, sy)).clamp(0.0, 255.0) as u8;
            }
        }
        d
    }

    #[test]
    fn lucas_kanade_finds_subpixel_shift() {
        let (w, h) = (320, 200);
        let a = Pyramid::new(&texture(w, h, 0.0, 0.0), w, h, 4);
        let b = Pyramid::new(&texture(w, h, 6.4, -3.3), w, h, 4);
        let pts = corners(&a.levels[0], 40, 12.0, &|_, _| true, &[]);
        assert!(pts.len() >= 20, "found {} corners", pts.len());
        let res = track(&a, &b, &pts, &pts, &LkOpts::default());
        let ok: Vec<(f32, f32)> = pts.iter().zip(&res).filter_map(|(p, r)| r.map(|q| (q.0 - p.0, q.1 - p.1))).collect();
        assert!(ok.len() >= pts.len() * 3 / 4, "{} of {} tracked", ok.len(), pts.len());
        for (dx, dy) in ok {
            assert!((dx - 6.4).abs() < 0.15 && (dy + 3.3).abs() < 0.15, "{dx} {dy}");
        }
    }

    #[test]
    fn feature_tracker_follows_a_pan() {
        let (w, h) = (256, 160);
        let mut ft = FeatureTracker::new(w, h, 120);
        for i in 0..12 {
            ft.push(&texture(w, h, i as f32 * 2.5, i as f32 * 0.8), None);
        }
        let long: Vec<&Track> = ft.tracks.iter().filter(|t| t.len() >= 12).collect();
        assert!(long.len() >= 30, "{} full-length tracks", long.len());
        for t in long {
            let (a, b) = (t[0], t[11]);
            assert!(((b.1 - a.1) - 27.5).abs() < 0.3 && ((b.2 - a.2) - 8.8).abs() < 0.3);
        }
    }
}
