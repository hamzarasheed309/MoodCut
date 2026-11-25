//! Grading verbs that work across shots: input color transforms and white
//! balance (`color ... input= wb=`), shared grades (`grade`), reference
//! stills (`still`) and shot matching (`match`). Everything they write is an
//! ordinary effect or property, so it stays editable and undoable.

use super::exec::{Exec, A};
use crate::model::anim::Anim;
use crate::model::project::*;
use crate::model::schema;
use crate::model::time::fmt_t;
use crate::model::value::fmt_num;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;

/// Per-channel lift / gamma / gain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lgg {
    pub lift: [f64; 3],
    pub gamma: [f64; 3],
    pub gain: [f64; 3],
}

impl Lgg {
    pub const IDENTITY: Lgg = Lgg { lift: [0.0; 3], gamma: [1.0; 3], gain: [1.0; 3] };
}

fn lw(v: [f64; 3]) -> f64 {
    0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]
}

fn arr(v: &Value) -> [f64; 3] {
    let a = |i: usize| v.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0);
    [a(0), a(1), a(2)]
}

/// Fits y = (A + B x)^(1/gamma) through (shadow, mid, highlight) means of one
/// channel, returning (lift, gamma, gain) in the colorwheels model
/// y = (gain * (x + lift * (1 - x)))^(1/gamma).
pub fn fit_channel(x: [f64; 3], y: [f64; 3]) -> (f64, f64, f64) {
    let (xs, xm, xh) = (x[0], x[1], x[2]);
    if (xh - xs).abs() < 1e-3 || y.iter().any(|v| *v <= 1e-4) {
        return (0.0, 1.0, if xm > 1e-4 { (y[1] / xm).clamp(0.2, 4.0) } else { 1.0 });
    }
    let mut g = 1.0f64;
    let (mut a, mut b) = (0.0, 1.0);
    for _ in 0..8 {
        let (ys, yh) = (y[0].powf(g), y[2].powf(g));
        b = (yh - ys) / (xh - xs);
        a = ys - b * xs;
        let base = (a + b * xm).max(1e-4);
        let ng = if (y[1] - 1.0).abs() > 1e-4 && (base - 1.0).abs() > 1e-4 { base.ln() / y[1].ln() } else { 1.0 };
        g = ng.clamp(0.4, 2.5);
    }
    let gain = (a + b).clamp(0.2, 4.0);
    let lift = (a / gain).clamp(-0.5, 0.5);
    (lift, g, gain)
}

/// Solves the correction taking `src` stats to `refr` stats.
pub fn solve(src: &Value, refr: &Value, exposure: bool, balance: bool) -> Lgg {
    let bands = ["shadows", "mids", "highs"];
    let sx: Vec<[f64; 3]> = bands.iter().map(|b| arr(&src[*b])).collect();
    let ry: Vec<[f64; 3]> = bands.iter().map(|b| arr(&refr[*b])).collect();
    let mut out = Lgg::IDENTITY;
    // luma-only fit (exposure)
    let lum = fit_channel([lw(sx[0]), lw(sx[1]), lw(sx[2])], [lw(ry[0]), lw(ry[1]), lw(ry[2])]);
    for c in 0..3 {
        let ch = fit_channel([sx[0][c], sx[1][c], sx[2][c]], [ry[0][c], ry[1][c], ry[2][c]]);
        let (l, g, k) = match (exposure, balance) {
            (true, true) => ch,
            (true, false) => lum,
            // balance only: the channel's difference from the luma fit
            (false, true) => (ch.0 - lum.0, 1.0 + (ch.1 - lum.1), 1.0 + (ch.2 - lum.2)),
            (false, false) => (0.0, 1.0, 1.0),
        };
        out.lift[c] = l;
        out.gamma[c] = g;
        out.gain[c] = k;
    }
    out
}

fn hex(v: [f64; 3]) -> String {
    let b = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}ff", b(v[0]), b(v[1]), b(v[2]))
}

