//! The MoodCut compositor: evaluates a comp at time t and renders it to a
//! premultiplied RGBA pixmap. Layers are rendered in parallel (rayon) into
//! bbox-sized buffers in output space, then composited bottom-to-top.

use super::decode::{FrameRef, FrameServer, ReaderSpec};
use super::effects::{self, FxCtx};
use super::eval::{hash_str, Ev};
use super::masks::{self, LBounds};
use super::pixel::{self, blur_pixmap, lerp_into, src_over, IRect, LayerBuf, Pm};
use super::shapes;
use super::text::{self, TextStyle};
use super::warp::{self, Cam};
use crate::model::anim::{Curve, Ease};
use crate::model::expr::ExprCtx;
use crate::model::project::*;
use crate::model::schema::{self, VCtx};
use crate::model::value::{Color, Val};
use anyhow::{anyhow, Result};
use rayon::prelude::*;
use resvg::tiny_skia::{BlendMode, FillRule, FilterQuality, Paint, Pixmap, PixmapPaint, Point, Rect, Stroke, Transform};
use std::collections::HashSet;

pub struct RenderCtx<'a> {
    pub p: &'a Project,
    pub frames: &'a FrameServer,
    /// Faster, lower-quality sampling (previews).
    pub draft: bool,
}

/// Output size for a comp at a preview scale (even dimensions for encoders).
pub fn out_size(comp: &Comp, scale: f32) -> (u32, u32) {
    let w = ((comp.w as f32 * scale).round() as u32 / 2 * 2).max(2);
    let h = ((comp.h as f32 * scale).round() as u32 / 2 * 2).max(2);
    (w, h)
}

/// Renders a comp frame at time `t` (seconds) and scale.
pub fn render_frame(rc: &RenderCtx, comp_id: &str, t: f64, scale: f32) -> Result<Pixmap> {
    let comp = rc.p.comp(comp_id).ok_or_else(|| anyhow!("no comp {comp_id}"))?;
    let (w, h) = out_size(comp, scale);
    let mut out = Pixmap::new(w, h).ok_or_else(|| anyhow!("bad output size"))?;
    let cr = CompR { rc, comp, sx: w as f32 / comp.w as f32, sy: h as f32 / comp.h as f32, ow: w as i32, oh: h as i32, path: String::new(), depth: 0 };
    cr.render(t, &mut out)?;
    Ok(out)
}

/// A layer's on-screen geometry at comp time `t`, in comp px: the corners of
/// its content box (after parenting and 3D projection) and its anchor point.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct LayerGeom {
    pub quad: [(f64, f64); 4],
    pub anchor: (f64, f64),
}

impl LayerGeom {
    /// Axis-aligned box (left, top, right, bottom).
    pub fn bbox(&self) -> (f64, f64, f64, f64) {
        let xs = self.quad.iter().map(|p| p.0);
        let ys = self.quad.iter().map(|p| p.1);
        (xs.clone().fold(f64::INFINITY, f64::min), ys.clone().fold(f64::INFINITY, f64::min), xs.fold(f64::NEG_INFINITY, f64::max), ys.fold(f64::NEG_INFINITY, f64::max))
    }
}

/// Geometry of `it` at comp time `t` without rendering it (None for layers with no picture).
pub fn layer_geom(p: &Project, comp: &Comp, it: &Item, t: f64) -> Option<LayerGeom> {
    let g = Geo { p, comp };
    let tl = t - it.start;
    let lists = Geo::lists(it.kind);
    let x0 = g.ectx(it, None);
    let ev0 = Ev::new(tl, &x0, g.vc(), &lists);
    let b = g.bounds(it, &ev0, &x0, tl)?;
    let x = g.ectx(it, Some(b));
    let ev = Ev::new(tl, &x, g.vc(), &lists);
    let w = g.world(it, t, &ev, b);
    let corners = [(b.x, b.y), (b.x + b.w, b.y), (b.x + b.w, b.y + b.h), (b.x, b.y + b.h)];
    let quad = g.screen_corners(it, t, &w, corners).unwrap_or_else(|| {
        corners.map(|(cx, cy)| {
            let mut pt = Point::from_xy(cx, cy);
            w.m.map_point(&mut pt);
            (pt.x as f64, pt.y as f64)
        })
    });
    let mut a = Point::from_xy(w.pos.0 as f32, w.pos.1 as f32);
    w.parent.map_point(&mut a);
    Some(LayerGeom { quad, anchor: (a.x as f64, a.y as f64) })
}

#[derive(Clone, Copy)]
struct Extra {
    /// Applied in comp px after the layer's world transform.
    mat: Transform,
    opacity: f32,
    rx: f64,
    ry: f64,
    /// 3D offset added before projection (cube transitions).
    off: (f64, f64, f64),
}

impl Extra {
    fn none() -> Extra {
        Extra { mat: Transform::identity(), opacity: 1.0, rx: 0.0, ry: 0.0, off: (0.0, 0.0, 0.0) }
    }
}

struct World {
    /// layer px -> comp px (2D, includes parent and anchor/content scale)
    m: Transform,
    opacity: f32,
    rx: f64,
    ry: f64,
    z: f64,
    pos: (f64, f64),
    rot: f64,
    /// user scale * content (fit) scale
    sc: (f64, f64),
    anchor: (f64, f64),
    parent: Transform,
}

enum Unit<'a> {
    Single(&'a Item, Option<(&'a Transition, bool, f64)>),
    Pair(&'a Item, &'a Item, &'a Transition, f64),
    Adjust(&'a Item, f64),
}

struct CompR<'a> {
    rc: &'a RenderCtx<'a>,
    comp: &'a Comp,
    sx: f32,
    sy: f32,
    ow: i32,
    oh: i32,
    path: String,
    depth: u32,
}

fn blend_of(s: &str) -> BlendMode {
    match s {
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "overlay" => BlendMode::Overlay,
        "darken" => BlendMode::Darken,
        "lighten" => BlendMode::Lighten,
        "colordodge" => BlendMode::ColorDodge,
        "colorburn" => BlendMode::ColorBurn,
        "hardlight" => BlendMode::HardLight,
        "softlight" => BlendMode::SoftLight,
        "difference" => BlendMode::Difference,
        "exclusion" => BlendMode::Exclusion,
        "hue" => BlendMode::Hue,
        "saturation" => BlendMode::Saturation,
        "color" => BlendMode::Color,
        "luminosity" => BlendMode::Luminosity,
        "add" | "lineardodge" => BlendMode::Plus,
        _ => BlendMode::SourceOver,
    }
}

pub fn composite(out: &mut Pixmap, lb: &LayerBuf, opacity: f32, blend: &str) {
    if opacity <= 0.001 {
        return;
    }
    if pixel::blend_custom(out, lb.pm.get(), lb.x, lb.y, opacity, blend) {
        return;
    }
    match blend_of(blend) {
        BlendMode::SourceOver => src_over(out, lb.pm.get(), lb.x, lb.y, opacity),
        bm => {
            out.draw_pixmap(lb.x, lb.y, lb.pm.get().as_ref(), &PixmapPaint { opacity: opacity.clamp(0.0, 1.0), blend_mode: bm, quality: FilterQuality::Nearest }, Transform::identity(), None);
        }
    }
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn bake_opacity(lb: &mut LayerBuf, op: f32) {
    if op >= 0.999 {
        return;
    }
    let m = (op.clamp(0.0, 1.0) * 255.0) as u8;
    let n = (lb.w() * lb.h()) as usize;
    pixel::apply_alpha_mask(lb.pm.make_mut(), &vec![m; n]);
}

/// Layer geometry that needs no media decoding: content bounds, fit scale and
/// world transforms. Shared by the compositor, align/distribute and the
/// editor's transform overlay, so they measure layers the same way.
#[derive(Clone, Copy)]
pub struct Geo<'a> {
    pub p: &'a Project,
    pub comp: &'a Comp,
}

impl<'a> Geo<'a> {
    pub fn vc(&self) -> VCtx {
        VCtx { cw: self.comp.w as f64, ch: self.comp.h as f64 }
    }

    pub fn ectx(&self, it: &Item, b: Option<LBounds>) -> ExprCtx {
        ExprCtx {
            dur: it.dur,
            fps: self.comp.fps,
            index: 0.0,
            w: b.map(|b| b.w as f64).unwrap_or(0.0),
            h: b.map(|b| b.h as f64).unwrap_or(0.0),
            cw: self.comp.w as f64,
            ch: self.comp.h as f64,
            seed: hash_str(&it.id),
        }
    }

    pub fn lists(kind: ItemKind) -> [&'static [schema::P]; 2] {
        [schema::item_props(kind), schema::LAYER]
    }

