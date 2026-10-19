//! Property-level commands: set, kf, expr, anim, fx/afx, tr, fade, mask, matte,
//! parent, color, lut.

use super::exec::{parse_speed, parse_track_spec, Exec, Mode, A, EPS};
use super::presets::{self, AnimOpts};
use crate::model::anim::{Anim, Ease, Key, LoopMode};
use crate::model::project::*;
use crate::model::schema::{self, Engine, P};
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Val};
use anyhow::{anyhow, bail, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SubKind {
    Fx,
    Afx,
    Mask,
    Shape,
    Anim,
}

impl SubKind {
    fn from_prefix(c: char) -> Option<SubKind> {
        Some(match c {
            'f' => SubKind::Fx,
            'a' => SubKind::Afx,
            'm' => SubKind::Mask,
            's' => SubKind::Shape,
            'x' => SubKind::Anim,
            _ => return None,
        })
    }
    fn name(&self) -> &'static str {
        match self {
            SubKind::Fx => "effect",
            SubKind::Afx => "audio effect",
            SubKind::Mask => "mask",
            SubKind::Shape => "shape",
            SubKind::Anim => "text animator",
        }
    }
}

pub fn sub_list(it: &Item, k: SubKind) -> &Vec<Effect> {
    match k {
        SubKind::Fx => &it.fx,
        SubKind::Afx => &it.afx,
        SubKind::Mask => &it.masks,
        SubKind::Shape => &it.shapes,
        SubKind::Anim => &it.anims,
    }
}

pub fn sub_list_mut(it: &mut Item, k: SubKind) -> &mut Vec<Effect> {
    match k {
        SubKind::Fx => &mut it.fx,
        SubKind::Afx => &mut it.afx,
        SubKind::Mask => &mut it.masks,
        SubKind::Shape => &mut it.shapes,
        SubKind::Anim => &mut it.anims,
    }
}

/// Property schema for a sub-object.
pub fn sub_schema(k: SubKind, kind: &str) -> Vec<&'static P> {
    match k {
        SubKind::Fx => schema::effect(kind).map(|e| e.params.iter().collect()).unwrap_or_default(),
        SubKind::Afx => schema::audio_effect(kind).map(|e| e.params.iter().collect()).unwrap_or_default(),
        SubKind::Mask => schema::MASK.iter().collect(),
        SubKind::Shape => schema::shape_props(kind).unwrap_or(&[]).iter().chain(schema::SHAPE_COMMON.iter()).collect(),
        SubKind::Anim => schema::ANIMATOR.iter().collect(),
    }
}

pub(crate) fn find_in<'a>(list: &[&'a P], name: &str) -> Option<&'a P> {
    let l = name.to_ascii_lowercase();
    list.iter().copied().find(|p| p.name.eq_ignore_ascii_case(&l)).or_else(|| list.iter().copied().find(|p| p.alias.iter().any(|a| a.eq_ignore_ascii_case(&l))))
}

pub(crate) fn unknown_prop(list: &[&P], name: &str, what: &str) -> anyhow::Error {
    let hint = schema::suggest(list.iter().map(|p| p.name), name).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
    let names: Vec<&str> = list.iter().map(|p| p.name).collect();
    anyhow!("unknown property '{name}' for {what}.{hint} Valid: {}", names.join(" "))
}

/// A resolved property target: an item or one of its sub-objects.
pub struct Target {
    pub item: String,
    pub sub: Option<(SubKind, String)>,
}

impl<'a> Exec<'a> {
    pub fn parse_sub(&self, spec: &str) -> Option<(String, SubKind, String)> {
        let (base, sub) = spec.rsplit_once('.')?;
        let mut ch = sub.chars();
        let k = SubKind::from_prefix(ch.next()?)?;
        if ch.as_str().is_empty() || !ch.as_str().chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let id = self.item_id(base).ok()?;
        Some((id, k, sub.to_string()))
    }

    pub fn target(&self, spec: &str) -> Result<Target> {
        if let Some((id, k, sid)) = self.parse_sub(spec) {
            let l = self.relocate(&id)?;
            if !sub_list(self.item_at(l), k).iter().any(|e| e.id == sid) {
                bail!("{id} has no {} '{sid}'", k.name());
            }
            return Ok(Target { item: id, sub: Some((k, sid)) });
        }
        Ok(Target { item: self.item_id(spec)?, sub: None })
    }

    /// Schema of a target plus a closure-friendly description.
    pub(crate) fn target_schema(&self, t: &Target) -> Result<(Vec<&'static P>, String)> {
        let l = self.relocate(&t.item)?;
        let it = self.item_at(l);
        Ok(match &t.sub {
            None => (schema::item_props(it.kind).iter().chain(schema::LAYER.iter()).collect(), it.kind.name().to_string()),
            Some((k, sid)) => {
                let e = sub_list(it, *k).iter().find(|e| &e.id == sid).unwrap();
                (sub_schema(*k, &e.kind), format!("{} {}", k.name(), e.kind))
            }
        })
    }

    pub(crate) fn target_anim_mut(&mut self, t: &Target, name: &str, def: Val) -> Result<&mut Anim> {
        let l = self.relocate(&t.item)?;
        let it = self.item_mut_at(l);
        let props = match &t.sub {
            None => &mut it.props,
            Some((k, sid)) => &mut sub_list_mut(it, *k).iter_mut().find(|e| &e.id == sid).unwrap().props,
        };
        Ok(props.entry(name.to_string()).or_insert_with(|| Anim::new(def)))
    }

    // ------------------------------------------------------------ set