/// A wheel color whose chroma push is `s` (luma-weighted sum zero), scaled
/// into range; and the brightness slider for the luma part.
fn wheel_of(v: [f64; 3], base: f64, k_chroma: f64, k_level: f64) -> (String, f64) {
    let m = lw(v);
    let mut s = [(v[0] - m) / k_chroma, (v[1] - m) / k_chroma, (v[2] - m) / k_chroma];
    let big = s.iter().map(|x| x.abs()).fold(0.0, f64::max);
    if big > 0.5 {
        s.iter_mut().for_each(|x| *x *= 0.5 / big);
    }
    (hex([0.5 + s[0], 0.5 + s[1], 0.5 + s[2]]), ((m - base) / k_level * 100.0).clamp(-100.0, 100.0))
}

/// colorwheels key=values for a lift/gamma/gain correction.
pub fn wheels_kvs(c: &Lgg, sat: f64, tag: &str) -> Vec<(String, String)> {
    let (sh, sl) = wheel_of(c.lift, 0.0, 0.6, 0.25);
    let (mi, ml) = wheel_of(c.gamma, 1.0, 1.6, 0.6);
    let (hi, hl) = wheel_of(c.gain, 1.0, 1.2, 0.5);
    let f = |x: f64| fmt_num((x * 100.0).round() / 100.0);
    vec![
        ("shadows".into(), sh),
        ("midtones".into(), mi),
        ("highlights".into(), hi),
        ("shadowsL".into(), f(sl)),
        ("midtonesL".into(), f(ml)),
        ("highlightsL".into(), f(hl)),
        ("sat".into(), f((sat * 100.0).clamp(0.0, 400.0))),
        ("tag".into(), tag.into()),
    ]
}

/// Mean absolute difference (0..1) between two measurements' band means.
pub fn residual(a: &Value, b: &Value) -> f64 {
    let mut d = 0.0;
    for band in ["shadows", "mids", "highs"] {
        let (x, y) = (arr(&a[band]), arr(&b[band]));
        d += (0..3).map(|c| (x[c] - y[c]).abs()).sum::<f64>();
    }
    d / 9.0
}

impl<'a> Exec<'a> {
    /// Times (comp seconds) to measure a layer at: 25%, 50% and 75% through it.
    fn sample_times(&self, id: &str) -> Result<Vec<f64>> {
        let l = self.relocate(id)?;
        let it = self.item_at(l);
        Ok([0.25, 0.5, 0.75].iter().map(|f| it.start + it.dur * f).collect())
    }

    /// Writes (or rewrites) the colorwheels effect tagged `tag` on `id`;
    /// `first` puts a new one at the start of the chain.
    fn put_wheels(&mut self, id: &str, tag: &str, kvs: &[(String, String)], first: bool) -> Result<String> {
        let l = self.relocate(id)?;
        let cx = self.vctx_of(l.0);
        let def = schema::effect("colorwheels").unwrap();
        let params = schema::effect_params(def);
        let mut props = Props::new();
        for (k, v) in kvs {
            let p = params.iter().find(|p| p.name == k.as_str()).ok_or_else(|| anyhow!("colorwheels has no {k}"))?;
            props.insert(p.name.to_string(), Anim::new(schema::clamp_val(p, schema::parse_val(p, v, cx)?)));
        }
        let it = self.item_mut_at(l);
        if let Some(e) = it.fx.iter_mut().find(|e| e.kind == "colorwheels" && e.props.get("tag").is_some_and(|t| t.v.str() == tag)) {
            e.props = props;
            return Ok(e.id.clone());
        }
        let fid = it.next_sub("f");
        let e = Effect { id: fid.clone(), kind: "colorwheels".into(), on: true, props };
        if first {
            it.fx.insert(0, e);
        } else {
            it.fx.push(e);
        }
        Ok(fid)
    }