    pub fn media(&self, it: &Item) -> Option<&'a Media> {
        if it.kind != ItemKind::Clip {
            return None;
        }
        self.p.media(it.src.as_deref()?)
    }

    pub fn text_style(&self, it: &Item, ev: &Ev) -> TextStyle {
        let p = &it.props;
        TextStyle {
            family: ev.str(p, "font"),
            weight: ev.num(p, "weight"),
            italic: ev.bool(p, "italic"),
            size: ev.f(p, "size").max(1.0),
            align: match ev.str(p, "align").as_str() {
                "left" => 0,
                "right" => 2,
                _ => 1,
            },
            line_h: ev.f(p, "lineH"),
            tracking: ev.f(p, "tracking"),
            box_w: ev.f(p, "boxW"),
            caps: ev.bool(p, "caps"),
        }
    }

    pub fn text_content(&self, it: &Item, tl: f64, x: &ExprCtx) -> String {
        text::expand_placeholders(it.text.as_deref().unwrap_or(""), tl, x)
    }

    /// Content bounds in layer px.
    pub fn bounds(&self, it: &Item, ev: &Ev, x: &ExprCtx, tl: f64) -> Option<LBounds> {
        match it.kind {
            ItemKind::Clip => {
                let m = self.media(it)?;
                if !m.is_visual() {
                    return None;
                }
                Some(LBounds { x: 0.0, y: 0.0, w: m.w.max(1) as f32, h: m.h.max(1) as f32 })
            }
            ItemKind::Comp => {
                let c = self.p.comp(it.src.as_deref()?)?;
                Some(LBounds { x: 0.0, y: 0.0, w: c.w as f32, h: c.h as f32 })
            }
            ItemKind::Solid => Some(LBounds { x: 0.0, y: 0.0, w: ev.f(&it.props, "w").max(1.0), h: ev.f(&it.props, "h").max(1.0) }),
            ItemKind::Adjust => Some(LBounds { x: 0.0, y: 0.0, w: self.comp.w as f32, h: self.comp.h as f32 }),
            ItemKind::Text => {
                let st = self.text_style(it, ev);
                let lay = text::layout(&self.text_content(it, tl, x), &st);
                if lay.glyphs.is_empty() {
                    return None;
                }
                Some(LBounds { x: 0.0, y: 0.0, w: lay.w.max(1.0), h: lay.h.max(1.0) })
            }
            ItemKind::Shape => {
                let sev = ev.with_lists(&[schema::SHAPE_COMMON]);
                let mut r: Option<Rect> = None;
                for s in it.shapes.iter().filter(|s| s.on) {
                    let lev = sev.with_lists(&[schema::shape_props(&s.kind).unwrap_or(&[]), schema::SHAPE_COMMON]);
                    if let Some((b, _)) = shapes::shape_bounds(s, &lev) {
                        r = Some(match r {
                            None => b,
                            Some(o) => Rect::from_ltrb(o.left().min(b.left()), o.top().min(b.top()), o.right().max(b.right()), o.bottom().max(b.bottom())).unwrap_or(o),
                        });
                    }
                }
                let r = r?;
                Some(LBounds { x: r.x(), y: r.y(), w: r.width().max(1.0), h: r.height().max(1.0) })
            }
            ItemKind::Null | ItemKind::Camera => None,
        }
    }

    /// Content (fit) scale for media/comp layers.
    pub fn fit_scale(&self, it: &Item, ev: &Ev, b: &LBounds) -> (f64, f64) {
        if !matches!(it.kind, ItemKind::Clip | ItemKind::Comp) {
            return (1.0, 1.0);
        }
        let fit = ev.str(&it.props, "fit");
        let (cw, ch) = (self.comp.w as f64, self.comp.h as f64);
        let (w, h) = (b.w as f64, b.h as f64);
        match fit.as_str() {
            "fit" | "blur" => {
                let s = (cw / w).min(ch / h);
                (s, s)
            }
            "fill" => {
                let s = (cw / w).max(ch / h);
                (s, s)
            }
            "stretch" => (cw / w, ch / h),
            _ => (1.0, 1.0),
        }
    }

    pub fn find_item(&self, id: &str) -> Option<&'a Item> {
        self.comp.items().find(|i| i.id == id)
    }

    /// Parent chain transform (comp px) at time t.
    pub fn parent_m(&self, it: &Item, t: f64, depth: u32) -> Transform {
        let Some(pid) = &it.parent else { return Transform::identity() };
        if depth > 32 {
            return Transform::identity();
        }
        let Some(p) = self.find_item(pid) else { return Transform::identity() };
        let x = self.ectx(p, None);
        let lists = Self::lists(p.kind);
        let ev = Ev::new(t - p.start, &x, self.vc(), &lists);
        let pos = ev.vec(&p.props, "pos");
        let rot = ev.f(&p.props, "rot");
        let sc = ev.vec(&p.props, "scale");
        let skew = ev.f(&p.props, "skew");
        let local = Transform::from_translate(pos.0 as f32, pos.1 as f32).pre_rotate(rot).pre_concat(Transform::from_skew(skew.to_radians().tan(), 0.0)).pre_scale(sc.0 as f32, sc.1 as f32);
        self.parent_m(p, t, depth + 1).pre_concat(local)
    }

    fn world(&self, it: &Item, t: f64, ev: &Ev, b: LBounds) -> World {
        let p = &it.props;
        let pos = ev.vec(p, "pos");
        let us = ev.vec(p, "scale");
        let fs = self.fit_scale(it, ev, &b);
        let sc = (us.0 * fs.0, us.1 * fs.1);
        let rot = ev.num(p, "rot");
        let an = ev.vec(p, "anchor");
        let anchor = (b.x as f64 + an.0 * b.w as f64, b.y as f64 + an.1 * b.h as f64);
        let skew = ev.f(p, "skew");
        let parent = self.parent_m(it, t, 0);
        let m = parent
            .pre_translate(pos.0 as f32, pos.1 as f32)
            .pre_rotate(rot as f32)
            .pre_concat(Transform::from_skew(skew.to_radians().tan(), 0.0))
            .pre_scale(sc.0 as f32, sc.1 as f32)
            .pre_translate(-anchor.0 as f32, -anchor.1 as f32);
        World { m, opacity: ev.f(p, "opacity").clamp(0.0, 1.0), rx: ev.num(p, "rx"), ry: ev.num(p, "ry"), z: ev.num(p, "z"), pos, rot, sc, anchor, parent }
    }


    /// Projected corners (comp px) of a 3D layer, or None for a 2D layer.
    fn screen_corners(&self, it: &Item, t: f64, w: &World, corners: [(f32, f32); 4]) -> Option<[(f64, f64); 4]> {
        let cam = self.camera(t);
        if !self.is_3d(it, w, cam.is_some()) {
            return None;
        }
        let cam = cam.unwrap_or_else(|| Cam::default_for(self.comp.w as f64, self.comp.h as f64));
        let q = self.project_corners(w, corners, (0.0, 0.0, (0.0, 0.0, 0.0)), &cam)?;
        Some(q.map(|(x, y)| {
            let mut pt = Point::from_xy(x as f32, y as f32);
            w.parent.map_point(&mut pt);
            (pt.x as f64, pt.y as f64)
        }))
    }

    /// True when a layer is drawn in 3D: rotated in X/Y, given depth, or switched to 3D while a camera is active.
    fn is_3d(&self, it: &Item, w: &World, camera: bool) -> bool {
        w.rx.abs() > 1e-6 || w.ry.abs() > 1e-6 || w.z.abs() > 1e-6 || (camera && it.props.get("3d").is_some_and(|a| a.v.bool()))
    }

    /// The active camera at comp time t: the topmost visible camera layer.
    pub fn camera(&self, t: f64) -> Option<Cam> {
        let it = self.comp.vtracks().filter(|tr| !tr.hidden).flat_map(|tr| tr.items.iter()).filter(|i| i.kind == ItemKind::Camera && !i.off && i.contains(t)).last()?;
        let (cw, ch) = (self.comp.w as f64, self.comp.h as f64);
        let x = self.ectx(it, None);
        let lists = Self::lists(ItemKind::Camera);
        let ev = Ev::new(t - it.start, &x, self.vc(), &lists);
        let p = &it.props;
        let zoom = Some(ev.num(p, "zoom")).filter(|z| *z > 0.0).unwrap_or(cw * 0.926);
        let pos = ev.vec(p, "pos");
        // a camera parented to a null follows it
        let mut pt = Point::from_xy(pos.0 as f32, pos.1 as f32);
        self.parent_m(it, t, 0).map_point(&mut pt);
        let poi = ev.bool(p, "aim").then(|| {
            let q = ev.vec(p, "poi");
            (q.0, q.1, ev.num(p, "poiZ"))
        });
        Some(Cam::new((pt.x as f64, pt.y as f64, ev.num(p, "z") - zoom), poi, ev.num(p, "rx"), ev.num(p, "ry"), ev.num(p, "rot"), zoom, cw, ch))
    }

    /// Layer corners through the camera (comp px, before the parent's 2D transform). None if any is behind it.
    fn project_corners(&self, w: &World, corners: [(f32, f32); 4], ex: (f64, f64, (f64, f64, f64)), cam: &Cam) -> Option<[(f64, f64); 4]> {
        let mut q = [(0.0, 0.0); 4];
        for (i, (cx, cy)) in corners.iter().enumerate() {
            let v = ((*cx as f64 - w.anchor.0) * w.sc.0, (*cy as f64 - w.anchor.1) * w.sc.1, 0.0);
            let r = warp::rotate3(v, w.rx + ex.0, w.ry + ex.1, w.rot);
            q[i] = cam.project((w.pos.0 + r.0 + ex.2 .0, w.pos.1 + r.1 + ex.2 .1, w.z + r.2 + ex.2 .2))?;
        }
        Some(q)
    }

    /// Camera-space depth of a 3D layer's anchor (None for 2D layers), for sorting.
    fn depth(&self, it: &Item, t: f64, cam: &Cam, camera: bool) -> Option<f64> {
        let x = self.ectx(it, None);
        let lists = Self::lists(it.kind);
        let ev = Ev::new(t - it.start, &x, self.vc(), &lists);
        let p = &it.props;
        let z = ev.num(p, "z");
        let flat = ev.num(p, "rx").abs() < 1e-6 && ev.num(p, "ry").abs() < 1e-6 && z.abs() < 1e-6;
        if flat && !(camera && ev.bool(p, "3d")) {
            return None;
        }
        let pos = ev.vec(p, "pos");
        let mut pt = Point::from_xy(pos.0 as f32, pos.1 as f32);
        self.parent_m(it, t, 0).map_point(&mut pt);
        Some(cam.depth((pt.x as f64, pt.y as f64, z)))
    }
}