    pub fn c_set(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("set <id|id.fN|V1> key=value ..."))?;
        let kvs = a.rest();
        if kvs.is_empty() {
            bail!("set needs key=value pairs");
        }
        // track settings
        if parse_track_spec(&spec).is_some() && self.p.locate(&spec).is_none() && self.comp().track(&spec).is_some() {
            let mut args: Vec<super::lex::Tok> = vec![super::lex::Tok::W("set".into()), super::lex::Tok::W(spec.clone())];
            args.extend(kvs.iter().map(|(k, v)| super::lex::Tok::KV(k.clone(), v.clone())));
            let cmd = super::lex::Cmd { line: 0, text: String::new(), verb: "track".into(), args };
            return self.cmd(&cmd);
        }
        if spec.contains('.') && self.parse_sub(&spec).is_some() {
            let t = self.target(&spec)?;
            return self.set_sub(&t, &kvs);
        }
        for id in self.targets(&spec)? {
            self.set_item(&id, &kvs)?;
        }
        Ok(())
    }

    pub(crate) fn set_sub(&mut self, t: &Target, kvs: &[(String, String)]) -> Result<()> {
        let (sk, sid) = t.sub.clone().unwrap();
        let l = self.relocate(&t.item)?;
        let cx = self.vctx_of(l.0);
        let mut changed = vec![];
        for (k, v) in kvs {
            let lk = k.to_ascii_lowercase();
            if lk == "on" || lk == "off" || lk == "enabled" {
                let on = if lk == "off" { !truthy(v) } else { truthy(v) };
                let it = self.item_mut_at(l);
                sub_list_mut(it, sk).iter_mut().find(|e| e.id == sid).unwrap().on = on;
                changed.push(format!("on={on}"));
                continue;
            }
            if lk == "kind" && matches!(sk, SubKind::Mask | SubKind::Anim | SubKind::Shape) {
                let valid: &[&str] = match sk {
                    SubKind::Mask => schema::MASK_KINDS,
                    SubKind::Anim => schema::ANIMATOR_KINDS,
                    _ => schema::SHAPE_KINDS,
                };
                let kv = if sk == SubKind::Shape { schema::normalize_shape_kind(v).map(|s| s.to_string()) } else { valid.iter().find(|x| x.eq_ignore_ascii_case(v)).map(|s| s.to_string()) };
                let kv = kv.ok_or_else(|| anyhow!("kind must be one of {}", valid.join("|")))?;
                let it = self.item_mut_at(l);
                sub_list_mut(it, sk).iter_mut().find(|e| e.id == sid).unwrap().kind = kv;
                changed.push(format!("kind={v}"));
                continue;
            }
            let (sch, what) = self.target_schema(t)?;
            let p = find_in(&sch, k).ok_or_else(|| unknown_prop(&sch, k, &what))?;
            let val = schema::clamp_val(p, schema::parse_val(p, v, cx)?);
            let anim = self.target_anim_mut(t, p.name, val.clone())?;
            *anim = Anim::new(val);
            changed.push(format!("{}={}", p.name, v));
        }
        self.note(format!("~{}.{} {}", t.item, sid, changed.join(" ")));
        Ok(())
    }

    fn set_item(&mut self, id: &str, kvs: &[(String, String)]) -> Result<()> {
        let mut changed = vec![];
        let mut props = vec![];
        for (k, v) in kvs {
            let l = self.relocate(id)?;
            let it = self.item_at(l).clone();
            match k.to_ascii_lowercase().as_str() {
                "start" | "at" => {
                    let ns = super::exec::rel_or_abs(v, it.start, |x| self.time(x))?;
                    let tid = self.p.comps[l.0].tracks[l.1].id.clone();
                    let mut moved = self.take_item(id)?;
                    moved.start = ns;
                    self.place(moved, &tid, Mode::Strict)?;
                    changed.push(format!("start={}", fmt_t(ns)));
                }
                "dur" | "duration" => {
                    let nd = super::exec::rel_or_abs(v, it.dur, |x| self.time(x))?;
                    let t = &self.p.comps[l.0].tracks[l.1];
                    if nd > it.dur && !t.is_free(it.end(), it.start + nd, Some(id)) {
                        bail!("{id}: new duration overlaps the next item");
                    }
                    self.trim_end_to(id, it.start + nd)?;
                    if self.clamp_to_source(id)? {
                        self.warn(format!("{id}: clamped to available media"));
                    }
                    changed.push(format!("dur={}", fmt_t(nd)));
                }
                "end" => {
                    let ne = super::exec::rel_or_abs(v, it.end(), |x| self.time(x))?;
                    let t = &self.p.comps[l.0].tracks[l.1];
                    if ne > it.end() && !t.is_free(it.end(), ne, Some(id)) {
                        bail!("{id}: new end overlaps the next item");
                    }
                    self.trim_end_to(id, ne)?;
                    self.clamp_to_source(id)?;
                    changed.push(format!("end={}", fmt_t(ne)));
                }
                "in" | "srcin" => {
                    let ni = super::exec::rel_or_abs(v, it.src_in, |x| self.time(x))?;
                    if ni < 0.0 {
                        bail!("in can't be negative");
                    }
                    self.item_mut_at(l).src_in = ni;
                    if self.clamp_to_source(id)? {
                        self.warn(format!("{id}: clamped to available media"));
                    }
                    changed.push(format!("in={}", fmt_t(ni)));
                }
                "name" => {
                    self.item_mut_at(l).name = if v == "-" || v.is_empty() { None } else { Some(v.clone()) };
                    changed.push(format!("name={v}"));
                }
                "text" | "content" => {
                    if it.kind != ItemKind::Text {
                        bail!("{id} is not a text item");
                    }
                    self.item_mut_at(l).text = Some(v.replace("\\n", "\n"));
                    changed.push("text".into());
                }
                "track" => {
                    let mut moved = self.take_item(id)?;
                    moved.start = it.start;
                    let dest = self.place(moved, v, Mode::Strict)?;
                    changed.push(format!("track={dest}"));
                }
                "speed" => {
                    self.set_speed(id, parse_speed(v)?, false)?;
                }
                "parent" => {
                    self.set_parent(id, v)?;
                    changed.push(format!("parent={v}"));
                }
                "off" | "disabled" => {
                    self.item_mut_at(l).off = truthy(v);
                    changed.push(format!("off={}", truthy(v)));
                }
                "on" | "enabled" => {
                    self.item_mut_at(l).off = !truthy(v);
                    changed.push(format!("off={}", !truthy(v)));
                }
                _ => props.push((k.clone(), v.clone())),
            }
        }
        if !props.is_empty() {
            self.apply_props(id, &props)?;
            for (k, v) in &props {
                changed.push(format!("{k}={v}"));
            }
        }
        self.note(format!("~{id} {}", changed.join(" ")));
        Ok(())
    }

    // ------------------------------------------------------------ keyframes

    pub fn c_kf(&mut self, a: &mut A, mode: u8) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("kf <id|id.fN> <prop> <t:v[@ease]> ... [ease=] [loop=cycle|pingpong|offset]"))?;
        let prop = a.shift().ok_or_else(|| anyhow!("kf needs a property name"))?;
        if self.kf_assist(&spec, &prop, a)? {
            return Ok(());
        }
        let def_ease = match a.take(&["ease", "e"]) {
            Some(e) => Ease::parse(&e)?,
            None => Ease::Bezier(0.33, 0.0, 0.67, 1.0),
        };
        let lp = a.take(&["loop"]);
        let t = self.target(&spec)?;
        Self::no_audio_fx_animation(&t, &prop)?;
        let (sch, what) = self.target_schema(&t)?;
        let p = find_in(&sch, &prop).ok_or_else(|| unknown_prop(&sch, &prop, &what))?;
        if !p.anim {
            bail!("'{}' can't be keyframed (static property)", p.name);
        }
        let l = self.relocate(&t.item)?;
        let cx = self.vctx_of(l.0);
        let dur = self.item_at(l).dur;
        let tokens: Vec<String> = a.pos.drain(..).collect();
        let mut keys = vec![];
        let mut times = vec![];
        for tok in &tokens {
            if mode == 2 {
                times.push(self.rel_time(tok, dur)?);
                continue;
            }
            let (ts, rest) = tok.split_once(':').ok_or_else(|| anyhow!("keyframe '{tok}' must look like time:value[@ease] e.g. 0:1 1.5:1.2@outBack"))?;
            let (vs, es) = match rest.rsplit_once('@') {
                Some((v, e)) if !e.is_empty() && !e.contains(',') => (v, Some(e)),
                _ => (rest, None),
            };
            let t_rel = self.rel_time(ts, dur)?;
            let v = schema::clamp_val(p, schema::parse_val(p, vs, cx)?);
            let e = match es {
                Some(e) => Ease::parse(e)?,
                None => {
                    if v.is_interpolable() {
                        def_ease
                    } else {
                        Ease::Hold
                    }
                }
            };
            keys.push(Key { t: t_rel, v, e });
        }
        if mode != 2 && keys.is_empty() {
            bail!("no keyframes given");
        }
        let def = schema::default_val(p, cx);
        let anim = self.target_anim_mut(&t, p.name, def)?;
        match mode {
            0 => anim.set_keys(keys.clone()),
            1 => {
                for k in keys.clone() {
                    anim.add_key(k);
                }
            }
            _ => {
                if times.is_empty() {
                    let v0 = anim.k.first().map(|k| k.v.clone()).unwrap_or(anim.v.clone());
                    anim.k.clear();
                    anim.v = v0;
                } else {
                    anim.k.retain(|k| !times.iter().any(|t| (k.t - t).abs() < 1e-3));
                }
            }
        }
        if let Some(lp) = lp {
            anim.lp = match lp.to_ascii_lowercase().as_str() {
                "cycle" | "loop" | "1" | "true" => Some(LoopMode::Cycle),
                "pingpong" => Some(LoopMode::Pingpong),
                "offset" => Some(LoopMode::Offset),
                "none" | "off" | "0" => None,
                o => bail!("loop must be cycle|pingpong|offset|none, not {o}"),
            };
        }
        let n = anim.k.len();
        let label = match &t.sub {
            Some((_, s)) => format!("{}.{}", t.item, s),
            None => t.item.clone(),
        };
        self.note(format!("~{label} {} {} key(s)", p.name, n));
        Ok(())
    }

    /// Audio effects are rendered as one FFmpeg filter chain with fixed settings,
    /// so animating them would silently do nothing: say so instead.
    fn no_audio_fx_animation(t: &Target, prop: &str) -> Result<()> {
        if let Some((SubKind::Afx, sid)) = &t.sub {
            bail!("{}.{sid} {prop}: audio effect settings can't be animated; automate the level with kf {} vol ... or split the clip and use two settings", t.item, t.item);
        }
        Ok(())
    }

    pub fn c_expr(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("expr <id> <prop> '<expression>' (or - to clear)"))?;
        let prop = a.shift().ok_or_else(|| anyhow!("expr needs a property"))?;
        let code = a.take(&["x", "code"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("expr needs an expression"))?;
        let t = self.target(&spec)?;
        Self::no_audio_fx_animation(&t, &prop)?;
        let (sch, what) = self.target_schema(&t)?;
        let p = find_in(&sch, &prop).ok_or_else(|| unknown_prop(&sch, &prop, &what))?;
        if !p.anim {
            bail!("'{}' is static and can't have an expression", p.name);
        }
        let l = self.relocate(&t.item)?;
        let def = schema::default_val(p, self.vctx_of(l.0));
        let clear = matches!(code.as_str(), "-" | "none" | "off" | "");
        if !clear {
            crate::model::expr::check(&code).map_err(|e| anyhow!("expression error: {e}"))?;
        }
        let anim = self.target_anim_mut(&t, p.name, def)?;
        anim.x = if clear { None } else { Some(code.clone()) };
        self.note(format!("~{} {} expr{}", t.item, p.name, if clear { " cleared" } else { "" }));
        Ok(())
    }

    pub fn c_anim(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("anim <ids> <preset[,preset]> [dur=] [delay=] [ease=] [amount=] [freq=] [at=T] [dist=]"))?;
        let names = a.shift().ok_or_else(|| anyhow!("anim needs a preset (help anim)"))?;
        let mut o = AnimOpts::default();
        if let Some(d) = a.take(&["dur", "duration"]) {
            o.dur = Some(self.time(&d)?);
        }
        if let Some(d) = a.take(&["delay"]) {
            o.delay = self.time(&d)?;
        }
        if let Some(e) = a.take(&["ease"]) {
            o.ease = Some(Ease::parse(&e)?);
        }
        o.amount = a.take(&["amount", "amt", "strength", "intensity"]).map(|s| s.trim_end_matches('%').parse::<f64>()).transpose()?;
        o.freq = a.take(&["freq", "speed", "hz"]).map(|s| s.parse::<f64>()).transpose()?;
        o.dist = a.take(&["dist", "distance"]).map(|s| s.trim_end_matches("px").parse::<f64>()).transpose()?;
        let at_abs = a.take(&["at", "t"]).map(|s| self.time(&s)).transpose()?;
        let mut targets = self.targets(&spec)?;
        if let Some(t) = at_abs {
            // `anim V1 punch at=12` hits the clip under 12s, not every clip on V1.
            let mut hit = Vec::new();
            for id in targets {
                let it = self.item_at(self.relocate(&id)?);
                if it.start - EPS <= t && t < it.end() - EPS {
                    hit.push(id);
                }
            }
            if hit.is_empty() {
                bail!("nothing in {spec} at {}", fmt_t(t));
            }
            targets = hit;
        }
        for id in targets {
            let l = self.relocate(&id)?;
            let it = self.item_at(l).clone();
            let mut oo = o.clone();
            if let Some(t) = at_abs {
                oo.at = Some(t - it.start);
            }
            let n_chars = it.text.as_deref().map(|s| s.chars().filter(|c| !c.is_whitespace()).count()).unwrap_or(0);
            for name in names.split(',') {
                self.add_anim_preset(&id, name.trim(), &oo, n_chars)?;
            }
            self.note(format!("~{id} anim {names}"));
        }
        Ok(())
    }

    // ------------------------------------------------------------ effects

    pub fn add_effect(&mut self, id: &str, name: &str, kvs: &[(String, String)], audio: bool) -> Result<String> {
        let def = if audio { schema::audio_effect(name) } else { schema::effect(name) };
        let def = def.ok_or_else(|| {
            let list = if audio { schema::AUDIO_EFFECTS } else { schema::EFFECTS };
            let cands = list.iter().flat_map(|e| std::iter::once(e.name).chain(e.alias.iter().copied()));
            let hint = schema::suggest(cands, name).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
            anyhow!("unknown {}effect '{name}'.{hint} (help {})", if audio { "audio " } else { "" }, if audio { "audio" } else { "effects" })
        })?;
        let l = self.relocate(id)?;
        let cx = self.vctx_of(l.0);
        let it = self.item_at(l).clone();
        if def.engine == Engine::Ff {
            let ok = it.kind == ItemKind::Clip && it.src.as_deref().and_then(|s| self.p.media(s)).map(|m| m.kind == MediaKind::Video).unwrap_or(false);
            if !ok {
                bail!("'{}' is an FFmpeg decode-time effect and only works on video clips", def.name);
            }
        }
        if audio && !matches!(it.kind, ItemKind::Clip | ItemKind::Comp) {
            bail!("{id} has no audio");
        }
        if !audio && it.kind == ItemKind::Clip && it.src.as_deref().and_then(|s| self.p.media(s)).map(|m| m.kind == MediaKind::Audio).unwrap_or(false) {
            bail!("{id} is an audio clip; use afx for audio effects");
        }
        let mut props = Props::new();
        for (k, v) in kvs {
            let p = schema::find(def.params, k).ok_or_else(|| unknown_prop(&def.params.iter().collect::<Vec<_>>(), k, def.name))?;
            props.insert(p.name.to_string(), Anim::new(schema::clamp_val(p, schema::parse_val(p, v, cx)?)));
        }
        if def.name == "lut" {
            let f = props.get("file").map(|a| a.v.str()).unwrap_or_default();
            if f.is_empty() {
                bail!("lut needs file=<path.cube>");
            }
            if !std::path::Path::new(&f).exists() {
                bail!("LUT file not found: {f}");
            }
        }
        if def.name == "cornerpin" && !props.contains_key("tr") {
            // defaults follow the comp size
            let (w, h) = (cx.cw, cx.ch);
            props.entry("tr".into()).or_insert(Anim::new(Val::V(w, 0.0)));
            props.entry("br".into()).or_insert(Anim::new(Val::V(w, h)));
            props.entry("bl".into()).or_insert(Anim::new(Val::V(0.0, h)));
        }
        let it = self.item_mut_at(l);
        let fid = it.next_sub(if audio { "a" } else { "f" });
        let e = Effect { id: fid.clone(), kind: def.name.to_string(), on: true, props };
        if audio {
            it.afx.push(e);
        } else {
            it.fx.push(e);
        }
        Ok(fid)
    }

    pub fn c_fx(&mut self, a: &mut A, audio: bool) -> Result<()> {
        let verb = if audio { "afx" } else { "fx" };
        let spec = a.shift().ok_or_else(|| anyhow!("{verb} <id> <effect> [params] | {verb} <id.fN> param=v|del|on|off|up|down | {verb} <id> clear"))?;
        // track / master audio effects
        if audio && self.is_audio_bus(&spec) {
            return self.track_afx(&spec, a);
        }
        // one effect on a track / the master: A1.a2, master.a1
        if audio {
            if let Some((bus, sid)) = spec.split_once('.').filter(|(b, s)| self.is_audio_bus(b) && s.starts_with(['a', 'A'])) {
                return self.track_afx_one(bus, &sid.to_ascii_lowercase(), a);
            }
        }
        if let Some((id, k, sid)) = self.parse_sub(&spec) {
            let action = a.shift();
            let l = self.relocate(&id)?;
            match action.as_deref().map(|s| s.to_ascii_lowercase()) {
                Some(ref s) if s == "del" || s == "rm" || s == "delete" || s == "remove" => {
                    sub_list_mut(self.item_mut_at(l), k).retain(|e| e.id != sid);
                    self.note(format!("-{id}.{sid}"));
                }
                Some(ref s) if s == "on" || s == "off" => {
                    let on = s == "on";
                    if let Some(e) = sub_list_mut(self.item_mut_at(l), k).iter_mut().find(|e| e.id == sid) {
                        e.on = on;
                    }
                    self.note(format!("~{id}.{sid} {}", if on { "on" } else { "off" }));
                }
                Some(ref s) if s == "up" || s == "down" => {
                    let list = sub_list_mut(self.item_mut_at(l), k);
                    let i = list.iter().position(|e| e.id == sid).unwrap();
                    let j = if s == "up" { i.saturating_sub(1) } else { (i + 1).min(list.len() - 1) };
                    list.swap(i, j);
                    self.note(format!("~{id}.{sid} moved {s}"));
                }
                Some(ref s) if s == "set" || s.is_empty() => {
                    let kvs = a.rest();
                    self.set_sub(&super::exec_props::Target { item: id, sub: Some((k, sid)) }, &kvs)?;
                }
                None => {
                    let kvs = a.rest();
                    if kvs.is_empty() {
                        bail!("nothing to do for {spec}");
                    }
                    self.set_sub(&super::exec_props::Target { item: id, sub: Some((k, sid)) }, &kvs)?;
                }
                Some(other) => bail!("unknown action '{other}' (set, del, on, off, up, down)"),
            }
            return Ok(());
        }
        let name = a.shift().ok_or_else(|| anyhow!("{verb} <id> <effect> [params]"))?;
        if name.eq_ignore_ascii_case("clear") || name.eq_ignore_ascii_case("none") {
            for id in self.targets(&spec)? {
                let l = self.relocate(&id)?;
                let it = self.item_mut_at(l);
                if audio {
                    it.afx.clear();
                } else {
                    it.fx.clear();
                }
                self.note(format!("~{id} {verb} cleared"));
            }
            return Ok(());
        }
        let kvs = a.rest();
        for id in self.targets(&spec)? {
            let fid = self.add_effect(&id, &name, &kvs, audio)?;
            let canon = if audio { schema::audio_effect(&name) } else { schema::effect(&name) }.map(|d| d.name).unwrap_or("");
            self.note(format!("+{id}.{fid} {canon}"));
        }
        Ok(())
    }

    /// A track (`A1`; video tracks carry their clips' audio too, not an item id) or `master`.
    fn is_audio_bus(&self, spec: &str) -> bool {
        spec.eq_ignore_ascii_case("master") || (parse_track_spec(spec).is_some() && self.p.locate(spec).is_none())
    }

    /// The effect list of an audio track or the master bus.
    fn bus_afx(&mut self, spec: &str) -> Result<&mut Vec<Effect>> {
        let ci = self.ci();
        Ok(if spec.eq_ignore_ascii_case("master") {
            &mut self.p.comps[ci].master.afx
        } else {
            &mut self.p.comps[ci].track_mut(spec).ok_or_else(|| anyhow!("no track {spec}"))?.afx
        })
    }

    fn track_afx(&mut self, spec: &str, a: &mut A) -> Result<()> {
        let name = a.shift().ok_or_else(|| anyhow!("afx <A1|master> <effect> [params] | clear | afx <A1.a1|master.a1> param=v|del|on|off|up|down"))?;
        let cx = self.vctx();
        if name.eq_ignore_ascii_case("clear") || name.eq_ignore_ascii_case("none") {
            self.bus_afx(spec)?.clear();
            self.note(format!("~{spec} afx cleared"));
            return Ok(());
        }
        let def = schema::audio_effect(&name).ok_or_else(|| {
            let hint = schema::suggest(schema::AUDIO_EFFECTS.iter().map(|e| e.name), &name).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
            anyhow!("unknown audio effect '{name}'.{hint} (help audio)")
        })?;
        let mut props = Props::new();
        for (k, v) in a.rest() {
            let p = schema::find(def.params, &k).ok_or_else(|| unknown_prop(&def.params.iter().collect::<Vec<_>>(), &k, def.name))?;
            props.insert(p.name.to_string(), Anim::new(schema::clamp_val(p, schema::parse_val(p, &v, cx)?)));
        }
        let list = self.bus_afx(spec)?;
        // ids are never reused, even after a delete
        let n = list.iter().filter_map(|e| e.id.strip_prefix('a').and_then(|x| x.parse::<u32>().ok())).max().unwrap_or(0) + 1;
        let fid = format!("a{n}");
        list.push(Effect { id: fid.clone(), kind: def.name.to_string(), on: true, props });
        self.note(format!("+{spec}.{fid} {}", def.name));
        Ok(())
    }

    /// Edits one track / master effect: `afx A1.a2 gain=3`, `del`, `on`, `off`, `up`, `down`.
    fn track_afx_one(&mut self, bus: &str, sid: &str, a: &mut A) -> Result<()> {
        let cx = self.vctx();
        let action = a.shift().map(|s| s.to_ascii_lowercase());
        let kvs = a.rest();
        let list = self.bus_afx(bus)?;
        let Some(i) = list.iter().position(|e| e.id == sid) else {
            let have: Vec<String> = list.iter().map(|e| format!("{}.{} {}", bus, e.id, e.kind)).collect();
            bail!("{bus} has no audio effect {sid}{}", if have.is_empty() { String::new() } else { format!(" (it has: {})", have.join(", ")) });
        };
        let note = match action.as_deref() {
            Some("del" | "rm" | "delete" | "remove") => {
                list.remove(i);
                format!("-{bus}.{sid}")
            }
            Some(s @ ("on" | "off")) => {
                list[i].on = s == "on";
                format!("~{bus}.{sid} {s}")
            }
            Some(s @ ("up" | "down")) => {
                let j = if s == "up" { i.saturating_sub(1) } else { (i + 1).min(list.len() - 1) };
                list.swap(i, j);
                format!("~{bus}.{sid} moved {s}")
            }
            None | Some("set" | "") => {
                if kvs.is_empty() {
                    bail!("nothing to do for {bus}.{sid}: give param=value, del, on, off, up or down");
                }
                let def = schema::audio_effect(&list[i].kind).ok_or_else(|| anyhow!("unknown audio effect '{}'", list[i].kind))?;
                let mut changed = vec![];
                for (k, v) in &kvs {
                    if matches!(k.to_ascii_lowercase().as_str(), "on" | "enabled") {
                        list[i].on = truthy(v);
                        changed.push(format!("on={}", list[i].on));
                        continue;
                    }
                    let p = schema::find(def.params, k).ok_or_else(|| unknown_prop(&def.params.iter().collect::<Vec<_>>(), k, def.name))?;
                    list[i].props.insert(p.name.to_string(), Anim::new(schema::clamp_val(p, schema::parse_val(p, v, cx)?)));
                    changed.push(format!("{}={v}", p.name));
                }
                format!("~{bus}.{sid} {}", changed.join(" "))
            }
            Some(other) => bail!("unknown action '{other}' (set, del, on, off, up, down)"),
        };
        self.note(note);
        Ok(())
    }

    pub fn del_sub(&mut self, spec: &str) -> Result<()> {
        let (id, k, sid) = self.parse_sub(spec).ok_or_else(|| anyhow!("unknown target '{spec}'"))?;
        let l = self.relocate(&id)?;
        let list = sub_list_mut(self.item_mut_at(l), k);
        let before = list.len();
        list.retain(|e| e.id != sid);
        if list.len() == before {
            bail!("{id} has no {} {sid}", k.name());
        }
        self.note(format!("-{id}.{sid}"));
        Ok(())
    }

    // ------------------------------------------------------------ transitions

    fn build_transition(&self, kind: &str, dur: f64, kvs: &[(String, String)]) -> Result<Option<Transition>> {
        if matches!(kind.to_ascii_lowercase().as_str(), "none" | "off" | "cut") {
            return Ok(None);
        }
        let (def, extras) = schema::transition(kind).ok_or_else(|| {
            let hint = schema::suggest(schema::TRANSITIONS.iter().map(|t| t.name), kind).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
            anyhow!("unknown transition '{kind}'.{hint} (help transitions)")
        })?;
        let mut props = std::collections::BTreeMap::new();
        for (k, v) in extras {
            props.insert(k.to_string(), v);
        }
        let cx = self.vctx();
        for (k, v) in kvs {
            if k.eq_ignore_ascii_case("ease") {
                Ease::parse(v)?;
                props.insert("ease".into(), Val::S(v.clone()));
                continue;
            }
            let p = schema::find(def.params, k).ok_or_else(|| unknown_prop(&def.params.iter().collect::<Vec<_>>(), k, def.name))?;
            props.insert(p.name.to_string(), schema::parse_val(p, v, cx)?);
        }
        Ok(Some(Transition { kind: def.name.to_string(), dur, props }))
    }

    pub(crate) fn adjacent_next(&self, id: &str) -> Option<String> {
        let (ci, ti, _) = self.p.locate(id)?;
        let it = self.p.item(id)?;
        let fps = self.p.comps[ci].fps;
        self.p.comps[ci].tracks[ti].items.iter().find(|o| o.id != id && (o.start - it.end()).abs() < 0.5 / fps).map(|o| o.id.clone())
    }

    pub fn c_tr(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("tr <a> <b> <kind> [dur] | tr <a> in|out <kind> [dur] | tr <V1> <kind> [dur]"))?;
        let dur_kv = a.take(&["dur", "duration", "d"]);
        // track-wide
        if (parse_track_spec(&spec).is_some() && self.p.locate(&spec).is_none()) || spec.eq_ignore_ascii_case("all") {
            let kind = a.shift().ok_or_else(|| anyhow!("tr <track> <kind> [dur]"))?;
            let dur = match dur_kv.or_else(|| a.shift()) {
                Some(d) => self.time(&d)?,
                None => 0.5,
            };
            let kvs = a.rest();
            let tr = self.build_transition(&kind, dur, &kvs)?;
            let tids: Vec<String> = if spec.eq_ignore_ascii_case("all") {
                self.comp().vtracks().map(|t| t.id.clone()).collect()
            } else {
                vec![self.comp().track(&spec).ok_or_else(|| anyhow!("no track {spec}"))?.id.clone()]
            };
            let mut n = 0;
            for tid in tids {
                let ids: Vec<String> = self.comp().track(&tid).unwrap().items.iter().map(|i| i.id.clone()).collect();
                for id in ids {
                    if let Some(next) = self.adjacent_next(&id) {
                        let d = dur.min(self.item_span(&id).map(|s| s.1 - s.0).unwrap_or(dur)).min(self.item_span(&next).map(|s| s.1 - s.0).unwrap_or(dur));
                        let l = self.relocate(&id)?;
                        self.item_mut_at(l).tout = tr.clone().map(|mut t| {
                            t.dur = d;
                            t
                        });
                        n += 1;
                    }
                }
            }
            self.note(format!("~{spec} {} transition(s) {kind}", n));
            return Ok(());
        }
        let a_id = self.item_id(&spec)?;
        let second = a.shift().ok_or_else(|| anyhow!("tr <a> <b|in|out> <kind> [dur]"))?;
        let (edge, b_id, kind) = match second.to_ascii_lowercase().as_str() {
            "in" | "out" => (Some(second.to_ascii_lowercase()), None, a.shift().ok_or_else(|| anyhow!("missing transition kind"))?),
            _ => {
                if let Ok(b) = self.item_id(&second) {
                    (None, Some(b), a.shift().ok_or_else(|| anyhow!("missing transition kind"))?)
                } else {
                    // `tr c1 dissolve` -> to the adjacent next clip, or out
                    match self.adjacent_next(&a_id) {
                        Some(n) => (None, Some(n), second.clone()),
                        None => (Some("out".into()), None, second.clone()),
                    }
                }
            }
        };
        let mut dur = match dur_kv.or_else(|| a.shift()) {
            Some(d) => self.time(&d)?,
            None => 0.5,
        };
        let kvs = a.rest();
        let a_span = self.item_span(&a_id).unwrap();
        match (edge, b_id) {
            (Some(e), _) => {
                dur = dur.min(a_span.1 - a_span.0);
                let tr = self.build_transition(&kind, dur, &kvs)?;
                let l = self.relocate(&a_id)?;
                if e == "in" {
                    self.item_mut_at(l).tin = tr;
                } else {
                    self.item_mut_at(l).tout = tr;
                }
                self.note(format!("~{a_id} {e}={kind} {}", fmt_num(dur)));
            }
            (None, Some(b)) => {
                let (la, lb) = (self.relocate(&a_id)?, self.relocate(&b)?);
                if la.0 != lb.0 || la.1 != lb.1 {
                    bail!("{a_id} and {b} must be on the same track");
                }
                let (first, second) = if self.item_at(la).start <= self.item_at(lb).start { (a_id.clone(), b.clone()) } else { (b.clone(), a_id.clone()) };
                let fs = self.item_span(&first).unwrap();
                let ss = self.item_span(&second).unwrap();
                if (fs.1 - ss.0).abs() > 0.5 / self.fps() {
                    bail!("{first} and {second} aren't adjacent (gap of {}s); move them together first", fmt_t(ss.0 - fs.1));
                }
                let maxd = (fs.1 - fs.0).min(ss.1 - ss.0);
                if dur > maxd {
                    self.warn(format!("transition shortened to {}s to fit the clips", fmt_t(maxd)));
                    dur = maxd;
                }
                let tr = self.build_transition(&kind, dur, &kvs)?;
                let l = self.relocate(&first)?;
                self.item_mut_at(l).tout = tr;
                let l2 = self.relocate(&second)?;
                self.item_mut_at(l2).tin = None;
                self.note(format!("~{first}>{second} {kind} {}", fmt_num(dur)));
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    pub fn c_fade(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("fade <ids> [in=s] [out=s] [video|audio]"))?;
        let only_video = a.flag("video");
        let only_audio = a.flag("audio");
        let both = a.shift();
        let fin = a.take(&["in"]).or_else(|| both.clone()).map(|s| self.time(&s)).transpose()?;
        let fout = a.take(&["out"]).or(both).map(|s| self.time(&s)).transpose()?;
        if fin.is_none() && fout.is_none() {
            bail!("fade needs in= and/or out=");
        }
        for id in self.targets(&spec)? {
            let l = self.relocate(&id)?;
            let it = self.item_at(l).clone();
            let has_audio = match it.kind {
                ItemKind::Clip => it.src.as_deref().and_then(|s| self.p.media(s)).map(|m| m.has_audio).unwrap_or(false),
                ItemKind::Comp => true,
                _ => false,
            };
            let is_visual = it.kind != ItemKind::Clip || it.src.as_deref().and_then(|s| self.p.media(s)).map(|m| m.is_visual()).unwrap_or(false);
            let m = self.item_mut_at(l);
            if !only_audio && is_visual {
                if let Some(d) = fin {
                    m.tin = if d > 0.0 { Some(Transition { kind: "dissolve".into(), dur: d.min(it.dur), props: Default::default() }) } else { None };
                }
                if let Some(d) = fout {
                    m.tout = if d > 0.0 { Some(Transition { kind: "dissolve".into(), dur: d.min(it.dur), props: Default::default() }) } else { None };
                }
            }
            if !only_video && has_audio {
                if let Some(d) = fin {
                    m.props.insert("fadeIn".into(), Anim::new(Val::N(d)));
                }
                if let Some(d) = fout {
                    m.props.insert("fadeOut".into(), Anim::new(Val::N(d)));
                }
            }
            self.note(format!("~{id} fade in={} out={}", fin.map(fmt_num).unwrap_or("-".into()), fout.map(fmt_num).unwrap_or("-".into())));
        }
        Ok(())
    }

    // ------------------------------------------------------------ masks / mattes / parenting

    pub fn c_mask(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("mask <id> <rect|ellipse|path|polygon|star> [x y w h feather invert ...] | mask <id.mN> ... | mask <id> clear"))?;
        if self.parse_sub(&spec).is_some() {
            let (id, k, sid) = self.parse_sub(&spec).unwrap();
            if k != SubKind::Mask {
                bail!("{spec} is not a mask");
            }
            if a.flag("del") || a.flag("delete") || a.flag("rm") {
                return self.del_sub(&spec);
            }
            a.flag("set");
            let inv = a.flag("invert");
            let mut kvs = a.rest();
            if inv {
                kvs.push(("invert".into(), "1".into()));
            }
            return self.set_sub(&Target { item: id, sub: Some((k, sid)) }, &kvs);
        }
        let kind = a.shift().ok_or_else(|| anyhow!("mask needs a shape (rect ellipse path polygon star)"))?.to_ascii_lowercase();
        if kind == "clear" || kind == "none" {
            for id in self.targets(&spec)? {
                let l = self.relocate(&id)?;
                self.item_mut_at(l).masks.clear();
                self.note(format!("~{id} masks cleared"));
            }
            return Ok(());
        }
        let kind = match kind.as_str() {
            "circle" | "oval" => "ellipse".to_string(),
            "square" | "box" => "rect".to_string(),
            k => k.to_string(),
        };
        if !schema::MASK_KINDS.contains(&kind.as_str()) {
            bail!("unknown mask shape '{kind}' ({})", schema::MASK_KINDS.join(" "));
        }
        let inv = a.flag("invert");
        let mut kvs = a.rest();
        if inv {
            kvs.push(("invert".into(), "1".into()));
        }
        for id in self.targets(&spec)? {
            let l = self.relocate(&id)?;
            let cx = self.vctx_of(l.0);
            let mut props = Props::new();
            for (k, v) in &kvs {
                let p = schema::find(schema::MASK, k).ok_or_else(|| unknown_prop(&schema::MASK.iter().collect::<Vec<_>>(), k, "mask"))?;
                props.insert(p.name.to_string(), Anim::new(schema::clamp_val(p, schema::parse_val(p, v, cx)?)));
            }
            if kind == "path" && !props.contains_key("d") {
                bail!("path masks need d='M0.1,0.1 L0.9,0.1 L0.5,0.9 Z' (normalized 0..1 coords)");
            }
            let it = self.item_mut_at(l);
            let mid = it.next_sub("m");
            it.masks.push(Effect { id: mid.clone(), kind: kind.clone(), on: true, props });
            self.note(format!("+{id}.{mid} mask {kind}"));
        }
        Ok(())
    }

    pub fn c_matte(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("matte <id> <src id> [alpha|alphainv|luma|lumainv] | matte <id> none"))?)?;
        let src = a.take(&["src", "by", "source"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("matte needs a source layer"))?;
        let l = self.relocate(&id)?;
        if matches!(src.to_ascii_lowercase().as_str(), "none" | "off") {
            self.item_mut_at(l).matte = None;
            self.note(format!("~{id} matte off"));
            return Ok(());
        }
        let sid = self.item_id(&src)?;
        if sid == id {
            bail!("a layer can't be its own matte");
        }
        let ls = self.relocate(&sid)?;
        if ls.0 != l.0 {
            bail!("matte source must be in the same comp");
        }
        let mode = match a.take(&["mode"]).or_else(|| a.shift()).unwrap_or_else(|| "alpha".into()).to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "alpha" => MatteMode::Alpha,
            "alphainv" | "alphainverted" | "inverted" | "inv" => MatteMode::AlphaInv,
            "luma" => MatteMode::Luma,
            "lumainv" | "lumainverted" => MatteMode::LumaInv,
            o => bail!("matte mode must be alpha|alphainv|luma|lumainv, not {o}"),
        };
        self.item_mut_at(l).matte = Some(Matte { src: sid.clone(), mode });
        self.note(format!("~{id} matte {sid} {:?}", mode).to_lowercase());
        Ok(())
    }

    pub fn c_parent(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("parent <ids> <parent|none>"))?;
        let parent = a.shift().ok_or_else(|| anyhow!("parent <ids> <parent|none>"))?;
        for id in self.targets(&spec)? {
            self.set_parent(&id, &parent)?;
            self.note(format!("~{id} parent={parent}"));
        }
        Ok(())
    }

    pub fn set_parent(&mut self, id: &str, parent: &str) -> Result<()> {
        let l = self.relocate(id)?;
        let cx = self.vctx_of(l.0);
        if matches!(parent.to_ascii_lowercase().as_str(), "none" | "off" | "-") {
            // convert back to comp space when possible
            let it = self.item_at(l).clone();
            if let Some(pid) = &it.parent {
                if let Some(pm) = self.static_parent_matrix(pid, cx) {
                    let m = self.item_mut_at(l);
                    convert_child(m, &pm, cx, false);
                }
            }
            self.item_mut_at(l).parent = None;
            return Ok(());
        }
        let pid = self.item_id(parent)?;
        if pid == id {
            bail!("a layer can't parent itself");
        }
        let lp = self.relocate(&pid)?;
        if lp.0 != l.0 {
            bail!("parent must be in the same comp");
        }
        // cycle check
        let mut cur = Some(pid.clone());
        let mut depth = 0;
        while let Some(c) = cur {
            if c == id {
                bail!("parenting {id} to {pid} would create a loop");
            }
            cur = self.p.item(&c).and_then(|i| i.parent.clone());
            depth += 1;
            if depth > 64 {
                break;
            }
        }
        if let Some(pm) = self.static_parent_matrix(&pid, cx) {
            let m = self.item_mut_at(l);
            convert_child(m, &pm, cx, true);
        }
        self.item_mut_at(l).parent = Some(pid);
        Ok(())
    }

    /// (pos, rot, scale) of a parent chain when fully static.
    fn static_parent_matrix(&self, pid: &str, cx: schema::VCtx) -> Option<[f64; 6]> {
        let mut m = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut cur = Some(pid.to_string());
        let mut depth = 0;
        while let Some(c) = cur {
            let it = self.p.item(&c)?;
            for k in ["pos", "rot", "scale"] {
                if it.props.get(k).map(|a| a.is_animated()).unwrap_or(false) {
                    return None;
                }
            }
            let pos = it.props.get("pos").map(|a| a.v.vec()).unwrap_or((cx.cw / 2.0, cx.ch / 2.0));
            let rot = it.props.get("rot").map(|a| a.v.num()).unwrap_or(0.0);
            let sc = it.props.get("scale").map(|a| a.v.vec()).unwrap_or((1.0, 1.0));
            let local = affine(pos, rot, sc);
            m = mul(&local, &m);
            cur = it.parent.clone();
            depth += 1;
            if depth > 64 {
                return None;
            }
        }
        Some(m)
    }

    // ------------------------------------------------------------ color

    pub fn c_color(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("color <ids> <preset|none> | color <ids> exposure=.. contrast=.. temp=.. sat=.."))?;
        let preset = a.shift();
        let kvs = a.rest();
        if let Some(p) = &preset {
            if !matches!(p.to_ascii_lowercase().as_str(), "none" | "off" | "reset") && presets::color_preset(p).is_none() {
                let hint = schema::suggest(presets::COLOR_PRESETS.iter().map(|(n, _)| *n), p).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
                bail!("unknown color preset '{p}'.{hint} (help color)");
            }
        }
        for id in self.targets(&spec)? {
            let l = self.relocate(&id)?;
            if let Some(p) = &preset {
                if matches!(p.to_ascii_lowercase().as_str(), "none" | "off" | "reset") {
                    // the look's extra effects go too, unless they were changed by hand
                    if let Some(prev) = self.item_at(l).fx.iter().find(|e| e.kind == "lumetri").map(|e| e.props.get("look").map(|a| a.v.str()).unwrap_or_default()) {
                        self.remove_look_extras(&id, &prev)?;
                    }
                    let l = self.relocate(&id)?;
                    self.item_mut_at(l).fx.retain(|e| e.kind != "lumetri");
                    self.note(format!("~{id} color reset"));
                    continue;
                }
            }
            let mut params: Vec<(String, String)> = vec![];
            let mut extras = vec![];
            // find or create lumetri
            let it = self.item_at(l).clone();
            let fid = match it.fx.iter().find(|e| e.kind == "lumetri") {
                Some(e) => e.id.clone(),
                None => self.add_effect(&id, "lumetri", &[], false)?,
            };
            if let Some(p) = &preset {
                let (lp, ex) = presets::color_preset(p).expect("checked above");
                // a new look replaces the previous one; hand-made changes stay
                let prev = it.fx.iter().find(|e| e.id == fid).and_then(|e| e.props.get("look")).map(|a| a.v.str()).unwrap_or_default();
                if !prev.is_empty() {
                    self.unapply_look(&id, &fid, &prev)?;
                }
                params.extend(lp.into_iter().map(|(k, v)| (k.to_string(), v.str())));
                params.push(("look".into(), p.to_ascii_lowercase()));
                extras = ex;
            }
            params.extend(kvs.iter().cloned());
            self.set_sub(&Target { item: id.clone(), sub: Some((SubKind::Fx, fid)) }, &params)?;
            for (ename, eparams) in extras {
                let l = self.relocate(&id)?;
                if self.item_at(l).fx.iter().any(|e| e.kind == ename) {
                    continue;
                }
                let kv: Vec<(String, String)> = eparams.into_iter().map(|(k, v)| (k.to_string(), v.str())).collect();
                let f = self.add_effect(&id, ename, &kv, false)?;
                self.note(format!("+{id}.{f} {ename}"));
            }
        }
        Ok(())
    }

    /// True when `e` still holds exactly what a look preset set for `params`
    /// (static values, nothing changed by hand since).
    fn holds_preset_values(e: &Effect, def: &schema::EffectDef, params: &[(&'static str, Val)], cx: schema::VCtx) -> bool {
        params.iter().all(|(k, v)| {
            let Some(p) = schema::find(def.params, k) else { return false };
            let Ok(want) = schema::parse_val(p, &v.str(), cx).map(|x| schema::clamp_val(p, x)) else { return false };
            e.props.get(p.name).is_some_and(|a| !a.is_animated() && a.v == want)
        })
    }

    /// Takes a previous look off the lumetri `fid` of `id`: its values go back to
    /// the defaults where nobody changed them, and its untouched extra effects go.
    fn unapply_look(&mut self, id: &str, fid: &str, look: &str) -> Result<()> {
        let Some((lp, _)) = presets::color_preset(look) else { return Ok(()) };
        let cx = self.vctx();
        let def = schema::effect("lumetri").ok_or_else(|| anyhow!("no lumetri effect"))?;
        let l = self.relocate(id)?;
        if let Some(e) = self.item_mut_at(l).fx.iter_mut().find(|e| e.id == fid) {
            for (k, v) in &lp {
                if Self::holds_preset_values(e, def, &[(k, v.clone())], cx) {
                    e.props.remove(*k);
                }
            }
            e.props.remove("look");
        }
        self.remove_look_extras(id, look)
    }

    /// Removes the extra effects (vignette, grain, curves...) a look added, if unchanged.
    fn remove_look_extras(&mut self, id: &str, look: &str) -> Result<()> {
        let Some((_, extras)) = presets::color_preset(look) else { return Ok(()) };
        let cx = self.vctx();
        let l = self.relocate(id)?;
        let mut gone = vec![];
        for (ename, eparams) in &extras {
            let Some(def) = schema::effect(ename) else { continue };
            let it = self.item_at(l);
            if let Some(e) = it.fx.iter().find(|e| e.kind == def.name && e.props.len() == eparams.len() && Self::holds_preset_values(e, def, eparams, cx)) {
                gone.push(e.id.clone());
            }
        }
        if !gone.is_empty() {
            self.item_mut_at(l).fx.retain(|e| !gone.contains(&e.id));
            for g in gone {
                self.note(format!("-{id}.{g}"));
            }
        }
        Ok(())
    }

    pub fn c_lut(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("lut <ids> <file.cube> [amount=1]"))?;
        let file = a.take(&["file", "path"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("lut needs a .cube file"))?;
        let amount = a.take(&["amount", "intensity"]).or_else(|| a.shift());
        let mut kv = vec![("file".to_string(), file)];
        if let Some(am) = amount {
            kv.push(("amount".into(), am));
        }
        for id in self.targets(&spec)? {
            let f = self.add_effect(&id, "lut", &kv, false)?;
            self.note(format!("+{id}.{f} lut"));
        }
        Ok(())
    }
}

pub(crate) fn truthy(v: &str) -> bool {
    !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")
}

/// 2D affine [a b c d e f] mapping (x,y) -> (a x + c y + e, b x + d y + f).
fn affine(pos: (f64, f64), rot_deg: f64, sc: (f64, f64)) -> [f64; 6] {
    let r = rot_deg.to_radians();
    let (s, c) = r.sin_cos();
    [c * sc.0, s * sc.0, -s * sc.1, c * sc.1, pos.0, pos.1]
}

/// m1 ∘ m2 (apply m2 first, then m1)... here: returns parent ∘ local chain where
/// `local` is applied first.
fn mul(local: &[f64; 6], acc: &[f64; 6]) -> [f64; 6] {
    // acc is the transform of the child-side so far; result = local ∘ acc
    let [a1, b1, c1, d1, e1, f1] = *local;
    let [a2, b2, c2, d2, e2, f2] = *acc;
    [a1 * a2 + c1 * b2, b1 * a2 + d1 * b2, a1 * c2 + c1 * d2, b1 * c2 + d1 * d2, a1 * e2 + c1 * f2 + e1, b1 * e2 + d1 * f2 + f1]
}

fn invert(m: &[f64; 6]) -> Option<[f64; 6]> {
    let [a, b, c, d, e, f] = *m;
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return None;
    }
    let ia = d / det;
    let ib = -b / det;
    let ic = -c / det;
    let id = a / det;
    Some([ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)])
}

