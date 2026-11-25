//! `track`: the trackers driving masks, nulls, corner pins and stabilization.
//!
//! ```text
//! track c1.m1 [range=a-b] [mode=pos|rotation|similarity|affine|perspective] [planar] [at=T]
//!                                                          the mask follows what is under it (After Effects' mask tracker methods)
//! track c1 region=cx,cy,w,h to=null [mode=pos|rotation|similarity|perspective]
//!                                                          new null with position (and rotation / scale) keys
//! track c1 corners='x,y x,y x,y x,y' to=cornerpin on=c5 [mode=perspective|points]
//!                                                          the plane inside the four corners drives c5's corner pin
//! track c1 region=cx,cy,w,h to=stabilize [mode=similarity] [smooth=1s] [zoom]
//! track c1 to=camera ...                                   the 3D camera tracker (see camtrack)
//! mask c1 trace [channel=luma|alpha|red|green|blue] [threshold=] [pick=x,y] [range=a-b every=0.5]
//! ```
//!
//! Tracking runs as a cached analysis of the clip's media: the point tracker
//! (`render::track`, normalized cross-correlation) for position and
//! similarity, the planar tracker (`render::planar`, Lucas-Kanade features
//! and RANSAC) for rotation, affine and perspective motion. The result is
//! ordinary keyframes, simplified so a 10 s track is tens of keys. The reply
//! names the ranges where the tracker lost confidence.

use super::exec::{Exec, A};
use super::exec_props::{SubKind, Target};
use crate::model::anim::{Anim, Ease, Key};
use crate::model::project::*;
use crate::model::schema;
use crate::model::time::fmt_t;
use crate::model::value::Val;
use crate::render::linalg::M3;
use crate::render::planar::Model;
use crate::render::track::{lost_ranges, simplify};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

/// Below this confidence a stretch is reported as lost.
const LOST: f64 = 0.5;

fn f4(v: f64) -> String {
    format!("{v:.4}")
}

/// Points 'x,y x,y ...' (fractions).
fn parse_points(s: &str) -> Result<Vec<(f64, f64)>> {
    s.split_whitespace()
        .map(|p| p.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))).ok_or_else(|| anyhow!("points are 'x,y x,y ...' (fractions of the frame), not '{p}'")))
        .collect()
}

fn rot_about(x: f64, y: f64, cx: f64, cy: f64, deg: f64) -> (f64, f64) {
    let (s, c) = deg.to_radians().sin_cos();
    let (dx, dy) = (x - cx, y - cy);
    (cx + dx * c - dy * s, cy + dx * s + dy * c)
}

/// A mask's outline as a path in layer fractions, with its rotation and its
/// placement (offset, scale, trot) baked in, drawn as `render::masks` draws
/// it on a `lw` x `lh` px layer, at the values `val` gives.
pub(crate) fn mask_as_path(kind: &str, val: &dyn Fn(&str) -> Val, lw: f64, lh: f64) -> Option<String> {
    use crate::render::shapes;
    let (x, y) = (val("x").num(), val("y").num());
    let rot = val("rot").num();
    let (ox, oy) = val("offset").vec();
    let sc = val("scale").num().max(1e-4);
    let trot = val("trot").num();
    let d = if kind == "path" {
        let d = val("d").str();
        let (cx, cy) = (x * lw, y * lh);
        if rot.abs() > 1e-9 {
            shapes::map_points(&d, |u, v| {
                let (a, b) = rot_about(u * lw, v * lh, cx, cy, rot);
                (a / lw, b / lh)
            })?
        } else {
            d
        }
    } else {
        shapes::mask_shape_d(kind, x, y, val("w").num(), val("h").num(), rot, val("radius").num(), val("points").num().round() as usize, val("expand").num(), lw, lh)?
    };
    if ox.abs() < 1e-12 && oy.abs() < 1e-12 && (sc - 1.0).abs() < 1e-9 && trot.abs() < 1e-9 {
        return Some(d);
    }
    // placement pivot: the shape's center, or the box of a path (control points included)
    let (px, py) = if kind == "path" {
        let pts: Vec<(f64, f64)> = crate::render::shapes::path_segs(&d)?
            .iter()
            .flat_map(|s| match *s {
                shapes::Seg::M(a, b) | shapes::Seg::L(a, b) => vec![(a, b)],
                shapes::Seg::C(a, b, c, e, f, g) => vec![(a, b), (c, e), (f, g)],
                shapes::Seg::Q(a, b, c, e) => vec![(a, b), (c, e)],
                shapes::Seg::Z => vec![],
            })
            .collect();
        if pts.is_empty() {
            return None;
        }
        let (x0, x1) = pts.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.0), m.1.max(p.0)));
        let (y0, y1) = pts.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.1), m.1.max(p.1)));
        ((x0 + x1) / 2.0 * lw, (y0 + y1) / 2.0 * lh)
    } else {
        (x * lw, y * lh)
    };
    shapes::map_points(&d, |u, v| {
        let (a, b) = (px + (u * lw - px) * sc, py + (v * lh - py) * sc);
        let (a, b) = rot_about(a, b, px, py, trot);
        ((a + ox * lw) / lw, (b + oy * lh) / lh)
    })
}

/// A path's vertices as a polygon (the region its features come from).
fn outline_poly(d: &str) -> Vec<(f64, f64)> {
    crate::render::shapes::path_vertices(d)
}

/// Bilinear map of a layer-fraction point through the layer's on-screen quad.
fn through_quad(q: &[(f64, f64); 4], u: f64, v: f64) -> (f64, f64) {
    let top = (q[0].0 + (q[1].0 - q[0].0) * u, q[0].1 + (q[1].1 - q[0].1) * u);
    let bot = (q[3].0 + (q[2].0 - q[3].0) * u, q[3].1 + (q[2].1 - q[3].1) * u);
    (top.0 + (bot.0 - top.0) * v, top.1 + (bot.1 - top.1) * v)
}

