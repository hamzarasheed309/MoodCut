//! Premiere Pro / After Effects editing tools as MoodCut Script commands:
//! rolling edits, lift / extract, rearrange (swap), paste attributes, stagger
//! and sequence layers, align / distribute, the keyframe assistant (easy ease,
//! time-reverse, bake), nulls and cameras, the work area and Essential Sound.
//! Each one is a single short line for an AI and a single gesture in the GUI,
//! and all of them go through the same atomic, undoable script path.

use super::exec::{parse_track_spec, Exec, Mode, A, EPS};
use super::exec_props::{find_in, sub_list_mut, unknown_prop, Target};
use crate::model::anim::{Anim, Ease, Key};
use crate::model::project::*;
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Val};
use crate::render::comp::{layer_geom, Geo};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

/// Transform properties copied by `paste ... transform`.
const TRANSFORM_PROPS: &[&str] = &["pos", "scale", "rot", "anchor", "rx", "ry", "z", "skew", "3d"];
const OPACITY_PROPS: &[&str] = &["opacity", "blend"];
const AUDIO_PROPS: &[&str] = &["vol", "pan", "mute", "fadeIn", "fadeOut"];
/// Audio effects that `sound` manages (removed again when the type changes).
const SOUND_AFX: &[&str] = &["voice", "deess"];

/// A signed relative time like `+0.5`, `-12f` or `0.5` (positive).
fn signed(ex: &Exec, s: &str) -> Result<f64> {
    let neg = s.trim().starts_with('-');
    let v = ex.time(s.trim().trim_start_matches(['+', '-']))?;
    Ok(if neg { -v } else { v })
}

pub(crate) fn parse_factor(s: &str) -> Result<f64> {
    let l = s.trim().to_ascii_lowercase();
    let v = match l.strip_suffix('%') {
        Some(p) => p.parse::<f64>().map(|x| x / 100.0),
        None => l.trim_end_matches('x').parse::<f64>(),
    }
    .map_err(|_| anyhow!("bad factor '{s}' (use 2, 2x or 200%)"))?;
    if !(v.is_finite() && v > 0.0) {
        bail!("factor must be positive");
    }
    Ok(v)
}

/// Moves every value of a 2D anim (static value and keyframes) by (dx, dy).
fn offset_vec(a: &mut Anim, dx: f64, dy: f64) {
    let shift = |v: &mut Val| {
        if let Val::V(x, y) = v {
            *x += dx;
            *y += dy;
        }
    };
    shift(&mut a.v);
    for k in a.k.iter_mut() {
        shift(&mut k.v);
    }
}

impl<'a> Exec<'a> {
    // ------------------------------------------------------------ selectors

    /// Item selectors usable anywhere ids are: `sel` (the editor's selection),
    /// `after:T`, `before:T`, `at:T`, `kind:text`, `name:intro`, each optionally
    /// limited to a track (`V2:after:10`). None when `part` isn't a selector.
    pub(crate) fn select(&self, part: &str) -> Result<Option<Vec<String>>> {
        let l = part.to_ascii_lowercase();
        if matches!(l.as_str(), "sel" | "selected" | "selection") {
            let comp = self.comp();
            let ids: Vec<String> = self.env.ui().selection.into_iter().filter(|id| comp.items().any(|i| &i.id == id)).collect();
            if ids.is_empty() {
                bail!("nothing is selected in the editor (select items in the timeline, or name them)");
            }
            return Ok(Some(ids));
        }
        let (track, rest) = match part.split_once(':') {
            Some((t, r)) if parse_track_spec(t).is_some() && self.comp().track(t).is_some() => (self.comp().track(t).map(|x| x.id.clone()), r),
            _ => (None, part),
        };
        let Some((key, val)) = rest.split_once(':') else { return Ok(None) };
        let key = key.to_ascii_lowercase();
        if !matches!(key.as_str(), "after" | "before" | "at" | "kind" | "name") {
            return Ok(None);
        }
        let t = if matches!(key.as_str(), "after" | "before" | "at") { self.time(val)? } else { 0.0 };
        let want = val.to_ascii_lowercase();
        let mut ids = vec![];
        for tr in &self.comp().tracks {
            if track.as_ref().is_some_and(|x| x != &tr.id) {
                continue;
            }
            for it in &tr.items {
                let hit = match key.as_str() {
                    "after" => it.start >= t - EPS,
                    "before" => it.end() <= t + EPS,
                    "at" => it.start <= t + EPS && t < it.end() - EPS,
                    "kind" => self.kind_matches(it, &want)?,
                    _ => it.name.as_deref().is_some_and(|n| n.to_ascii_lowercase().contains(&want)) || it.text.as_deref().is_some_and(|x| x.to_ascii_lowercase().contains(&want)),
                };
                if hit {
                    ids.push(it.id.clone());
                }
            }
        }
        if ids.is_empty() {
            bail!("nothing matches '{part}'");
        }
        Ok(Some(ids))
    }

    fn kind_matches(&self, it: &Item, k: &str) -> Result<bool> {
        let media = || it.src.as_deref().and_then(|s| self.p.media(s)).filter(|_| it.kind == ItemKind::Clip);
        Ok(match k {
            "clip" | "clips" | "media" => it.kind == ItemKind::Clip,
            "video" => media().is_some_and(|m| m.kind == MediaKind::Video && m.gfx.is_none()),
            "audio" => media().is_some_and(|m| m.kind == MediaKind::Audio),
            "image" | "still" | "photo" => media().is_some_and(|m| m.is_still() && m.gfx.is_none()),
            "graphic" | "html" => media().is_some_and(|m| m.gfx.is_some()),
            "text" | "title" => it.kind == ItemKind::Text,
            "shape" => it.kind == ItemKind::Shape,
            "solid" => it.kind == ItemKind::Solid,
            "adjust" | "adjustment" => it.kind == ItemKind::Adjust,
            "comp" | "nested" | "precomp" => it.kind == ItemKind::Comp,
            "null" => it.kind == ItemKind::Null,
            "camera" => it.kind == ItemKind::Camera,
            other => bail!("unknown kind '{other}' (clip video audio image graphic text shape solid adjust comp null camera)"),
        })
    }

    /// The editor's playhead when it falls inside `it`, else the item's start.
    fn measure_time(&self, it: &Item, at: Option<f64>) -> f64 {
        match at.or(self.env.ui().playhead) {
            Some(t) if t >= it.start - EPS && t < it.end() - EPS => t,
            Some(t) if at.is_some() => t,
            _ => it.start,
        }
    }

    /// Removes an item from its track without detaching layers that refer to
    /// it (parent, matte), for moves that put it back.
    pub(crate) fn take_item(&mut self, id: &str) -> Result<Item> {
        let (c, t, i) = self.relocate(id)?;
        Ok(self.p.comps[c].tracks[t].items.remove(i))
    }

    // ------------------------------------------------------------ null & camera

