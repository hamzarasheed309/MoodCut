//! Auto-trace: the outline of one region of a grayscale frame as a polygon,
//! as After Effects' Auto-trace makes masks from a layer's alpha or luma.
//! The frame is thresholded, the region chosen (the one under a picked
//! point, or the largest), its outer boundary followed pixel by pixel
//! (Moore neighbors) and simplified (Ramer-Douglas-Peucker).

/// Neighbors clockwise from west (y down).
const OFF: [(i64, i64); 8] = [(-1, 0), (-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1)];

/// The traced outline in pixel coordinates (closed, without repeating the
/// first point) and the region's area as a fraction of the frame. `seed`
/// picks the region under that pixel; otherwise the largest one is traced.
/// `tol` is the simplification tolerance in pixels.
pub fn trace(g: &[u8], w: usize, h: usize, thr: u8, invert: bool, seed: Option<(usize, usize)>, tol: f64) -> Option<(Vec<(f64, f64)>, f64)> {
    if w < 2 || h < 2 || g.len() < w * h {
        return None;
    }
    let on: Vec<bool> = g[..w * h].iter().map(|&v| (v >= thr) != invert).collect();
    // 4-connected regions
    let mut label = vec![0u32; w * h];
    let mut sizes = vec![0usize];
    let mut stack: Vec<usize> = vec![];
    for i in 0..w * h {
        if !on[i] || label[i] != 0 {
            continue;
        }
        let id = sizes.len() as u32;
        sizes.push(0);
        label[i] = id;
        stack.push(i);
        while let Some(p) = stack.pop() {
            sizes[id as usize] += 1;
            let (x, y) = (p % w, p / w);
            let mut near = [usize::MAX; 4];
            if x > 0 {
                near[0] = p - 1;
            }
            if x + 1 < w {
                near[1] = p + 1;
            }
            if y > 0 {
                near[2] = p - w;
            }
            if y + 1 < h {
                near[3] = p + w;
            }
            for q in near {
                if q != usize::MAX && on[q] && label[q] == 0 {
                    label[q] = id;
                    stack.push(q);
                }
            }
        }
    }
    let target = match seed.map(|(x, y)| label[y.min(h - 1) * w + x.min(w - 1)]) {
        Some(l) if l != 0 => l,
        _ => (1..sizes.len()).max_by_key(|&l| sizes[l])? as u32,
    };
    let start = label.iter().position(|&l| l == target)?;
    let (sx, sy) = ((start % w) as i64, (start / w) as i64);
    let inside = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && label[y as usize * w + x as usize] == target;
    let mut pts = vec![(sx, sy)];
    let (mut cx, mut cy) = (sx, sy);
    // the backtrack pixel (outside the region) as a direction from the
    // current pixel; west of the first pixel is outside by construction
    let mut back = 0usize;
    let mut first_state = None;
    let cap = 4 * w * h;
    loop {
        let Some((k, nx, ny)) = (1..=8).find_map(|k| {
            let d = (back + k) % 8;
            let (nx, ny) = (cx + OFF[d].0, cy + OFF[d].1);
            inside(nx, ny).then_some((k, nx, ny))
        }) else {
            break; // a single pixel
        };
        let pd = (back + k - 1) % 8;
        let (bx, by) = (cx + OFF[pd].0, cy + OFF[pd].1);
        back = OFF.iter().position(|&o| (nx + o.0, ny + o.1) == (bx, by)).unwrap_or(0);
        // the walk is a state machine: a repeated state closes the loop
        let state = (nx, ny, back);
        match first_state {
            None => first_state = Some(state),
            Some(s) if s == state => break,
            _ => {}
        }
        pts.push((nx, ny));
        (cx, cy) = (nx, ny);
        if pts.len() > cap {
            break;
        }
    }
    if pts.len() > 1 && pts.last() == Some(&(sx, sy)) {
        pts.pop();
    }
    let poly: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| (x as f64 + 0.5, y as f64 + 0.5)).collect();
    Some((simplify_closed(&poly, tol.max(0.1)), sizes[target as usize] as f64 / (w * h) as f64))
}

