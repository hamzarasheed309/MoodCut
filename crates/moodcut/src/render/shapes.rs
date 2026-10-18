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
