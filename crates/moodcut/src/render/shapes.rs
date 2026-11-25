//! Vector shapes for shape layers and masks: geometry, gradients, strokes,
//! trim paths (draw-on) and repeaters.

use super::eval::Ev;
use crate::model::project::Shape;
use crate::model::value::Color;
use resvg::tiny_skia::{
    FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder, PathSegment, Pixmap, Point, RadialGradient, Rect, Shader, SpreadMode, Stroke, StrokeDash, Transform,
};

pub fn parse_svg_path(d: &str, sx: f32, sy: f32, ox: f32, oy: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let tx = |x: f64| x as f32 * sx + ox;
    let ty = |y: f64| y as f32 * sy + oy;
    for seg in svgtypes::SimplifyingPathParser::from(d) {
        match seg.ok()? {
            svgtypes::SimplePathSegment::MoveTo { x, y } => pb.move_to(tx(x), ty(y)),
            svgtypes::SimplePathSegment::LineTo { x, y } => pb.line_to(tx(x), ty(y)),
            svgtypes::SimplePathSegment::CurveTo { x1, y1, x2, y2, x, y } => pb.cubic_to(tx(x1), ty(y1), tx(x2), ty(y2), tx(x), ty(y)),
            svgtypes::SimplePathSegment::Quadratic { x1, y1, x, y } => pb.quad_to(tx(x1), ty(y1), tx(x), ty(y)),
            svgtypes::SimplePathSegment::ClosePath => pb.close(),
        }
    }
    pb.finish()
}

pub fn rounded_rect_path(cx: f32, cy: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let rect = Rect::from_xywh(cx - w / 2.0, cy - h / 2.0, w.max(0.01), h.max(0.01))?;
    super::text::rounded_rect(rect, r)
}

pub fn ellipse_path(cx: f32, cy: f32, w: f32, h: f32) -> Option<Path> {
    PathBuilder::from_oval(Rect::from_xywh(cx - w / 2.0, cy - h / 2.0, w.max(0.01), h.max(0.01))?)
}