/// Ramer-Douglas-Peucker on an open polyline (end points kept).
pub fn rdp(pts: &[(f64, f64)], eps: f64) -> Vec<(f64, f64)> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (p, q) = (pts[a], pts[b]);
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let len = (dx * dx + dy * dy).sqrt();
        let (mut far, mut fd) = (a, -1.0);
        for (i, r) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = if len < 1e-12 { ((r.0 - p.0).powi(2) + (r.1 - p.1).powi(2)).sqrt() } else { ((r.0 - p.0) * dy - (r.1 - p.1) * dx).abs() / len };
            if d > fd {
                fd = d;
                far = i;
            }
        }
        if fd > eps {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// RDP on a closed outline: split at the point farthest from the first.
pub fn simplify_closed(pts: &[(f64, f64)], eps: f64) -> Vec<(f64, f64)> {
    if pts.len() < 4 {
        return pts.to_vec();
    }
    let p0 = pts[0];
    let far = (1..pts.len()).max_by(|&a, &b| {
        let da = (pts[a].0 - p0.0).powi(2) + (pts[a].1 - p0.1).powi(2);
        let db = (pts[b].0 - p0.0).powi(2) + (pts[b].1 - p0.1).powi(2);
        da.total_cmp(&db)
    });
    let Some(far) = far else { return pts.to_vec() };
    let a = rdp(&pts[..=far], eps);
    let mut tail: Vec<(f64, f64)> = pts[far..].to_vec();
    tail.push(p0);
    let b = rdp(&tail, eps);
    let mut out = a;
    out.extend_from_slice(&b[1..b.len() - 1]);
    out
}

/// A closed polygon (pixel coordinates in a w x h frame) as a mask path in
/// 0..1 layer fractions.
pub fn to_path(poly: &[(f64, f64)], w: usize, h: usize) -> String {
    let f = |v: f64| {
        let s = format!("{v:.4}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() || s == "-0" { "0".to_string() } else { s }
    };
    let mut d = String::new();
    for (i, (x, y)) in poly.iter().enumerate() {
        d.push_str(if i == 0 { "M" } else { " L" });
        d.push_str(&format!("{},{}", f(x / w as f64), f(y / h as f64)));
    }
    if !d.is_empty() {
        d.push_str(" Z");
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_a_square_to_four_corners() {
        let (w, h) = (40, 30);
        let mut g = vec![0u8; w * h];
        for y in 5..20 {
            for x in 10..30 {
                g[y * w + x] = 255;
            }
        }
        // a smaller blob elsewhere: the largest region wins
        g[25 * w + 3] = 255;
        let (poly, area) = trace(&g, w, h, 128, false, None, 1.0).unwrap();
        assert_eq!(poly.len(), 4, "{poly:?}");
        assert!((area - 300.0 / 1200.0).abs() < 1e-9);
        let xs: Vec<f64> = poly.iter().map(|p| p.0).collect();
        assert!(xs.iter().any(|&x| (x - 10.5).abs() < 1e-9) && xs.iter().any(|&x| (x - 29.5).abs() < 1e-9));
        // the picked region instead
        let (one, _) = trace(&g, w, h, 128, false, Some((3, 25)), 1.0).unwrap();
        assert_eq!(one.len(), 1);
        let d = to_path(&poly, w, h);
        assert!(d.starts_with('M') && d.ends_with('Z'));
    }

    #[test]
    fn traces_a_diagonal_shape() {
        let (w, h) = (32, 32);
        let mut g = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                if (x as i32 - 16).abs() + (y as i32 - 16).abs() < 10 {
                    g[y * w + x] = 255;
                }
            }
        }
        let (poly, _) = trace(&g, w, h, 128, false, None, 1.0).unwrap();
        assert!((4..=8).contains(&poly.len()), "{poly:?}");
    }
}