/// A video clip being tracked, with its timing.
pub(crate) struct Clip {
    pub id: String,
    pub loc: (usize, usize, usize),
    pub start: f64,
    pub dur: f64,
    pub src_in: f64,
    pub speed: f64,
    pub media: Media,
}

/// Rows of a planar track: (source time, transform in frame fractions, confidence).
type PlanarRows = Vec<(f64, M3, f64)>;

impl<'a> Exec<'a> {
    /// `track` with a layer (not a track subcommand): the tracker.
    pub fn c_tracker(&mut self, a: &mut A) -> Result<()> {
        let usage = "track c1.m1 [range=a-b] [mode=pos|rotation|similarity|affine|perspective] | track c1 region=cx,cy,w,h to=null [mode=] | track c1 corners='x,y x,y x,y x,y' to=cornerpin on=c5 | track c1 to=camera";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let (item, mask) = match spec.split_once('.') {
            Some((i, m)) => (i.to_string(), Some(m.to_string())),
            None => (spec.clone(), None),
        };
        let to = a.take(&["to", "into"]).map(|s| s.to_ascii_lowercase());
        if matches!(to.as_deref(), Some("camera" | "cam" | "3d" | "camtrack" | "3dcamera")) {
            if mask.is_some() {
                bail!("a camera solve tracks the whole clip: track {item} to=camera (or camtrack {item})");
            }
            let id = self.item_id(&item)?;
            return self.camtrack_solve(&id, a);
        }
        let clip = self.track_clip(&item)?;
        // the time the region is drawn at, and the range to track (timeline seconds)
        let at = match a.take(&["at", "from"]) {
            Some(s) => self.time(&s)?,
            None => self.env.ui().playhead.filter(|t| *t >= clip.start && *t < clip.start + clip.dur).unwrap_or(clip.start),
        };
        let (ta, tb) = match a.take(&["range"]) {
            Some(r) => {
                let (x, y) = r.split_once('-').ok_or_else(|| anyhow!("range=a-b (timeline seconds)"))?;
                (self.time(x.trim())?, self.time(y.trim())?)
            }
            None => (clip.start, clip.start + clip.dur),
        };
        let (ta, tb) = (ta.max(clip.start), tb.min(clip.start + clip.dur));
        if tb <= ta {
            bail!("{} isn't on the timeline in that range ({}-{}s)", clip.id, fmt_t(clip.start), fmt_t(clip.start + clip.dur));
        }
        let at = at.clamp(ta, (tb - 1e-3).max(ta));
        let src = |t: f64| clip.src_in + (t - clip.start) * clip.speed;
        let mut opts = BTreeMap::new();
        opts.insert("t0".to_string(), f4(src(ta)));
        opts.insert("t1".to_string(), f4(src(tb)));
        opts.insert("from".to_string(), f4(src(at)));
        opts.insert("mw".to_string(), clip.media.w.to_string());
        opts.insert("mh".to_string(), clip.media.h.to_string());
        opts.insert("fps".to_string(), f4(if clip.media.fps > 0.0 { clip.media.fps } else { 30.0 }));
        let mode_given = a.take(&["mode", "m", "method"]).map(|s| s.to_ascii_lowercase());
        // the planar tracker for any model (it follows many features, not one patch)
        let planar_flag = a.flag("planar");
        if let Some(c) = a.take(&["corners", "pts"]) {
            let on = a.take(&["on", "target"]).ok_or_else(|| anyhow!("corners tracking needs on=<layer> (the layer whose corner pin follows)"))?;
            if to.as_deref().is_some_and(|t| t != "cornerpin" && t != "pin") {
                bail!("corners tracking drives a corner pin: to=cornerpin on=<layer>");
            }
            if matches!(mode_given.as_deref(), Some("points" | "point" | "4point" | "corners")) {
                // four separate points (the corners themselves must be trackable)
                opts.insert("corners".to_string(), c);
                return self.track_corners(&clip, &on, &opts, &src);
            }
            // the plane inside the four corners, corners carried by its perspective
            let quad = parse_points(&c)?;
            if quad.len() != 4 {
                bail!("corners needs four points 'x,y x,y x,y x,y' (fractions, clockwise from top-left)");
            }
            let mut po = opts.clone();
            po.insert("poly".to_string(), c.clone());
            po.insert("mode".to_string(), "perspective".into());
            let fits = self.run_planar(&clip, &po)?;
            let rows: Vec<Vec<f64>> = fits
                .iter()
                .map(|(t, m, conf)| {
                    let mut r = vec![*t];
                    for &(x, y) in &quad {
                        let (u, v) = crate::render::planar::apply(m, x, y);
                        r.push(u);
                        r.push(v);
                    }
                    r.push(*conf);
                    r
                })
                .collect();
            return self.key_corners(&clip, &on, &rows);
        }
        let mode = match mode_given.as_deref().map(Model::parse) {
            None => Model::Translate,
            Some(Some(m)) => m,
            Some(None) => bail!("mode= takes pos, rotation, similarity, affine or perspective"),
        };
        if let Some(m) = mask {
            return self.track_mask(&clip, &m, at, mode, planar_flag, opts);
        }
        let region = a.take(&["region", "r"]).ok_or_else(|| anyhow!(usage))?;
        opts.insert("region".to_string(), region);
        opts.insert("mode".to_string(), "pos".into());
        if matches!(to.as_deref(), Some("stabilize" | "stable" | "stabilise")) {
            if matches!(mode, Model::Rigid | Model::Similarity) {
                opts.insert("mode".to_string(), "similarity".into());
            }
            let smooth = a.take(&["smooth"]).map(|s| s.trim_end_matches('s').parse::<f64>()).transpose().map_err(|_| anyhow!("smooth=<seconds>"))?.unwrap_or(0.0);
            let zoom = a.flag("zoom") || a.take(&["zoom"]).is_some_and(|z| z != "0");
            return self.track_stabilize(&clip, &opts, smooth, zoom);
        }
        match to.as_deref() {
            Some("null") | None => self.track_null(&clip, &opts, mode, planar_flag),
            Some(o) => bail!("track {} region=... to=null (not {o}); for a corner pin use corners=... to=cornerpin on=<layer>; for a 3D camera to=camera", clip.id),
        }
    }