    fn tagged_fx(&self, id: &str, tag: &str) -> Option<String> {
        let l = self.relocate(id).ok()?;
        self.item_at(l).fx.iter().find(|e| e.kind == "colorwheels" && e.props.get("tag").is_some_and(|t| t.v.str() == tag)).map(|e| e.id.clone())
    }

    /// `color <ids> input=<profile>` and `wb=auto|pick:x,y|off`.
    pub fn color_extras(&mut self, ids: &[String], input: Option<String>, wb: Option<String>) -> Result<()> {
        for id in ids {
            if let Some(inp) = &input {
                let p = schema::find(schema::CLIP, "input").unwrap();
                let l = self.relocate(id)?;
                if self.item_at(l).kind != ItemKind::Clip {
                    bail!("{id} isn't a clip: input transforms apply to footage");
                }
                let v = schema::parse_val(p, inp, self.vctx_of(l.0))?;
                self.item_mut_at(l).props.insert("input".into(), Anim::new(v));
                self.note(format!("~{id} input={inp}"));
            }
            if let Some(w) = &wb {
                let lw_ = w.to_ascii_lowercase();
                if matches!(lw_.as_str(), "off" | "none" | "reset") {
                    if let Some(fid) = self.tagged_fx(id, "wb") {
                        let l = self.relocate(id)?;
                        self.item_mut_at(l).fx.retain(|e| e.id != fid);
                    }
                    self.note(format!("~{id} wb off"));
                    continue;
                }
                let region = match lw_.strip_prefix("pick:") {
                    Some(xy) => {
                        let (x, y) = xy.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))).ok_or_else(|| anyhow!("wb=pick:x,y (fractions of the frame, a point that should be neutral gray or white)"))?;
                        Some((x, y, 0.02, 0.02))
                    }
                    None if lw_ == "auto" => None,
                    None => bail!("wb=auto | pick:x,y | off"),
                };
                let times = self.sample_times(id)?;
                let old = self.tagged_fx(id, "wb");
                let st = self.env.measure(self.p, id, if region.is_some() { &times[1..2] } else { &times[..] }, old.as_deref(), region)?;
                // near-neutral pixels (or, for a picked patch, its mean)
                let n = arr(&st["neutral"]);
                let m = lw([n[0], n[1], n[2]]);
                if n.iter().any(|v| *v < 0.02) {
                    bail!("{id}: that point is too dark to balance on; pick something light and neutral");
                }
                let gain = [m / n[0], m / n[1], m / n[2]].map(|g| g.clamp(0.4, 2.5));
                let c = Lgg { lift: [0.0; 3], gamma: [1.0; 3], gain };
                let kvs = wheels_kvs(&c, 1.0, "wb");
                let fid = self.put_wheels(id, "wb", &kvs, true)?;
                self.note(format!("~{id}.{fid} wb gains r{:.2} g{:.2} b{:.2} (neutral was {:.0},{:.0},{:.0})", gain[0], gain[1], gain[2], n[0] * 255.0, n[1] * 255.0, n[2] * 255.0));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ shared grades