    pub fn c_null(&mut self, a: &mut A, camera: bool) -> Result<()> {
        let track = self.take_track(a);
        let comp_dur = self.comp().duration();
        let (start, dur) = self.overlay_start_dur(a, if comp_dur > 0.0 { comp_dur } else { 10.0 })?;
        let alias = a.take(&["as", "name"]);
        let kids = a.take(&["children", "kids", "for"]);
        let layers3d = if camera { a.take(&["layers"]) } else { None };
        let mv = if camera { a.take(&["move", "preset"]) } else { None };
        let amount = a.take(&["amount", "amt"]).map(|s| s.trim_end_matches(['%', 'x']).parse::<f64>()).transpose()?;
        let ease = a.take(&["ease"]).unwrap_or_else(|| "inOutSine".into());
        Ease::parse(&ease)?;
        let kind = if camera { ItemKind::Camera } else { ItemKind::Null };
        let mut it = self.new_item(kind, start, dur);
        it.name = alias;
        let id = it.id.clone();
        let tid = self.place_overlay(it, track)?;
        let kv = a.rest();
        self.apply_props(&id, &kv)?;
        self.note(format!("+{id} {} {tid} {}-{}", kind.name(), fmt_t(start), fmt_t(start + dur)));
        self.created.push(id.clone());
        if let Some(k) = kids {
            for c in self.targets(&k)? {
                if c != id {
                    self.set_parent(&c, &id)?;
                    self.note(format!("~{c} parent={id}"));
                }
            }
        }
        if let Some(l) = layers3d {
            for c in self.targets(&l)? {
                if c != id {
                    self.apply_props(&c, &[("3d".into(), "1".into())])?;
                    self.note(format!("~{c} 3d=true"));
                }
            }
        }
        if let Some(m) = mv {
            self.camera_move(&id, &m, amount, &ease)?;
        }
        Ok(())
    }