impl<'a> CompR<'a> {
    fn g(&self) -> Geo<'a> {
        Geo { p: self.rc.p, comp: self.comp }
    }

    fn out_t(&self) -> Transform {
        Transform::from_scale(self.sx, self.sy)
    }

    fn out_rect(&self) -> IRect {
        IRect { x: 0, y: 0, w: self.ow, h: self.oh }
    }

    fn vc(&self) -> VCtx {
        self.g().vc()
    }

    fn ectx(&self, it: &Item, b: Option<LBounds>) -> ExprCtx {
        self.g().ectx(it, b)
    }

    fn lists(kind: ItemKind) -> [&'static [schema::P]; 2] {
        Geo::lists(kind)
    }

    fn media(&self, it: &Item) -> Option<&'a Media> {
        self.g().media(it)
    }

    fn text_style(&self, it: &Item, ev: &Ev) -> TextStyle {
        self.g().text_style(it, ev)
    }

    fn text_content(&self, it: &Item, tl: f64, x: &ExprCtx) -> String {
        self.g().text_content(it, tl, x)
    }

    fn bounds(&self, it: &Item, ev: &Ev, x: &ExprCtx, tl: f64) -> Option<LBounds> {
        self.g().bounds(it, ev, x, tl)
    }

    fn fit_scale(&self, it: &Item, ev: &Ev, b: &LBounds) -> (f64, f64) {
        self.g().fit_scale(it, ev, b)
    }

    fn find_item(&self, id: &str) -> Option<&'a Item> {
        self.g().find_item(id)
    }

    fn world(&self, it: &Item, t: f64, ev: &Ev, b: LBounds) -> World {
        self.g().world(it, t, ev, b)
    }

    /// Max scale factor of a transform (for choosing raster resolutions).
    fn max_scale(m: &Transform) -> f32 {
        let sx = (m.sx * m.sx + m.ky * m.ky).sqrt();
        let sy = (m.kx * m.kx + m.sy * m.sy).sqrt();
        sx.max(sy)
    }

    /// Source time (seconds) for a clip at item-local time.
    fn src_time(it: &Item, tl: f64, ev: &Ev, media_fps: f64) -> f64 {
        if it.props.get("hold").map(|a| a.v.bool()).unwrap_or(false) {
            return it.src_in;
        }
        if let Some(r) = it.props.get("remap") {
            if r.is_animated() {
                let v = ev.num(&it.props, "remap");
                if v >= 0.0 {
                    return v;
                }
            }
        }
        let sp = it.speed();
        if it.reversed() {
            it.src_in + (it.dur - tl) * sp - 0.5 / media_fps.max(1.0)
        } else {
            it.src_in + tl * sp
        }
    }

    /// Raster source for media/comp layers, decoded at roughly `need` px per layer px.
    fn raster(&self, it: &Item, tl: f64, ev: &Ev, need: f32, quantize: bool) -> Result<Option<FrameRef>> {
        match it.kind {
            ItemKind::Clip => {
                let Some(m) = self.media(it) else { return Ok(None) };
                // decode at the needed size (ffmpeg's SIMD scaler also does upscales);
                // snap near-exact sizes so common cases hit the zero-copy path, else
                // quantize to limit decoder restarts while a layer zooms
                let max_px = 8192.0 / m.w.max(m.h).max(1) as f32;
                let mut ds = need.clamp(0.02, 4.0f32.min(max_px));
                if quantize {
                    ds = if ds < 1.0 { (ds * 16.0).ceil() / 16.0 } else { (ds * 8.0).ceil() / 8.0 };
                }
                if m.kind == MediaKind::Image {
                    ds = ds.min(1.0);
                }
                let dw = ((m.w as f32 * ds).round() as u32).max(2);
                let dh = ((m.h as f32 * ds).round() as u32).max(2);
                let (dw, dh) = if m.kind == MediaKind::Video { ((dw + 1) / 2 * 2, (dh + 1) / 2 * 2) } else { (dw, dh) };
                match m.kind {
                    MediaKind::Image => Ok(Some(self.rc.frames.image(&m.path, dw, dh)?)),
                    MediaKind::Svg => {
                        let k = need.clamp(0.05, 4.0);
                        let (sw, sh) = (((m.w as f32 * k).round() as u32).max(1), ((m.h as f32 * k).round() as u32).max(1));
                        Ok(Some(self.rc.frames.svg(&m.path, sw, sh)?))
                    }
                    MediaKind::Video => {
                        let sp = it.speed();
                        let remapped = it.props.get("remap").map(|a| a.is_animated()).unwrap_or(false);
                        let rate = if remapped { m.fps.max(1.0) } else { self.comp.fps / sp };
                        let spec = ReaderSpec {
                            path: m.path.clone(),
                            w: dw,
                            h: dh,
                            rate,
                            vf: effects::ff_filters(&it.fx),
                            alpha: m.alpha,
                            hw: self.rc.frames.hw,
                            src_dur: m.dur,
                            interp: ev.str(&it.props, "interp"),
                        };
                        let st = Self::src_time(it, tl, ev, m.fps);
                        if it.props.get("hold").map(|a| a.v.bool()).unwrap_or(false) {
                            return Ok(Some(self.rc.frames.video_still(&spec, st)?));
                        }
                        let owner = format!("{}{}", self.path, it.id);
                        Ok(Some(self.rc.frames.video(&owner, &spec, st)?))
                    }
                    MediaKind::Audio => Ok(None),
                }
            }
            ItemKind::Comp => {
                let Some(c) = it.src.as_deref().and_then(|s| self.rc.p.comp(s)) else { return Ok(None) };
                if self.depth > 16 {
                    return Ok(None);
                }
                let mut ct = it.src_in + tl * it.speed();
                if let Some(r) = it.props.get("remap") {
                    if r.is_animated() {
                        ct = ev.num(&it.props, "remap");
                    }
                }
                let k = need.clamp(0.05, 2.0).min(8192.0 / c.w.max(c.h) as f32);
                let (w, h) = (((c.w as f32 * k).round() as u32).max(2), ((c.h as f32 * k).round() as u32).max(2));
                let mut pm = Pixmap::new(w, h).ok_or_else(|| anyhow!("bad nested size"))?;
                let child = CompR { rc: self.rc, comp: c, sx: w as f32 / c.w as f32, sy: h as f32 / c.h as f32, ow: w as i32, oh: h as i32, path: format!("{}{}/", self.path, it.id), depth: self.depth + 1 };
                child.render(ct, &mut pm)?;
                Ok(Some(std::sync::Arc::new(pm)))
            }
            _ => Ok(None),
        }
    }

    /// True when the layer's scale can change over time (decode size must then be quantized).
    fn zooming(&self, it: &Item) -> bool {
        let mut cur = Some(it);
        let mut depth = 0;
        while let Some(i) = cur {
            if ["scale", "rx", "ry", "z"].iter().any(|k| i.props.get(*k).map(|a| a.is_animated()).unwrap_or(false)) {
                return true;
            }
            cur = i.parent.as_deref().and_then(|p| self.find_item(p));
            depth += 1;
            if depth > 32 {
                break;
            }
        }
        false
    }

    fn is_raster(it: &Item) -> bool {
        matches!(it.kind, ItemKind::Clip | ItemKind::Comp)
    }

    /// Draws vector content (text/shape/solid) with transform `m` (layer px -> buffer px).
    fn draw_vector(&self, it: &Item, ev: &Ev, x: &ExprCtx, tl: f64, pm: &mut Pixmap, m: Transform, scale: f32) {
        let p = &it.props;
        match it.kind {
            ItemKind::Solid => {
                let (w, h) = (ev.f(p, "w"), ev.f(p, "h"));
                if let Some(path) = shapes::rounded_rect_path(w / 2.0, h / 2.0, w, h, ev.f(p, "radius")) {
                    let b = Rect::from_xywh(0.0, 0.0, w.max(1.0), h.max(1.0)).unwrap();
                    let paint = shapes::paint_for(ev.color(p, "color"), ev.color(p, "color2"), &ev.str(p, "gradType"), ev.f(p, "gradAngle"), b, 1.0);
                    pm.fill_path(&path, &paint, FillRule::Winding, m, None);
                }
            }
            ItemKind::Shape => {
                for s in it.shapes.iter().filter(|s| s.on) {
                    let lev = ev.with_lists(&[schema::shape_props(&s.kind).unwrap_or(&[]), schema::SHAPE_COMMON]);
                    shapes::draw_shape(pm, s, &lev, m, 1.0);
                }
            }
            ItemKind::Text => self.draw_text(it, ev, x, tl, pm, m, scale),
            _ => {}
        }
    }

    fn draw_text(&self, it: &Item, ev: &Ev, x: &ExprCtx, tl: f64, pm: &mut Pixmap, m: Transform, scale: f32) {
        let p = &it.props;
        let st = self.text_style(it, ev);
        let content = self.text_content(it, tl, x);
        let lay = text::layout(&content, &st);
        if lay.glyphs.is_empty() {
            return;
        }
        let color = ev.color(p, "color");
        let color2 = ev.color(p, "color2");
        let stroke_c = ev.color(p, "stroke");
        let sw = ev.f(p, "strokeW");
        let bg = ev.color(p, "bg");
        let bounds = Rect::from_xywh(0.0, 0.0, lay.w.max(1.0), lay.h.max(1.0)).unwrap();
        if bg.a > 0.0 {
            let mut paint = Paint { anti_alias: true, ..Default::default() };
            paint.set_color(bg.to_skia());
            let rad = ev.f(p, "bgRadius");
            for r in text::bg_rects(&lay, ev.f(p, "bgPad"), ev.str(p, "bgMode") == "block") {
                if let Some(path) = text::rounded_rect(r, rad) {
                    pm.fill_path(&path, &paint, FillRule::Winding, m, None);
                }
            }
        }
        let states = text::glyph_states(&lay, it, tl, x.seed);
        let reveal = ev.f(p, "reveal").clamp(0.0, 1.0);
        let n_vis = if reveal >= 0.9999 { usize::MAX } else { (reveal * lay.n_chars as f32 + 1e-4).floor() as usize };
        let animated = states.iter().any(|s| !s.is_identity()) || n_vis != usize::MAX;
        let stroke = Stroke { width: sw * 2.0, line_join: resvg::tiny_skia::LineJoin::Round, line_cap: resvg::tiny_skia::LineCap::Round, ..Default::default() };
        let has_stroke = stroke_c.a > 0.0 && sw > 0.0;
        if !animated {
            if let Some(path) = &lay.combined {
                if has_stroke {
                    let mut sp = Paint { anti_alias: true, ..Default::default() };
                    sp.set_color(stroke_c.to_skia());
                    pm.stroke_path(path, &sp, &stroke, m, None);
                }
                if color.a > 0.0 {
                    let paint = shapes::paint_for(color, color2, "linear", 90.0, bounds, 1.0);
                    pm.fill_path(path, &paint, FillRule::Winding, m, None);
                }
            }
        } else {
            let (bx, by) = (0, 0);
            let _ = (bx, by);
            for (gi, g) in lay.glyphs.iter().enumerate() {
                let Some(path) = &g.path else { continue };
                if g.ch != usize::MAX && g.ch >= n_vis {
                    continue;
                }
                let s = &states[gi];
                if s.opacity <= 0.002 {
                    continue;
                }
                let gm = m.pre_concat(text::glyph_transform(&lay, gi, s, st.align));
                let base = match s.color {
                    Some((c, a)) => color.lerp(&c, a),
                    None => color,
                };
                let draw = |target: &mut Pixmap, tm: Transform| {
                    if has_stroke {
                        let mut sp = Paint { anti_alias: true, ..Default::default() };
                        sp.set_color(stroke_c.with_alpha(stroke_c.a * s.opacity).to_skia());
                        target.stroke_path(path, &sp, &stroke, tm, None);
                    }
                    if base.a > 0.0 {
                        let c2 = if s.color.is_some() { Color::TRANSPARENT } else { color2 };
                        let paint = shapes::paint_for(base, c2, "linear", 90.0, bounds, s.opacity);
                        target.fill_path(path, &paint, FillRule::Winding, tm, None);
                    }
                };
                let blur = s.blur * scale;
                if blur > 0.5 {
                    // blur this glyph in its own small buffer
                    let pb = path.bounds();
                    let pad = sw * 2.0 + blur * 2.0;
                    let mut pts = [Point::from_xy(pb.left() - pad, pb.top() - pad), Point::from_xy(pb.right() + pad, pb.top() - pad), Point::from_xy(pb.right() + pad, pb.bottom() + pad), Point::from_xy(pb.left() - pad, pb.bottom() + pad)];
                    gm.map_points(&mut pts);
                    let r = IRect::bound(&pts.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()).pad((blur * 2.0) as i32).intersect(&IRect { x: 0, y: 0, w: pm.width() as i32, h: pm.height() as i32 });
                    if r.is_empty() {
                        continue;
                    }
                    let mut tmp = pixel::new_pixmap(r.w as u32, r.h as u32);
                    draw(&mut tmp, Transform::from_translate(-r.x as f32, -r.y as f32).pre_concat(gm));
                    blur_pixmap(&mut tmp, blur / 2.0, 0);
                    src_over(pm, &tmp, r.x, r.y, 1.0);
                } else {
                    draw(pm, gm);
                }
            }
        }
        let sh = ev.color(p, "shadow");
        if sh.a > 0.0 {
            let (dx, dy) = (ev.f(p, "shadowX") * scale, ev.f(p, "shadowY") * scale);
            effects::drop_shadow(pm, sh, dx, dy, ev.f(p, "shadowBlur") * scale / 2.0);
        }
    }

    /// Extra layer-space margin (px) for vector content (strokes, shadows, animators).
    fn vector_margin(&self, it: &Item, ev: &Ev) -> f32 {
        let p = &it.props;
        match it.kind {
            ItemKind::Text => {
                let mut m = ev.f(p, "strokeW") * 2.0 + 4.0;
                if ev.color(p, "bg").a > 0.0 {
                    m += ev.f(p, "bgPad") * 1.2;
                }
                if ev.color(p, "shadow").a > 0.0 {
                    m += ev.f(p, "shadowX").abs().max(ev.f(p, "shadowY").abs()) + ev.f(p, "shadowBlur") * 2.0;
                }
                let size = ev.f(p, "size");
                for a in it.anims.iter().filter(|a| a.on) {
                    let g = |k: &str| a.props.get(k).map(|v| v.v.num() as f32).unwrap_or(0.0);
                    let sc = a.props.get("scale").map(|v| v.v.num() as f32).unwrap_or(1.0);
                    m += g("x").abs() + g("y").abs() + g("blur") * 2.0 + size * (sc - 1.0).abs() * 1.5 + g("tracking").abs() * 20.0;
                    if a.props.get("rot").is_some() {
                        m += size;
                    }
                }
                m
            }
            ItemKind::Shape => {
                let mut m: f32 = 2.0;
                for s in &it.shapes {
                    m = m.max(s.props.get("strokeW").map(|v| v.v.num() as f32).unwrap_or(0.0) + 2.0);
                }
                m
            }
            _ => 1.0,
        }
    }

    fn fx_margin(&self, it: &Item, ev: &Ev, scale: f32) -> i32 {
        it.fx.iter().map(|e| effects::margin(e, ev, scale)).sum::<f32>().ceil() as i32
    }

    fn has_3d(&self, it: &Item, w: &World, ex: &Extra, camera: bool) -> bool {
        self.g().is_3d(it, w, camera) || ex.rx.abs() > 1e-6 || ex.ry.abs() > 1e-6 || ex.off != (0.0, 0.0, 0.0)
    }

    fn cornerpin<'b>(it: &'b Item) -> Option<&'b Effect> {
        it.fx.iter().find(|e| e.on && e.kind == "cornerpin")
    }

    /// Renders an item into an output-space layer buffer (before opacity/blend).
    /// Time effects run here: posterize time quantizes the layer's clock and
    /// echo composites it with copies of its earlier frames.
    fn render_item(&self, it: &Item, t: f64, ex: &Extra) -> Result<Option<(LayerBuf, f32, String)>> {
        let t = match it.fx.iter().find(|e| e.on && e.kind == "posterizetime") {
            Some(e) => {
                let fps = e.props.get("fps").map(|a| a.v.num()).unwrap_or(12.0).clamp(0.1, 240.0);
                it.start + (((t - it.start) * fps) + 1e-6).floor() / fps
            }
            None => t,
        };
        if let Some(e) = it.fx.iter().find(|e| e.on && e.kind == "echo") {
            return self.render_echo(it, t, ex, e);
        }
        self.render_item_motion(it, t, ex)
    }

    fn render_echo(&self, it: &Item, t: f64, ex: &Extra, e: &Effect) -> Result<Option<(LayerBuf, f32, String)>> {
        let g = |k: &str, d: f64| e.props.get(k).map(|a| a.v.num()).unwrap_or(d);
        let (dt, n, decay) = (g("time", 0.05), g("count", 4.0).round().clamp(1.0, 16.0) as i32, g("decay", 0.6).clamp(0.0, 1.0) as f32);
        let mode = e.props.get("mode").map(|a| a.v.str()).unwrap_or_else(|| "behind".into());
        let cur = self.render_item_motion(it, t, ex)?;
        let blend = cur.as_ref().map(|c| c.2.clone()).unwrap_or_else(|| it.props.get("blend").map(|a| a.v.str()).unwrap_or_else(|| "normal".into()));
        let mut echoes = vec![];
        for k in 1..=n {
            let tk = t - dt * k as f64;
            if tk < it.start - 1e-9 || tk >= it.end() {
                continue;
            }
            if let Some((lb, op, _)) = self.render_item_motion(it, tk, ex)? {
                echoes.push((lb, op * decay.powi(k)));
            }
        }
        if echoes.is_empty() {
            return Ok(cur);
        }
        let mut all: Vec<(LayerBuf, f32)> = echoes;
        let echo_mode = match mode.as_str() {
            "add" => "add",
            "screen" => "screen",
            _ => "normal",
        };
        if mode == "front" {
            if let Some((lb, op, _)) = cur {
                all.insert(0, (lb, op));
            }
        } else {
            all.reverse();
            if let Some((lb, op, _)) = cur {
                all.push((lb, op));
            }
        }
        let r = all.iter().fold(IRect { x: 0, y: 0, w: 0, h: 0 }, |acc, (b, _)| acc.union(&b.rect()));
        let mut out = LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32).ok_or_else(|| anyhow!("alloc"))?;
        for (i, (b, op)) in all.iter().enumerate() {
            let shifted = LayerBuf { pm: Pm::Shared(std::sync::Arc::new(b.pm.get().clone())), x: b.x - r.x, y: b.y - r.y };
            composite(out.pm.make_mut(), &shifted, *op, if i == 0 { "normal" } else { echo_mode });
        }
        Ok(Some((out, 1.0, blend)))
    }

    fn render_item_motion(&self, it: &Item, t: f64, ex: &Extra) -> Result<Option<(LayerBuf, f32, String)>> {
        let mblur = it.props.get("mblur").map(|a| a.v.bool()).unwrap_or(false);
        if mblur && ["pos", "scale", "rot", "rx", "ry"].iter().any(|k| it.props.get(*k).map(|a| a.is_animated()).unwrap_or(false)) {
            return self.render_mblur(it, t, ex);
        }
        self.render_item_at(it, t, ex)
    }

    fn render_mblur(&self, it: &Item, t: f64, ex: &Extra) -> Result<Option<(LayerBuf, f32, String)>> {
        let n = if self.rc.draft { 4 } else { 8 };
        let span = self.comp.shutter / 360.0 / self.comp.fps;
        let mut bufs = vec![];
        let mut meta = None;
        for k in 0..n {
            let dt = (k as f64 / (n - 1) as f64 - 0.5) * span;
            if let Some((lb, op, bl)) = self.render_item_at(it, t + dt, ex)? {
                meta = Some((op, bl));
                bufs.push(lb);
            }
        }
        let Some((op, bl)) = meta else { return Ok(None) };
        let r = bufs.iter().fold(IRect { x: 0, y: 0, w: 0, h: 0 }, |acc, b| acc.union(&b.rect()));
        let mut acc = vec![0u32; (r.w * r.h * 4) as usize];
        for b in &bufs {
            let d = b.pm.get().data();
            let bw = b.w() as i32;
            for y in 0..b.h() as i32 {
                let oy = y + b.y - r.y;
                for x in 0..bw {
                    let ox = x + b.x - r.x;
                    let si = ((y * bw + x) * 4) as usize;
                    let di = ((oy * r.w + ox) * 4) as usize;
                    for c in 0..4 {
                        acc[di + c] += d[si + c] as u32;
                    }
                }
            }
        }
        let nb = bufs.len() as u32;
        let data: Vec<u8> = acc.iter().map(|v| (v / nb) as u8).collect();
        Ok(Some((LayerBuf { pm: Pm::Owned(pixel::pixmap_from_vec(data, r.w as u32, r.h as u32)), x: r.x, y: r.y }, op, bl)))
    }

    fn render_item_at(&self, it: &Item, t: f64, ex: &Extra) -> Result<Option<(LayerBuf, f32, String)>> {
        let tl = t - it.start;
        let lists = Self::lists(it.kind);
        let x0 = self.ectx(it, None);
        let ev0 = Ev::new(tl, &x0, self.vc(), &lists);
        let Some(b) = self.bounds(it, &ev0, &x0, tl) else { return Ok(None) };
        let x = self.ectx(it, Some(b));
        let ev = Ev::new(tl, &x, self.vc(), &lists);
        let w = self.world(it, t, &ev, b);
        let opacity = w.opacity * ex.opacity;
        if opacity <= 0.001 {
            return Ok(None);
        }
        let blend = ev.str(&it.props, "blend");
        let scale = self.sx.max(self.sy);
        let out_t = self.out_t();
        let fxm = self.fx_margin(it, &ev, scale);
        let clip = self.out_rect().pad(fxm.max(0) + 2);
        let mut pre_blurred = false;
        let cam = self.g().camera(t);
        let mut lb = if let Some(cp) = Self::cornerpin(it) {
            let pev = ev.with_lists(&[schema::effect("cornerpin").unwrap().params]);
            let q: Vec<(f64, f64)> = ["tl", "tr", "br", "bl"]
                .iter()
                .map(|k| {
                    let (px, py) = pev.vec(&cp.props, k);
                    let mut pt = Point::from_xy(px as f32, py as f32);
                    out_t.pre_concat(ex.mat).map_point(&mut pt);
                    (pt.x as f64, pt.y as f64)
                })
                .collect();
            let k = (Self::max_scale(&out_t) * 1.5).min(4.0);
            match self.flat(it, &ev, &x, tl, b, k)? {
                Some(src) => warp::warp_to_quad(&src, [q[0], q[1], q[2], q[3]], clip),
                None => None,
            }
        } else if self.has_3d(it, &w, ex, cam.is_some()) {
            let corners = [(b.x, b.y), (b.x + b.w, b.y), (b.x + b.w, b.y + b.h), (b.x, b.y + b.h)];
            let post = out_t.pre_concat(ex.mat).pre_concat(w.parent);
            let cam = cam.unwrap_or_else(|| Cam::default_for(self.comp.w as f64, self.comp.h as f64));
            // a layer crossing the camera plane is not drawn (After Effects clips it)
            let Some(q) = self.g().project_corners(&w, corners, (ex.rx, ex.ry, ex.off), &cam) else { return Ok(None) };
            let q: Vec<(f64, f64)> = q
                .iter()
                .map(|(px, py)| {
                    let mut pt = Point::from_xy(*px as f32, *py as f32);
                    post.map_point(&mut pt);
                    (pt.x as f64, pt.y as f64)
                })
                .collect();
            let k = (Self::max_scale(&post.pre_scale(w.sc.0 as f32, w.sc.1 as f32)) * 1.2).clamp(0.05, 4.0);
            match self.flat(it, &ev, &x, tl, b, k)? {
                Some(src) => warp::warp_to_quad(&src, [q[0], q[1], q[2], q[3]], clip),
                None => None,
            }
        } else {
            let m_out = out_t.pre_concat(ex.mat).pre_concat(w.m);
            self.render_2d(it, &ev, &x, tl, b, m_out, clip, scale, &mut pre_blurred)?
        };
        let Some(lbuf) = lb.as_mut() else { return Ok(None) };
        // masks & crop (mask paths live in layer px, transformed like the content)
        if !it.masks.is_empty() || it.fx.iter().any(|e| e.kind == "crop" && e.on) {
            let m_out = out_t.pre_concat(ex.mat).pre_concat(w.m);
            masks::apply_masks(lbuf, &it.masks, &it.fx, &ev, b, m_out, scale);
        }
        // native effects
        if it.fx.iter().any(|e| e.on) {
            let fcx = FxCtx { ev: &ev, scale, out_w: self.ow, out_h: self.oh, frame: (t * self.comp.fps).round() as i64, seed: x.seed };
            // a leading blur may already have been applied at source resolution
            for e in it.fx.iter().filter(|e| e.on).skip(pre_blurred as usize).filter(|e| !matches!(e.kind.as_str(), "crop" | "cornerpin" | "posterizetime" | "echo")) {
                effects::apply(e, lbuf, &fcx);
            }
        }
        // track matte
        if let Some(mt) = &it.matte {
            if let Some(src) = self.find_item(&mt.src) {
                let ms = self.render_item_at(src, t, &Extra::none())?;
                apply_matte(lbuf, ms.as_ref().map(|(b, o, _)| (b, *o)), mt.mode);
            }
        }
        Ok(lb.map(|l| (l, opacity, blend)))
    }

    /// Renders content flat in layer space at `k` px per layer px (for warps).
    fn flat(&self, it: &Item, ev: &Ev, x: &ExprCtx, tl: f64, b: LBounds, k: f32) -> Result<Option<Pixmap>> {
        let k = k.clamp(0.02, 8.0);
        if Self::is_raster(it) {
            let fs = self.fit_scale(it, ev, &b);
            let need = k * fs.0.max(fs.1) as f32;
            let Some(src) = self.raster(it, tl, ev, need, true)? else { return Ok(None) };
            return Ok(Some((*src).clone()));
        }
        let (w, h) = (((b.w * k).ceil() as u32).clamp(1, 8192), ((b.h * k).ceil() as u32).clamp(1, 8192));
        let mut pm = pixel::new_pixmap(w, h);
        let m = Transform::from_scale(w as f32 / b.w, h as f32 / b.h).pre_translate(-b.x, -b.y);
        self.draw_vector(it, ev, x, tl, &mut pm, m, k);
        Ok(Some(pm))
    }

    /// `fit=blur`: the frame fitted over a blurred, dimmed copy of itself that
    /// fills the comp (portrait footage in landscape and vice versa).
    fn blur_fill(&self, src: &Pixmap, m: Transform, clip: IRect) -> Option<LayerBuf> {
        let r = self.out_rect().intersect(&clip);
        if r.is_empty() {
            return None;
        }
        let (ow, oh) = (self.ow.max(1) as f32, self.oh.max(1) as f32);
        let (dw, dh) = (src.width() as f32, src.height() as f32);
        // background: fill-scaled into a small buffer, blurred there, dimmed
        let sw = 192u32;
        let sh = ((sw as f32 * oh / ow).round() as u32).max(1);
        let mut small = pixel::new_pixmap(sw, sh);
        let k = (sw as f32 / dw).max(sh as f32 / dh);
        pixel::draw_affine(&mut small, src, Transform::from_translate(sw as f32 / 2.0, sh as f32 / 2.0).pre_scale(k, k).pre_translate(-dw / 2.0, -dh / 2.0), false);
        pixel::blur_pixmap(&mut small, sw as f32 / 48.0, 0);
        for px in small.data_mut().chunks_exact_mut(4) {
            for c in &mut px[..3] {
                *c = (*c as u16 * 150 / 256) as u8;
            }
        }
        let mut lb = LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32)?;
        let to_lb = Transform::from_translate(-r.x as f32, -r.y as f32);
        let pm = lb.pm.make_mut();
        pixel::draw_affine(pm, &small, to_lb.pre_scale(ow / sw as f32, oh / sh as f32), false);
        let mut fg = pixel::new_pixmap(r.w as u32, r.h as u32);
        pixel::draw_affine(&mut fg, src, to_lb.pre_concat(m), Self::max_scale(&m) > 1.3 && !self.rc.draft);
        pm.draw_pixmap(0, 0, fg.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        Some(lb)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_2d(&self, it: &Item, ev: &Ev, x: &ExprCtx, tl: f64, b: LBounds, m_out: Transform, clip: IRect, scale: f32, pre_blurred: &mut bool) -> Result<Option<LayerBuf>> {
        let fxm = self.fx_margin(it, ev, scale);
        if Self::is_raster(it) {
            let reduce = effects::blur_source_scale(&it.fx, ev, scale);
            let need = Self::max_scale(&m_out) * reduce;
            let Some(mut src) = self.raster(it, tl, ev, need, self.zooming(it))? else { return Ok(None) };
            let (dw, dh) = (src.width() as f32, src.height() as f32);
            // decoded px -> output px
            let m = m_out.pre_scale(b.w / dw, b.h / dh);
            if reduce < 1.0 {
                // Strong leading blur (e.g. blurred background fill): blur the small
                // source instead of the full-size layer, then upscale bilinearly.
                // Edge-clamped, so fills don't darken toward the frame edges.
                let e = it.fx.iter().find(|e| e.on).expect("blur_source_scale saw a blur");
                let pev = ev.with_lists(&[schema::effect("blur").unwrap().params]);
                let sigma = pev.f(&e.props, "radius") * scale / 2.0 / Self::max_scale(&m).max(1e-3);
                let mut pm = (*src).clone();
                pixel::blur_pixmap(&mut pm, sigma, 0);
                src = std::sync::Arc::new(pm);
                *pre_blurred = true;
            }
            if it.kind == ItemKind::Clip && ev.str(&it.props, "fit") == "blur" {
                return Ok(self.blur_fill(&src, m, clip));
            }
            let mut pts = [Point::from_xy(0.0, 0.0), Point::from_xy(dw, 0.0), Point::from_xy(dw, dh), Point::from_xy(0.0, dh)];
            m.map_points(&mut pts);
            let r = IRect::bound(&pts.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>());
            if r.intersect(&self.out_rect()).is_empty() {
                return Ok(None);
            }
            // fast path: integer translation, no resampling
            let is_ident = (m.sx - 1.0).abs() < 1e-4 && (m.sy - 1.0).abs() < 1e-4 && m.kx.abs() < 1e-5 && m.ky.abs() < 1e-5 && (m.tx - m.tx.round()).abs() < 1e-3 && (m.ty - m.ty.round()).abs() < 1e-3;
            if is_ident {
                let mut lb = LayerBuf { pm: Pm::Shared(src.clone()), x: m.tx.round() as i32, y: m.ty.round() as i32 };
                if fxm > 0 {
                    let rr = lb.rect().pad(fxm).intersect(&clip);
                    lb.expand_to(rr.union(&lb.rect().intersect(&clip)));
                }
                return Ok(Some(lb));
            }
            let r = r.pad(fxm).intersect(&clip);
            if r.is_empty() {
                return Ok(None);
            }
            let mut lb = LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32).ok_or_else(|| anyhow!("alloc"))?;
            let pm = lb.pm.make_mut();
            let up = Self::max_scale(&m) > 1.3;
            pixel::draw_affine(pm, &src, Transform::from_translate(-r.x as f32, -r.y as f32).pre_concat(m), up && !self.rc.draft && !*pre_blurred);
            return Ok(Some(lb));
        }
        // vector content
        let vm = self.vector_margin(it, ev);
        let mut pts = [
            Point::from_xy(b.x - vm, b.y - vm),
            Point::from_xy(b.x + b.w + vm, b.y - vm),
            Point::from_xy(b.x + b.w + vm, b.y + b.h + vm),
            Point::from_xy(b.x - vm, b.y + b.h + vm),
        ];
        m_out.map_points(&mut pts);
        let r = IRect::bound(&pts.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()).pad(fxm).intersect(&clip);
        if r.is_empty() {
            return Ok(None);
        }
        let mut lb = LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32).ok_or_else(|| anyhow!("alloc"))?;
        let m = Transform::from_translate(-r.x as f32, -r.y as f32).pre_concat(m_out);
        self.draw_vector(it, ev, x, tl, lb.pm.make_mut(), m, Self::max_scale(&m_out));
        Ok(Some(lb))
    }

    // ------------------------------------------------------------ transitions

    fn tr_ease(tr: &Transition, default: Ease) -> Ease {
        tr.props.get("ease").and_then(|v| Ease::parse(&v.str()).ok()).unwrap_or(default)
    }

    fn render_side(&self, it: Option<&Item>, t: f64, ex: Extra) -> Result<Option<LayerBuf>> {
        let Some(it) = it else { return Ok(None) };
        Ok(self.render_item(it, t, &ex)?.map(|(mut lb, op, _)| {
            bake_opacity(&mut lb, op);
            lb
        }))
    }

    fn render_transition(&self, tr: &Transition, a: Option<&Item>, b: Option<&Item>, p: f64, t: f64) -> Result<Option<LayerBuf>> {
        let (cw, ch) = (self.comp.w as f32, self.comp.h as f32);
        let c = (cw / 2.0, ch / 2.0);
        let kind = tr.kind.as_str();
        let dir = tr.props.get("dir").map(|v| v.str()).unwrap_or_else(|| "left".into());
        let dv = match dir.as_str() {
            "left" => (-1.0f32, 0.0f32),
            "right" => (1.0, 0.0),
            "up" => (0.0, -1.0),
            "down" => (0.0, 1.0),
            "h" => (1.0, 0.0),
            "v" => (0.0, 1.0),
            _ => (-1.0, 0.0),
        };
        let num = |k: &str, d: f64| tr.props.get(k).map(|v| v.num()).unwrap_or(d);
        let linear_kinds = ["dissolve", "dip", "fadeblack", "fadewhite", "flash", "luma"];
        let pe = Self::tr_ease(tr, if linear_kinds.contains(&kind) { Ease::Linear } else { Ease::InOut(Curve::Cubic) }).apply(p) as f32;
        let about = |s: f32, rot: f32| Transform::from_translate(c.0, c.1).pre_rotate(rot).pre_scale(s, s).pre_translate(-c.0, -c.1);
        let mut ea = Extra::none();
        let mut eb = Extra::none();
        let mut over_order_b_first = false;
        let mut combine_lerp = true;
        match kind {
            "slide" => {
                eb.mat = Transform::from_translate(-dv.0 * (1.0 - pe) * cw, -dv.1 * (1.0 - pe) * ch);
                combine_lerp = false;
            }
            "push" | "whip" => {
                ea.mat = Transform::from_translate(dv.0 * pe * cw, dv.1 * pe * ch);
                eb.mat = Transform::from_translate(-dv.0 * (1.0 - pe) * cw, -dv.1 * (1.0 - pe) * ch);
                combine_lerp = false;
            }
            "reveal" => {
                ea.mat = Transform::from_translate(dv.0 * pe * cw, dv.1 * pe * ch);
                over_order_b_first = true;
                combine_lerp = false;
            }
            "zoom" => {
                let amt = num("amount", 1.5) as f32;
                ea.mat = about(1.0 + (amt - 1.0) * pe, 0.0);
                eb.mat = about(1.0 / amt + (1.0 - 1.0 / amt) * pe, 0.0);
            }
            "zoomout" => {
                ea.mat = about((1.0 - pe).max(0.001), 0.0);
                ea.opacity = 1.0 - pe * 0.6;
                over_order_b_first = true;
                combine_lerp = false;
            }
            "spin" => {
                let turns = num("turns", 0.5) as f32 * 360.0;
                ea.mat = about(1.0 - pe * 0.6, pe * turns);
                eb.mat = about(0.4 + 0.6 * pe, -(1.0 - pe) * turns);
            }
            "flip" => {
                let vert = dir == "v";
                if pe < 0.5 {
                    if vert {
                        ea.rx = pe as f64 * 180.0;
                    } else {
                        ea.ry = pe as f64 * 180.0;
                    }
                } else if vert {
                    eb.rx = -(1.0 - pe as f64) * 180.0;
                } else {
                    eb.ry = -(1.0 - pe as f64) * 180.0;
                }
            }
            "cube" => {
                // both clips are faces of a cube turning about its center, half a face behind the screen
                let vert = matches!(dir.as_str(), "up" | "down");
                let half = if vert { ch as f64 / 2.0 } else { cw as f64 / 2.0 };
                let deg = 90.0 * pe as f64;
                let (ta, tb) = match dir.as_str() {
                    "right" | "up" => (-deg, 90.0 - deg),
                    _ => (deg, deg - 90.0),
                };
                let face = |e: &mut Extra, th: f64| {
                    let (sn, cs) = th.to_radians().sin_cos();
                    if vert {
                        e.rx = th;
                        e.off = (0.0, half * sn, half - half * cs);
                    } else {
                        e.ry = th;
                        e.off = (-half * sn, 0.0, half - half * cs);
                    }
                };
                face(&mut ea, ta);
                face(&mut eb, tb);
                over_order_b_first = ta.abs() < tb.abs();
                combine_lerp = false;
            }
            _ => {}
        }
        let (show_a, show_b) = match kind {
            "flip" => (pe < 0.5, pe >= 0.5),
            "pixelate" | "glitch" => (true, true),
            _ => (true, true),
        };
        let la = if show_a { self.render_side(a, t, ea)? } else { None };
        let lb = if show_b { self.render_side(b, t, eb)? } else { None };
        if la.is_none() && lb.is_none() && !matches!(kind, "dip" | "fadeblack" | "fadewhite" | "flash") {
            return Ok(None);
        }
        if !combine_lerp {
            let r = la.as_ref().map(|l| l.rect()).unwrap_or(IRect { x: 0, y: 0, w: 0, h: 0 }).union(&lb.as_ref().map(|l| l.rect()).unwrap_or(IRect { x: 0, y: 0, w: 0, h: 0 })).intersect(&self.out_rect());
            if r.is_empty() {
                return Ok(None);
            }
            let mut out = LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32).ok_or_else(|| anyhow!("alloc"))?;
            let order: Vec<&Option<LayerBuf>> = if over_order_b_first { vec![&lb, &la] } else { vec![&la, &lb] };
            for l in order.into_iter().flatten() {
                src_over(out.pm.make_mut(), l.pm.get(), l.x - r.x, l.y - r.y, 1.0);
            }
            if kind == "whip" {
                let amount = ((p * std::f64::consts::PI).sin() as f32) * 0.12 * cw.max(ch) * self.sx;
                blur_pixmap(out.pm.make_mut(), amount / 2.0, if dv.0 != 0.0 { 1 } else { 2 });
            }
            return Ok(Some(out));
        }
        // lerp-based transitions over the union (full frame for dips)
        let full = matches!(kind, "dip" | "fadeblack" | "fadewhite" | "flash" | "luma");
        let mut r = la.as_ref().map(|l| l.rect()).unwrap_or(IRect { x: 0, y: 0, w: 0, h: 0 }).union(&lb.as_ref().map(|l| l.rect()).unwrap_or(IRect { x: 0, y: 0, w: 0, h: 0 }));
        if full {
            r = r.union(&self.out_rect());
        }
        let r = r.intersect(&self.out_rect());
        if r.is_empty() {
            return Ok(None);
        }
        let mut aa = match la {
            Some(mut l) => {
                l.expand_to(r);
                l
            }
            None => LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32).unwrap(),
        };
        let mut bb = match lb {
            Some(mut l) => {
                l.expand_to(r);
                l
            }
            None => LayerBuf::new(r.x, r.y, r.w as u32, r.h as u32).unwrap(),
        };
        let soft = num("soft", 0.08).max(0.0005) as f32;
        let (ow, oh) = (self.ow as f32, self.oh as f32);
        let (rx0, ry0) = (r.x as f32, r.y as f32);
        let pp = pe;
        let edge = move |u: f32| ((pp * (1.0 + soft) - u) / soft).clamp(0.0, 1.0);
        match kind {
            "dip" | "fadeblack" | "fadewhite" => {
                let col = match kind {
                    "fadeblack" => Color::BLACK,
                    "fadewhite" => Color::WHITE,
                    _ => tr.props.get("color").map(|v| v.color()).unwrap_or(Color::BLACK),
                };
                let cpx = col.to_premul_u8();
                let mut solid = pixel::new_pixmap(r.w as u32, r.h as u32);
                for px in solid.data_mut().chunks_exact_mut(4) {
                    px.copy_from_slice(&cpx);
                }
                if pe < 0.5 {
                    let k = pe * 2.0;
                    lerp_into(aa.pm.make_mut(), &solid, |_, _, _| k);
                    return Ok(Some(aa));
                } else {
                    let k = (pe - 0.5) * 2.0;
                    let mut s2 = solid;
                    lerp_into(&mut s2, bb.pm.get(), |_, _, _| k);
                    return Ok(Some(LayerBuf { pm: Pm::Owned(s2), x: r.x, y: r.y }));
                }
            }
            "dissolve" => lerp_into(aa.pm.make_mut(), bb.pm.get(), |_, _, _| pp),
            "wipe" => {
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let (gx, gy) = ((x as f32 + rx0 + 0.5) / ow, (y as f32 + ry0 + 0.5) / oh);
                    let u = match dir.as_str() {
                        "left" => 1.0 - gx,
                        "right" => gx,
                        "up" => 1.0 - gy,
                        _ => gy,
                    };
                    edge(u)
                });
            }
            "iris" | "irisclose" | "diamond" => {
                let close = kind == "irisclose";
                let diamond = kind == "diamond";
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let (dx, dy) = ((x as f32 + rx0 + 0.5) / ow - 0.5, (y as f32 + ry0 + 0.5) / oh - 0.5);
                    let d = if diamond { dx.abs() + dy.abs() } else { (dx * dx * (ow / oh).powi(2) + dy * dy).sqrt() / (0.5 * ((ow / oh).powi(2) + 1.0).sqrt()) };
                    let d = d.clamp(0.0, 1.0);
                    if close {
                        1.0 - (((1.0 - pp) * (1.0 + soft) - d) / soft).clamp(0.0, 1.0)
                    } else {
                        edge(d)
                    }
                });
            }
            "clock" => {
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let (dx, dy) = ((x as f32 + rx0 + 0.5) - ow / 2.0, (y as f32 + ry0 + 0.5) - oh / 2.0);
                    let a = (dx.atan2(-dy) / std::f32::consts::TAU).rem_euclid(1.0);
                    edge(a)
                });
            }
            "barn" => {
                let vert = dir == "v";
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let u = if vert { ((y as f32 + ry0 + 0.5) / oh - 0.5).abs() * 2.0 } else { ((x as f32 + rx0 + 0.5) / ow - 0.5).abs() * 2.0 };
                    edge(u)
                });
            }
            "blinds" => {
                let n = num("count", 10.0).max(1.0) as f32;
                let horiz = dir == "h";
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let g = if horiz { (x as f32 + rx0) / ow } else { (y as f32 + ry0) / oh };
                    edge((g * n).fract())
                });
            }
            "luma" => {
                let inv = tr.props.get("invert").map(|v| v.bool()).unwrap_or(false);
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |_, _, bp| {
                    let a = bp[3] as f32;
                    let l = if a > 0.0 { (0.2126 * bp[0] as f32 + 0.7152 * bp[1] as f32 + 0.0722 * bp[2] as f32) / a } else { 0.0 };
                    let u = if inv { 1.0 - l } else { l };
                    edge(1.0 - u)
                });
            }
            "blur" => {
                let amt = num("amount", 40.0) as f32 * self.sx;
                blur_pixmap(aa.pm.make_mut(), amt * (pe * 2.0).min(1.0) / 2.0, 0);
                blur_pixmap(bb.pm.make_mut(), amt * (2.0 - pe * 2.0).min(1.0) / 2.0, 0);
                let k = smoothstep(0.3, 0.7, pe as f64) as f32;
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |_, _, _| k);
            }
            "pixelate" => {
                let mx = num("size", 60.0);
                let s = (mx * (1.0 - (2.0 * pe as f64 - 1.0).abs())).max(1.0);
                let pix = Effect { id: String::new(), kind: "pixelate".into(), on: true, props: [("size".to_string(), crate::model::anim::Anim::new(Val::N(s)))].into_iter().collect() };
                let x = ExprCtx::default();
                let ev = Ev::new(0.0, &x, self.vc(), &[]);
                let fcx = FxCtx { ev: &ev, scale: self.sx, out_w: self.ow, out_h: self.oh, frame: 0, seed: 0 };
                effects::apply(&pix, &mut aa, &fcx);
                effects::apply(&pix, &mut bb, &fcx);
                let k = smoothstep(0.45, 0.55, pe as f64) as f32;
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |_, _, _| k);
            }
            "glitch" | "flash" => {
                let k = smoothstep(0.4, 0.6, pe as f64) as f32;
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |_, _, _| k);
                let peak = 1.0 - (2.0 * pe - 1.0).abs();
                if kind == "glitch" {
                    let g = Effect { id: String::new(), kind: "glitch".into(), on: true, props: [("amount".to_string(), crate::model::anim::Anim::new(Val::N(peak as f64)))].into_iter().collect() };
                    let x = ExprCtx::default();
                    let ev = Ev::new(t, &x, self.vc(), &[]);
                    let fcx = FxCtx { ev: &ev, scale: self.sx, out_w: self.ow, out_h: self.oh, frame: (t * self.comp.fps) as i64, seed: 77 };
                    effects::apply(&g, &mut aa, &fcx);
                } else {
                    let col = tr.props.get("color").map(|v| v.color()).unwrap_or(Color::WHITE);
                    let f = peak.powf(1.5);
                    let cp = [col.r * 255.0, col.g * 255.0, col.b * 255.0];
                    let pm = aa.pm.make_mut();
                    pm.data_mut().par_chunks_mut(4).for_each(|px| {
                        let na = px[3] as f32 + (255.0 - px[3] as f32) * f;
                        for i in 0..3 {
                            px[i] = (px[i] as f32 + cp[i] * f).min(na) as u8;
                        }
                        px[3] = na as u8;
                    });
                }
            }
            "blocks" => {
                let bs = (num("size", 48.0) as f32 * self.sx).max(1.0);
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let r = (effects::hash2(((x as f32 + rx0) / bs) as u32, ((y as f32 + ry0) / bs) as u32, 0x51ed) & 0xffff) as f32 / 65535.0;
                    if r < pp {
                        1.0
                    } else {
                        0.0
                    }
                });
            }
            "checker" => {
                let cs = ow / num("count", 8.0).max(1.0) as f32;
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let (gx, gy) = ((x as f32 + rx0) / cs, (y as f32 + ry0) / cs);
                    let even = (gx.floor() as i64 + gy.floor() as i64).rem_euclid(2) == 0;
                    let u = gx.fract();
                    if (if even { pp * 2.0 } else { pp * 2.0 - 1.0 }) > u {
                        1.0
                    } else {
                        0.0
                    }
                });
            }
            "inset" | "irisbox" | "iriscross" | "band" => {
                let n = num("count", 8.0).max(1.0) as f32;
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| {
                    let (gx, gy) = ((x as f32 + rx0 + 0.5) / ow, (y as f32 + ry0 + 0.5) / oh);
                    let (dx, dy) = ((gx - 0.5).abs() * 2.0, (gy - 0.5).abs() * 2.0);
                    let u = match kind {
                        "irisbox" => dx.max(dy),
                        "iriscross" => dx.min(dy),
                        "band" => {
                            let (along, across) = if dir == "v" { (gy, gx) } else { (gx, gy) };
                            if ((across * n).floor() as i64) % 2 == 0 {
                                along
                            } else {
                                1.0 - along
                            }
                        }
                        _ => match dir.as_str() {
                            "tr" => (1.0 - gx).max(gy),
                            "bl" => gx.max(1.0 - gy),
                            "br" => (1.0 - gx).max(1.0 - gy),
                            "center" => dx.max(dy),
                            _ => gx.max(gy),
                        },
                    };
                    edge(u)
                });
            }
            "filmdissolve" | "additive" => {
                let film = kind == "filmdissolve";
                let (ka, kb) = ((2.0 * (1.0 - pp)).min(1.0), (2.0 * pp).min(1.0));
                let lin: Vec<f32> = (0..256).map(|v| (v as f32 / 255.0).powf(2.2)).collect();
                mix_px(aa.pm.make_mut(), bb.pm.get(), |a, b| {
                    let mut o = [0u8; 4];
                    for k in 0..4 {
                        let v = if film && k < 3 {
                            (lin[a[k] as usize] * (1.0 - pp) + lin[b[k] as usize] * pp).powf(1.0 / 2.2) * 255.0
                        } else if film {
                            a[k] as f32 * (1.0 - pp) + b[k] as f32 * pp
                        } else {
                            a[k] as f32 * ka + b[k] as f32 * kb
                        };
                        o[k] = v.round().clamp(0.0, 255.0) as u8;
                    }
                    o[0] = o[0].min(o[3]);
                    o[1] = o[1].min(o[3]);
                    o[2] = o[2].min(o[3]);
                    o
                });
            }
            "leak" => {
                let k = smoothstep(0.35, 0.65, pe as f64) as f32;
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |_, _, _| k);
                let peak = 1.0 - (2.0 * pe - 1.0).abs();
                let col = tr.props.get("color").cloned().unwrap_or_else(|| Val::S("#ff7a18".into()));
                let col = crate::model::anim::Anim::new(Val::C(col.color()));
                let leak = Effect { id: String::new(), kind: "lightleak".into(), on: true, props: [("amount".to_string(), crate::model::anim::Anim::new(Val::N(peak as f64 * 0.95))), ("color".to_string(), col)].into_iter().collect() };
                let x = ExprCtx::default();
                let ev = Ev::new(t, &x, self.vc(), &[]);
                let fcx = FxCtx { ev: &ev, scale: self.sx, out_w: self.ow, out_h: self.oh, frame: (t * self.comp.fps) as i64, seed: 11 };
                effects::apply(&leak, &mut aa, &fcx);
            }
            "burn" => {
                let col = tr.props.get("color").map(|v| v.color()).unwrap_or(Color::rgba(1.0, 0.42, 0.0, 1.0));
                let (w, h) = (aa.w() as usize, aa.h() as usize);
                // noise on a coarse grid; the burn front moves through it
                let step = 4usize;
                let (gw, gh) = (w / step + 2, h / step + 2);
                let grid: Vec<f32> = (0..gw * gh)
                    .into_par_iter()
                    .map(|i| super::noise::fbm(((i % gw * step) as f32 + rx0) / ow * 3.0, ((i / gw * step) as f32 + ry0) / ow * 3.0, 0.0, 4.0, super::noise::Kind::Smooth, 77))
                    .collect();
                let noise = move |x: usize, y: usize| {
                    let (fx, fy) = (x as f32 / step as f32, y as f32 / step as f32);
                    let (ix, iy) = ((fx as usize).min(gw - 2), (fy as usize).min(gh - 2));
                    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
                    let g = |a: usize, b: usize| grid[b * gw + a];
                    (g(ix, iy) * (1.0 - tx) + g(ix + 1, iy) * tx) * (1.0 - ty) + (g(ix, iy + 1) * (1.0 - tx) + g(ix + 1, iy + 1) * tx) * ty
                };
                let e = pp * (1.0 + 2.0 * soft) - soft;
                lerp_into(aa.pm.make_mut(), bb.pm.get(), |x, y, _| ((e - noise(x as usize, y as usize)) / soft + 0.5).clamp(0.0, 1.0));
                let glow_k = 1.0 - (2.0 * pp - 1.0).powi(2);
                let cp = [col.r * 255.0, col.g * 255.0, col.b * 255.0];
                aa.pm.make_mut().data_mut().par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..w {
                        let g = (-((noise(x, y) - e) / (soft * 1.5)).powi(2)).exp() * glow_k;
                        if g < 0.01 {
                            continue;
                        }
                        let px = &mut row[x * 4..x * 4 + 4];
                        let na = (px[3] as f32 + (255.0 - px[3] as f32) * g).min(255.0);
                        for i in 0..3 {
                            px[i] = (px[i] as f32 + cp[i] * g).min(na) as u8;
                        }
                        px[3] = na as u8;
                    }
                });
            }
            _ => lerp_into(aa.pm.make_mut(), bb.pm.get(), |_, _, _| pp),
        }
        Ok(Some(aa))
    }

    // ------------------------------------------------------------ frame

    fn units(&self, t: f64) -> Vec<Unit<'a>> {
        let mut out = vec![];
        let matte_src: HashSet<&str> = self.comp.items().filter_map(|i| i.matte.as_ref().map(|m| m.src.as_str())).collect();
        let fd = 0.5 / self.comp.fps.max(1.0);
        for tr in self.comp.vtracks() {
            if tr.hidden {
                continue;
            }
            let items = &tr.items;
            let mut used = vec![false; items.len()];
            for i in 0..items.len().saturating_sub(1) {
                let (a, b) = (&items[i], &items[i + 1]);
                let Some(trn) = &a.tout else { continue };
                if (b.start - a.end()).abs() > fd || a.off || b.off {
                    continue;
                }
                let d = trn.dur.max(1e-3);
                let cut = a.end();
                let (w0, w1) = (cut - d / 2.0, cut + d / 2.0);
                if t >= w0 && t < w1 {
                    out.push(Unit::Pair(a, b, trn, (t - w0) / d));
                    used[i] = true;
                    used[i + 1] = true;
                }
            }
            for (i, it) in items.iter().enumerate() {
                if used[i] || it.off || it.kind.is_invisible() || !it.contains(t) || matte_src.contains(it.id.as_str()) {
                    continue;
                }
                let prev_adj = i > 0 && items[i - 1].tout.is_some() && (it.start - items[i - 1].end()).abs() <= fd;
                let next_adj = i + 1 < items.len() && (items[i + 1].start - it.end()).abs() <= fd;
                let mut edge = None;
                if let Some(tn) = &it.tin {
                    if !prev_adj && t < it.start + tn.dur {
                        edge = Some((tn, true, ((t - it.start) / tn.dur.max(1e-3)).clamp(0.0, 1.0)));
                    }
                }
                if let Some(tn) = &it.tout {
                    if !next_adj && t >= it.end() - tn.dur {
                        edge = Some((tn, false, ((t - (it.end() - tn.dur)) / tn.dur.max(1e-3)).clamp(0.0, 1.0)));
                    }
                }
                if it.kind == ItemKind::Adjust {
                    let k = match edge {
                        Some((_, true, p)) => p,
                        Some((_, false, p)) => 1.0 - p,
                        None => 1.0,
                    };
                    out.push(Unit::Adjust(it, k));
                } else {
                    out.push(Unit::Single(it, edge));
                }
            }
        }
        out
    }

    fn render_unit(&self, u: &Unit, t: f64) -> Result<Option<(LayerBuf, f32, String)>> {
        match u {
            Unit::Single(it, None) => self.render_item(it, t, &Extra::none()),
            Unit::Single(it, Some((tr, is_in, p))) => {
                let (a, b, prog) = if *is_in { (None, Some(*it), *p) } else { (Some(*it), None, *p) };
                let blend = it.props.get("blend").map(|v| v.v.str()).unwrap_or_else(|| "normal".into());
                Ok(self.render_transition(tr, a, b, prog, t)?.map(|lb| (lb, 1.0, blend)))
            }
            Unit::Pair(a, b, tr, p) => Ok(self.render_transition(tr, Some(*a), Some(*b), *p, t)?.map(|lb| (lb, 1.0, "normal".to_string()))),
            Unit::Adjust(..) => Ok(None),
        }
    }

    /// Compositing order of a group: track order, except that runs of
    /// consecutive 3D layers are sorted back to front through the camera.
    fn stacking_order(&self, group: &[Unit], t: f64) -> Vec<usize> {
        let mut order: Vec<usize> = (0..group.len()).collect();
        if group.len() < 2 {
            return order;
        }
        let g = self.g();
        let active = g.camera(t);
        let cam = active.unwrap_or_else(|| Cam::default_for(self.comp.w as f64, self.comp.h as f64));
        let depth: Vec<Option<f64>> = group
            .iter()
            .map(|u| match u {
                Unit::Single(it, _) => g.depth(it, t, &cam, active.is_some()),
                _ => None,
            })
            .collect();
        let mut i = 0;
        while i < order.len() {
            let mut j = i;
            while j < order.len() && depth[j].is_some() {
                j += 1;
            }
            if j - i > 1 {
                order[i..j].sort_by(|a, b| depth[*b].partial_cmp(&depth[*a]).unwrap_or(std::cmp::Ordering::Equal));
            }
            i = j.max(i + 1);
        }
        order
    }

    fn apply_adjust(&self, it: &Item, k: f64, t: f64, out: &mut Pixmap) {
        let tl = t - it.start;
        let lists = Self::lists(it.kind);
        let b = LBounds { x: 0.0, y: 0.0, w: self.comp.w as f32, h: self.comp.h as f32 };
        let x = self.ectx(it, Some(b));
        let ev = Ev::new(tl, &x, self.vc(), &lists);
        let op = ev.f(&it.props, "opacity") * k as f32;
        if op <= 0.001 {
            return;
        }
        let w = self.world(it, t, &ev, b);
        let mut lb = LayerBuf { pm: Pm::Owned(out.clone()), x: 0, y: 0 };
        let scale = self.sx.max(self.sy);
        let fcx = FxCtx { ev: &ev, scale, out_w: self.ow, out_h: self.oh, frame: (t * self.comp.fps).round() as i64, seed: x.seed };
        for e in it.fx.iter().filter(|e| e.on) {
            effects::apply(e, &mut lb, &fcx);
        }
        if !it.masks.is_empty() || it.fx.iter().any(|e| e.kind == "crop") {
            masks::apply_masks(&mut lb, &it.masks, &it.fx, &ev, b, self.out_t().pre_concat(w.m), scale);
        }
        let blend = ev.str(&it.props, "blend");
        composite(out, &lb, op, &blend);
    }

    fn render(&self, t: f64, out: &mut Pixmap) -> Result<()> {
        let bg = self.comp.bg.to_premul_u8();
        if bg[3] > 0 {
            for px in out.data_mut().chunks_exact_mut(4) {
                px.copy_from_slice(&bg);
            }
        } else {
            out.fill(resvg::tiny_skia::Color::TRANSPARENT);
        }
        let units = self.units(t);
        // groups separated by adjustment layers; each group renders in parallel
        let mut start = 0;
        while start < units.len() {
            let end = units[start..].iter().position(|u| matches!(u, Unit::Adjust(..))).map(|p| start + p).unwrap_or(units.len());
            let group = &units[start..end];
            let rendered: Vec<Result<Option<(LayerBuf, f32, String)>>> = if group.len() > 1 { group.par_iter().map(|u| self.render_unit(u, t)).collect() } else { group.iter().map(|u| self.render_unit(u, t)).collect() };
            let mut rendered: Vec<Option<Result<Option<(LayerBuf, f32, String)>>>> = rendered.into_iter().map(Some).collect();
            for i in self.stacking_order(group, t) {
                if let Some((lb, op, blend)) = rendered[i].take().unwrap()? {
                    composite(out, &lb, op, &blend);
                }
            }
            if end < units.len() {
                if let Unit::Adjust(it, k) = &units[end] {
                    self.apply_adjust(it, *k, t, out);
                }
                start = end + 1;
            } else {
                break;
            }
        }
        Ok(())
    }
}