pub fn polygon_path(cx: f32, cy: f32, n: usize, r: f32) -> Option<Path> {
    let n = n.max(3);
    let mut pb = PathBuilder::new();
    for i in 0..n {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / n as f32;
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    pb.finish()
}

pub fn star_path(cx: f32, cy: f32, n: usize, r1: f32, r2: f32) -> Option<Path> {
    let n = n.max(2);
    let mut pb = PathBuilder::new();
    for i in 0..n * 2 {
        let r = if i % 2 == 0 { r1 } else { r2 };
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / n as f32;
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    pb.finish()
}

/// Shape geometry in shape-local coordinates (centered at origin for primitives).
pub fn geometry(s: &Shape, ev: &Ev) -> Option<Path> {
    let p = &s.props;
    match s.kind.as_str() {
        "rect" => rounded_rect_path(0.0, 0.0, ev.f(p, "w"), ev.f(p, "h"), ev.f(p, "radius")),
        "ellipse" => ellipse_path(0.0, 0.0, ev.f(p, "w"), ev.f(p, "h")),
        "polygon" => polygon_path(0.0, 0.0, ev.num(p, "points").round() as usize, ev.f(p, "radius")),
        "star" => star_path(0.0, 0.0, ev.num(p, "points").round() as usize, ev.f(p, "r1"), ev.f(p, "r2")),
        "path" => parse_svg_path(&ev.str(p, "d"), 1.0, 1.0, 0.0, 0.0),
        "line" => {
            let (a, b) = (ev.vec(p, "from"), ev.vec(p, "to"));
            let mut pb = PathBuilder::new();
            pb.move_to(a.0 as f32, a.1 as f32);
            pb.line_to(b.0 as f32, b.1 as f32);
            pb.finish()
        }
        "arrow" => {
            let (a, b) = (ev.vec(p, "from"), ev.vec(p, "to"));
            let head = ev.f(p, "head");
            let ang = ((b.1 - a.1) as f32).atan2((b.0 - a.0) as f32);
            let mut pb = PathBuilder::new();
            pb.move_to(a.0 as f32, a.1 as f32);
            pb.line_to(b.0 as f32, b.1 as f32);
            for side in [-1.0f32, 1.0] {
                let ha = ang + std::f32::consts::PI + side * 0.5;
                pb.move_to(b.0 as f32, b.1 as f32);
                pb.line_to(b.0 as f32 + head * ha.cos(), b.1 as f32 + head * ha.sin());
            }
            pb.finish()
        }
        _ => None,
    }
}

/// Local transform of a shape inside its layer.
pub fn local_transform(s: &Shape, ev: &Ev) -> Transform {
    let p = &s.props;
    let (ox, oy) = ev.vec(p, "offset");
    let rot = ev.f(p, "rot");
    let (sx, sy) = ev.vec(p, "scale");
    Transform::from_translate(ox as f32, oy as f32).pre_rotate(rot).pre_scale(sx as f32, sy as f32)
}

pub fn path_length(p: &Path) -> f32 {
    let mut len = 0.0;
    let mut cur = Point::from_xy(0.0, 0.0);
    let mut start = cur;
    let dist = |a: Point, b: Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    for seg in p.segments() {
        match seg {
            PathSegment::MoveTo(pt) => {
                cur = pt;
                start = pt;
            }
            PathSegment::LineTo(pt) => {
                len += dist(cur, pt);
                cur = pt;
            }
            PathSegment::QuadTo(c, pt) => {
                let mut prev = cur;
                for i in 1..=16 {
                    let t = i as f32 / 16.0;
                    let mt = 1.0 - t;
                    let q = Point::from_xy(mt * mt * cur.x + 2.0 * mt * t * c.x + t * t * pt.x, mt * mt * cur.y + 2.0 * mt * t * c.y + t * t * pt.y);
                    len += dist(prev, q);
                    prev = q;
                }
                cur = pt;
            }
            PathSegment::CubicTo(c1, c2, pt) => {
                let mut prev = cur;
                for i in 1..=24 {
                    let t = i as f32 / 24.0;
                    let mt = 1.0 - t;
                    let q = Point::from_xy(
                        mt * mt * mt * cur.x + 3.0 * mt * mt * t * c1.x + 3.0 * mt * t * t * c2.x + t * t * t * pt.x,
                        mt * mt * mt * cur.y + 3.0 * mt * mt * t * c1.y + 3.0 * mt * t * t * c2.y + t * t * t * pt.y,
                    );
                    len += dist(prev, q);
                    prev = q;
                }
                cur = pt;
            }
            PathSegment::Close => {
                len += dist(cur, start);
                cur = start;
            }
        }
    }
    len
}

pub fn paint_for(color: Color, color2: Color, grad_type: &str, angle: f32, bounds: Rect, opacity: f32) -> Paint<'static> {
    let mut paint = Paint { anti_alias: true, ..Default::default() };
    let c1 = color.with_alpha(color.a * opacity);
    if color2.a <= 0.0 && color2.r == 0.0 && color2.g == 0.0 && color2.b == 0.0 {
        paint.set_color(c1.to_skia());
        return paint;
    }
    let c2 = color2.with_alpha(color2.a * opacity);
    let stops = vec![GradientStop::new(0.0, c1.to_skia()), GradientStop::new(1.0, c2.to_skia())];
    let (cx, cy) = (bounds.x() + bounds.width() / 2.0, bounds.y() + bounds.height() / 2.0);
    let shader = if grad_type == "radial" {
        RadialGradient::new(Point::from_xy(cx, cy), 0.0, Point::from_xy(cx, cy), bounds.width().max(bounds.height()) / 2.0, stops, SpreadMode::Pad, Transform::identity())
    } else {
        let a = angle.to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        let half = (bounds.width() * dx.abs() + bounds.height() * dy.abs()) / 2.0;
        LinearGradient::new(Point::from_xy(cx - dx * half, cy - dy * half), Point::from_xy(cx + dx * half, cy + dy * half), stops, SpreadMode::Pad, Transform::identity())
    };
    paint.shader = shader.unwrap_or(Shader::SolidColor(c1.to_skia()));
    paint
}

fn is_none(c: &Color) -> bool {
    c.a <= 0.0
}

/// Draws one shape (with repeater/trim) into `pm` using `m` (layer -> buffer transform).
pub fn draw_shape(pm: &mut Pixmap, s: &Shape, ev: &Ev, m: Transform, layer_opacity: f32) {
    let p = &s.props;
    let Some(geom) = geometry(s, ev) else { return };
    let local = local_transform(s, ev);
    let opacity = ev.f(p, "opacity") * layer_opacity;
    if opacity <= 0.0 {
        return;
    }
    let fill = ev.color(p, "fill");
    let fill2 = ev.color(p, "fill2");
    let stroke_c = ev.color(p, "stroke");
    let sw = ev.f(p, "strokeW");
    let bounds = geom.bounds();
    let gt = ev.str(p, "gradType");
    let ga = ev.f(p, "gradAngle");
    let copies = ev.num(p, "repeat").round().max(0.0) as usize;
    let rep_off = ev.vec(p, "repOffset");
    let rep_rot = ev.f(p, "repRot");
    let rep_sc = ev.vec(p, "repScale");
    let rep_op = ev.f(p, "repOpacity");
    // trim paths -> dash
    let ts = ev.f(p, "trimStart").clamp(0.0, 100.0);
    let te = ev.f(p, "trimEnd").clamp(0.0, 100.0);
    let to = ev.f(p, "trimOffset");
    let trimmed = ts > 0.001 || te < 99.999;
    let mut stroke_visible = !is_none(&stroke_c) && sw > 0.0;
    let mut dash = None;
    if trimmed {
        let span = (te - ts).abs();
        if span < 0.001 {
            stroke_visible = false;
        } else {
            let a = (ts.min(te) + to).rem_euclid(100.0);
            let len = path_length(&geom).max(0.001);
            let vis = span / 100.0 * len;
            let start = a / 100.0 * len;
            let gap = len * 2.0 + 1.0;
            let period = vis + gap;
            dash = StrokeDash::new(vec![vis.max(0.001), gap], (period - start).rem_euclid(period));
        }
    } else {
        let d = ev.str(p, "dash");
        let arr: Vec<f32> = d.split(',').filter_map(|x| x.trim().parse::<f32>().ok()).filter(|x| *x > 0.0).collect();
        if arr.len() >= 2 && arr.len() % 2 == 0 {
            dash = StrokeDash::new(arr, 0.0);
        } else if arr.len() == 1 {
            dash = StrokeDash::new(vec![arr[0], arr[0]], 0.0);
        }
    }
    let cap = match ev.str(p, "cap").as_str() {
        "butt" => LineCap::Butt,
        "square" => LineCap::Square,
        _ => LineCap::Round,
    };
    let join = match ev.str(p, "join").as_str() {
        "miter" => LineJoin::Miter,
        "bevel" => LineJoin::Bevel,
        _ => LineJoin::Round,
    };
    let rep_step = Transform::from_translate(rep_off.0 as f32, rep_off.1 as f32).pre_rotate(rep_rot).pre_scale(rep_sc.0 as f32, rep_sc.1 as f32);
    let mut acc = Transform::identity();
    for k in 0..=copies {
        let o = if copies == 0 { opacity } else { opacity * (1.0 + (rep_op - 1.0) * k as f32 / copies as f32) };
        let tm = m.pre_concat(acc).pre_concat(local);
        if !is_none(&fill) {
            let paint = paint_for(fill, fill2, &gt, ga, bounds, o);
            pm.fill_path(&geom, &paint, FillRule::Winding, tm, None);
        }
        if stroke_visible {
            let mut paint = Paint { anti_alias: true, ..Default::default() };
            paint.set_color(stroke_c.with_alpha(stroke_c.a * o).to_skia());
            let stroke = Stroke { width: sw, line_cap: cap, line_join: join, dash: dash.clone(), ..Default::default() };
            pm.stroke_path(&geom, &paint, &stroke, tm, None);
        }
        acc = acc.pre_concat(rep_step);
    }
}

/// Geometry bounds of a shape (layer space, after its local transform), plus stroke margin.
pub fn shape_bounds(s: &Shape, ev: &Ev) -> Option<(Rect, f32)> {
    let geom = geometry(s, ev)?;
    let local = local_transform(s, ev);
    let b = geom.clone().transform(local)?.bounds();
    let sw = if ev.color(&s.props, "stroke").a > 0.0 { ev.f(&s.props, "strokeW") } else { 0.0 };
    let copies = ev.num(&s.props, "repeat").round().max(0.0) as usize;
    if copies == 0 {
        return Some((b, sw));
    }
    // include repeater copies
    let rep_off = ev.vec(&s.props, "repOffset");
    let rep_rot = ev.f(&s.props, "repRot");
    let rep_sc = ev.vec(&s.props, "repScale");
    let step = Transform::from_translate(rep_off.0 as f32, rep_off.1 as f32).pre_rotate(rep_rot).pre_scale(rep_sc.0 as f32, rep_sc.1 as f32);
    let mut acc = Transform::identity();
    let (mut l, mut t, mut r, mut bt) = (b.left(), b.top(), b.right(), b.bottom());
    for _ in 0..copies {
        acc = acc.pre_concat(step);
        if let Some(p) = geom.clone().transform(acc.pre_concat(local)) {
            let bb = p.bounds();
            l = l.min(bb.left());
            t = t.min(bb.top());
            r = r.max(bb.right());
            bt = bt.max(bb.bottom());
        }
    }
    Some((Rect::from_ltrb(l, t, r, bt)?, sw))
}

// ---------------------------------------------------------------- path vertices (roto)

/// An absolute path segment in normalized mask coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    M(f64, f64),
    L(f64, f64),
    C(f64, f64, f64, f64, f64, f64),
    Q(f64, f64, f64, f64),
    Z,
}