/// Converts a child's static transform into (to_parent=true) or out of the parent's space.
fn convert_child(it: &mut Item, pm: &[f64; 6], cx: schema::VCtx, to_parent: bool) {
    for k in ["pos", "rot", "scale"] {
        if it.props.get(k).map(|a| a.is_animated()).unwrap_or(false) {
            return;
        }
    }
    let pos = it.props.get("pos").map(|a| a.v.vec()).unwrap_or((cx.cw / 2.0, cx.ch / 2.0));
    let rot = it.props.get("rot").map(|a| a.v.num()).unwrap_or(0.0);
    let sc = it.props.get("scale").map(|a| a.v.vec()).unwrap_or((1.0, 1.0));
    let prot = pm[1].atan2(pm[0]).to_degrees();
    let psx = (pm[0] * pm[0] + pm[1] * pm[1]).sqrt();
    let psy = (pm[2] * pm[2] + pm[3] * pm[3]).sqrt();
    let (np, nr, ns) = if to_parent {
        let Some(inv) = invert(pm) else { return };
        let x = inv[0] * pos.0 + inv[2] * pos.1 + inv[4];
        let y = inv[1] * pos.0 + inv[3] * pos.1 + inv[5];
        ((x, y), rot - prot, (sc.0 / psx.max(1e-9), sc.1 / psy.max(1e-9)))
    } else {
        let x = pm[0] * pos.0 + pm[2] * pos.1 + pm[4];
        let y = pm[1] * pos.0 + pm[3] * pos.1 + pm[5];
        ((x, y), rot + prot, (sc.0 * psx, sc.1 * psy))
    };
    it.props.insert("pos".into(), Anim::new(Val::V(np.0, np.1)));
    if nr.abs() > 1e-9 || it.props.contains_key("rot") {
        it.props.insert("rot".into(), Anim::new(Val::N(nr)));
    }
    if (ns.0 - 1.0).abs() > 1e-9 || (ns.1 - 1.0).abs() > 1e-9 || it.props.contains_key("scale") {
        it.props.insert("scale".into(), Anim::new(Val::V(ns.0, ns.1)));
    }
}