    /// Camera move presets written as ordinary keyframes / expressions.
    fn camera_move(&mut self, id: &str, mv: &str, amount: Option<f64>, ease: &str) -> Result<()> {
        let comp_id = self.comp.clone();
        let (cw, ch) = (self.comp().w as f64, self.comp().h as f64);
        let dur = self.item_span(id).map(|(s, e)| e - s).unwrap_or(5.0);
        let zoom = cw * 0.926;
        let (cx, cy) = (cw / 2.0, ch / 2.0);
        let n = fmt_num;
        let script = match mv.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "pushin" | "dollyin" | "push" => format!("kf {id} z 0:0@{ease} end:{}", n(amount.unwrap_or(zoom * 0.3))),
            "pullout" | "dollyout" | "pull" => format!("kf {id} z 0:{}@{ease} end:0", n(amount.unwrap_or(zoom * 0.3))),
            "truck" | "truckright" | "truckleft" => {
                let d = amount.unwrap_or(cw * 0.12) * if mv.to_ascii_lowercase().ends_with("left") { -1.0 } else { 1.0 };
                format!("kf {id} pos 0:{},{cy}@{ease} end:{},{cy}\nkf {id} poi 0:{},{cy}@{ease} end:{},{cy}", n(cx - d / 2.0), n(cx + d / 2.0), n(cx - d / 2.0), n(cx + d / 2.0))
            }
            "pedestal" | "pedestalup" | "pedestaldown" | "crane" => {
                let d = amount.unwrap_or(ch * 0.12) * if mv.to_ascii_lowercase().ends_with("down") { 1.0 } else { -1.0 };
                format!("kf {id} pos 0:{cx},{}@{ease} end:{cx},{}\nkf {id} poi 0:{cx},{}@{ease} end:{cx},{}", n(cy - d / 2.0), n(cy + d / 2.0), n(cy - d / 2.0), n(cy + d / 2.0))
            }
            "orbit" | "orbitleft" | "orbitright" | "arc" => {
                let ang = amount.unwrap_or(30.0) * if mv.to_ascii_lowercase().ends_with("left") { -1.0 } else { 1.0 };
                let th = format!("rad(ease(time,0,{},0,{}))", n(dur), n(ang));
                format!("expr {id} pos '[{} + {}*sin({th}), value[1]]'\nexpr {id} z '{}*(1-cos({th}))'", n(cx), n(zoom), n(zoom))
            }
            "drift" | "float" => format!("kf {id} z 0:0@{ease} end:{}\nexpr {id} pos 'value + wiggle(0.4,{})'", n(amount.unwrap_or(zoom * 0.08)), n(cw * 0.008)),
            "shake" | "handheld" => format!("expr {id} pos 'value + wiggle(5,{})'", n(amount.unwrap_or(cw * 0.006))),
            other => bail!("unknown camera move '{other}' (pushin pullout truck truckleft pedestal pedestaldown orbit orbitleft drift shake)"),
        };
        self.run_sub(&comp_id, &script, false)?;
        Ok(())
    }

    // ------------------------------------------------------------ timeline edits

    /// Rolling edit: moves the cut between `id` and the clip after it.
    pub fn c_roll(&mut self, a: &mut A) -> Result<()> {
        let usage = "roll <id> <±seconds> | roll <id> to=T  (moves the cut between <id> and the clip after it)";
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!(usage))?)?;
        let to = a.take(&["to", "at"]);
        let by = a.take(&["by"]).or_else(|| a.shift());
        let next = self.adjacent_next(&id).ok_or_else(|| anyhow!("{id} has no clip right after it to roll with (use trim)"))?;
        let (s0, e0) = self.item_span(&id).unwrap();
        let (_, e1) = self.item_span(&next).unwrap();
        let cut = match (to, by) {
            (Some(t), _) => self.time(&t)?,
            (None, Some(b)) => e0 + signed(self, &b)?,
            _ => bail!(usage),
        };
        let fd = 1.0 / self.fps();
        if cut < s0 + fd - 1e-9 || cut > e1 - fd + 1e-9 {
            bail!("rolling the cut to {} would leave less than a frame of {id} or {next}", fmt_t(cut));
        }
        if cut > e0 {
            self.trim_start_to(&next, cut)?;
            self.trim_end_to(&id, cut)?;
            if self.clamp_to_source(&id)? {
                bail!("{id} has no more media to roll the cut to {}", fmt_t(cut));
            }
        } else {
            self.trim_end_to(&id, cut)?;
            self.trim_start_to(&next, cut)?;
        }
        // a transition on the cut must still fit both sides
        let room = (cut - s0).min(e1 - cut);
        let l = self.relocate(&id)?;
        if let Some(t) = self.item_mut_at(l).tout.as_mut() {
            t.dur = t.dur.min(room);
        }
        let d = cut - e0;
        self.note(format!("~{id}>{next} roll {}{} cut={}", if d >= 0.0 { "+" } else { "-" }, fmt_t(d.abs()), fmt_t(cut)));
        Ok(())
    }

    /// `lift` removes a time range and leaves a gap; `extract` also closes it.
    pub fn c_lift(&mut self, a: &mut A, extract: bool) -> Result<()> {
        let verb = if extract { "extract" } else { "lift" };
        let ci = self.ci();
        let tracks = a.take(&["tracks", "track"]);
        let in_s = a.take(&["in", "from"]);
        let out_s = a.take(&["out", "to"]);
        let pos: Vec<String> = a.pos.drain(..).collect();
        let (t0, t1) = if let (Some(x), Some(y)) = (&in_s, &out_s) {
            (self.time(x)?, self.time(y)?)
        } else if pos.len() == 2 {
            (self.time(&pos[0])?, self.time(&pos[1])?)
        } else if pos.len() == 1 {
            let (s, e) = self.item_span(&pos[0]).ok_or_else(|| anyhow!("{verb} <in> <out> | {verb} <id> | {verb} (uses the work area)"))?;
            (s, e)
        } else if let Some(w) = self.comp().work {
            w
        } else {
            bail!("{verb} needs a range: {verb} 2 5 | {verb} in=2 out=5 | {verb} c3 (its span) | set a work area first (workarea 2 5)");
        };
        if t1 <= t0 + EPS {
            bail!("{verb}: out ({}) must be after in ({})", fmt_t(t1), fmt_t(t0));
        }
        let tids: Vec<String> = match tracks.as_deref() {
            Some(s) if !s.eq_ignore_ascii_case("all") => s.split(',').map(|x| self.comp().track(x.trim()).map(|t| t.id.clone()).ok_or_else(|| anyhow!("no track {x}"))).collect::<Result<_>>()?,
            _ => self.comp().tracks.iter().filter(|t| !t.locked).map(|t| t.id.clone()).collect(),
        };
        for tid in &tids {
            if self.comp().track(tid).is_some_and(|t| t.locked) {
                bail!("track {tid} is locked");
            }
            self.clear_range(ci, tid, t0, t1, None)?;
            if extract {
                let t = self.p.comps[ci].track_mut(tid).unwrap();
                for it in t.items.iter_mut().filter(|i| i.start >= t1 - EPS) {
                    it.start -= t1 - t0;
                }
                t.sort();
            }
        }
        self.note(format!("~{verb} {}-{} ({}s) on {}", fmt_t(t0), fmt_t(t1), fmt_t(t1 - t0), tids.join(",")));
        Ok(())
    }

    /// Rearrange: adjacent clips trade places; others trade positions.
    pub fn c_swap(&mut self, a: &mut A) -> Result<()> {
        let usage = "swap <a> <b>";
        let x = self.item_id(&a.shift().ok_or_else(|| anyhow!(usage))?)?;
        let y = self.item_id(&a.shift().ok_or_else(|| anyhow!(usage))?)?;
        if x == y {
            bail!("swap needs two different items");
        }
        let (lx, ly) = (self.relocate(&x)?, self.relocate(&y)?);
        if lx.0 != ly.0 {
            bail!("{x} and {y} must be in the same comp");
        }
        let (ix, iy) = (self.item_at(lx).clone(), self.item_at(ly).clone());
        let (tx, ty) = (self.p.comps[lx.0].tracks[lx.1].id.clone(), self.p.comps[ly.0].tracks[ly.1].id.clone());
        let fd = 0.5 / self.fps();
        let adjacent = tx == ty && ((ix.end() - iy.start).abs() < fd || (iy.end() - ix.start).abs() < fd);
        if adjacent {
            let (first, second) = if ix.start <= iy.start { (ix, iy) } else { (iy, ix) };
            let s = first.start;
            let lf = self.relocate(&first.id)?;
            {
                let f = self.item_mut_at(lf);
                f.start = s + second.dur;
                f.tin = None;
                f.tout = second.tout.clone();
            }
            let ls = self.relocate(&second.id)?;
            {
                let sc = self.item_mut_at(ls);
                sc.start = s;
                sc.tin = first.tin.clone();
                sc.tout = first.tout.clone().map(|mut t| {
                    t.dur = t.dur.min(first.dur).min(second.dur);
                    t
                });
            }
            self.p.comps[lx.0].tracks[lx.1].sort();
        } else {
            let mut a_ = self.take_item(&x)?;
            let mut b_ = self.take_item(&y)?;
            std::mem::swap(&mut a_.start, &mut b_.start);
            self.place(a_, &ty, Mode::Strict).map_err(|e| anyhow!("{x} doesn't fit at {}: {e}", fmt_t(iy.start)))?;
            self.place(b_, &tx, Mode::Strict).map_err(|e| anyhow!("{y} doesn't fit at {}: {e}", fmt_t(ix.start)))?;
        }
        self.note(format!("~{x}<->{y}"));
        Ok(())
    }

    /// Paste attributes: copy effects, transform, opacity, masks, audio or speed.
    pub fn c_paste(&mut self, a: &mut A) -> Result<()> {
        let usage = "paste <from> <to ids> [fx|color|transform|opacity|masks|audio|speed|anims|all] [stack]";
        let src = self.item_id(&a.shift().ok_or_else(|| anyhow!(usage))?)?;
        let to = a.take(&["to"]).or_else(|| a.shift()).ok_or_else(|| anyhow!(usage))?;
        let mut what: Vec<String> = a.pos.drain(..).map(|s| s.to_ascii_lowercase()).collect();
        if let Some(w) = a.take(&["what", "attrs"]) {
            what.extend(w.split(',').map(|s| s.trim().to_ascii_lowercase()));
        }
        let stack = what.iter().any(|w| w == "stack");
        what.retain(|w| w != "stack");
        let known = ["fx", "effects", "color", "transform", "motion", "opacity", "masks", "audio", "speed", "time", "anims", "all"];
        if let Some(bad) = what.iter().find(|w| !known.contains(&w.as_str())) {
            bail!("paste: unknown attribute '{bad}' ({})", known.join(" "));
        }
        let all = what.iter().any(|w| w == "all");
        let has = |k: &[&str]| all || what.iter().any(|w| k.contains(&w.as_str()));
        let defaults = what.is_empty();
        let (p_fx, p_color) = (has(&["fx", "effects"]) || defaults, has(&["color"]));
        let p_tr = has(&["transform", "motion"]) || defaults;
        let p_op = has(&["opacity"]) || defaults;
        let p_masks = has(&["masks"]) || defaults;
        let (p_audio, p_speed, p_anims) = (has(&["audio"]), has(&["speed", "time"]), has(&["anims"]));
        let s = self.item_at(self.relocate(&src)?).clone();
        let src_video = s.src.as_deref().and_then(|m| self.p.media(m)).is_some_and(|m| m.kind == MediaKind::Video);
        for id in self.targets(&to)? {
            if id == src {
                continue;
            }
            let l = self.relocate(&id)?;
            let kind = self.item_at(l).kind;
            let is_video = self.item_at(l).src.as_deref().and_then(|m| self.p.media(m)).is_some_and(|m| m.kind == MediaKind::Video);
            let mut done = vec![];
            let copy_props = |it: &mut Item, names: &[&str]| {
                for k in names {
                    if crate::model::schema::item_prop(kind, k).is_none() {
                        continue;
                    }
                    match s.props.get(*k) {
                        Some(v) => {
                            it.props.insert(k.to_string(), v.clone());
                        }
                        None => {
                            it.props.remove(*k);
                        }
                    }
                }
            };
            if p_tr {
                copy_props(self.item_mut_at(l), TRANSFORM_PROPS);
                done.push("transform");
            }
            if p_op {
                copy_props(self.item_mut_at(l), OPACITY_PROPS);
                done.push("opacity");
            }
            if p_fx || p_color {
                let mut n = 0;
                for e in &s.fx {
                    let Some(def) = crate::model::schema::effect(&e.kind) else { continue };
                    if !p_fx && def.cat != "color" {
                        continue;
                    }
                    if def.engine == crate::model::schema::Engine::Ff && !(is_video && src_video) {
                        continue;
                    }
                    let it = self.item_mut_at(l);
                    if !stack {
                        it.fx.retain(|x| x.kind != e.kind);
                    }
                    let fid = it.next_sub("f");
                    it.fx.push(Effect { id: fid, ..e.clone() });
                    n += 1;
                }
                if n > 0 {
                    done.push(if p_fx { "fx" } else { "color" });
                }
            }
            if p_masks && !s.masks.is_empty() {
                let it = self.item_mut_at(l);
                it.masks.clear();
                for m in &s.masks {
                    let mid = it.next_sub("m");
                    it.masks.push(Effect { id: mid, ..m.clone() });
                }
                done.push("masks");
            }
            if p_audio && matches!(kind, ItemKind::Clip | ItemKind::Comp) {
                copy_props(self.item_mut_at(l), AUDIO_PROPS);
                let it = self.item_mut_at(l);
                for e in &s.afx {
                    if !stack {
                        it.afx.retain(|x| x.kind != e.kind);
                    }
                    let aid = it.next_sub("a");
                    it.afx.push(Effect { id: aid, ..e.clone() });
                }
                done.push("audio");
            }
            if p_anims && kind == ItemKind::Text && s.kind == ItemKind::Text {
                let it = self.item_mut_at(l);
                it.anims.clear();
                for x in &s.anims {
                    let xid = it.next_sub("x");
                    it.anims.push(Effect { id: xid, ..x.clone() });
                }
                done.push("anims");
            }
            if p_speed && matches!(kind, ItemKind::Clip | ItemKind::Comp) {
                self.set_speed(&id, s.speed(), false)?;
                let l = self.relocate(&id)?;
                for k in ["reverse", "interp"] {
                    match s.props.get(k) {
                        Some(v) => {
                            self.item_mut_at(l).props.insert(k.into(), v.clone());
                        }
                        None => {
                            self.item_mut_at(l).props.remove(k);
                        }
                    }
                }
                done.push("speed");
            }
            self.note(format!("~{id} pasted {} from {src}", if done.is_empty() { "nothing".to_string() } else { done.join(",") }));
        }
        Ok(())
    }

    /// Offsets items in time (`by=`) or lays them end to end (`seq`).
    pub fn c_stagger(&mut self, a: &mut A) -> Result<()> {
        let usage = "stagger <ids> by=0.1 [from=T] [order=forward|reverse|random|time] | stagger <ids> seq [overlap=0.5]";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let seq = a.flag("seq") || a.flag("sequence");
        let by = a.take(&["by", "step", "delay"]).map(|v| signed(self, &v)).transpose()?;
        let overlap = a.take(&["overlap"]).map(|v| self.time(&v)).transpose()?.unwrap_or(0.0);
        let from = a.take(&["from", "at", "start"]).map(|v| self.time(&v)).transpose()?;
        let order = a.take(&["order"]).unwrap_or_else(|| "forward".into()).to_ascii_lowercase();
        if !seq && by.is_none() {
            bail!(usage);
        }
        let mut ids: Vec<String> = vec![];
        for id in self.targets(&spec)? {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        match order.as_str() {
            "forward" | "list" => {}
            "reverse" => ids.reverse(),
            "time" | "start" => ids.sort_by(|x, y| self.item_span(x).map(|s| s.0).partial_cmp(&self.item_span(y).map(|s| s.0)).unwrap_or(std::cmp::Ordering::Equal)),
            "random" => {
                let key = |s: &str| crate::render::eval::hash_str(s);
                ids.sort_by_key(|s| key(s));
            }
            o => bail!("order must be forward|reverse|random|time, not {o}"),
        }
        let t0 = from.unwrap_or_else(|| ids.iter().filter_map(|i| self.item_span(i)).map(|s| s.0).fold(f64::INFINITY, f64::min));
        let mut cur = t0;
        let mut moved = vec![];
        for (i, id) in ids.iter().enumerate() {
            let (s, e) = self.item_span(id).ok_or_else(|| anyhow!("unknown item {id}"))?;
            let ns = if seq { cur } else { t0 + i as f64 * by.unwrap_or(0.0) };
            cur = ns + (e - s) - overlap;
            if ns < -EPS {
                bail!("{id} would start before 0");
            }
            let (ci, ti, _) = self.relocate(id)?;
            let tid = self.p.comps[ci].tracks[ti].id.clone();
            let mut it = self.take_item(id)?;
            it.start = ns.max(0.0);
            moved.push((tid, it));
        }
        // put everything back, then check: items of the group may pass each other
        for (tid, it) in moved {
            let ci = self.ci();
            let t = self.p.comps[ci].track_mut(&tid).unwrap();
            t.items.push(it);
            t.sort();
        }
        for id in &ids {
            let (ci, ti, _) = self.relocate(id)?;
            let it = self.item_at((ci, ti, self.relocate(id)?.2)).clone();
            let ov = self.p.comps[ci].tracks[ti].overlapping(it.start, it.end(), Some(id));
            if let Some(o) = ov.first() {
                bail!("{id} would overlap {} on {}; put the items on separate tracks first (move <id> track=V3)", o.id, self.p.comps[ci].tracks[ti].id);
            }
        }
        self.note(format!("~stagger {} item(s){} from {}", ids.len(), if seq { " in sequence".to_string() } else { format!(" by {}", fmt_t(by.unwrap_or(0.0))) }, fmt_t(t0)));
        Ok(())
    }

    // ------------------------------------------------------------ align & distribute

    fn layer_box(&self, id: &str, at: Option<f64>) -> Result<(f64, f64, f64, f64)> {
        let l = self.relocate(id)?;
        let it = self.item_at(l);
        let t = self.measure_time(it, at);
        layer_geom(self.p, &self.p.comps[l.0], it, t).map(|g| g.bbox()).ok_or_else(|| anyhow!("{id} has nothing visible to align"))
    }

    /// Moves a layer by (dx, dy) comp px (all position keyframes move together).
    fn nudge(&mut self, id: &str, dx: f64, dy: f64, at: Option<f64>) -> Result<()> {
        if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
            return Ok(());
        }
        let l = self.relocate(id)?;
        let cx = self.vctx_of(l.0);
        let it = self.item_at(l).clone();
        let (mut dx, mut dy) = (dx, dy);
        if it.parent.is_some() {
            // the position lives in the parent's space
            let pm = Geo { p: self.p, comp: &self.p.comps[l.0] }.parent_m(&it, self.measure_time(&it, at), 0);
            let det = (pm.sx * pm.sy - pm.kx * pm.ky) as f64;
            if det.abs() > 1e-12 {
                let (a, b, c, d) = (pm.sx as f64, pm.ky as f64, pm.kx as f64, pm.sy as f64);
                (dx, dy) = ((d * dx - c * dy) / det, (-b * dx + a * dy) / det);
            }
        }
        let anim = self.item_mut_at(l).props.entry("pos".into()).or_insert_with(|| Anim::new(Val::V(cx.cw / 2.0, cx.ch / 2.0)));
        offset_vec(anim, dx, dy);
        Ok(())
    }

    /// Relative transform: `nudge <ids> [pos=dx,dy] [scale=1.2] [rot=15] [key]`.
    /// Without `key` the whole animation moves; with it a keyframe is set at the playhead (`at=`).
    pub fn c_nudge(&mut self, a: &mut A) -> Result<()> {
        let usage = "nudge <ids> [pos=dx,dy] [scale=1.2] [rot=15] [key] [at=T]";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let key = a.flag("key");
        let at = a.take(&["at", "t"]).map(|s| self.time(&s)).transpose()?;
        let parse2 = |v: &str| -> Result<(f64, f64)> {
            let p: Vec<f64> = v.split(',').map(|x| x.trim().trim_end_matches("px").parse::<f64>()).collect::<std::result::Result<_, _>>().map_err(|_| anyhow!("expected x,y numbers, got '{v}'"))?;
            Ok(match p.as_slice() {
                [x] => (*x, *x),
                [x, y] => (*x, *y),
                _ => bail!("expected x,y, got '{v}'"),
            })
        };
        let dpos = a.take(&["pos", "by", "move"]).map(|v| parse2(&v)).transpose()?;
        let dscale = a.take(&["scale", "size"]).map(|v| -> Result<(f64, f64)> {
            let (x, y) = parse2(v.trim_end_matches('%'))?;
            Ok(if v.ends_with('%') { (x / 100.0, y / 100.0) } else { (x, y) })
        }).transpose()?;
        let drot = a.take(&["rot", "rotate"]).map(|v| v.trim_end_matches("deg").parse::<f64>()).transpose()?;
        if dpos.is_none() && dscale.is_none() && drot.is_none() {
            bail!(usage);
        }
        for id in self.targets(&spec)? {
            let l = self.relocate(&id)?;
            let it = self.item_at(l).clone();
            if it.kind == ItemKind::Camera && dscale.is_some() {
                bail!("{id}: cameras have no scale (change zoom)");
            }
            let tl = self.measure_time(&it, at) - it.start;
            let cx = self.vctx_of(l.0);
            let mut done = vec![];
            if let Some((dx, dy)) = dpos {
                if key {
                    // the delta in the parent's space, applied to the value at the playhead
                    let (mut ddx, mut ddy) = (dx, dy);
                    if it.parent.is_some() {
                        let pm = Geo { p: self.p, comp: &self.p.comps[l.0] }.parent_m(&it, it.start + tl, 0);
                        let det = (pm.sx * pm.sy - pm.kx * pm.ky) as f64;
                        if det.abs() > 1e-12 {
                            let (a_, b, c, d) = (pm.sx as f64, pm.ky as f64, pm.kx as f64, pm.sy as f64);
                            (ddx, ddy) = ((d * dx - c * dy) / det, (-b * dx + a_ * dy) / det);
                        }
                    }
                    self.key_at(&id, "pos", tl, Val::V(cx.cw / 2.0, cx.ch / 2.0), &|v| match v {
                        Val::V(x, y) => Val::V(x + ddx, y + ddy),
                        o => o,
                    })?;
                } else {
                    self.nudge(&id, dx, dy, at)?;
                }
                done.push(format!("pos {}{},{}{}", if dx >= 0.0 { "+" } else { "" }, fmt_num(dx), if dy >= 0.0 { "+" } else { "" }, fmt_num(dy)));
            }
            if let Some((fx, fy)) = dscale {
                let f = &|v: Val| match v {
                    Val::V(x, y) => Val::V(x * fx, y * fy),
                    o => o,
                };
                if key {
                    self.key_at(&id, "scale", tl, Val::V(1.0, 1.0), f)?;
                } else {
                    self.map_anim(&id, "scale", Val::V(1.0, 1.0), f)?;
                }
                done.push(format!("scale x{}", fmt_num(fx)));
            }
            if let Some(r) = drot {
                let f = &|v: Val| Val::N(v.num() + r);
                if key {
                    self.key_at(&id, "rot", tl, Val::N(0.0), f)?;
                } else {
                    self.map_anim(&id, "rot", Val::N(0.0), f)?;
                }
                done.push(format!("rot {}{}", if r >= 0.0 { "+" } else { "" }, fmt_num(r)));
            }
            self.note(format!("~{id} {}{}", done.join(" "), if key { format!(" key@{}", fmt_t(tl)) } else { String::new() }));
        }
        Ok(())
    }

    /// Applies `f` to a property's static value and every keyframe.
    fn map_anim(&mut self, id: &str, prop: &str, def: Val, f: &dyn Fn(Val) -> Val) -> Result<()> {
        let l = self.relocate(id)?;
        let a = self.item_mut_at(l).props.entry(prop.into()).or_insert_with(|| Anim::new(def));
        a.v = f(a.v.clone());
        for k in a.k.iter_mut() {
            k.v = f(k.v.clone());
        }
        Ok(())
    }

    /// Sets a keyframe at item time `tl` to `f(value there)` (keyframing a static property starts its animation).
    fn key_at(&mut self, id: &str, prop: &str, tl: f64, def: Val, f: &dyn Fn(Val) -> Val) -> Result<()> {
        let l = self.relocate(id)?;
        let a = self.item_mut_at(l).props.entry(prop.into()).or_insert_with(|| Anim::new(def));
        let cur = a.keyed(tl);
        let ease = a.k.iter().rev().find(|k| k.t <= tl).map(|k| k.e).unwrap_or(Ease::Bezier(0.33, 0.0, 0.67, 1.0));
        if a.k.is_empty() && tl > 1e-6 {
            // hold the old value up to here, like turning on the stopwatch in AE
            a.add_key(Key { t: 0.0, v: cur.clone(), e: ease });
        }
        a.add_key(Key { t: tl, v: f(cur), e: ease });
        Ok(())
    }

    pub fn c_align(&mut self, a: &mut A) -> Result<()> {
        let usage = "align <ids> left|hcenter|right|top|vcenter|bottom|center [to=comp|safe|sel] [margin=px] [at=T]";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let to = a.take(&["to", "within"]).unwrap_or_else(|| "comp".into()).to_ascii_lowercase();
        let margin = a.take(&["margin", "pad"]).map(|s| s.trim_end_matches("px").parse::<f64>()).transpose()?.unwrap_or(0.0);
        let at = a.take(&["at", "t"]).map(|s| self.time(&s)).transpose()?;
        let how: Vec<String> = a.pos.drain(..).map(|s| s.to_ascii_lowercase()).collect();
        if how.is_empty() {
            bail!(usage);
        }
        let ids = self.targets(&spec)?;
        let boxes: Vec<(String, (f64, f64, f64, f64))> = ids.iter().map(|id| Ok((id.clone(), self.layer_box(id, at)?))).collect::<Result<_>>()?;
        let (cw, ch) = (self.comp().w as f64, self.comp().h as f64);
        let frame = match to.as_str() {
            "comp" | "frame" | "composition" => (0.0, 0.0, cw, ch),
            "safe" | "titlesafe" => (cw * 0.05, ch * 0.05, cw * 0.95, ch * 0.95),
            "sel" | "selection" | "group" => boxes.iter().fold((f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |acc, (_, b)| (acc.0.min(b.0), acc.1.min(b.1), acc.2.max(b.2), acc.3.max(b.3))),
            o => bail!("align to must be comp|safe|sel, not {o}"),
        };
        for (id, b) in &boxes {
            let (mut dx, mut dy) = (0.0, 0.0);
            for h in &how {
                match h.as_str() {
                    "left" | "l" => dx = frame.0 + margin - b.0,
                    "right" | "r" => dx = frame.2 - margin - b.2,
                    "hcenter" | "h" | "centerx" => dx = (frame.0 + frame.2 - b.0 - b.2) / 2.0,
                    "top" | "t" => dy = frame.1 + margin - b.1,
                    "bottom" | "b" => dy = frame.3 - margin - b.3,
                    "vcenter" | "v" | "middle" | "centery" => dy = (frame.1 + frame.3 - b.1 - b.3) / 2.0,
                    "center" | "c" => {
                        dx = (frame.0 + frame.2 - b.0 - b.2) / 2.0;
                        dy = (frame.1 + frame.3 - b.1 - b.3) / 2.0;
                    }
                    o => bail!("unknown alignment '{o}' (left hcenter right top vcenter bottom center)"),
                }
            }
            self.nudge(id, dx, dy, at)?;
            self.note(format!("~{id} align {} ({}{},{}{})", how.join(","), if dx >= 0.0 { "+" } else { "" }, fmt_num(dx.round()), if dy >= 0.0 { "+" } else { "" }, fmt_num(dy.round())));
        }
        Ok(())
    }

    pub fn c_distribute(&mut self, a: &mut A) -> Result<()> {
        let usage = "distribute <ids> h|v [gap=px] [at=T]";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let axis = a.shift().unwrap_or_else(|| "h".into()).to_ascii_lowercase();
        let horiz = match axis.as_str() {
            "h" | "x" | "horizontal" | "hcenter" => true,
            "v" | "y" | "vertical" | "vcenter" => false,
            o => bail!("distribute axis must be h or v, not {o}"),
        };
        let gap = a.take(&["gap", "spacing"]).map(|s| s.trim_end_matches("px").parse::<f64>()).transpose()?;
        let at = a.take(&["at", "t"]).map(|s| self.time(&s)).transpose()?;
        let ids = self.targets(&spec)?;
        if ids.len() < 2 || (ids.len() < 3 && gap.is_none()) {
            bail!("distribute needs 3+ layers (or 2+ with gap=)");
        }
        let mut boxes: Vec<(String, (f64, f64, f64, f64))> = ids.iter().map(|id| Ok((id.clone(), self.layer_box(id, at)?))).collect::<Result<_>>()?;
        let lo = |b: &(f64, f64, f64, f64)| if horiz { b.0 } else { b.1 };
        let hi = |b: &(f64, f64, f64, f64)| if horiz { b.2 } else { b.3 };
        boxes.sort_by(|x, y| (lo(&x.1) + hi(&x.1)).partial_cmp(&(lo(&y.1) + hi(&y.1))).unwrap_or(std::cmp::Ordering::Equal));
        let n = boxes.len();
        let (c0, c1) = ((lo(&boxes[0].1) + hi(&boxes[0].1)) / 2.0, (lo(&boxes[n - 1].1) + hi(&boxes[n - 1].1)) / 2.0);
        let mut edge = hi(&boxes[0].1);
        for (i, (id, b)) in boxes.iter().enumerate().skip(1) {
            let d = match gap {
                Some(g) => {
                    let d = edge + g - lo(b);
                    edge = hi(b) + d;
                    d
                }
                None if i == n - 1 => 0.0,
                None => c0 + (c1 - c0) * i as f64 / (n - 1) as f64 - (lo(b) + hi(b)) / 2.0,
            };
            if horiz {
                self.nudge(id, d, 0.0, at)?;
            } else {
                self.nudge(id, 0.0, d, at)?;
            }
        }
        self.note(format!("~distribute {} layer(s) {}", n, if horiz { "h" } else { "v" }));
        Ok(())
    }

    // ------------------------------------------------------------ keyframe assistant

    /// Resolves `spec` (ids or one id.fN) to property targets.
    fn prop_targets(&self, spec: &str) -> Result<Vec<Target>> {
        if self.parse_sub(spec).is_some() {
            return Ok(vec![self.target(spec)?]);
        }
        Ok(self.targets(spec)?.into_iter().map(|item| Target { item, sub: None }).collect())
    }

    /// Canonical property names for a target (aliases resolved), or None for all.
    fn prop_names(&self, t: &Target, props: &Option<Vec<String>>) -> Result<Option<Vec<String>>> {
        let Some(list) = props else { return Ok(None) };
        let (sch, what) = self.target_schema(t)?;
        list.iter().map(|n| find_in(&sch, n).map(|p| p.name.to_string()).ok_or_else(|| unknown_prop(&sch, n, &what))).collect::<Result<Vec<_>>>().map(Some)
    }

    /// Runs `f` on every keyframed anim of a target (item props, and with
    /// `all` also its effects, masks, shapes and animators).
    fn each_keyed(&mut self, t: &Target, names: &Option<Vec<String>>, f: &mut dyn FnMut(&str, &mut Anim)) -> Result<()> {
        let l = self.relocate(&t.item)?;
        let it = self.item_mut_at(l);
        let mut visit = |props: &mut Props| {
            for (k, a) in props.iter_mut() {
                if names.as_ref().is_some_and(|n| !n.iter().any(|x| x == k)) {
                    continue;
                }
                f(k, a);
            }
        };
        match &t.sub {
            Some((k, sid)) => {
                if let Some(e) = sub_list_mut(it, *k).iter_mut().find(|e| &e.id == sid) {
                    visit(&mut e.props);
                }
            }
            None => {
                visit(&mut it.props);
                if names.is_none() {
                    for list in [&mut it.fx, &mut it.afx, &mut it.masks, &mut it.shapes, &mut it.anims] {
                        for e in list.iter_mut() {
                            visit(&mut e.props);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `ease <targets> [props|all] <ease> [at=T]`: Easy Ease and friends on existing keyframes.
    pub fn c_ease(&mut self, a: &mut A) -> Result<()> {
        let usage = "ease <ids|id.f1> [prop,prop|all] <ease> [at=T]  e.g. ease c3 pos,scale easy | ease t2 all outBack";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let at = a.take(&["at", "t"]).map(|s| self.time(&s)).transpose()?;
        let mut pos: Vec<String> = a.pos.drain(..).collect();
        let ease_s = a.take(&["ease", "e"]).or_else(|| pos.pop()).ok_or_else(|| anyhow!(usage))?;
        let ease = Ease::parse(&ease_s)?;
        let props: Option<Vec<String>> = pos.pop().filter(|p| !p.eq_ignore_ascii_case("all")).map(|p| p.split(',').map(|s| s.trim().to_string()).collect());
        if !pos.is_empty() {
            bail!(usage);
        }
        let fd = 0.5 / self.fps();
        let mut total = 0;
        for t in self.prop_targets(&spec)? {
            let names = self.prop_names(&t, &props)?;
            let start = self.item_span(&t.item).map(|s| s.0).unwrap_or(0.0);
            let rel = at.map(|x| x - start);
            let mut n = 0;
            self.each_keyed(&t, &names, &mut |_, anim| {
                let last = anim.k.len().saturating_sub(1);
                for (i, k) in anim.k.iter_mut().enumerate() {
                    if i == last || rel.is_some_and(|r| (k.t - r).abs() > fd) {
                        continue;
                    }
                    k.e = ease;
                    n += 1;
                }
            })?;
            let label = match &t.sub {
                Some((_, s)) => format!("{}.{s}", t.item),
                None => t.item.clone(),
            };
            self.note(format!("~{label} ease={} on {n} key(s)", ease.name()));
            total += n;
        }
        if total == 0 {
            self.warn("no keyframes matched (add keyframes with kf first; the last key of a property has no segment to ease)");
        }
        Ok(())
    }

    /// Keyframe assistant inside `kf`: `kf <target> <prop|all> reverse | shift=±T |
    /// stretch=2 | bake [step=1f] | clear`. Returns false when the line is a normal `kf`.
    pub(crate) fn kf_assist(&mut self, spec: &str, prop: &str, a: &mut A) -> Result<bool> {
        let word = a.pos.first().map(|w| w.to_ascii_lowercase());
        let action = match word.as_deref() {
            Some(w @ ("reverse" | "bake" | "clear" | "smooth")) => Some(w.to_string()),
            _ => None,
        };
        let shift = a.take(&["shift", "offset"]);
        let stretch = a.take(&["stretch", "scale"]);
        if action.is_none() && shift.is_none() && stretch.is_none() {
            if prop.eq_ignore_ascii_case("all") {
                bail!("kf <id> all needs reverse | shift=±T | stretch=2 | bake | clear");
            }
            return Ok(false);
        }
        if action.is_some() {
            a.pos.remove(0);
        }
        let step = a.take(&["step", "every"]).map(|s| self.time(&s)).transpose()?;
        let props = (!prop.eq_ignore_ascii_case("all")).then(|| vec![prop.to_string()]);
        let fps = self.fps();
        let dt = shift.as_deref().map(|s| signed(self, s)).transpose()?;
        let k = stretch.as_deref().map(parse_factor).transpose()?;
        let mut changed = 0;
        for t in self.prop_targets(spec)? {
            let names = self.prop_names(&t, &props)?;
            let l = self.relocate(&t.item)?;
            let it = self.item_at(l).clone();
            if action.as_deref() == Some("bake") {
                // evaluate expressions (and loops) exactly as the renderer does
                let g = Geo { p: self.p, comp: &self.p.comps[l.0] };
                let lists = Geo::lists(it.kind);
                let x0 = g.ectx(&it, None);
                let b = g.bounds(&it, &crate::render::eval::Ev::new(0.0, &x0, g.vc(), &lists), &x0, 0.0);
                let x = g.ectx(&it, b);
                let st = step.unwrap_or(1.0 / fps).max(1e-3);
                let n = ((it.dur / st).ceil() as usize).min(20000);
                let mut cnt = 0;
                self.each_keyed(&t, &names, &mut |_, anim| {
                    if !anim.is_animated() {
                        return;
                    }
                    let mut keys: Vec<Key> = (0..=n)
                        .map(|i| {
                            let tt = (i as f64 * st).min(it.dur);
                            let v = anim.eval(tt, &x);
                            let e = if v.is_interpolable() { Ease::Linear } else { Ease::Hold };
                            Key { t: tt, v, e }
                        })
                        .collect();
                    // drop keys in the middle of constant runs
                    let mut i = 1;
                    while i + 1 < keys.len() {
                        if keys[i].v == keys[i - 1].v && keys[i].v == keys[i + 1].v {
                            keys.remove(i);
                        } else {
                            i += 1;
                        }
                    }
                    anim.x = None;
                    anim.lp = None;
                    anim.set_keys(keys);
                    cnt += 1;
                })?;
                changed += cnt;
                continue;
            }
            self.each_keyed(&t, &names, &mut |_, anim| {
                if anim.k.is_empty() {
                    return;
                }
                let first = anim.k[0].t;
                let last = anim.k[anim.k.len() - 1].t;
                match action.as_deref() {
                    Some("reverse") => {
                        let old = anim.k.clone();
                        let n = old.len();
                        let keys = (0..n)
                            .map(|j| {
                                let i = n - 1 - j;
                                let e = if i > 0 { old[i - 1].e.reversed() } else { Ease::Linear };
                                Key { t: first + last - old[i].t, v: old[i].v.clone(), e }
                            })
                            .collect();
                        anim.set_keys(keys);
                    }
                    Some("clear") => {
                        anim.v = anim.k[0].v.clone();
                        anim.k.clear();
                        anim.lp = None;
                    }
                    Some("smooth") => {
                        for key in anim.k.iter_mut() {
                            if key.v.is_interpolable() {
                                key.e = Ease::Bezier(0.33, 0.0, 0.67, 1.0);
                            }
                        }
                    }
                    _ => {
                        let keys = anim
                            .k
                            .iter()
                            .map(|key| {
                                let mut nk = key.clone();
                                if let Some(f) = k {
                                    nk.t = first + (nk.t - first) * f;
                                }
                                if let Some(d) = dt {
                                    nk.t += d;
                                }
                                nk
                            })
                            .collect();
                        anim.set_keys(keys);
                    }
                }
                changed += 1;
            })?;
        }
        let what = action.clone().unwrap_or_else(|| {
            let mut v = vec![];
            if let Some(d) = dt {
                v.push(format!("shift {}{}", if d >= 0.0 { "+" } else { "-" }, fmt_t(d.abs())));
            }
            if let Some(f) = k {
                v.push(format!("stretch {}x", fmt_num(f)));
            }
            v.join(" ")
        });
        if changed == 0 {
            self.warn(format!("{spec}: no animated properties to {what}"));
        }
        self.note(format!("~{spec} {prop} {what} ({changed} propert{})", if changed == 1 { "y" } else { "ies" }));
        Ok(true)
    }

    // ------------------------------------------------------------ work area & markers

    pub fn c_workarea(&mut self, a: &mut A) -> Result<()> {
        let usage = "workarea <in> <out> | workarea <id> | workarea all | workarea clear";
        let first = a.take(&["in", "from"]).or_else(|| a.shift()).ok_or_else(|| anyhow!(usage))?;
        let second = a.take(&["out", "to"]).or_else(|| a.shift());
        let ci = self.ci();
        let w = match (first.to_ascii_lowercase().as_str(), &second) {
            ("clear" | "off" | "none", _) => None,
            ("all" | "comp", _) => Some((0.0, self.comp().duration())),
            (_, Some(out)) => Some((self.time(&first)?, self.time(out)?)),
            _ => Some(self.item_span(&first).ok_or_else(|| anyhow!(usage))?),
        };
        if let Some((x, y)) = w {
            if x < -EPS || y <= x + EPS {
                bail!("work area needs 0 <= in < out (got {}-{})", fmt_t(x), fmt_t(y));
            }
        }
        let id = self.comp.clone();
        self.p.comps[ci].work = w;
        match w {
            Some((x, y)) => self.note(format!("~{id} workarea {}-{}", fmt_t(x), fmt_t(y))),
            None => self.note(format!("~{id} workarea cleared")),
        }
        Ok(())
    }

    pub(crate) fn find_marker(&self, key: &str) -> Result<usize> {
        let ms = &self.comp().markers;
        if let Some(i) = ms.iter().position(|m| m.name.eq_ignore_ascii_case(key)) {
            return Ok(i);
        }
        let t = self.time(key).map_err(|_| anyhow!("no marker named '{key}'"))?;
        let fd = 0.5 / self.fps();
        ms.iter().position(|m| (m.t - t).abs() <= fd).ok_or_else(|| anyhow!("no marker at {}", fmt_t(t)))
    }

    /// Applies marker fields from key=value pairs.
    pub(crate) fn marker_fields(&self, m: &mut Marker, kvs: Vec<(String, String)>) -> Result<()> {
        for (k, v) in kvs {
            match k.to_ascii_lowercase().as_str() {
                "name" => m.name = v,
                "t" | "at" | "time" => m.t = self.time(&v)?,
                "color" => m.color = (!matches!(v.as_str(), "none" | "-" | "")).then(|| v.clone()),
                "dur" | "duration" => m.dur = self.time(&v)?.max(0.0),
                "note" | "comment" => m.note = v,
                other => bail!("unknown marker field '{other}' (name t color dur note)"),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ Essential Sound

    pub fn c_sound(&mut self, a: &mut A) -> Result<()> {
        let usage = "sound <clips> dialogue|music|sfx|ambience|clear [amount=1] [match=1] [under=A1] [duck=-12]";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let raw = a.take(&["type", "as"]).or_else(|| a.shift()).ok_or_else(|| anyhow!(usage))?;
        let ty = match raw.to_ascii_lowercase().as_str() {
            "dialogue" | "dialog" | "voice" | "speech" | "vo" => "dialogue",
            "music" | "song" | "bed" => "music",
            "sfx" | "fx" | "effects" | "soundeffects" => "sfx",
            "ambience" | "ambient" | "room" | "atmos" | "roomtone" => "ambience",
            "clear" | "none" | "reset" => "clear",
            o => bail!("unknown sound type '{o}' (dialogue music sfx ambience clear)"),
        };
        let amount = a.take(&["amount", "strength"]).map(|s| s.parse::<f64>()).transpose()?.unwrap_or(1.0).clamp(0.0, 1.0);
        let do_match = !matches!(a.take(&["match", "loudness"]).map(|s| s.to_ascii_lowercase()).as_deref(), Some("0" | "false" | "off" | "no"));
        let under = a.take(&["under", "by"]);
        let duck_db = a.take(&["duck"]).map(|s| s.trim_end_matches("dB").trim_end_matches("db").parse::<f64>()).transpose()?.unwrap_or(-12.0);
        // clip loudness targets before the master bus (dialogue on top, beds well under it)
        let target = match ty {
            "dialogue" => -20.0,
            "music" => -28.0,
            "sfx" => -24.0,
            _ => -32.0,
        };
        let ids = self.targets(&spec)?;
        let mut tracks: Vec<String> = vec![];
        for id in &ids {
            let l = self.relocate(id)?;
            let tid = self.p.comps[l.0].tracks[l.1].id.clone();
            let it = self.item_at(l).clone();
            let Some(m) = it.src.as_deref().and_then(|s| self.p.media(s)).filter(|m| m.has_audio || m.kind == MediaKind::Audio).cloned() else {
                bail!("{id} has no audio");
            };
            self.item_mut_at(l).afx.retain(|e| !SOUND_AFX.contains(&e.kind.as_str()));
            let mut done = vec![ty.to_string()];
            if ty == "dialogue" {
                self.add_effect(id, "voice", &[("amount".into(), fmt_num(amount))], true)?;
                self.add_effect(id, "deess", &[("amount".into(), fmt_num(0.4 * amount))], true)?;
                done.push("voice+deess".into());
            }
            if ty == "clear" {
                self.item_mut_at(self.relocate(id)?).props.remove("vol");
            } else if do_match {
                match self.env.analyze(self.p, &m.id, "loudness", &BTreeMap::new()) {
                    Ok(v) => {
                        if let Some(lufs) = v["lufs"].as_f64() {
                            let g = (target - lufs).clamp(-30.0, 24.0);
                            self.apply_props(id, &[("vol".into(), fmt_num((g * 10.0).round() / 10.0))])?;
                            done.push(format!("vol={}dB ({} LUFS -> {})", fmt_num((g * 10.0).round() / 10.0), fmt_num(lufs.round()), fmt_num(target)));
                        }
                    }
                    Err(e) => self.warn(format!("{id}: loudness not matched ({e:#})")),
                }
            }
            if !tracks.contains(&tid) {
                tracks.push(tid);
            }
            self.note(format!("~{id} sound {}", done.join(" ")));
        }
        if ty == "music" || ty == "ambience" {
            let by = under.or_else(|| {
                let c = self.comp();
                c.tracks.iter().find(|t| !tracks.contains(&t.id) && t.items.iter().any(|i| i.afx.iter().any(|e| e.kind == "voice"))).map(|t| t.id.clone())
            });
            match by {
                Some(by) => {
                    let by_id = self.comp().track(&by).ok_or_else(|| anyhow!("no track {by}"))?.id.clone();
                    let ci = self.ci();
                    for tid in &tracks {
                        if tid == &by_id {
                            continue;
                        }
                        let t = self.p.comps[ci].track_mut(tid).unwrap();
                        t.duck = Some(Duck { by: by_id.clone(), amount: -duck_db.abs(), attack: 20.0, release: 400.0 });
                        self.note(format!("~{tid} ducks {}dB under {by_id}", fmt_num(-duck_db.abs())));
                    }
                }
                None if ty == "music" => self.warn("no dialogue track to duck under: mark the speech with `sound <clips> dialogue` or give under=A1"),
                None => {}
            }
        }
        Ok(())
    }
}