    /// `grade save <name> from=c1` | `grade apply <name> <ids>` | `grade detach <ids>` | `grade del <name>` | `grade list`
    pub fn c_grade(&mut self, a: &mut A) -> Result<()> {
        /// A color effect: copied, reset and versioned as part of the grade.
        fn is_color(e: &Effect) -> bool {
            schema::effect(&e.kind).is_some_and(|d| d.cat == "color")
        }
        let usage = "grade save <name> from=<clip> | grade apply <name> <ids> [pre] | grade detach <ids> [pre] | grade copy <from> <ids> [append] | grade version <clip> save|load|del <name> | grade reset <ids> | grade nodes <clip> | grade del <name> | grade list";
        let sub = a.shift().ok_or_else(|| anyhow!(usage))?.to_ascii_lowercase();
        match sub.as_str() {
            "save" | "new" | "make" => {
                let name = a.shift().ok_or_else(|| anyhow!(usage))?;
                let from = a.take(&["from", "src"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("grade save <name> from=<clip>"))?;
                let id = self.item_id(&from)?;
                let l = self.relocate(&id)?;
                // the clip's color effects move into the grade, and the clip links to it
                let it = self.item_mut_at(l);
                let (color, rest): (Vec<Effect>, Vec<Effect>) = it.fx.drain(..).partition(|e| schema::effect(&e.kind).is_some_and(|d| d.cat == "color") && !e.props.get("tag").is_some_and(|t| t.v.str() == "match" || t.v.str() == "wb"));
                it.fx = rest;
                let linked = it.grade.clone();
                let mut color = color;
                if let Some(prev) = linked.and_then(|g| self.p.grades.get(&g).cloned()) {
                    // re-saving from a linked clip keeps the shared part
                    let mut merged = prev;
                    merged.extend(color);
                    color = merged;
                }
                if color.is_empty() {
                    bail!("{id} has no color effects to save as a grade");
                }
                let n = color.len();
                self.p.grades.insert(name.clone(), color);
                let l = self.relocate(&id)?;
                self.item_mut_at(l).grade = Some(name.clone());
                self.note(format!("+grade {name} ({n} effects) from {id}; {id} now uses it"));
            }
            "apply" | "link" | "use" => {
                let name = a.shift().ok_or_else(|| anyhow!(usage))?;
                if !self.p.grades.contains_key(&name) {
                    bail!("no grade '{name}' ({})", self.p.grades.keys().cloned().collect::<Vec<_>>().join(" "));
                }
                let pre = a.flag("pre") || a.flag("preclip") || a.take(&["group"]).is_some_and(|g| g == "pre");
                let spec = a.shift().ok_or_else(|| anyhow!("grade apply {name} <ids> [pre]"))?;
                for id in self.targets(&spec)? {
                    let l = self.relocate(&id)?;
                    let it = self.item_mut_at(l);
                    if pre {
                        it.pregrade = Some(name.clone());
                    } else {
                        it.grade = Some(name.clone());
                    }
                    self.note(format!("~{id} {}={name}", if pre { "pregrade" } else { "grade" }));
                }
            }
            "detach" | "unlink" => {
                let pre = a.flag("pre") || a.flag("preclip");
                let spec = a.shift().ok_or_else(|| anyhow!("grade detach <ids> [pre]"))?;
                for id in self.targets(&spec)? {
                    let l = self.relocate(&id)?;
                    let linked = if pre { self.item_at(l).pregrade.clone() } else { self.item_at(l).grade.clone() };
                    let Some(g) = linked else { continue };
                    let fx = self.p.grades.get(&g).cloned().unwrap_or_default();
                    let it = self.item_mut_at(l);
                    let mut copied = Vec::with_capacity(fx.len());
                    for mut e in fx {
                        e.id = it.next_sub("f");
                        copied.push(e);
                    }
                    if pre {
                        // a pre-clip grade ran first: its effects go before the clip's own
                        copied.append(&mut it.fx);
                        it.fx = copied;
                        it.pregrade = None;
                    } else {
                        it.fx.extend(copied);
                        it.grade = None;
                    }
                    self.note(format!("~{id} detached from {g} (its effects copied)"));
                }
            }
            "copy" | "paste" | "from" => {
                // Resolve's copy grade: the source clip's color effects, unlinked
                let from = a.take(&["from", "src"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("grade copy <from clip> <ids> [append]"))?;
                let spec = a.take(&["to"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("grade copy <from clip> <ids> [append]"))?;
                let append = a.flag("append") || a.flag("add");
                let src_id = self.item_id(&from)?;
                let ls = self.relocate(&src_id)?;
                let src = self.item_at(ls).clone();
                let color: Vec<Effect> = src.fx.iter().filter(|e| is_color(e)).cloned().collect();
                if color.is_empty() && src.grade.is_none() && src.pregrade.is_none() {
                    bail!("{src_id} has no grade to copy");
                }
                for id in self.targets(&spec)? {
                    if id == src_id {
                        continue;
                    }
                    let l = self.relocate(&id)?;
                    let it = self.item_mut_at(l);
                    if !append {
                        it.fx.retain(|e| !is_color(e));
                    }
                    for e in &color {
                        let mut e = e.clone();
                        e.id = it.next_sub("f");
                        it.fx.push(e);
                    }
                    it.grade = src.grade.clone().or(it.grade.take());
                    it.pregrade = src.pregrade.clone().or(it.pregrade.take());
                    self.note(format!("~{id} grade copied from {src_id} ({} effects{})", color.len(), if append { ", appended" } else { "" }));
                }
            }
            "reset" | "clear" => {
                let spec = a.shift().ok_or_else(|| anyhow!("grade reset <ids>"))?;
                for id in self.targets(&spec)? {
                    let l = self.relocate(&id)?;
                    let it = self.item_mut_at(l);
                    let n = it.fx.len();
                    it.fx.retain(|e| !is_color(e));
                    let removed = n - it.fx.len();
                    it.grade = None;
                    it.pregrade = None;
                    self.note(format!("~{id} grade reset ({removed} color effects removed)"));
                }
            }
            "version" | "versions" | "ver" => {
                let usage = "grade version <clip> save|load|del <name> | grade version <clip> list";
                let id = self.item_id(&a.shift().ok_or_else(|| anyhow!(usage))?)?;
                let op = a.shift().unwrap_or_else(|| "list".into()).to_ascii_lowercase();
                let l = self.relocate(&id)?;
                match op.as_str() {
                    "save" | "add" => {
                        let name = a.shift().unwrap_or_else(|| format!("v{}", self.item_at(l).versions.len() + 1));
                        let it = self.item_mut_at(l);
                        let color: Vec<Effect> = it.fx.iter().filter(|e| is_color(e)).cloned().collect();
                        let n = color.len();
                        it.versions.insert(name.clone(), color);
                        self.note(format!("+{id} version {name} ({n} color effects)"));
                    }
                    "load" | "use" | "switch" => {
                        let name = a.shift().ok_or_else(|| anyhow!(usage))?;
                        let it = self.item_mut_at(l);
                        let Some(v) = it.versions.get(&name).cloned() else {
                            bail!("{id} has no version '{name}' ({})", it.versions.keys().cloned().collect::<Vec<_>>().join(" "));
                        };
                        // the grade in use is kept as the version 'last' so the switch can be undone
                        let cur: Vec<Effect> = it.fx.iter().filter(|e| is_color(e)).cloned().collect();
                        it.versions.insert("last".into(), cur);
                        it.fx.retain(|e| !is_color(e));
                        it.fx.extend(v);
                        self.note(format!("~{id} version {name} loaded (previous grade kept as 'last')"));
                    }
                    "del" | "delete" | "rm" => {
                        let name = a.shift().ok_or_else(|| anyhow!(usage))?;
                        if self.item_mut_at(l).versions.remove(&name).is_none() {
                            bail!("{id} has no version '{name}'");
                        }
                        self.note(format!("-{id} version {name}"));
                    }
                    "list" | "ls" => {
                        let it = self.item_at(l);
                        let lines: Vec<String> = it.versions.iter().map(|(n, fx)| format!("{n}: {}", fx.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(" > "))).collect();
                        self.note(if lines.is_empty() { format!("{id} has no saved versions") } else { lines.join("\n") });
                    }
                    o => bail!("grade version {o}? {usage}"),
                }
            }
            "nodes" | "node" | "tree" => {
                let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("grade nodes <clip>"))?)?;
                self.note(crate::mcp::docs::grade_view(self.p, Some(id.as_str())));
            }
            "del" | "delete" | "rm" => {
                let name = a.shift().ok_or_else(|| anyhow!(usage))?;
                if self.p.grades.remove(&name).is_none() {
                    bail!("no grade '{name}'");
                }
                for c in self.p.comps.iter_mut() {
                    for t in c.tracks.iter_mut() {
                        for it in t.items.iter_mut() {
                            if it.grade.as_deref() == Some(name.as_str()) {
                                it.grade = None;
                            }
                            if it.pregrade.as_deref() == Some(name.as_str()) {
                                it.pregrade = None;
                            }
                        }
                    }
                }
                self.note(format!("-grade {name}"));
            }
            "list" | "ls" => {
                let lines: Vec<String> = self
                    .p
                    .grades
                    .iter()
                    .map(|(n, fx)| {
                        let users = self.p.comps.iter().flat_map(|c| c.tracks.iter()).flat_map(|t| t.items.iter()).filter(|i| i.grade.as_deref() == Some(n.as_str()) || i.pregrade.as_deref() == Some(n.as_str())).count();
                        format!("{n}: {} ({users} clips)", fx.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(" > "))
                    })
                    .collect();
                self.note(if lines.is_empty() { "no shared grades".into() } else { lines.join("\n") });
            }
            o => bail!("grade {o}? {usage}"),
        }
        Ok(())
    }

    // ------------------------------------------------------------ stills

    /// `still save <name> from=c1 [at=T]` | `still del <name>` | `still list`
    pub fn c_still(&mut self, a: &mut A) -> Result<()> {
        let usage = "still save <name> from=<clip> [at=T] | still del <name> | still list";
        let sub = a.shift().ok_or_else(|| anyhow!(usage))?.to_ascii_lowercase();
        match sub.as_str() {
            "save" | "grab" | "new" => {
                let name = a.shift().ok_or_else(|| anyhow!(usage))?;
                let from = a.take(&["from", "src"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("still save <name> from=<clip>"))?;
                let id = self.item_id(&from)?;
                let l = self.relocate(&id)?;
                let it = self.item_at(l).clone();
                let t = match a.take(&["at", "t"]) {
                    Some(s) => self.time(&s)?,
                    None => self.env.ui().playhead.filter(|t| *t >= it.start && *t < it.end()).unwrap_or(it.start + it.dur / 2.0),
                };
                let stats = self.env.measure(self.p, &id, &[t], None, None)?;
                let mut fx = it.fx.clone();
                if let Some(g) = it.grade.as_ref().and_then(|g| self.p.grades.get(g)) {
                    fx.extend(g.iter().cloned());
                }
                self.p.stills.insert(name.clone(), Still { item: id.clone(), t, fx, stats });
                self.note(format!("+still {name} from {id} at {}", fmt_t(t)));
            }
            "del" | "delete" | "rm" => {
                let name = a.shift().ok_or_else(|| anyhow!(usage))?;
                if self.p.stills.remove(&name).is_none() {
                    bail!("no still '{name}'");
                }
                self.note(format!("-still {name}"));
            }
            "list" | "ls" => {
                let lines: Vec<String> = self.p.stills.iter().map(|(n, s)| format!("{n}: {} at {}", s.item, fmt_t(s.t))).collect();
                self.note(if lines.is_empty() { "no stills".into() } else { lines.join("\n") });
            }
            o => bail!("still {o}? {usage}"),
        }
        Ok(())
    }

    // ------------------------------------------------------------ match

    /// `match <ids> to=<clip>|still:<name> [what=exposure,balance,sat]`
    pub fn c_match(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("match <ids> to=<clip>|still:<name> [what=exposure,balance,sat]"))?;
        let eqf = a.flag("eq");
        let loudf = a.flag("loudness") || a.flag("loud");
        let to = a.take(&["to", "ref", "reference"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("match needs to=<clip> or to=still:<name>"))?;
        let what = a.take(&["what", "only"]).unwrap_or_else(|| if eqf || loudf { String::new() } else { "exposure,balance,sat".into() }).to_ascii_lowercase();
        // audio: tonal (eq) and level (loudness) matching
        if eqf || loudf || what.contains("eq") || what.contains("loud") {
            return self.match_audio(&spec, &to, eqf || what.contains("eq"), loudf || what.contains("loud"));
        }
        let (exposure, balance, sat) = (what.contains("exp") || what.contains("all"), what.contains("bal") || what.contains("all"), what.contains("sat") || what.contains("all"));
        if !(exposure || balance || sat) {
            bail!("what= takes exposure, balance and/or sat");
        }
        let (refr, ref_name) = match to.strip_prefix("still:") {
            Some(n) => (self.p.stills.get(n).ok_or_else(|| anyhow!("no still '{n}' (still save {n} from=<clip>)"))?.stats.clone(), format!("still:{n}")),
            None => {
                let rid = self.item_id(&to)?;
                let times = self.sample_times(&rid)?;
                (self.env.measure(self.p, &rid, &times, None, None)?, rid)
            }
        };
        if refr.get("empty").is_some() {
            bail!("{ref_name} has nothing visible to match to");
        }
        for id in self.targets(&spec)? {
            if id == ref_name {
                continue;
            }
            let times = self.sample_times(&id)?;
            let old = self.tagged_fx(&id, "match");
            let src = self.env.measure(self.p, &id, &times, old.as_deref(), None)?;
            if src.get("empty").is_some() {
                self.warn(format!("{id}: nothing visible to match"));
                continue;
            }
            let c = solve(&src, &refr, exposure, balance);
            let s = if sat {
                let (a0, b0) = (src["sat"].as_f64().unwrap_or(0.0), refr["sat"].as_f64().unwrap_or(0.0));
                if a0 > 0.01 { (b0 / a0).clamp(0.3, 2.5) } else { 1.0 }
            } else {
                1.0
            };
            let before = residual(&src, &refr);
            let fid = self.put_wheels(&id, "match", &wheels_kvs(&c, s, "match"), false)?;
            let after = self.env.measure(self.p, &id, &times, None, None).map(|m| residual(&m, &refr)).unwrap_or(f64::NAN);
            self.note(format!("~{id}.{fid} match to {ref_name}: difference {:.1}% -> {:.1}%", before * 100.0, after * 100.0));
            if after > 0.06 {
                self.warn(format!("{id} still differs from {ref_name} by {:.1}%: different lighting or content; finish by eye with preview ref=", after * 100.0));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fit_recovers_gain_and_gamma() {
        let f = |x: f64| (1.2 * (x + 0.05 * (1.0 - x))).powf(1.0 / 1.3);
        let x = [0.15, 0.4, 0.75];
        let (l, g, k) = fit_channel(x, [f(x[0]), f(x[1]), f(x[2])]);
        assert!((g - 1.3).abs() < 0.05 && (k - 1.2).abs() < 0.08 && (l - 0.05).abs() < 0.05, "{l} {g} {k}");
    }

    #[test]
    fn solving_a_known_offset_reduces_it() {
        let src = json!({"shadows": [0.10, 0.10, 0.10], "mids": [0.40, 0.40, 0.40], "highs": [0.70, 0.70, 0.70], "sat": 0.2});
        let refr = json!({"shadows": [0.12, 0.10, 0.08], "mids": [0.48, 0.44, 0.40], "highs": [0.80, 0.76, 0.70], "sat": 0.3});
        let c = solve(&src, &refr, true, true);
        // apply the correction to the source bands and compare
        let apply = |x: f64, ch: usize| (c.gain[ch] * (x + c.lift[ch] * (1.0 - x))).max(0.0).powf(1.0 / c.gamma[ch]);
        let fixed = json!({
            "shadows": (0..3).map(|k| apply(0.10, k)).collect::<Vec<_>>(),
            "mids": (0..3).map(|k| apply(0.40, k)).collect::<Vec<_>>(),
            "highs": (0..3).map(|k| apply(0.70, k)).collect::<Vec<_>>(),
        });
        assert!(residual(&fixed, &refr) < residual(&src, &refr) / 4.0);
        let kv = wheels_kvs(&c, 1.5, "match");
        assert!(kv.iter().any(|(k, v)| k == "sat" && v == "150"));
    }
}