impl Seg {
    fn end(&self) -> Option<(f64, f64)> {
        match *self {
            Seg::M(x, y) | Seg::L(x, y) | Seg::C(_, _, _, _, x, y) | Seg::Q(_, _, x, y) => Some((x, y)),
            Seg::Z => None,
        }
    }
}

/// Parses an SVG path into absolute segments (arcs and shorthand become curves).
pub fn path_segs(d: &str) -> Option<Vec<Seg>> {
    let mut out = vec![];
    for seg in svgtypes::SimplifyingPathParser::from(d) {
        out.push(match seg.ok()? {
            svgtypes::SimplePathSegment::MoveTo { x, y } => Seg::M(x, y),
            svgtypes::SimplePathSegment::LineTo { x, y } => Seg::L(x, y),
            svgtypes::SimplePathSegment::CurveTo { x1, y1, x2, y2, x, y } => Seg::C(x1, y1, x2, y2, x, y),
            svgtypes::SimplePathSegment::Quadratic { x1, y1, x, y } => Seg::Q(x1, y1, x, y),
            svgtypes::SimplePathSegment::ClosePath => Seg::Z,
        });
    }
    Some(out)
}

fn n4(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.to_string() }
}

/// Writes segments back as a compact absolute path string.
pub fn segs_to_d(segs: &[Seg]) -> String {
    let p = |x: f64, y: f64| format!("{},{}", n4(x), n4(y));
    segs.iter()
        .map(|s| match *s {
            Seg::M(x, y) => format!("M{}", p(x, y)),
            Seg::L(x, y) => format!("L{}", p(x, y)),
            Seg::C(a, b, c, d, x, y) => format!("C{} {} {}", p(a, b), p(c, d), p(x, y)),
            Seg::Q(a, b, x, y) => format!("Q{} {}", p(a, b), p(x, y)),
            Seg::Z => "Z".into(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Maps every point of a path (vertices and tangent handles) through `f`.
/// Exact for affine maps; under a perspective map the handles are carried
/// along too, as After Effects does when it tracks a mask in perspective.
pub fn map_points(d: &str, f: impl Fn(f64, f64) -> (f64, f64)) -> Option<String> {
    let segs = path_segs(d)?;
    let out: Vec<Seg> = segs
        .iter()
        .map(|s| match *s {
            Seg::M(x, y) => {
                let p = f(x, y);
                Seg::M(p.0, p.1)
            }
            Seg::L(x, y) => {
                let p = f(x, y);
                Seg::L(p.0, p.1)
            }
            Seg::C(a, b, c, e, x, y) => {
                let (p1, p2, p) = (f(a, b), f(c, e), f(x, y));
                Seg::C(p1.0, p1.1, p2.0, p2.1, p.0, p.1)
            }
            Seg::Q(a, b, x, y) => {
                let (p1, p) = (f(a, b), f(x, y));
                Seg::Q(p1.0, p1.1, p.0, p.1)
            }
            Seg::Z => Seg::Z,
        })
        .collect();
    Some(segs_to_d(&out))
}

/// A drawn path as `d` in fractions of a `lw` x `lh` px layer.
pub fn path_to_d(p: &Path, lw: f64, lh: f64) -> String {
    let (sx, sy) = (1.0 / lw.max(1e-9), 1.0 / lh.max(1e-9));
    let q = |pt: Point| (pt.x as f64 * sx, pt.y as f64 * sy);
    let segs: Vec<Seg> = p
        .segments()
        .map(|s| match s {
            PathSegment::MoveTo(a) => {
                let a = q(a);
                Seg::M(a.0, a.1)
            }
            PathSegment::LineTo(a) => {
                let a = q(a);
                Seg::L(a.0, a.1)
            }
            PathSegment::QuadTo(c, a) => {
                let (c, a) = (q(c), q(a));
                Seg::Q(c.0, c.1, a.0, a.1)
            }
            PathSegment::CubicTo(c1, c2, a) => {
                let (c1, c2, a) = (q(c1), q(c2), q(a));
                Seg::C(c1.0, c1.1, c2.0, c2.1, a.0, a.1)
            }
            PathSegment::Close => Seg::Z,
        })
        .collect();
    segs_to_d(&segs)
}

/// A shape mask (rect with corner radius, ellipse, polygon or star) as a
/// path in layer fractions, drawn as the mask renderer draws it: centered on
/// (x, y), w x h of the layer plus `expand` px each side, turned `rot`
/// degrees, on a `lw` x `lh` px layer.
#[allow(clippy::too_many_arguments)]
pub fn mask_shape_d(kind: &str, x: f64, y: f64, w: f64, h: f64, rot: f64, radius: f64, points: usize, expand: f64, lw: f64, lh: f64) -> Option<String> {
    let (cx, cy) = ((x * lw) as f32, (y * lh) as f32);
    let pw = ((w * lw + expand * 2.0) as f32).max(0.01);
    let ph = ((h * lh + expand * 2.0) as f32).max(0.01);
    let path = match kind {
        "rect" => rounded_rect_path(cx, cy, pw, ph, radius as f32),
        "ellipse" => ellipse_path(cx, cy, pw, ph),
        "polygon" => polygon_path(cx, cy, points, pw.min(ph) / 2.0),
        "star" => star_path(cx, cy, points, pw.min(ph) / 2.0, pw.min(ph) / 4.0),
        _ => None,
    }?;
    let path = if rot.abs() > 1e-9 { path.transform(Transform::from_rotate_at(rot as f32, cx, cy))? } else { path };
    Some(path_to_d(&path, lw, lh))
}

/// Makes vertex `n` (1-based) the first of its closed subpath (After
/// Effects' Set First Vertex), which decides how shapes with the same vertex
/// count morph into each other.
pub fn first_vertex(d: &str, n: usize) -> Option<String> {
    let segs = path_segs(d)?;
    let verts = vertex_segs(&segs);
    let idx = *verts.get(n.checked_sub(1)?)?;
    let (start, end) = subpath_of(&segs, idx);
    if idx == start {
        return Some(d.to_string());
    }
    let closed = segs[start..end].iter().any(|s| matches!(s, Seg::Z));
    if !closed {
        return None;
    }
    // the subpath's drawing segments, the implicit closing line made explicit
    let first = segs[start].end()?;
    let mut draw: Vec<Seg> = segs[start + 1..end].iter().copied().filter(|s| !matches!(s, Seg::Z)).collect();
    if draw.last().and_then(|s| s.end()).is_none_or(|e| (e.0 - first.0).abs() > 1e-9 || (e.1 - first.1).abs() > 1e-9) {
        draw.push(Seg::L(first.0, first.1));
    }
    // rotate so the drawing starts at the new first vertex
    let k = idx - start; // draw[k - 1] ends at the new start
    let new_start = segs[idx].end()?;
    let mut out = vec![Seg::M(new_start.0, new_start.1)];
    out.extend(draw[k..].iter().copied());
    out.extend(draw[..k].iter().copied());
    out.push(Seg::Z);
    let mut all: Vec<Seg> = segs[..start].to_vec();
    all.extend(out);
    all.extend(segs[end..].iter().copied());
    Some(segs_to_d(&all))
}

/// Turns vertex `n` (1-based) into a smooth point (handles along the line
/// through its neighbors, a third of the way to each) or a corner (handles
/// on the vertex), as After Effects' Convert Vertex tool.
pub fn convert_vertex(d: &str, n: usize, smooth: bool) -> Option<String> {
    let segs = path_segs(d)?;
    let verts = vertex_segs(&segs);
    let i = n.checked_sub(1)?;
    let idx = *verts.get(i)?;
    let v = segs[idx].end()?;
    let (start, end) = subpath_of(&segs, idx);
    let closed = segs[start..end].iter().any(|s| matches!(s, Seg::Z));
    let sub: Vec<usize> = verts.iter().copied().filter(|&j| j >= start && j < end).collect();
    let pos = sub.iter().position(|&j| j == idx)?;
    let pt = |k: usize| segs[sub[k]].end();
    let prev = if pos > 0 { pt(pos - 1) } else if closed { pt(sub.len() - 1) } else { None };
    let next = if pos + 1 < sub.len() { pt(pos + 1) } else if closed { pt(0) } else { None };
    if !smooth {
        return set_handles(d, n, prev.map(|_| v), next.map(|_| v));
    }
    let (a, b) = match (prev, next) {
        (Some(p), Some(q)) => (p, q),
        (Some(p), None) => (p, (2.0 * v.0 - p.0, 2.0 * v.1 - p.1)),
        (None, Some(q)) => ((2.0 * v.0 - q.0, 2.0 * v.1 - q.1), q),
        (None, None) => return None,
    };
    let dir = (b.0 - a.0, b.1 - a.1);
    let len = dir.0.hypot(dir.1).max(1e-12);
    let u = (dir.0 / len, dir.1 / len);
    let lin = prev.map(|p| (p.0 - v.0).hypot(p.1 - v.1) / 3.0).unwrap_or(0.0);
    let lout = next.map(|q| (q.0 - v.0).hypot(q.1 - v.1) / 3.0).unwrap_or(0.0);
    let inh = prev.map(|_| (v.0 - u.0 * lin, v.1 - u.1 * lin));
    let outh = next.map(|_| (v.0 + u.0 * lout, v.1 + u.1 * lout));
    set_handles(d, n, inh, outh)
}

/// Index of each vertex's segment. A closing segment that returns to the
/// subpath's start is not a vertex of its own.
fn vertex_segs(segs: &[Seg]) -> Vec<usize> {
    let mut out = vec![];
    let mut start = None;
    for (i, s) in segs.iter().enumerate() {
        let Some(e) = s.end() else { continue };
        if let Seg::M(..) = s {
            start = Some(e);
        } else {
            let closes = matches!(segs.get(i + 1), Some(Seg::Z) | None) && start.is_some_and(|st: (f64, f64)| (st.0 - e.0).abs() < 1e-9 && (st.1 - e.1).abs() < 1e-9);
            if closes {
                continue;
            }
        }
        out.push(i);
    }
    out
}

/// Vertices (segment end points, not control points) in order, from 1.
pub fn path_vertices(d: &str) -> Vec<(f64, f64)> {
    let Some(segs) = path_segs(d) else { return vec![] };
    vertex_segs(&segs).into_iter().filter_map(|i| segs[i].end()).collect()
}

/// Moves vertex `n` (1-based) to (x, y); the curve handles touching it move
/// with it, as dragging a vertex does in After Effects.
pub fn move_vertex(d: &str, n: usize, x: f64, y: f64) -> Option<String> {
    let mut segs = path_segs(d)?;
    let idx = *vertex_segs(&segs).get(n.checked_sub(1)?)?;
    let (ox, oy) = segs[idx].end()?;
    let (dx, dy) = (x - ox, y - oy);
    let shift = |s: &mut Seg, which: u8| {
        if let Seg::C(a, b, c, e, fx, fy) = *s {
            *s = if which == 1 { Seg::C(a + dx, b + dy, c, e, fx, fy) } else { Seg::C(a, b, c + dx, e + dy, fx, fy) };
        }
    };
    segs[idx] = match segs[idx] {
        Seg::M(..) => Seg::M(x, y),
        Seg::L(..) => Seg::L(x, y),
        Seg::C(a, b, c, e, _, _) => Seg::C(a, b, c + dx, e + dy, x, y),
        Seg::Q(a, b, _, _) => Seg::Q(a, b, x, y),
        Seg::Z => return None,
    };
    if let Some(next) = segs.get_mut(idx + 1) {
        shift(next, 1);
    }
    // a moved start point also moves a closing segment that returns to it
    if let Seg::M(..) = segs[idx] {
        let end = segs[idx + 1..].iter().position(|s| matches!(s, Seg::M(..))).map(|p| idx + 1 + p).unwrap_or(segs.len());
        if let Some(last) = segs[idx + 1..end].iter().rposition(|s| s.end().is_some()).map(|p| idx + 1 + p) {
            if segs[last].end().is_some_and(|e| (e.0 - ox).abs() < 1e-9 && (e.1 - oy).abs() < 1e-9) && last != idx + 1 {
                segs[last] = match segs[last] {
                    Seg::L(..) => Seg::L(x, y),
                    Seg::C(a, b, c, e, _, _) => Seg::C(a, b, c + dx, e + dy, x, y),
                    Seg::Q(a, b, _, _) => Seg::Q(a, b, x, y),
                    o => o,
                };
            }
        }
    }
    Some(segs_to_d(&segs))
}

/// Subpaths as vertex lists: (points, closed). Control points are dropped.
fn subpath_points(segs: &[Seg]) -> Vec<(Vec<(f64, f64)>, bool)> {
    let mut out = vec![];
    let mut i = 0;
    while i < segs.len() {
        let Seg::M(x, y) = segs[i] else {
            i += 1;
            continue;
        };
        let mut pts = vec![(x, y)];
        let mut closed = false;
        let mut j = i + 1;
        while j < segs.len() {
            match segs[j] {
                Seg::M(..) => break,
                Seg::Z => {
                    closed = true;
                    j += 1;
                    break;
                }
                s => {
                    if let Some(e) = s.end() {
                        pts.push(e);
                    }
                }
            }
            j += 1;
        }
        // a last point back on the start is the start (and closes the shape)
        if pts.len() > 2 {
            let (f, l) = (pts[0], *pts.last().unwrap());
            if (f.0 - l.0).abs() < 1e-9 && (f.1 - l.1).abs() < 1e-9 {
                pts.pop();
                closed = true;
            }
        }
        out.push((pts, closed));
        i = j;
    }
    out
}

/// RotoBezier: a smooth cubic path through the vertices of `d` with
/// automatic (Catmull-Rom) tangents, as After Effects' RotoBezier masks.
/// `tension` 0 is Catmull-Rom, 1 gives corners, negative values round more.
pub fn rotobezier(d: &str, tension: f64) -> Option<String> {
    let segs = path_segs(d)?;
    let k = 1.0 - tension.clamp(-1.0, 1.0);
    let mut out = vec![];
    for (pts, closed) in subpath_points(&segs) {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        let tangent = |i: usize| -> (f64, f64) {
            let (prev, next, s) = if closed {
                (pts[(i + n - 1) % n], pts[(i + 1) % n], 0.5)
            } else if i == 0 {
                (pts[0], pts[1], 1.0)
            } else if i == n - 1 {
                (pts[n - 2], pts[n - 1], 1.0)
            } else {
                (pts[i - 1], pts[i + 1], 0.5)
            };
            ((next.0 - prev.0) * s * k, (next.1 - prev.1) * s * k)
        };
        out.push(Seg::M(pts[0].0, pts[0].1));
        let count = if closed { n } else { n - 1 };
        for i in 0..count {
            let j = (i + 1) % n;
            let (ti, tj) = (tangent(i), tangent(j));
            out.push(Seg::C(pts[i].0 + ti.0 / 3.0, pts[i].1 + ti.1 / 3.0, pts[j].0 - tj.0 / 3.0, pts[j].1 - tj.1 / 3.0, pts[j].0, pts[j].1));
        }
        if closed {
            out.push(Seg::Z);
        }
    }
    (!out.is_empty()).then(|| segs_to_d(&out))
}

/// The subpath [start, end) holding segment `idx`.
fn subpath_of(segs: &[Seg], idx: usize) -> (usize, usize) {
    let start = segs[..=idx].iter().rposition(|s| matches!(s, Seg::M(..))).unwrap_or(0);
    let end = segs[start + 1..].iter().position(|s| matches!(s, Seg::M(..))).map(|p| start + 1 + p).unwrap_or(segs.len());
    (start, end)
}

/// Splits a segment that starts at `p0` at parameter `t` (de Casteljau).
fn split_seg(p0: (f64, f64), s: Seg, t: f64) -> Option<(Seg, Seg)> {
    let l = |a: (f64, f64), b: (f64, f64)| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    Some(match s {
        Seg::L(x, y) => {
            let m = l(p0, (x, y));
            (Seg::L(m.0, m.1), Seg::L(x, y))
        }
        Seg::Q(a, b, x, y) => {
            let (q0, q1) = (l(p0, (a, b)), l((a, b), (x, y)));
            let m = l(q0, q1);
            (Seg::Q(q0.0, q0.1, m.0, m.1), Seg::Q(q1.0, q1.1, x, y))
        }
        Seg::C(a, b, c, e, x, y) => {
            let (p1, p2, p3) = ((a, b), (c, e), (x, y));
            let (q0, q1, q2) = (l(p0, p1), l(p1, p2), l(p2, p3));
            let (r0, r1) = (l(q0, q1), l(q1, q2));
            let m = l(r0, r1);
            (Seg::C(q0.0, q0.1, r0.0, r0.1, m.0, m.1), Seg::C(r1.0, r1.1, q2.0, q2.1, x, y))
        }
        _ => return None,
    })
}

/// Adds a vertex after vertex `n` (1-based) by splitting the segment that
/// follows it in half, so the shape doesn't change (as clicking a segment
/// with After Effects' Add Vertex tool). On a closed shape the segment after
/// the last vertex is the one back to the start.
pub fn insert_vertex(d: &str, n: usize) -> Option<String> {
    let mut segs = path_segs(d)?;
    let idx = *vertex_segs(&segs).get(n.checked_sub(1)?)?;
    let p0 = segs[idx].end()?;
    let (start, end) = subpath_of(&segs, idx);
    let first = segs[start].end()?;
    match segs.get(idx + 1).copied() {
        Some(s) if idx + 1 < end && s.end().is_some() => {
            let (a, b) = split_seg(p0, s, 0.5)?;
            segs[idx + 1] = a;
            segs.insert(idx + 2, b);
        }
        // the implicit closing line (Z) after the last vertex
        Some(Seg::Z) => segs.insert(idx + 1, Seg::L((p0.0 + first.0) / 2.0, (p0.1 + first.1) / 2.0)),
        _ => return None,
    }
    Some(segs_to_d(&segs))
}

/// Deletes vertex `n` (1-based). Its neighbors join with the curve handles
/// they had. A shape keeps at least three vertices (an open path two).
pub fn delete_vertex(d: &str, n: usize) -> Option<String> {
    let mut segs = path_segs(d)?;
    let verts = vertex_segs(&segs);
    let idx = *verts.get(n.checked_sub(1)?)?;
    let (start, end) = subpath_of(&segs, idx);
    let in_sub = verts.iter().filter(|&&i| i >= start && i < end).count();
    let closed = segs[start..end].iter().any(|s| matches!(s, Seg::Z));
    if in_sub <= if closed { 3 } else { 2 } {
        return None;
    }
    if let Seg::M(ox, oy) = segs[idx] {
        // the next vertex becomes the start; a closing segment that returned
        // to the old start now returns to the new one
        let next = segs.get(idx + 1).copied()?;
        let (nx, ny) = next.end()?;
        segs[idx] = Seg::M(nx, ny);
        segs.remove(idx + 1);
        let end = end - 1;
        if let Some(last) = segs[idx + 1..end].iter().rposition(|s| s.end().is_some()).map(|p| idx + 1 + p) {
            if segs[last].end().is_some_and(|e| (e.0 - ox).abs() < 1e-9 && (e.1 - oy).abs() < 1e-9) {
                segs[last] = match segs[last] {
                    Seg::L(..) => Seg::L(nx, ny),
                    Seg::C(a, b, c, e, _, _) => Seg::C(a, b, c, e, nx, ny),
                    Seg::Q(a, b, _, _) => Seg::Q(a, b, nx, ny),
                    o => o,
                };
            }
        }
        return Some(segs_to_d(&segs));
    }
    let removed = segs[idx];
    match segs.get(idx + 1).copied() {
        Some(next) if idx + 1 < end && next.end().is_some() => {
            // the joined segment keeps the outgoing handle of the vertex
            // before and the incoming handle of the vertex after
            let (x, y) = next.end()?;
            let c1 = match removed {
                Seg::C(a, b, ..) => Some((a, b)),
                _ => None,
            };
            let c2 = match next {
                Seg::C(_, _, c, e, ..) => Some((c, e)),
                _ => None,
            };
            segs[idx + 1] = match (c1, c2) {
                (None, None) => Seg::L(x, y),
                (a, b) => {
                    let p0 = segs[..idx].iter().rev().find_map(|s| s.end())?;
                    let a = a.unwrap_or(p0);
                    let b = b.unwrap_or((x, y));
                    Seg::C(a.0, a.1, b.0, b.1, x, y)
                }
            };
            segs.remove(idx);
        }
        _ => {
            segs.remove(idx);
        }
    }
    Some(segs_to_d(&segs))
}

/// Sets the tangent handles of vertex `n` (1-based) in absolute 0..1
/// coordinates: `inh` shapes the curve arriving at it, `outh` the curve
/// leaving it. Straight segments become curves as needed.
pub fn set_handles(d: &str, n: usize, inh: Option<(f64, f64)>, outh: Option<(f64, f64)>) -> Option<String> {
    let mut segs = path_segs(d)?;
    let idx = *vertex_segs(&segs).get(n.checked_sub(1)?)?;
    let v = segs[idx].end()?;
    let (start, mut end) = subpath_of(&segs, idx);
    let prev_end = |segs: &[Seg], i: usize| segs[..i].iter().rev().find_map(|s| s.end());
    if let Some((hx, hy)) = inh {
        // the segment arriving at the vertex: its own, or for the start
        // point the closing segment (made explicit if it was implicit)
        let arr = if matches!(segs[idx], Seg::M(..)) {
            let closing = segs[idx + 1..end].iter().rposition(|s| s.end().is_some_and(|e| (e.0 - v.0).abs() < 1e-9 && (e.1 - v.1).abs() < 1e-9)).map(|p| idx + 1 + p);
            match closing {
                Some(c) => Some(c),
                None if segs[start..end].iter().any(|s| matches!(s, Seg::Z)) => {
                    let z = segs[start..end].iter().position(|s| matches!(s, Seg::Z)).map(|p| start + p)?;
                    segs.insert(z, Seg::L(v.0, v.1));
                    end += 1;
                    Some(z)
                }
                None => None,
            }
        } else {
            Some(idx)
        };
        if let Some(i) = arr {
            let p0 = prev_end(&segs, i)?;
            segs[i] = match segs[i] {
                Seg::C(a, b, _, _, x, y) => Seg::C(a, b, hx, hy, x, y),
                Seg::L(x, y) => Seg::C(p0.0, p0.1, hx, hy, x, y),
                Seg::Q(a, b, x, y) => Seg::C(p0.0 + (a - p0.0) * 2.0 / 3.0, p0.1 + (b - p0.1) * 2.0 / 3.0, hx, hy, x, y),
                o => o,
            };
        }
    }
    if let Some((hx, hy)) = outh {
        // the segment leaving the vertex (the implicit close counts too)
        let i = idx + 1;
        if i < end {
            match segs[i] {
                Seg::Z => {
                    let first = segs[start].end()?;
                    segs.insert(i, Seg::C(hx, hy, first.0, first.1, first.0, first.1));
                }
                Seg::C(_, _, c, e, x, y) => segs[i] = Seg::C(hx, hy, c, e, x, y),
                Seg::L(x, y) => segs[i] = Seg::C(hx, hy, x, y, x, y),
                Seg::Q(a, b, x, y) => segs[i] = Seg::C(hx, hy, x + (a - x) * 2.0 / 3.0, y + (b - y) * 2.0 / 3.0, x, y),
                Seg::M(..) => {}
            }
        }
    }
    Some(segs_to_d(&segs))
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    const SQ: &str = "M0.2,0.2 L0.8,0.2 L0.8,0.8 L0.2,0.8 Z";

    #[test]
    fn rotobezier_passes_through_vertices() {
        let d = rotobezier(SQ, 0.0).unwrap();
        let v = path_vertices(&d);
        assert_eq!(v.len(), 4);
        assert!((v[2].0 - 0.8).abs() < 1e-6 && (v[2].1 - 0.8).abs() < 1e-6);
        assert!(d.contains('C'));
        // tension 1: handles sit on the vertices (corners)
        let sharp = rotobezier(SQ, 1.0).unwrap();
        assert!(sharp.starts_with("M0.2,0.2 C0.2,0.2 0.8,0.2 0.8,0.2"));
    }

    #[test]
    fn insert_and_delete_vertices() {
        let d = insert_vertex(SQ, 1).unwrap();
        let v = path_vertices(&d);
        assert_eq!(v.len(), 5);
        assert!((v[1].0 - 0.5).abs() < 1e-6 && (v[1].1 - 0.2).abs() < 1e-6);
        // after the last vertex: the closing segment is split
        let d = insert_vertex(SQ, 4).unwrap();
        assert_eq!(path_vertices(&d).len(), 5);
        let d = delete_vertex(SQ, 2).unwrap();
        assert_eq!(path_vertices(&d), vec![(0.2, 0.2), (0.8, 0.8), (0.2, 0.8)]);
        // a triangle can't lose a vertex
        assert!(delete_vertex(&d, 1).is_none());
        // deleting the start moves the start to the next vertex
        let d = delete_vertex(SQ, 1).unwrap();
        assert_eq!(path_vertices(&d)[0], (0.8, 0.2));
    }

    #[test]
    fn handles_turn_lines_into_curves() {
        let d = set_handles(SQ, 2, Some((0.7, 0.1)), Some((0.9, 0.3))).unwrap();
        let segs = path_segs(&d).unwrap();
        assert!(matches!(segs[1], Seg::C(_, _, c, e, _, _) if (c - 0.7).abs() < 1e-9 && (e - 0.1).abs() < 1e-9));
        assert!(matches!(segs[2], Seg::C(a, b, ..) if (a - 0.9).abs() < 1e-9 && (b - 0.3).abs() < 1e-9));
        assert_eq!(path_vertices(&d).len(), 4);
    }
}

#[cfg(test)]
mod vertex_tests {
    use super::*;

    #[test]
    fn moves_one_vertex() {
        let d = "M0.1,0.1 L0.9,0.1 L0.5,0.9 Z";
        assert_eq!(path_vertices(d).len(), 3);
        let moved = move_vertex(d, 2, 0.8, 0.2).unwrap();
        assert_eq!(moved, "M0.1,0.1 L0.8,0.2 L0.5,0.9 Z");
        assert!(move_vertex(d, 4, 0.0, 0.0).is_none());
        assert!(move_vertex(d, 0, 0.0, 0.0).is_none());
    }

    #[test]
    fn curve_handles_follow() {
        let d = "M0,0 C0.1,0 0.2,0 0.3,0 C0.4,0 0.5,0 0.6,0";
        let moved = move_vertex(d, 2, 0.3, 0.1).unwrap();
        let segs = path_segs(&moved).unwrap();
        assert_eq!(segs[1], Seg::C(0.1, 0.0, 0.2, 0.1, 0.3, 0.1));
        assert_eq!(segs[2], Seg::C(0.4, 0.1, 0.5, 0.0, 0.6, 0.0));
    }
}
