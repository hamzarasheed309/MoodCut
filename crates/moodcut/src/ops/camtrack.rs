//! `camtrack`: the 3D camera tracker (After Effects' 3D Camera Tracker).
//!
//! ```text
//! camtrack c1 [range=a-b] [at=T] [fov=deg] [nodal] [points=500]   solve: a camera layer that moves like the real one, plus 3D track points
//! camtrack c1 points [t=T] [near=x,y] [n=20]                     the solved points on screen at t (p1 is the one seen longest)
//! camtrack c1 ground=p3,p7,p9 [origin=p3]                        the floor: re-orients the world; the camera and attached layers follow
//! camtrack c1 attach text 'Hello' at=p3,p7,p9 [size=1] [face=plane|camera] [as=name] [props]
//! camtrack c1 attach solid|null|shape <kind>|m4|<layer id> at=p12 ...
//! camtrack c1 del                                                  forgets the solve (the camera and layers stay)
//! ```
//!
//! The solve runs as a cached analysis of the clip's media (`render::sfm`).
//! Its result is ordinary keyframes on a one-node camera layer, and 3D
//! layers placed with ordinary position / rotation / scale values, so
//! everything stays editable. World units are comp px: with no ground set,
//! the camera at the reference frame is the comp's default camera and the
//! solved points sit around z = 0, so a layer attached there shows at its
//! natural size.

use super::exec::{Exec, A};
use super::exec_props::Target;
use crate::model::anim::{Ease, Key};
use crate::model::project::*;
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Val};
use crate::render::linalg::*;
use crate::render::sfm::{fit_plane, fov_of};
use crate::render::track::{lost_ranges, simplify};
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::BTreeMap;

/// A solve read back from the analysis.
pub(crate) struct Solve {
    /// Analysis frame size (px) and focal length.
    pub w: f64,
    pub h: f64,
    pub f: f64,
    pub rms: f64,
    pub nodal: bool,
    /// (source time, world-to-camera rotation and camera center) per frame.
    pub frames: Vec<(f64, Option<(M3, V3)>)>,
    /// Points: position (solve space), error px, frames seen. Numbered from 1.
    pub points: Vec<(V3, f64, usize)>,
}