    /// Runs the planar tracker; `opts` holds poly= and mode=.
    fn run_planar(&mut self, clip: &Clip, opts: &BTreeMap<String, String>) -> Result<PlanarRows> {
        let v = self.env.analyze(&*self.p, &clip.media.id, "planar", opts)?;
        let rows: PlanarRows = v["samples"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| {
                        let x: Vec<f64> = r.as_array()?.iter().map(|n| n.as_f64().unwrap_or(0.0)).collect();
                        (x.len() >= 11).then(|| (x[0], [[x[1], x[2], x[3]], [x[4], x[5], x[6]], [x[7], x[8], x[9]]], x[10]))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if rows.is_empty() {
            bail!("the planar tracker decoded no frames for {}", clip.id);
        }
        Ok(rows)
    }

    pub(crate) fn track_clip(&self, id: &str) -> Result<Clip> {
        let l = self.relocate(id)?;
        let it = self.item_at(l);
        if it.kind != ItemKind::Clip {
            bail!("{id} is a {}; tracking needs a video clip", it.kind.name());
        }
        let media = it.src.as_deref().and_then(|s| self.p.media(s)).cloned().ok_or_else(|| anyhow!("{id} has no media"))?;
        if !media.has_video {
            bail!("{id} has no picture to track");
        }
        if it.reversed() || it.props.get("remap").is_some_and(|r| r.is_animated()) || it.props.get("hold").is_some_and(|h| h.v.bool()) {
            bail!("{id} is reversed, remapped or frozen; track before changing its timing");
        }
        Ok(Clip { id: it.id.clone(), loc: l, start: it.start, dur: it.dur, src_in: it.src_in, speed: it.speed().max(1e-6), media })
    }

    fn run_track(&mut self, clip: &Clip, opts: &BTreeMap<String, String>) -> Result<Vec<Vec<f64>>> {
        let v = self.env.analyze(self.p, &clip.media.id, "track", opts)?;
        let rows: Vec<Vec<f64>> = v["samples"].as_array().map(|a| a.iter().map(|r| r.as_array().map(|x| x.iter().map(|n| n.as_f64().unwrap_or(0.0)).collect()).unwrap_or_default()).collect()).unwrap_or_default();
        if rows.is_empty() {
            bail!("the tracker decoded no frames for {}", clip.id);
        }
        Ok(rows)
    }

    /// Reply line: what was tracked, how many keys, where it got lost.
    fn track_report(&mut self, what: &str, clip: &Clip, times: &[f64], conf: &[f64], keys: usize) {
        let tl = |s: f64| clip.start + (s - clip.src_in) / clip.speed;
        let mut line = format!("~{what} tracked {}-{}s, {keys} keys", fmt_t(tl(times[0])), fmt_t(tl(*times.last().unwrap())));
        let lost = lost_ranges(conf, LOST);
        for (a, b) in lost.iter().take(6) {
            let worst = conf[*a..=*b].iter().cloned().fold(1.0, f64::min);
            line.push_str(&format!("; !lost {}-{} (conf {:.2})", fmt_t(tl(times[*a])), fmt_t(tl(times[*b])), worst));
        }
        if !lost.is_empty() {
            line.push_str("; check those frames with preview view=overlay and fix by hand");
        }
        self.note(line);
    }

    /// After Effects' mask tracker. Position, rotation and similarity write
    /// the mask's placement (offset, scale, trot) so its shape keys (roto)
    /// stay separate; affine and perspective key the path itself (a shape
    /// mask becomes a path, as AE converts it).
    fn track_mask(&mut self, clip: &Clip, mid: &str, at: f64, mode: Model, planar: bool, mut opts: BTreeMap<String, String>) -> Result<()> {
        let it = self.item_at(clip.loc).clone();
        let mk = it.masks.iter().find(|m| m.id.eq_ignore_ascii_case(mid)).cloned().ok_or_else(|| anyhow!("{} has no mask {mid}", clip.id))?;
        if schema::is_pixel_mask(&mk.kind) {
            bail!("{}.{} is a {} qualifier; it already follows the picture", clip.id, mk.id, mk.kind);
        }
        if mk.kind == "gradient" && matches!(mode, Model::Affine | Model::Perspective) {
            bail!("{}.{} is a gradient window; it follows pos, rotation or similarity motion", clip.id, mk.id);
        }
        let tl0 = at - clip.start;
        let vc = self.vctx_of(clip.loc.0);
        let val = |name: &str| -> Val { mk.props.get(name).map(|a| a.keyed(tl0)).unwrap_or_else(|| schema::find(schema::MASK, name).map(|p| schema::default_val(p, vc)).unwrap_or_default()) };
        let (ox, oy) = val("offset").vec();
        let (s0, r0) = (val("scale").num(), val("trot").num());
        // region: the mask's center and size at `at`, placement included
        let (cx, cy, w, h) = if mk.kind == "path" {
            let vs = crate::render::shapes::path_vertices(&val("d").str());
            if vs.is_empty() {
                bail!("{}.{} has no path to track", clip.id, mk.id);
            }
            let (x0, x1) = vs.iter().fold((f64::MAX, f64::MIN), |a, v| (a.0.min(v.0), a.1.max(v.0)));
            let (y0, y1) = vs.iter().fold((f64::MAX, f64::MIN), |a, v| (a.0.min(v.1), a.1.max(v.1)));
            ((x0 + x1) / 2.0 + ox, (y0 + y1) / 2.0 + oy, (x1 - x0) * s0, (y1 - y0) * s0)
        } else {
            (val("x").num() + ox, val("y").num() + oy, val("w").num() * s0, val("h").num() * s0)
        };
        let (mw, mh) = (clip.media.w.max(1) as f64, clip.media.h.max(1) as f64);
        if matches!(mode, Model::Affine | Model::Perspective) {
            let d0 = mask_as_path(&mk.kind, &val, mw, mh).ok_or_else(|| anyhow!("{}.{} can't be turned into a path to track", clip.id, mk.id))?;
            let poly = outline_poly(&d0);
            if poly.len() < 3 {
                bail!("{}.{} is too small to track", clip.id, mk.id);
            }
            opts.insert("poly".into(), poly.iter().map(|(x, y)| format!("{},{}", f4(*x), f4(*y))).collect::<Vec<_>>().join(" "));
            opts.insert("mode".into(), mode.name().into());
            let fits = self.run_planar(clip, &opts)?;
            return self.key_mask_path(clip, &mk, &d0, &fits);
        }
        let sim = matches!(mode, Model::Similarity | Model::Rigid);
        let rows: Vec<Vec<f64>> = if planar || mode == Model::Rigid {
            // the planar tracker over the mask's own area
            let (hw, hh) = ((w / 2.0).clamp(0.01, 0.5), (h / 2.0).clamp(0.01, 0.5));
            let poly = format!("{},{} {},{} {},{} {},{}", f4(cx - hw), f4(cy - hh), f4(cx + hw), f4(cy - hh), f4(cx + hw), f4(cy + hh), f4(cx - hw), f4(cy + hh));
            let mut po = opts.clone();
            po.insert("poly".into(), poly);
            po.insert("mode".into(), mode.name().into());
            self.run_planar(clip, &po)?.iter().map(|(t, m, c)| vec![*t, m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], *c]).collect()
        } else {
            // track the inside of the shape, at most a quarter of the frame
            let (w, h) = ((w * 0.8).clamp(0.02, 0.5), (h * 0.8).clamp(0.02, 0.5));
            opts.insert("region".into(), format!("{},{},{},{}", f4(cx), f4(cy), f4(w), f4(h)));
            opts.insert("mode".into(), if sim { "similarity".into() } else { "pos".into() });
            self.run_track(clip, &opts)?
        };
        let times: Vec<f64> = rows.iter().map(|r| r[0]).collect();
        let conf: Vec<f64> = rows.iter().map(|r| r.get(7).copied().unwrap_or(0.0)).collect();
        let (mut xs, mut ys, mut ss, mut rs) = (vec![], vec![], vec![], vec![]);
        for r in &rows {
            let m = &r[1..7];
            xs.push(ox + m[0] * cx + m[1] * cy + m[2] - cx);
            ys.push(oy + m[3] * cx + m[4] * cy + m[5] - cy);
            // scale and rotation of the px-space matrix
            let (a, d) = (m[0], m[3] * mh / mw);
            ss.push(s0 * (a * a + d * d).sqrt());
            rs.push(r0 + d.atan2(a).to_degrees());
        }
        let keep = if sim { simplify(&times, &[xs.clone(), ys.clone(), ss.clone(), rs.clone()], &[0.0012, 0.0012, 0.004, 0.2]) } else { simplify(&times, &[xs.clone(), ys.clone()], &[0.0012, 0.0012]) };
        let local = |s: f64| (s - clip.src_in) / clip.speed;
        let target = Target { item: clip.id.clone(), sub: Some((SubKind::Mask, mk.id.clone())) };
        let key = |t: f64, v: Val| Key { t, v, e: Ease::Linear };
        let off: Vec<Key> = keep.iter().map(|&i| key(local(times[i]), Val::V(xs[i], ys[i]))).collect();
        self.target_anim_mut(&target, "offset", Val::V(ox, oy))?.set_keys(off);
        if sim {
            // rotation keeps the size: only similarity scales
            if mode == Model::Similarity {
                let sk: Vec<Key> = keep.iter().map(|&i| key(local(times[i]), Val::N(ss[i]))).collect();
                self.target_anim_mut(&target, "scale", Val::N(s0))?.set_keys(sk);
            }
            let rk: Vec<Key> = keep.iter().map(|&i| key(local(times[i]), Val::N(rs[i]))).collect();
            self.target_anim_mut(&target, "trot", Val::N(r0))?.set_keys(rk);
        }
        self.keep_lost(&target, clip, &times, &conf)?;
        self.track_report(&format!("{}.{} ({})", clip.id, mk.id, mode.name()), clip, &times, &conf, keep.len());
        Ok(())
    }

    /// Keys a mask's path from a planar track: `d0` (the shape at the drawn
    /// frame, placement baked in) carried by each frame's transform. Shape
    /// masks become path masks; their placement and rotation reset.
    fn key_mask_path(&mut self, clip: &Clip, mk: &Mask, d0: &str, fits: &PlanarRows) -> Result<()> {
        use crate::render::shapes;
        let times: Vec<f64> = fits.iter().map(|f| f.0).collect();
        let conf: Vec<f64> = fits.iter().map(|f| f.2).collect();
        let ds: Vec<String> = fits.iter().map(|(_, m, _)| shapes::map_points(d0, |x, y| crate::render::planar::apply(m, x, y)).unwrap_or_else(|| d0.to_string())).collect();
        // one channel per vertex coordinate, for key reduction
        let verts: Vec<Vec<(f64, f64)>> = ds.iter().map(|d| shapes::path_vertices(d)).collect();
        let nv = verts.first().map(|v| v.len()).unwrap_or(0);
        let mut chans: Vec<Vec<f64>> = vec![vec![]; nv * 2];
        for v in &verts {
            for k in 0..nv {
                let p = v.get(k).copied().unwrap_or((0.0, 0.0));
                chans[k * 2].push(p.0);
                chans[k * 2 + 1].push(p.1);
            }
        }
        let keep = if nv > 0 { simplify(&times, &chans, &vec![0.0012; nv * 2]) } else { (0..times.len()).collect() };
        let local = |s: f64| (s - clip.src_in) / clip.speed;
        let l = self.relocate(&clip.id)?;
        let it = self.item_mut_at(l);
        let m = it.masks.iter_mut().find(|m| m.id == mk.id).ok_or_else(|| anyhow!("{} lost its mask {}", clip.id, mk.id))?;
        let was = m.kind.clone();
        if m.kind != "path" {
            m.kind = "path".into();
            m.props.remove("expand");
        }
        for p in ["offset", "scale", "trot", "rot"] {
            m.props.remove(p);
        }
        let mut d = crate::model::anim::Anim::new(Val::S(d0.to_string()));
        d.set_keys(keep.iter().map(|&i| Key { t: local(times[i]), v: Val::S(ds[i].clone()), e: Ease::Linear }).collect());
        m.props.insert("d".into(), d);
        let target = Target { item: clip.id.clone(), sub: Some((SubKind::Mask, mk.id.clone())) };
        self.keep_lost(&target, clip, &times, &conf)?;
        if was != "path" {
            self.note(format!("~{}.{} {was} mask is now a path (its vertices follow the perspective)", clip.id, mk.id));
        }
        self.track_report(&format!("{}.{} (path, perspective)", clip.id, mk.id), clip, &times, &conf, keep.len());
        Ok(())
    }

    /// `track <clip> region=... to=null [mode=]`: a null that follows the
    /// region's position, and with rotation / similarity / perspective its
    /// rotation and scale too (After Effects' Track Motion with rotation and
    /// scale). Parent layers to it.
    fn track_null(&mut self, clip: &Clip, opts: &BTreeMap<String, String>, mode: Model, planar: bool) -> Result<()> {
        let reg: Vec<f64> = opts["region"].split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if reg.len() != 4 {
            bail!("region=cx,cy,w,h (fractions of the frame, center first, like masks)");
        }
        let (cx, cy) = (reg[0], reg[1]);
        let turns = mode != Model::Translate;
        // rows: [t, 2x3 transform, conf]
        let rows: Vec<Vec<f64>> = if turns || planar {
            let (hw, hh) = ((reg[2] / 2.0).clamp(0.01, 0.5), (reg[3] / 2.0).clamp(0.01, 0.5));
            let mut po = opts.clone();
            po.remove("region");
            po.insert("poly".into(), format!("{},{} {},{} {},{} {},{}", f4(cx - hw), f4(cy - hh), f4(cx + hw), f4(cy - hh), f4(cx + hw), f4(cy + hh), f4(cx - hw), f4(cy + hh)));
            po.insert("mode".into(), mode.name().into());
            let fits = self.run_planar(clip, &po)?;
            fits.iter()
                .map(|(t, m, c)| {
                    // the region center and a point right of it, as a local frame
                    let p0 = crate::render::planar::apply(m, cx, cy);
                    let p1 = crate::render::planar::apply(m, cx + 0.02, cy);
                    let p2 = crate::render::planar::apply(m, cx, cy + 0.02);
                    vec![*t, (p1.0 - p0.0) / 0.02, (p2.0 - p0.0) / 0.02, p0.0 - cx * (p1.0 - p0.0) / 0.02 - cy * (p2.0 - p0.0) / 0.02, (p1.1 - p0.1) / 0.02, (p2.1 - p0.1) / 0.02, p0.1 - cx * (p1.1 - p0.1) / 0.02 - cy * (p2.1 - p0.1) / 0.02, *c]
                })
                .collect()
        } else {
            self.run_track(clip, opts)?
        };
        let times: Vec<f64> = rows.iter().map(|r| r[0]).collect();
        let conf: Vec<f64> = rows.iter().map(|r| r.get(7).copied().unwrap_or(0.0)).collect();
        let tl = |s: f64| clip.start + (s - clip.src_in) / clip.speed;
        let it = self.item_at(clip.loc).clone();
        let comp = self.p.comps[clip.loc.0].clone();
        let (mut xs, mut ys, mut rs, mut ss) = (vec![], vec![], vec![], vec![]);
        let mut ref_dir: Option<(f64, f64)> = None;
        let mut last_rot = 0.0f64;
        for r in &rows {
            let m = &r[1..7];
            let at = |u: f64, v: f64| (m[0] * u + m[1] * v + m[2], m[3] * u + m[4] * v + m[5]);
            let g = crate::render::comp::layer_geom(&*self.p, &comp, &it, tl(r[0])).ok_or_else(|| anyhow!("{} has no picture at {}s", clip.id, fmt_t(tl(r[0]))))?;
            let (u, v) = at(cx, cy);
            let (x, y) = through_quad(&g.quad, u, v);
            xs.push(x);
            ys.push(y);
            // screen direction of a short step to the right of the tracked point
            let (u1, v1) = at(cx + 0.02, cy);
            let (x1, y1) = through_quad(&g.quad, u1, v1);
            let d = (x1 - x, y1 - y);
            let rd = *ref_dir.get_or_insert(d);
            let ang = (d.1.atan2(d.0) - rd.1.atan2(rd.0)).to_degrees();
            let ang = crate::render::linalg::unwrap_near(ang, last_rot);
            last_rot = ang;
            rs.push(ang);
            ss.push((d.0.hypot(d.1) / rd.0.hypot(rd.1).max(1e-9)).max(1e-6));
        }
        let keep = if turns { simplify(&times, &[xs.clone(), ys.clone(), rs.clone(), ss.clone()], &[0.75, 0.75, 0.05, 0.002]) } else { simplify(&times, &[xs.clone(), ys.clone()], &[0.75, 0.75]) };
        let (t0, t1) = (tl(times[0]), tl(*times.last().unwrap()));
        for c in super::lex::parse_script(&format!("null at={} dur={}", f4(t0), f4((t1 - t0).max(1.0 / comp.fps))))? {
            self.cmd(&c)?;
        }
        let nid = self.created.last().cloned().ok_or_else(|| anyhow!("couldn't make the null"))?;
        let target = Target { item: nid.clone(), sub: None };
        let keys: Vec<Key> = keep.iter().map(|&i| Key { t: tl(times[i]) - t0, v: Val::V(xs[i], ys[i]), e: Ease::Linear }).collect();
        self.target_anim_mut(&target, "pos", Val::V(xs[0], ys[0]))?.set_keys(keys);
        if turns {
            let rk: Vec<Key> = keep.iter().map(|&i| Key { t: tl(times[i]) - t0, v: Val::N(rs[i]), e: Ease::Linear }).collect();
            self.target_anim_mut(&target, "rot", Val::N(0.0))?.set_keys(rk);
            if mode != Model::Rigid {
                let sk: Vec<Key> = keep.iter().map(|&i| Key { t: tl(times[i]) - t0, v: Val::V(ss[i], ss[i]), e: Ease::Linear }).collect();
                self.target_anim_mut(&target, "scale", Val::V(1.0, 1.0))?.set_keys(sk);
            }
        }
        self.track_report(&format!("{} -> {nid} ({})", clip.id, mode.name()), clip, &times, &conf, keep.len());
        self.note(format!("  parent layers to it: parent <ids> {nid}"));
        Ok(())
    }

    /// Four NCC points, each tracked on its own (`mode=points`).
    fn track_corners(&mut self, clip: &Clip, on: &str, opts: &BTreeMap<String, String>, _src: &dyn Fn(f64) -> f64) -> Result<()> {
        let rows = self.run_track(clip, opts)?;
        self.key_corners(clip, on, &rows)
    }

    /// Corner pin keys on `on` from rows `[t, x1, y1, .. x4, y4, conf]` (frame fractions).
    fn key_corners(&mut self, clip: &Clip, on: &str, rows: &[Vec<f64>]) -> Result<()> {
        let tid = self.item_id(on)?;
        let times: Vec<f64> = rows.iter().map(|r| r[0]).collect();
        let conf: Vec<f64> = rows.iter().map(|r| r.get(9).copied().unwrap_or(0.0)).collect();
        let tl = |s: f64| clip.start + (s - clip.src_in) / clip.speed;
        let it = self.item_at(clip.loc).clone();
        let comp = self.p.comps[clip.loc.0].clone();
        let mut pts: Vec<Vec<f64>> = vec![vec![]; 8];
        for r in rows {
            let g = crate::render::comp::layer_geom(&*self.p, &comp, &it, tl(r[0])).ok_or_else(|| anyhow!("{} has no picture at {}s", clip.id, fmt_t(tl(r[0]))))?;
            for k in 0..4 {
                let (x, y) = through_quad(&g.quad, r[1 + k * 2], r[2 + k * 2]);
                pts[k * 2].push(x);
                pts[k * 2 + 1].push(y);
            }
        }
        // the target layer's corner pin (made if missing)
        let lt = self.relocate(&tid)?;
        let tstart = self.item_at(lt).start;
        let fid = match self.item_at(lt).fx.iter().find(|e| e.kind == "cornerpin") {
            Some(e) => e.id.clone(),
            None => {
                for c in super::lex::parse_script(&format!("fx {tid} cornerpin"))? {
                    self.cmd(&c)?;
                }
                let lt = self.relocate(&tid)?;
                self.item_at(lt).fx.iter().find(|e| e.kind == "cornerpin").map(|e| e.id.clone()).ok_or_else(|| anyhow!("couldn't add a corner pin to {tid}"))?
            }
        };
        let keep = simplify(&times, &pts, &[0.75; 8]);
        let target = Target { item: tid.clone(), sub: Some((SubKind::Fx, fid.clone())) };
        for (k, name) in ["tl", "tr", "br", "bl"].iter().enumerate() {
            let keys: Vec<Key> = keep.iter().map(|&i| Key { t: tl(times[i]) - tstart, v: Val::V(pts[k * 2][i], pts[k * 2 + 1][i]), e: Ease::Linear }).collect();
            self.target_anim_mut(&target, name, Val::V(pts[k * 2][0], pts[k * 2 + 1][0]))?.set_keys(keys);
        }
        self.track_report(&format!("{} corners -> {tid}.{fid}", clip.id), clip, &times, &conf, keep.len() * 4);
        Ok(())
    }

    /// `mask c1 trace`: auto-trace a mask path from the clip's luma, alpha or
    /// one color channel (After Effects' Auto-trace). With `range=a-b` it
    /// traces every `every=` seconds and keys the path (the shapes morph even
    /// when their vertex counts differ).
    pub fn mask_trace(&mut self, spec: &str, a: &mut A) -> Result<()> {
        let id = self.item_id(spec)?;
        let clip = self.track_clip(&id)?;
        let channel = a.take(&["channel", "ch", "from"]).unwrap_or_else(|| "luma".into()).to_ascii_lowercase();
        if !matches!(channel.as_str(), "luma" | "alpha" | "a" | "red" | "r" | "green" | "g" | "blue" | "b") {
            bail!("trace channel=luma|alpha|red|green|blue (not {channel})");
        }
        let thr = a
            .take(&["threshold", "thr"])
            .map(|s| s.trim_end_matches('%').parse::<f64>().map(|v| if v > 1.0 { v / 100.0 } else { v }))
            .transpose()
            .map_err(|_| anyhow!("threshold=0..1"))?
            .unwrap_or(0.5);
        let invert = a.flag("invert") || a.flag("inv");
        let window = a.flag("window");
        let tol = a.take(&["tolerance", "tol"]).map(|s| s.parse::<f64>()).transpose().map_err(|_| anyhow!("tolerance=<px>"))?.unwrap_or(1.5);
        let pick = a.take(&["pick", "seed"]);
        let smooth = a.take(&["smooth", "rotobezier"]).map(|s| !matches!(s.as_str(), "0" | "false" | "off")).unwrap_or(true);
        let every = a.take(&["every", "step"]).map(|s| s.trim_end_matches('s').parse::<f64>()).transpose().map_err(|_| anyhow!("every=<seconds>"))?.unwrap_or(0.5).max(1.0 / 60.0);
        let times: Vec<f64> = match a.take(&["range"]) {
            Some(r) => {
                let (x, y) = r.split_once('-').ok_or_else(|| anyhow!("range=a-b (timeline seconds)"))?;
                let (ta, tb) = (self.time(x.trim())?.max(clip.start), self.time(y.trim())?.min(clip.start + clip.dur - 1e-3));
                if tb < ta {
                    bail!("{} isn't on the timeline in that range", clip.id);
                }
                let n = (((tb - ta) / every).floor() as usize + 1).min(120);
                (0..n).map(|i| ta + i as f64 * every).collect()
            }
            None => {
                let t = match a.take(&["at"]) {
                    Some(s) => self.time(&s)?,
                    None => self.env.ui().playhead.filter(|t| *t >= clip.start && *t < clip.start + clip.dur).unwrap_or(clip.start),
                };
                vec![t.clamp(clip.start, (clip.start + clip.dur - 1e-3).max(clip.start))]
            }
        };
        let mut ds: Vec<(f64, String)> = vec![];
        let mut points = 0;
        for &t in &times {
            let mut opts = BTreeMap::new();
            opts.insert("t".to_string(), f4(clip.src_in + (t - clip.start) * clip.speed));
            opts.insert("mw".to_string(), clip.media.w.to_string());
            opts.insert("mh".to_string(), clip.media.h.to_string());
            opts.insert("channel".to_string(), channel.clone());
            opts.insert("threshold".to_string(), f4(thr));
            opts.insert("tol".to_string(), f4(tol));
            if invert {
                opts.insert("invert".to_string(), "1".into());
            }
            if let Some(p) = &pick {
                opts.insert("pick".to_string(), p.clone());
            }
            let v = self.env.analyze(self.p, &clip.media.id, "trace", &opts)?;
            let d = v["d"].as_str().unwrap_or_default().to_string();
            if d.is_empty() {
                continue;
            }
            points = points.max(v["points"].as_u64().unwrap_or(0));
            ds.push((t - clip.start, d));
        }
        let Some((_, d0)) = ds.first().cloned() else { bail!("nothing traced in {}", clip.id) };
        let l = clip.loc;
        let cx = self.vctx_of(l.0);
        let mut props = Props::new();
        for (k, v) in a.rest() {
            let p = schema::find(schema::MASK, &k).ok_or_else(|| anyhow!("unknown mask property '{k}'"))?;
            props.insert(p.name.to_string(), Anim::new(schema::clamp_val(p, schema::parse_val(p, &v, cx)?)));
        }
        let mut d = Anim::new(Val::S(d0));
        if ds.len() > 1 {
            d.set_keys(ds.iter().map(|(t, s)| Key { t: *t, v: Val::S(s.clone()), e: Ease::Linear }).collect());
        }
        props.insert("d".into(), d);
        if smooth {
            props.insert("smooth".into(), Anim::new(Val::B(true)));
        }
        if window {
            props.insert("cut".into(), Anim::new(Val::B(false)));
        }
        let it = self.item_mut_at(l);
        let mid = it.next_sub("m");
        it.masks.push(Effect { id: mid.clone(), kind: "path".into(), on: true, props });
        let keys = if ds.len() > 1 { format!(", {} keys", ds.len()) } else { String::new() };
        self.note(format!("+{id}.{mid} mask path traced from {channel} (up to {points} vertices{keys}){}", if window { " window" } else { "" }));
        self.note(format!("  check it with preview view=overlay id={id}.{mid}; refine=8 snaps the edge to the picture"));
        Ok(())
    }

    /// `track c1 region=… to=stabilize`: the clip's own position (and with
    /// mode=similarity its rotation and scale) keyed so the tracked feature
    /// holds still; `smooth=<s>` keeps camera moves slower than that and
    /// removes only the shake; `zoom` scales up to hide the moving edges.
    fn track_stabilize(&mut self, clip: &Clip, opts: &BTreeMap<String, String>, smooth: f64, zoom: bool) -> Result<()> {
        let reg: Vec<f64> = opts["region"].split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if reg.len() != 4 {
            bail!("region=cx,cy,w,h (fractions of the frame, center first, like masks)");
        }
        let it = self.item_at(clip.loc).clone();
        if it.parent.is_some() {
            bail!("{} is parented; unparent it before stabilizing", clip.id);
        }
        let sim = opts.get("mode").is_some_and(|m| m == "similarity");
        let rows = self.run_track(clip, opts)?;
        let times: Vec<f64> = rows.iter().map(|r| r[0]).collect();
        let conf: Vec<f64> = rows.iter().map(|r| r.get(7).copied().unwrap_or(0.0)).collect();
        let tl = |s: f64| clip.start + (s - clip.src_in) / clip.speed;
        let comp = self.p.comps[clip.loc.0].clone();
        let vc = self.vctx_of(clip.loc.0);
        let (cx, cy) = (reg[0], reg[1]);
        let (mw, mh) = (clip.media.w.max(1) as f64, clip.media.h.max(1) as f64);
        let from: f64 = opts.get("from").and_then(|v| v.parse().ok()).unwrap_or(times[0]);
        let r_ref = (0..times.len()).min_by(|&a, &b| (times[a] - from).abs().total_cmp(&(times[b] - from).abs())).unwrap_or(0);
        let angle = |m: &[f64]| (m[3] * mh / mw).atan2(m[0]);
        let size = |m: &[f64]| (m[0] * m[0] + (m[3] * mh / mw).powi(2)).sqrt().max(1e-6);
        let mut feats = Vec::with_capacity(rows.len());
        for (i, r) in rows.iter().enumerate() {
            let m = &r[1..7];
            let (u, v) = (m[0] * cx + m[1] * cy + m[2], m[3] * cx + m[4] * cy + m[5]);
            let g = crate::render::comp::layer_geom(self.p, &comp, &it, tl(times[i])).ok_or_else(|| anyhow!("{} has no picture at {}s", clip.id, fmt_t(tl(times[i]))))?;
            feats.push((through_quad(&g.quad, u, v), g.anchor));
        }
        let p_ref = feats[r_ref].0;
        let (a_ref, s_ref) = (angle(&rows[r_ref][1..7]), size(&rows[r_ref][1..7]));
        let (mut dx, mut dy, mut dr, mut ds) = (vec![], vec![], vec![], vec![]);
        for (i, ((px, py), (ax, ay))) in feats.iter().copied().enumerate() {
            let m = &rows[i][1..7];
            let (rot, sc) = if sim { (-(angle(m) - a_ref), s_ref / size(m)) } else { (0.0, 1.0) };
            // where the feature lands after the counter-rotation and scale
            // about the anchor, then the move that brings it back
            let (c, s) = (rot.cos(), rot.sin());
            let (vx, vy) = ((px - ax) * sc, (py - ay) * sc);
            let (qx, qy) = (ax + vx * c - vy * s, ay + vx * s + vy * c);
            dx.push(p_ref.0 - qx);
            dy.push(p_ref.1 - qy);
            dr.push(rot.to_degrees());
            ds.push(sc);
        }
        // smooth: keep the slow part of the motion (a moving average over
        // `smooth` seconds), remove only the shake
        if smooth > 0.0 && times.len() > 2 {
            let fps = (times.len() - 1) as f64 / (times[times.len() - 1] - times[0]).max(1e-6);
            let half = ((smooth * fps) / 2.0).round().max(1.0) as usize;
            let avg = |v: &[f64]| -> Vec<f64> {
                (0..v.len())
                    .map(|i| {
                        let (a, b) = (i.saturating_sub(half), (i + half).min(v.len() - 1));
                        v[a..=b].iter().sum::<f64>() / (b - a + 1) as f64
                    })
                    .collect()
            };
            for v in [&mut dx, &mut dy, &mut dr] {
                let slow = avg(v);
                v.iter_mut().zip(slow).for_each(|(x, s)| *x -= s);
            }
            let slow = avg(&ds);
            ds.iter_mut().zip(slow).for_each(|(x, s)| *x /= s.max(1e-6));
        }
        let (cw, ch) = (comp.w as f64, comp.h as f64);
        let zf = if zoom {
            let mx = dx.iter().fold(0.0f64, |m, v| m.max(v.abs())) / cw;
            let my = dy.iter().fold(0.0f64, |m, v| m.max(v.abs())) / ch;
            let mr = dr.iter().fold(0.0f64, |m, v| m.max(v.abs())).to_radians();
            1.0 + 2.0 * mx.max(my) + mr * 0.5 + 0.005
        } else {
            1.0
        };
        let local = |i: usize| tl(times[i]) - clip.start;
        let lv = |name: &str, t: f64| -> Val { it.props.get(name).map(|a| a.keyed(t)).unwrap_or_else(|| schema::find(schema::LAYER, name).map(|p| schema::default_val(p, vc)).unwrap_or_default()) };
        let (mut xs, mut ys, mut rs, mut sxs, mut sys) = (vec![], vec![], vec![], vec![], vec![]);
        for i in 0..rows.len() {
            let (px, py) = lv("pos", local(i)).vec();
            xs.push(px + dx[i]);
            ys.push(py + dy[i]);
            rs.push(lv("rot", local(i)).num() + dr[i]);
            let (sx, sy) = lv("scale", local(i)).vec();
            sxs.push(sx * ds[i] * zf);
            sys.push(sy * ds[i] * zf);
        }
        let keep = if sim { simplify(&times, &[xs.clone(), ys.clone(), rs.clone(), sxs.clone()], &[0.5, 0.5, 0.05, 0.001]) } else { simplify(&times, &[xs.clone(), ys.clone()], &[0.5, 0.5]) };
        let target = Target { item: clip.id.clone(), sub: None };
        let key = |i: usize, v: Val| Key { t: local(i), v, e: Ease::Linear };
        let pos_keys: Vec<Key> = keep.iter().map(|&i| key(i, Val::V(xs[i], ys[i]))).collect();
        self.target_anim_mut(&target, "pos", Val::V(xs[0], ys[0]))?.set_keys(pos_keys);
        if sim {
            let rot_keys: Vec<Key> = keep.iter().map(|&i| key(i, Val::N(rs[i]))).collect();
            self.target_anim_mut(&target, "rot", Val::N(rs[0]))?.set_keys(rot_keys);
        }
        if sim || zoom {
            let scale_keys: Vec<Key> = keep.iter().map(|&i| key(i, Val::V(sxs[i], sys[i]))).collect();
            let slot = self.target_anim_mut(&target, "scale", Val::V(sxs[0], sys[0]))?;
            if sim {
                slot.set_keys(scale_keys);
            } else if slot.k.is_empty() {
                slot.v = Val::V(sxs[0], sys[0]);
            } else {
                for k in slot.k.iter_mut() {
                    let (a, b) = k.v.vec();
                    k.v = Val::V(a * zf, b * zf);
                }
            }
        }
        let how = if smooth > 0.0 { format!("smoothed over {smooth}s") } else { "locked".to_string() };
        let zoomed = if zoom { format!(", zoom {:.0}%", zf * 100.0) } else { String::new() };
        self.track_report(&format!("{} stabilized ({how}{zoomed})", clip.id), clip, &times, &conf, keep.len());
        Ok(())
    }

    /// Remembers where a mask's track lost confidence (item-local seconds),
    /// so `check` can point at those frames after the edit.
    fn keep_lost(&mut self, target: &Target, clip: &Clip, times: &[f64], conf: &[f64]) -> Result<()> {
        let local = |s: f64| (s - clip.src_in) / clip.speed;
        let spec = lost_ranges(conf, LOST).iter().map(|(a, b)| format!("{}-{}", f4(local(times[*a])), f4(local(times[*b])))).collect::<Vec<_>>().join(" ");
        let slot = self.target_anim_mut(target, "trackLost", Val::S(String::new()))?;
        slot.k.clear();
        slot.v = Val::S(spec);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_mapping_is_bilinear() {
        let q = [(0.0, 0.0), (100.0, 0.0), (100.0, 50.0), (0.0, 50.0)];
        assert_eq!(through_quad(&q, 0.5, 0.5), (50.0, 25.0));
        assert_eq!(through_quad(&q, 1.0, 0.0), (100.0, 0.0));
    }
}