/// Combines two same-size premultiplied buffers pixel by pixel into `a`, in parallel.
fn mix_px(a: &mut Pixmap, b: &Pixmap, f: impl Fn(&[u8], &[u8]) -> [u8; 4] + Sync) {
    let w = a.width() as usize;
    let bd = b.data();
    a.data_mut().par_chunks_mut(w * 4).zip(bd.par_chunks(w * 4)).for_each(|(ra, rb)| {
        for (pa, pb) in ra.chunks_exact_mut(4).zip(rb.chunks_exact(4)) {
            let o = f(pa, pb);
            pa.copy_from_slice(&o);
        }
    });
}

fn apply_matte(lb: &mut LayerBuf, matte: Option<(&LayerBuf, f32)>, mode: MatteMode) {
    let (w, h) = (lb.w() as i32, lb.h() as i32);
    let inv = matches!(mode, MatteMode::AlphaInv | MatteMode::LumaInv);
    let luma = matches!(mode, MatteMode::Luma | MatteMode::LumaInv);
    let mut mask = vec![if inv { 255u8 } else { 0u8 }; (w * h) as usize];
    if let Some((m, op)) = matte {
        let md = m.pm.get().data();
        let mw = m.w() as i32;
        let mh = m.h() as i32;
        for y in 0..h {
            let my = y + lb.y - m.y;
            if my < 0 || my >= mh {
                continue;
            }
            for x in 0..w {
                let mx = x + lb.x - m.x;
                if mx < 0 || mx >= mw {
                    continue;
                }
                let i = ((my * mw + mx) * 4) as usize;
                let v = if luma { (0.2126 * md[i] as f32 + 0.7152 * md[i + 1] as f32 + 0.0722 * md[i + 2] as f32) * op } else { md[i + 3] as f32 * op };
                let v = v.clamp(0.0, 255.0) as u8;
                mask[(y * w + x) as usize] = if inv { 255 - v } else { v };
            }
        }
    }
    pixel::apply_alpha_mask(lb.pm.make_mut(), &mask);
}