pub(crate) fn read_solve(v: &Value) -> Result<Solve> {
    let num = |x: &Value| x.as_f64().unwrap_or(0.0);
    let frames: Vec<(f64, Option<(M3, V3)>)> = v["frames"]
        .as_array()
        .ok_or_else(|| anyhow!("the camera solve has no frames"))?
        .iter()
        .map(|r| {
            let a = r.as_array().cloned().unwrap_or_default();
            let t = a.first().map(num).unwrap_or(0.0);
            let pose = (a.len() >= 7).then(|| (rodrigues([num(&a[1]), num(&a[2]), num(&a[3])]), [num(&a[4]), num(&a[5]), num(&a[6])]));
            (t, pose)
        })
        .collect();
    let points: Vec<(V3, f64, usize)> = v["points"]
        .as_array()
        .map(|ps| {
            ps.iter()
                .filter_map(|p| {
                    let a = p.as_array()?;
                    (a.len() >= 5).then(|| ([num(&a[0]), num(&a[1]), num(&a[2])], num(&a[3]), a[4].as_u64().unwrap_or(0) as usize))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Solve { w: num(&v["w"]).max(1.0), h: num(&v["h"]).max(1.0), f: num(&v["f"]).max(1.0), rms: num(&v["rms"]), nodal: v["nodal"].as_bool().unwrap_or(false), frames, points })
}

/// Solve space to comp world: `a + s * r * (x - o)`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct World {
    a: V3,
    s: f64,
    r: M3,
    o: V3,
}

impl World {
    /// The mapping of a solve: by default the reference camera becomes the
    /// comp's default camera; with a ground plane, the floor origin sits at
    /// the comp center (z = 0) and the floor is horizontal (y down).
    pub fn of(cs: &CamSolve, cw: f64, ch: f64, zoom: f64) -> World {
        let s = cs.scale.unwrap_or(zoom).max(1e-6);
        match cs.ground.as_deref() {
            Some(g) if g.len() == 12 => World { a: [cw / 2.0, ch / 2.0, 0.0], s, r: [[g[0], g[1], g[2]], [g[3], g[4], g[5]], [g[6], g[7], g[8]]], o: [g[9], g[10], g[11]] },
            _ => World { a: [cw / 2.0, ch / 2.0, -zoom], s, r: I3, o: [0.0; 3] },
        }
    }
    pub fn point(&self, x: V3) -> V3 {
        add(self.a, scale(mulv(&self.r, sub(x, self.o)), self.s))
    }
    /// Camera-to-world rotation of a solved camera (`r` maps world to camera).
    pub fn cam_rot(&self, r: &M3) -> M3 {
        mul(&self.r, &tr(r))
    }
}

/// Solved points in comp world coordinates for the camera layer `cam`:
/// (number, position, error px, frames seen).
pub(crate) fn world_points(p: &Project, cam: &str, v: &Value) -> Option<Vec<(usize, V3, f64, usize)>> {
    let cs = p.camsolves.get(cam)?;
    let (ci, _, _) = p.locate(cam)?;
    let comp = &p.comps[ci];
    let zoom = p.item(cam).and_then(|c| c.props.get("zoom")).map(|a| a.v.num()).filter(|z| *z > 0.0).unwrap_or(comp.w as f64 * 0.926);
    let world = World::of(cs, comp.w as f64, comp.h as f64, zoom);
    let s = read_solve(v).ok()?;
    Some(s.points.iter().enumerate().map(|(i, (x, e, n))| (i + 1, world.point(*x), *e, *n)).collect())
}

/// Parses `p3,p7,p9` (or `3,7,9`) into point numbers.
fn point_list(s: &str, count: usize) -> Result<Vec<usize>> {
    let mut out = vec![];
    for t in s.split([',', ' ', ';']).filter(|t| !t.trim().is_empty()) {
        let t = t.trim();
        let n: usize = t.trim_start_matches(['p', 'P']).parse().map_err(|_| anyhow!("points are p1..p{count} (got '{t}')"))?;
        if n == 0 || n > count {
            bail!("no point {t}: the solve has p1..p{count} (camtrack <clip> points lists them)");
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if out.is_empty() {
        bail!("give points as at=p3,p7,p9");
    }
    Ok(out)
}

fn f4(v: f64) -> String {
    format!("{v:.4}")
}

impl<'a> Exec<'a> {
    /// `camtrack <clip> ...`
    pub fn c_camtrack(&mut self, a: &mut A) -> Result<()> {
        let usage = "camtrack <clip> [range=a-b] [at=T] [fov=deg] [nodal] | camtrack <clip> points [t=] [near=x,y] [n=] | camtrack <clip> ground=pA,pB,pC [origin=pA] | camtrack <clip> attach text 'Hi'|solid|null|shape <kind>|<media>|<layer> at=pA,pB,pC [size=] [face=plane|camera] | camtrack <clip> del";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let id = self.item_id(&spec)?;
        // a camera made by a solve stands for its clip
        let clip_id = self.p.camsolves.get(&id).map(|s| s.clip.clone()).unwrap_or_else(|| id.clone());
        let sub = a.pos.first().map(|s| s.to_ascii_lowercase()).filter(|s| matches!(s.as_str(), "points" | "pts" | "list" | "attach" | "place" | "create" | "ground" | "del" | "delete" | "rm" | "clear" | "solve" | "info"));
        if let Some(s) = &sub {
            a.shift();
            match s.as_str() {
                "points" | "pts" | "list" | "info" => return self.camtrack_points(&clip_id, a),
                "attach" | "place" | "create" => return self.camtrack_attach(&clip_id, a),
                "ground" => return self.camtrack_ground(&clip_id, a),
                "del" | "delete" | "rm" | "clear" => {
                    let cam = self.solve_of(&clip_id)?.0;
                    self.p.camsolves.remove(&cam);
                    self.note(format!("-{clip_id} camera solve (camera {cam} and attached layers kept)"));
                    return Ok(());
                }
                _ => {}
            }
        }
        if a.take(&["ground", "floor"]).map(|g| a.pos.insert(0, g)).is_some() {
            return self.camtrack_ground(&clip_id, a);
        }
        self.camtrack_solve(&clip_id, a)
    }

    /// The camera layer and solve record of a clip.
    fn solve_of(&self, clip: &str) -> Result<(String, CamSolve)> {
        self.p
            .camsolves
            .iter()
            .filter(|(cam, s)| s.clip == clip && self.p.locate(cam).is_some())
            .map(|(c, s)| (c.clone(), s.clone()))
            .next()
            .ok_or_else(|| anyhow!("{clip} has no camera solve yet: camtrack {clip}"))
    }

    /// The cached analysis of a solve.
    fn solve_data(&self, cs: &CamSolve) -> Result<Solve> {
        let v = self.env.analyze(&*self.p, &cs.media, "camtrack", &cs.opts)?;
        read_solve(&v)
    }

    /// Comp px per analysis px for a clip that fills the comp centered and
    /// still (the solve's principal point is the frame center).
    fn solve_frame_scale(&self, clip: &super::tracking::Clip, analysis_w: f64) -> Result<f64> {
        let it = self.item_at(clip.loc).clone();
        let comp = self.p.comps[clip.loc.0].clone();
        let (cw, ch) = (comp.w as f64, comp.h as f64);
        let mut k: Option<f64> = None;
        for t in [clip.start, clip.start + clip.dur * 0.5, clip.start + clip.dur - 1e-3] {
            let g = crate::render::comp::layer_geom(&*self.p, &comp, &it, t).ok_or_else(|| anyhow!("{} has no picture at {}s", clip.id, fmt_t(t)))?;
            let q = g.quad;
            let level = (q[1].1 - q[0].1).abs() < 0.5 && (q[3].0 - q[0].0).abs() < 0.5;
            let (cx, cy) = ((q[0].0 + q[2].0) / 2.0, (q[0].1 + q[2].1) / 2.0);
            let w = q[1].0 - q[0].0;
            if !level || w <= 0.0 || (cx - cw / 2.0).abs() > 1.0 || (cy - ch / 2.0).abs() > 1.0 {
                bail!(
                    "{} must sit centered, level and unflipped in the comp for a camera solve (its center is at {:.0},{:.0}); reset its pos/rot/scale or precomp it, then camtrack the precomp's clip",
                    clip.id,
                    cx,
                    cy
                );
            }
            let kk = w / analysis_w;
            if let Some(k0) = k {
                if (kk - k0).abs() > 1e-3 * k0 {
                    bail!("{} changes size over time; precomp it before camera tracking", clip.id);
                }
            }
            k = Some(kk);
        }
        k.ok_or_else(|| anyhow!("{} has no picture", clip.id))
    }

    /// `camtrack <clip> [range=] [at=] [fov=] [nodal] [points=]`: solves and
    /// (re)writes the camera layer.
    pub(crate) fn camtrack_solve(&mut self, id: &str, a: &mut A) -> Result<()> {
        let clip = self.track_clip(id)?;
        let at = match a.take(&["at", "from", "ref"]) {
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
        if let Some(f) = a.take(&["fov", "angle", "aov"]) {
            let v: f64 = f.trim_end_matches("deg").trim_end_matches('°').parse().map_err(|_| anyhow!("fov=<horizontal field of view in degrees>, e.g. fov=54"))?;
            if !(5.0..=160.0).contains(&v) {
                bail!("fov is the horizontal angle of view in degrees (5-160)");
            }
            opts.insert("fov".to_string(), fmt_num(v));
        }
        if a.flag("nodal") || a.flag("tripod") || a.take(&["nodal"]).is_some_and(|v| v != "0") {
            opts.insert("nodal".to_string(), "1".into());
        }
        if let Some(n) = a.take(&["points", "features", "detail"]) {
            let v: f64 = n.parse().map_err(|_| anyhow!("points=<features to track, 100-1500>"))?;
            opts.insert("points".to_string(), fmt_num(v.clamp(100.0, 1500.0)));
        }
        let v = self.env.analyze(&*self.p, &clip.media.id, "camtrack", &opts)?;
        let solve = read_solve(&v)?;
        let k = self.solve_frame_scale(&clip, solve.w)?;
        let zoom = solve.f * k;
        // the camera layer: the one an earlier solve of this clip made, or a new one
        let cam_id = match self.solve_of(&clip.id) {
            Ok((c, _)) => c,
            Err(_) => {
                let comp_id = self.p.comps[clip.loc.0].id.clone();
                let made = self.run_sub(&comp_id, &format!("camera at={} dur={} aim=0 as='3D Tracker Camera'", f4(ta), f4(tb - ta)), true)?;
                made.last().cloned().ok_or_else(|| anyhow!("couldn't make the camera layer"))?
            }
        };
        let prev = self.p.camsolves.get(&cam_id).cloned();
        let cs = CamSolve {
            clip: clip.id.clone(),
            media: clip.media.id.clone(),
            opts,
            // a new solve has a new solve space: the old ground and placements don't apply
            ground: None,
            scale: None,
            attached: prev.map(|p| p.attached).unwrap_or_default(),
        };
        self.p.camsolves.insert(cam_id.clone(), cs.clone());
        let keys = self.bake_camera(&cam_id, &cs, &solve, &clip, zoom)?;
        let solved: Vec<usize> = (0..solve.frames.len()).filter(|&i| solve.frames[i].1.is_some()).collect();
        let tl = |s: f64| clip.start + (s - clip.src_in) / clip.speed;
        let (first, last) = (solved.first().map(|&i| tl(solve.frames[i].0)).unwrap_or(ta), solved.last().map(|&i| tl(solve.frames[i].0)).unwrap_or(tb));
        self.note(format!(
            "+{cam_id} camera solved from {}: {}-{}s, {} points, error {:.2} px, zoom {:.0} px (fov {:.0} deg){}, {keys} keys",
            clip.id,
            fmt_t(first),
            fmt_t(last),
            solve.points.len(),
            solve.rms,
            zoom,
            fov_of(solve.f, solve.w),
            if solve.nodal { ", tripod (rotation only)" } else { "" }
        ));
        let conf: Vec<f64> = solve.frames.iter().map(|f| if f.1.is_some() { 1.0 } else { 0.0 }).collect();
        for (x, y) in lost_ranges(&conf, 0.5).iter().take(4) {
            self.warn(format!("{} unsolved {}-{}s (the camera holds its neighbors' path there): try a range without it or more detail", clip.id, fmt_t(tl(solve.frames[*x].0)), fmt_t(tl(solve.frames[*y].0))));
        }
        if solve.rms > 1.5 {
            self.warn(format!("{}: solve error {:.1} px is high (moving objects, rolling shutter, a zoom or a lens change); try range= on a steadier stretch or fov= if you know the lens", clip.id, solve.rms));
        }
        if solve.nodal {
            self.note("  tripod shot: the camera only turns, so the points sit at one distance; ground= doesn't apply, attach still works for far things (sky, signs, walls)".to_string());
        }
        if !self.p.camsolves.get(&cam_id).map(|c| c.attached.is_empty()).unwrap_or(true) {
            self.note("  layers attached to the old solve keep their places; attach them again to follow this one".to_string());
            if let Some(c) = self.p.camsolves.get_mut(&cam_id) {
                c.attached.clear();
            }
        }
        self.note(format!(
            "  next: preview view=points id={} t=<time> shows the numbered points; camtrack {} ground=pA,pB,pC sets the floor; camtrack {} attach text 'Title' at=pA,pB,pC places a 3D layer on them",
            clip.id, clip.id, clip.id
        ));
        Ok(())
    }

    /// Writes the solved path to the camera layer. Returns the key count.
    fn bake_camera(&mut self, cam_id: &str, cs: &CamSolve, solve: &Solve, clip: &super::tracking::Clip, zoom: f64) -> Result<usize> {
        let comp = &self.p.comps[clip.loc.0];
        let (cw, ch) = (comp.w as f64, comp.h as f64);
        let world = World::of(cs, cw, ch, zoom);
        let cl = self.relocate(cam_id)?;
        let cam_start = self.item_at(cl).start;
        let tl = |s: f64| clip.start + (s - clip.src_in) / clip.speed;
        let (mut ts, mut xs, mut ys, mut zs, mut rxs, mut rys, mut rzs) = (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
        let mut near: Option<V3> = None;
        for (t, pose) in &solve.frames {
            let Some((r, c)) = pose else { continue };
            let wc = world.point(*c);
            let e = to_euler(&world.cam_rot(r), near);
            near = Some(e);
            ts.push(tl(*t) - cam_start);
            xs.push(wc[0]);
            ys.push(wc[1]);
            zs.push(wc[2] + zoom);
            rxs.push(e[0]);
            rys.push(e[1]);
            rzs.push(e[2]);
        }
        if ts.is_empty() {
            bail!("the solve has no camera positions");
        }
        let keep = simplify(&ts, &[xs.clone(), ys.clone(), zs.clone(), rxs.clone(), rys.clone(), rzs.clone()], &[0.2, 0.2, 0.2, 0.01, 0.01, 0.01]);
        let target = Target { item: cam_id.to_string(), sub: None };
        let key = |i: usize, v: Val| Key { t: ts[i], v, e: Ease::Linear };
        let pos: Vec<Key> = keep.iter().map(|&i| key(i, Val::V(xs[i], ys[i]))).collect();
        self.target_anim_mut(&target, "pos", Val::V(xs[0], ys[0]))?.set_keys(pos);
        for (name, vals) in [("z", &zs), ("rx", &rxs), ("ry", &rys), ("rot", &rzs)] {
            let ks: Vec<Key> = keep.iter().map(|&i| key(i, Val::N(vals[i]))).collect();
            self.target_anim_mut(&target, name, Val::N(vals[0]))?.set_keys(ks);
        }
        let z = self.target_anim_mut(&target, "zoom", Val::N(zoom))?;
        z.k.clear();
        z.v = Val::N(zoom);
        let aim = self.target_anim_mut(&target, "aim", Val::B(false))?;
        aim.k.clear();
        aim.v = Val::B(false);
        Ok(keep.len())
    }

    /// `camtrack <clip> points [t=] [near=x,y] [n=20]`
    fn camtrack_points(&mut self, clip_id: &str, a: &mut A) -> Result<()> {
        let (cam, cs) = self.solve_of(clip_id)?;
        let solve = self.solve_data(&cs)?;
        let clip = self.track_clip(clip_id)?;
        let t = match a.take(&["t", "at"]) {
            Some(s) => self.time(&s)?,
            None => self.env.ui().playhead.filter(|t| *t >= clip.start && *t < clip.start + clip.dur).unwrap_or(clip.start + clip.dur / 2.0),
        };
        let src = clip.src_in + (t - clip.start) * clip.speed;
        let near = match a.take(&["near", "around"]) {
            Some(s) => {
                let (x, y) = s.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))).ok_or_else(|| anyhow!("near=x,y (fractions of the frame)"))?;
                Some((x, y))
            }
            None => None,
        };
        let n: usize = a.take(&["n", "count"]).map(|s| s.parse()).transpose().map_err(|_| anyhow!("n=<count>"))?.unwrap_or(20).clamp(1, 200);
        // the solved frame nearest t
        let fi = (0..solve.frames.len()).filter(|&i| solve.frames[i].1.is_some()).min_by(|&x, &y| (solve.frames[x].0 - src).abs().total_cmp(&(solve.frames[y].0 - src).abs()));
        let Some(fi) = fi else { bail!("{clip_id}'s solve has no solved frames") };
        let (r, c) = solve.frames[fi].1.unwrap();
        let mut vis: Vec<(usize, f64, f64, f64, usize)> = vec![];
        for (i, (x, e, views)) in solve.points.iter().enumerate() {
            let xc = mulv(&r, sub(*x, c));
            if xc[2] <= 1e-6 {
                continue;
            }
            let (u, v) = ((solve.w / 2.0 + solve.f * xc[0] / xc[2]) / solve.w, (solve.h / 2.0 + solve.f * xc[1] / xc[2]) / solve.h);
            if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                vis.push((i + 1, u, v, *e, *views));
            }
        }
        match near {
            Some((x, y)) => vis.sort_by(|p, q| ((p.1 - x).powi(2) + (p.2 - y).powi(2)).total_cmp(&((q.1 - x).powi(2) + (q.2 - y).powi(2)))),
            None => vis.sort_by(|p, q| q.4.cmp(&p.4)),
        }
        let total = vis.len();
        let lines: Vec<String> = vis.iter().take(n).map(|(k, u, v, e, views)| format!("p{k} {:.3},{:.3} seen {views}f err {:.2}px", u, v, e)).collect();
        self.note(format!(
            "{clip_id} solve ({cam}, error {:.2}px, lens {:.1}° horizontal{}) at {}s: {total} of {} points on screen{}:\n{}",
            solve.rms,
            fov_of(solve.f, solve.w),
            if solve.nodal { ", tripod" } else { "" },
            fmt_t(t),
            solve.points.len(),
            if near.is_some() { ", nearest first" } else { ", longest-seen first" },
            lines.join("\n")
        ));
        Ok(())
    }

    /// `camtrack <clip> ground=pA,pB,pC [origin=pA]`: the plane through the
    /// points becomes the floor (horizontal, y down), the origin (or their
    /// center) the comp center at z = 0.
    fn camtrack_ground(&mut self, clip_id: &str, a: &mut A) -> Result<()> {
        let (cam, mut cs) = self.solve_of(clip_id)?;
        let solve = self.solve_data(&cs)?;
        let spec = a.take(&["ground", "floor", "plane"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("camtrack {clip_id} ground=pA,pB,pC (three or more points on the floor)"))?;
        if matches!(spec.to_ascii_lowercase().as_str(), "off" | "none" | "reset") {
            cs.ground = None;
        } else {
            if solve.nodal {
                bail!("{clip_id} is a tripod solve: every point sits at one distance, so there is no floor to find");
            }
            let ids = point_list(&spec, solve.points.len())?;
            if ids.len() < 3 {
                bail!("a ground plane needs three or more points (got {})", ids.len());
            }
            let pts: Vec<V3> = ids.iter().map(|&k| solve.points[k - 1].0).collect();
            let (c, mut n) = fit_plane(&pts).ok_or_else(|| anyhow!("those points are in a line; pick points that span an area of the floor"))?;
            // the normal points up: toward the reference camera (the solve origin)
            if dot(n, sub([0.0; 3], c)) < 0.0 {
                n = scale(n, -1.0);
            }
            let down = scale(n, -1.0);
            let mut fwd = sub([0.0, 0.0, 1.0], scale(n, n[2]));
            if norm(fwd) < 1e-3 {
                fwd = sub([1.0, 0.0, 0.0], scale(n, n[0]));
            }
            let fwd = unit(fwd);
            let right = cross(down, fwd);
            let origin = match a.take(&["origin", "o"]) {
                Some(o) => solve.points[point_list(&o, solve.points.len())?[0] - 1].0,
                None => c,
            };
            let mut g: Vec<f64> = vec![];
            for row in [right, down, fwd] {
                g.extend_from_slice(&row);
            }
            g.extend_from_slice(&origin);
            cs.ground = Some(g);
        }
        self.p.camsolves.insert(cam.clone(), cs.clone());
        let clip = self.track_clip(clip_id)?;
        let zoom = self.relocate(&cam).ok().and_then(|l| self.item_at(l).props.get("zoom").map(|z| z.v.num())).filter(|z| *z > 0.0).unwrap_or(solve.f);
        let keys = self.bake_camera(&cam, &cs, &solve, &clip, zoom)?;
        let mut moved = 0;
        for at in cs.attached.clone() {
            if self.p.locate(&at.item).is_some() {
                self.place_on_points(&cam, &cs, &solve, zoom, &at.item, &at.pts, &at.face, at.size)?;
                moved += 1;
            }
        }
        let what = if cs.ground.is_some() { format!("floor set from {spec}") } else { "floor cleared".to_string() };
        self.note(format!("~{cam} {what}: camera re-keyed ({keys} keys){}", if moved > 0 { format!(", {moved} attached layers moved with it") } else { String::new() }));
        Ok(())
    }

    /// Positions, turns and scales `item` onto solved points.
    #[allow(clippy::too_many_arguments)]
    fn place_on_points(&mut self, cam: &str, cs: &CamSolve, solve: &Solve, zoom: f64, item: &str, pts: &[usize], face: &str, size: f64) -> Result<()> {
        let l = self.relocate(cam)?;
        let comp = &self.p.comps[l.0];
        let world = World::of(cs, comp.w as f64, comp.h as f64, zoom);
        let wp: Vec<V3> = pts.iter().map(|&k| world.point(solve.points[k - 1].0)).collect();
        let n = wp.len() as f64;
        let p = scale(wp.iter().fold([0.0; 3], |s, x| add(s, *x)), 1.0 / n);
        // the reference camera (solve space origin) in the world
        let cam_c = world.point([0.0; 3]);
        let cam_r = world.cam_rot(&I3);
        let col = |m: &M3, k: usize| [m[0][k], m[1][k], m[2][k]];
        let (cam_down, cam_fwd) = (col(&cam_r, 1), col(&cam_r, 2));
        let plane = if face != "camera" && wp.len() >= 3 { fit_plane(&wp) } else { None };
        let rl: M3 = match plane {
            Some((_, mut nrm)) => {
                if dot(nrm, sub(cam_c, p)) < 0.0 {
                    nrm = scale(nrm, -1.0);
                }
                let proj = |v: V3| sub(v, scale(nrm, dot(v, nrm)));
                let mut d = proj(cam_down);
                if norm(d) < 0.3 {
                    d = scale(proj(cam_fwd), -1.0);
                }
                let y = unit(d);
                let z = scale(nrm, -1.0);
                let x = cross(y, z);
                [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
            }
            None => cam_r,
        };
        let e = to_euler(&rl, None);
        let depth = dot(sub(p, cam_c), cam_fwd).max(1.0);
        let k = size * depth / zoom.max(1e-6);
        let kv: Vec<(String, String)> = vec![
            ("3d".into(), "1".into()),
            ("pos".into(), format!("{},{}", fmt_num((p[0] * 100.0).round() / 100.0), fmt_num((p[1] * 100.0).round() / 100.0))),
            ("z".into(), fmt_num((p[2] * 100.0).round() / 100.0)),
            ("rx".into(), fmt_num((e[0] * 1000.0).round() / 1000.0)),
            ("ry".into(), fmt_num((e[1] * 1000.0).round() / 1000.0)),
            ("rot".into(), fmt_num((e[2] * 1000.0).round() / 1000.0)),
            ("scale".into(), fmt_num((k * 10000.0).round() / 10000.0)),
        ];
        self.apply_props(item, &kv)
    }

    /// `camtrack <clip> attach <what> at=pA[,pB,pC] [size=1] [face=plane|camera] [as=] [props]`
    fn camtrack_attach(&mut self, clip_id: &str, a: &mut A) -> Result<()> {
        let usage = "camtrack <clip> attach text 'Title'|solid [#color]|null|shape <kind>|<media>|<layer id> at=pA,pB,pC [size=1] [face=plane|camera]";
        let (cam, cs) = self.solve_of(clip_id)?;
        let solve = self.solve_data(&cs)?;
        let at = a.take(&["at", "on", "points", "pts"]).ok_or_else(|| anyhow!("attach needs at=pA[,pB,pC] (see camtrack {clip_id} points or preview view=points id={clip_id})"))?;
        let pts = point_list(&at, solve.points.len())?;
        let size = a.take(&["size", "k"]).map(|s| s.trim_end_matches('%').parse::<f64>().map(|v| if s.ends_with('%') { v / 100.0 } else { v })).transpose().map_err(|_| anyhow!("size=<multiplier of the natural size>"))?.unwrap_or(1.0);
        let face = a.take(&["face", "facing", "orient"]).unwrap_or_else(|| if pts.len() >= 3 { "plane".into() } else { "camera".into() }).to_ascii_lowercase();
        if !matches!(face.as_str(), "plane" | "camera") {
            bail!("face=plane (lie on the points' surface) or face=camera");
        }
        let alias = a.take(&["as", "name"]);
        let what = a.shift().ok_or_else(|| anyhow!(usage))?;
        let cl = self.relocate(&cam)?;
        let (start, dur) = {
            let c = self.item_at(cl);
            (c.start, c.dur)
        };
        let comp_id = self.p.comps[cl.0].id.clone();
        let timing = format!("at={} dur={}", f4(start), f4(dur));
        let script = match what.to_ascii_lowercase().as_str() {
            "text" | "title" => {
                let content = a.shift().ok_or_else(|| anyhow!("attach text '<words>' at=..."))?;
                Some(format!("text {} {timing}", super::lex::quote(&content)))
            }
            "solid" => {
                let color = a.pos.first().filter(|c| c.starts_with('#')).cloned();
                if color.is_some() {
                    a.shift();
                }
                Some(format!("solid {} w=400 h=400 {timing}", color.unwrap_or_else(|| "#ffffff".into())))
            }
            "null" => Some(format!("null {timing}")),
            "shape" => {
                let kind = a.shift().unwrap_or_else(|| "rect".into());
                Some(format!("shape {kind} {timing}"))
            }
            _ => None,
        };
        let item = match script {
            Some(s) => {
                let made = self.run_sub(&comp_id, &s, true)?;
                made.last().cloned().ok_or_else(|| anyhow!("couldn't make the layer to attach"))?
            }
            None => {
                // media from the library, or a layer that exists
                if self.p.media(&what).is_some() {
                    let made = self.run_sub(&comp_id, &format!("add {what} {timing}"), true)?;
                    made.last().cloned().ok_or_else(|| anyhow!("couldn't add {what}"))?
                } else {
                    let id = self.item_id(&what).map_err(|_| anyhow!("attach what? {usage}"))?;
                    if id == clip_id || id == cam {
                        bail!("attach another layer, not the tracked clip or its camera");
                    }
                    id
                }
            }
        };
        if let Some(n) = alias {
            let l = self.relocate(&item)?;
            self.item_mut_at(l).name = Some(n);
        }
        // adding a layer can move items around: find the camera again
        let cl = self.relocate(&cam)?;
        let zoom = self.item_at(cl).props.get("zoom").map(|z| z.v.num()).filter(|z| *z > 0.0).unwrap_or(solve.f);
        self.place_on_points(&cam, &cs, &solve, zoom, &item, &pts, &face, size)?;
        let rest = a.rest();
        if !rest.is_empty() {
            self.apply_props(&item, &rest)?;
        }
        if let Some(c) = self.p.camsolves.get_mut(&cam) {
            c.attached.retain(|x| x.item != item);
            c.attached.push(Attached { item: item.clone(), pts: pts.clone(), face: face.clone(), size });
        }
        let names: Vec<String> = pts.iter().map(|k| format!("p{k}")).collect();
        self.note(format!("+{item} attached to {} ({}, 3D, follows {cam})", names.join(","), if face == "plane" { "lying on their plane" } else { "facing the camera" }));
        if !self.created.contains(&item) {
            self.created.push(item);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_mapping_puts_the_reference_camera_at_the_default() {
        let cs = CamSolve::default();
        let w = World::of(&cs, 1920.0, 1080.0, 1777.0);
        let c = w.point([0.0; 3]);
        assert!((c[0] - 960.0).abs() < 1e-9 && (c[1] - 540.0).abs() < 1e-9 && (c[2] + 1777.0).abs() < 1e-9);
        // a point at depth 1 straight ahead lands on the comp's z = 0 plane
        let p = w.point([0.0, 0.0, 1.0]);
        assert!(p[2].abs() < 1e-9);
        assert_eq!(point_list("p3, p7,p3", 10).unwrap(), vec![3, 7]);
        assert!(point_list("p11", 10).is_err());
    }
}
