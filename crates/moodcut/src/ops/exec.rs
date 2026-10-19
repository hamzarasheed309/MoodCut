//! MoodCut Script executor: applies parsed commands to a project transactionally.
//! The engine runs scripts on a clone of the project and only commits when every
//! line succeeds, so a failed batch never leaves the timeline half-edited.

use super::lex::{parse_script, Cmd, Tok};
use crate::model::anim::Anim;
use crate::model::project::*;
use crate::model::schema::{self, VCtx};
use crate::model::time::{fmt_t, parse_time, TimeCtx};
use crate::model::value::{fmt_num, Color, Val};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

/// Side effects the executor needs from the host (probing files, analysis).
pub trait Env {
    /// Probes a media file. The returned Media has an empty id.
    fn probe(&self, path: &str) -> Result<Media>;
    /// Runs (or fetches cached) analysis for a media item.
    fn analyze(&self, project: &Project, media: &str, kind: &str, opts: &BTreeMap<String, String>) -> Result<serde_json::Value>;
    /// What the person is doing in the editor window (selection, playhead), so
    /// scripts can say `sel` and `playhead`.
    fn ui(&self) -> Ui {
        Ui::default()
    }
    /// Renders (or reuses) a procedural sound effect file (see `crate::sfx`) and probes it.
    fn sfx(&self, kind: &str, dur: f64, pitch: f64, seed: u64) -> Result<Media> {
        let _ = (kind, dur, pitch, seed);
        bail!("sound effects need the MoodCut engine")
    }
}

/// Editor-window state shared with scripts (and shown to AI clients by `view`).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Ui {
    #[serde(default)]
    pub selection: Vec<String>,
    #[serde(default)]
    pub playhead: Option<f64>,
    /// Comp shown in the editor.
    #[serde(default)]
    pub comp: Option<String>,
}

#[derive(Debug)]
pub struct ExecError {
    pub line: usize,
    pub cmd: String,
    pub msg: String,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "line {} `{}`: {}", self.line, self.cmd, self.msg)
    }
}

pub struct ExecOut {
    pub log: Vec<String>,
    pub comp: String,
    pub ops: usize,
}

pub struct Exec<'a> {
    pub p: &'a mut Project,
    pub env: &'a dyn Env,
    pub comp: String,
    pub created: Vec<String>,
    pub log: Vec<String>,
}

pub const EPS: f64 = 1e-4;

/// Runs a script against `p`. On error the project may be partially modified,
/// so callers should run on a clone (the engine does).
pub fn run(p: &mut Project, env: &dyn Env, comp: &str, script: &str) -> std::result::Result<ExecOut, ExecError> {
    let cmds = parse_script(script).map_err(|e| ExecError { line: 0, cmd: String::new(), msg: e.to_string() })?;
    let comp = if p.comp(comp).is_some() { comp.to_string() } else { p.main.clone() };
    let mut ex = Exec { p, env, comp, created: vec![], log: vec![] };
    for c in &cmds {
        ex.cmd(c).map_err(|e| ExecError { line: c.line, cmd: c.text.clone(), msg: format!("{e:#}") })?;
    }
    Ok(ExecOut { log: compact_log(ex.log), comp: ex.comp, ops: cmds.len() })
}

/// `c12` -> ("c", 12)
fn split_id(id: &str) -> Option<(&str, u32)> {
    let d = id.find(|c: char| c.is_ascii_digit())?;
    if d == 0 || !id[..d].chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some((&id[..d], id[d..].parse().ok()?))
}

/// `c2..c31` -> ("c", 2, 31)
fn parse_id_range(s: &str) -> Option<(&str, u32, u32)> {
    let (a, b) = s.split_once("..")?;
    let ((pa, na), (pb, nb)) = (split_id(a)?, split_id(b)?);
    (pa == pb && na <= nb).then_some((pa, na, nb))
}

/// Merges log lines that differ only by item id (`~c2.f1 sat=110` x30 ->
/// `~c2..c31.f1 sat=110`), at the position of the first one, to keep replies small.
fn compact_log(log: Vec<String>) -> Vec<String> {
    // (sign, id prefix, id number, rest of line)
    fn parts(l: &str) -> Option<(char, &str, u32, &str)> {
        let sign = l.chars().next().filter(|c| "+~-".contains(*c))?;
        let body = &l[1..];
        let end = body.find(|c: char| c == '.' || c == ' ').unwrap_or(body.len());
        let (pre, n) = split_id(&body[..end])?;
        Some((sign, pre, n, &body[end..]))
    }
    let mut groups: std::collections::HashMap<(char, &str, &str), Vec<u32>> = std::collections::HashMap::new();
    for l in &log {
        if let Some((sign, pre, n, rest)) = parts(l) {
            groups.entry((sign, pre, rest)).or_default().push(n);
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(log.len());
    for l in &log {
        let Some((sign, pre, _, rest)) = parts(l) else {
            out.push(l.clone());
            continue;
        };
        let Some(nums) = groups.get(&(sign, pre, rest)) else { continue }; // already merged
        if nums.len() < 3 {
            out.push(l.clone());
            continue;
        }
        let mut nums = nums.clone();
        nums.sort_unstable();
        nums.dedup();
        let mut runs: Vec<String> = vec![];
        let mut k = 0;
        while k < nums.len() {
            let mut e = k;
            while e + 1 < nums.len() && nums[e + 1] == nums[e] + 1 {
                e += 1;
            }
            runs.push(if e > k { format!("{pre}{}..{pre}{}", nums[k], nums[e]) } else { format!("{pre}{}", nums[k]) });
            k = e + 1;
        }
        out.push(format!("{sign}{}{rest}", runs.join(",")));
        groups.remove(&(sign, pre, rest));
    }
    out
}

/// Argument accessor that tracks which key=values were consumed.
#[derive(Clone)]
pub struct A {
    pub pos: Vec<String>,
    kv: Vec<(String, String, bool)>,
}

impl A {
    /// Arguments for calling another command internally.
    pub fn from_pos(pos: Vec<String>) -> A {
        A { pos, kv: vec![] }
    }

    pub fn new(c: &Cmd) -> A {
        let mut pos = vec![];
        let mut kv = vec![];
        for t in &c.args {
            match t {
                Tok::W(s) | Tok::Q(s) => pos.push(s.clone()),
                Tok::KV(k, v) => kv.push((k.clone(), v.clone(), false)),
            }
        }
        A { pos, kv }
    }
    pub fn take(&mut self, names: &[&str]) -> Option<String> {
        for (k, v, used) in self.kv.iter_mut() {
            if !*used && names.iter().any(|n| n.eq_ignore_ascii_case(k)) {
                *used = true;
                return Some(v.clone());
            }
        }
        None
    }
    pub fn flag(&mut self, name: &str) -> bool {
        if let Some(i) = self.pos.iter().position(|p| p.eq_ignore_ascii_case(name)) {
            self.pos.remove(i);
            return true;
        }
        if let Some(v) = self.take(&[name]) {
            return !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off");
        }
        false
    }
    pub fn rest(&mut self) -> Vec<(String, String)> {
        let mut out = vec![];
        for (k, v, used) in self.kv.iter_mut() {
            if !*used {
                *used = true;
                out.push((k.clone(), v.clone()));
            }
        }
        out
    }
    pub fn shift(&mut self) -> Option<String> {
        if self.pos.is_empty() {
            None
        } else {
            Some(self.pos.remove(0))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Strict,
    Overwrite,
    Insert,
}

pub fn parse_track_spec(s: &str) -> Option<(TrackKind, usize)> {
    let mut ch = s.chars();
    let k = match ch.next()? {
        'V' | 'v' => TrackKind::Video,
        'A' | 'a' => TrackKind::Audio,
        _ => return None,
    };
    let n: usize = ch.as_str().parse().ok()?;
    if n == 0 || n > 99 {
        return None;
    }
    Some((k, n))
}

pub fn parse_speed(s: &str) -> Result<f64> {
    let l = s.trim().to_ascii_lowercase();
    let v = if let Some(p) = l.strip_suffix('%') {
        p.parse::<f64>().map(|x| x / 100.0)
    } else {
        l.trim_end_matches('x').parse::<f64>()
    }
    .map_err(|_| anyhow!("bad speed '{s}' (use 2, 2x, 50%)"))?;
    if !(0.01..=100.0).contains(&v.abs()) {
        bail!("speed must be between 0.01 and 100");
    }
    Ok(v)
}

impl<'a> Exec<'a> {
    // ------------------------------------------------------------ helpers

    pub fn ci(&self) -> usize {
        self.p.comps.iter().position(|c| c.id == self.comp).unwrap_or(0)
    }
    pub fn comp(&self) -> &Comp {
        &self.p.comps[self.ci()]
    }
    pub fn comp_mut(&mut self) -> &mut Comp {
        let i = self.ci();
        &mut self.p.comps[i]
    }
    pub fn vctx(&self) -> VCtx {
        let c = self.comp();
        VCtx { cw: c.w as f64, ch: c.h as f64 }
    }
    pub fn vctx_of(&self, ci: usize) -> VCtx {
        let c = &self.p.comps[ci];
        VCtx { cw: c.w as f64, ch: c.h as f64 }
    }
    pub fn fps(&self) -> f64 {
        self.comp().fps
    }
    pub fn note(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
    }
    pub fn warn(&mut self, s: impl Into<String>) {
        self.log.push(format!("! {}", s.into()));
    }

    fn resolve_ref<'k>(&'k self, key: &'k str) -> Result<String> {
        if key == "$" || key == "$last" {
            return self.created.last().cloned().ok_or_else(|| anyhow!("'$' used before anything was created in this script"));
        }
        if let Some(n) = key.strip_prefix('$') {
            let i: usize = n.parse().map_err(|_| anyhow!("bad reference '{key}'"))?;
            return self.created.get(i.wrapping_sub(1)).cloned().ok_or_else(|| anyhow!("'{key}': only {} item(s) created so far", self.created.len()));
        }
        Ok(key.to_string())
    }

    /// Locates an item: (comp idx, track idx, item idx).
    pub fn loc(&self, key: &str) -> Result<(usize, usize, usize)> {
        let key = self.resolve_ref(key)?;
        // prefer the active comp
        let ci = self.ci();
        for (ti, t) in self.p.comps[ci].tracks.iter().enumerate() {
            for (ii, it) in t.items.iter().enumerate() {
                if it.id == key {
                    return Ok((ci, ti, ii));
                }
            }
        }
        if let Some(l) = self.p.locate(&key) {
            return Ok(l);
        }
        let ids: Vec<String> = self.p.comps[ci].items().flat_map(|i| std::iter::once(i.id.clone()).chain(i.name.clone())).collect();
        let hint = schema::suggest(ids.iter().map(|s| s.as_str()), &key).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
        bail!("unknown item '{key}'.{hint} (view the timeline for ids)")
    }

    pub fn item_at(&self, l: (usize, usize, usize)) -> &Item {
        &self.p.comps[l.0].tracks[l.1].items[l.2]
    }
    pub fn item_mut_at(&mut self, l: (usize, usize, usize)) -> &mut Item {
        &mut self.p.comps[l.0].tracks[l.1].items[l.2]
    }
    pub fn item_id(&self, key: &str) -> Result<String> {
        let l = self.loc(key)?;
        Ok(self.item_at(l).id.clone())
    }

    /// Re-locates an item by id after mutations that may have reordered vectors.
    pub fn relocate(&self, id: &str) -> Result<(usize, usize, usize)> {
        self.p.locate(id).ok_or_else(|| anyhow!("item '{id}' vanished"))
    }

    /// Span of an item, or of a track's contents (`V1.end` = where the last clip on V1 ends).
    pub fn item_span(&self, key: &str) -> Option<(f64, f64)> {
        if parse_track_spec(key).is_some() && self.p.item(key).is_none() {
            let t = self.comp().track(key)?;
            let a = t.items.iter().map(|i| i.start).fold(f64::INFINITY, f64::min);
            let b = t.items.iter().map(|i| i.end()).fold(0.0, f64::max);
            return a.is_finite().then_some((a, b));
        }
        let key = self.resolve_ref(key).ok()?;
        self.p.item(&key).map(|i| (i.start, i.end()))
    }

    pub fn time_in(&self, s: &str, pct_of: Option<f64>, end: Option<f64>) -> Result<f64> {
        let comp = self.comp();
        let now = self.env.ui().playhead;
        let cx = TimeCtx {
            fps: comp.fps,
            comp_end: end.unwrap_or_else(|| comp.duration()),
            item: &|k| self.item_span(k),
            marker: &|m| comp.markers.iter().find(|mk| mk.name.eq_ignore_ascii_case(m)).map(|mk| mk.t),
            pct_of,
            now,
        };
        parse_time(s, &cx)
    }

    /// Timeline time (comp-level).
    pub fn time(&self, s: &str) -> Result<f64> {
        self.time_in(s, None, None)
    }

    /// Item-relative time (keyframes): `end` = item duration, `%` of duration.
    pub fn rel_time(&self, s: &str, dur: f64) -> Result<f64> {
        self.time_in(s, Some(dur), Some(dur))
    }

    pub fn targets(&self, spec: &str) -> Result<Vec<String>> {
        let mut out = vec![];
        for part in spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if part.eq_ignore_ascii_case("all") || part == "*" {
                out.extend(self.comp().items().map(|i| i.id.clone()));
            } else if let Some((pre, a, b)) = parse_id_range(part) {
                // c2..c31, the range notation used in compacted logs
                let before = out.len();
                out.extend(self.comp().items().filter(|i| split_id(&i.id).is_some_and(|(p, n)| p == pre && (a..=b).contains(&n))).map(|i| i.id.clone()));
                if out.len() == before {
                    bail!("no items in range '{part}'");
                }
            } else if parse_track_spec(part).is_some() && self.comp().track(part).is_some() && self.p.locate(part).is_none() {
                out.extend(self.comp().track(part).unwrap().items.iter().map(|i| i.id.clone()));
            } else if let Some(ids) = self.select(part)? {
                out.extend(ids);
            } else {
                out.push(self.item_id(part)?);
            }
        }
        if out.is_empty() {
            bail!("no targets matched '{spec}'");
        }
        Ok(out)
    }

    pub fn media_for(&self, key: &str) -> Result<Media> {
        if let Some(m) = self.p.media(key) {
            return Ok(m.clone());
        }
        let ids: Vec<&str> = self.p.media.iter().flat_map(|m| [m.id.as_str(), m.name.as_str()]).collect();
        let hint = schema::suggest(ids.into_iter(), key).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
        bail!("unknown media '{key}'.{hint}")
    }

    pub fn import_path(&mut self, path: &str, alias: Option<String>) -> Result<String> {
        let norm = path.trim().to_string();
        if let Some(m) = self.p.media.iter().find(|m| same_path(&m.path, &norm)) {
            return Ok(m.id.clone());
        }
        let mut m = self.env.probe(&norm)?;
        m.id = self.p.next_media_id();
        if let Some(a) = alias {
            m.name = a;
        }
        let id = m.id.clone();
        self.note(format!("+{} {} {} {}", id, m.name, kind_s(m.kind), media_summary(&m)));
        self.p.media.push(m);
        Ok(id)
    }

    /// Resolves a media key or a file path (auto-imports paths).
    pub fn media_or_import(&mut self, key: &str) -> Result<String> {
        if let Some(m) = self.p.media(key) {
            return Ok(m.id.clone());
        }
        if key.contains('/') || key.contains('\\') || std::path::Path::new(key).extension().is_some() && std::path::Path::new(key).exists() {
            return self.import_path(key, None);
        }
        Ok(self.media_for(key)?.id)
    }

    pub fn track_kind_for(&self, item: &Item) -> TrackKind {
        if item.kind == ItemKind::Clip {
            if let Some(m) = item.src.as_deref().and_then(|s| self.p.media(s)) {
                if m.kind == MediaKind::Audio {
                    return TrackKind::Audio;
                }
            }
        }
        TrackKind::Video
    }

    /// Lowest track (>= min_idx) of `kind` free in [a,b); creates tracks as needed.
    pub fn pick_track(&mut self, kind: TrackKind, a: f64, b: f64, min_idx: usize) -> String {
        let ci = self.ci();
        let count = self.p.comps[ci].track_count(kind);
        for idx in min_idx.max(1)..=count + 1 {
            let id = self.p.comps[ci].ensure_track(kind, idx);
            let t = self.p.comps[ci].track(&id).unwrap();
            if !t.locked && t.is_free(a, b, None) {
                return id;
            }
        }
        self.p.comps[ci].ensure_track(kind, count + 1)
    }

    /// Track above anything overlapping [a,b) (for titles/overlays).
    pub fn pick_track_above(&mut self, kind: TrackKind, a: f64, b: f64) -> String {
        let top = self
            .comp()
            .tracks
            .iter()
            .filter(|t| t.kind == kind && !t.overlapping(a, b, None).is_empty())
            .map(|t| t.index())
            .max()
            .unwrap_or(0);
        self.pick_track(kind, a, b, top + 1)
    }

    /// Inserts `item` on track `tid` of the active comp honoring `mode`.
    pub fn place(&mut self, item: Item, tid: &str, mode: Mode) -> Result<String> {
        let ci = self.ci();
        let fps = self.fps();
        let (kind, idx) = parse_track_spec(tid).ok_or_else(|| anyhow!("bad track '{tid}' (use V1, V2, A1 ...)"))?;
        let tid = self.p.comps[ci].ensure_track(kind, idx);
        let want = self.track_kind_for(&item);
        if want != kind && !(item.kind == ItemKind::Clip && kind == TrackKind::Audio) {
            bail!("{} goes on {} tracks, not {}", item.kind.name(), if want == TrackKind::Video { "video (V#)" } else { "audio (A#)" }, tid);
        }
        if item.dur < 1.0 / fps - 1e-9 {
            bail!("duration must be at least one frame");
        }
        let (a, b) = (item.start, item.end());
        if item.start < -EPS {
            bail!("start time can't be negative");
        }
        {
            let t = self.p.comps[ci].track(&tid).unwrap();
            if t.locked {
                bail!("track {tid} is locked");
            }
        }
        match mode {
            Mode::Strict => {
                let t = self.p.comps[ci].track(&tid).unwrap();
                let ov = t.overlapping(a, b, Some(&item.id));
                if !ov.is_empty() {
                    let ids: Vec<String> = ov.iter().map(|i| format!("{} {}-{}", i.id, fmt_t(i.start), fmt_t(i.end()))).collect();
                    bail!("overlaps {} on {tid}; use another track, mode=overwrite, or mode=insert", ids.join(", "));
                }
            }
            Mode::Overwrite => self.clear_range(ci, &tid, a, b, Some(&item.id))?,
            Mode::Insert => self.insert_space(ci, &tid, a, item.dur)?,
        }
        let t = self.p.comps[ci].track_mut(&tid).unwrap();
        t.items.push(item);
        t.sort();
        Ok(tid)
    }

    /// Removes/trims/splits items on a track so that [a,b) is empty.
    pub fn clear_range(&mut self, ci: usize, tid: &str, a: f64, b: f64, except: Option<&str>) -> Result<()> {
        let ids: Vec<String> = self.p.comps[ci].track(tid).unwrap().overlapping(a, b, except).iter().map(|i| i.id.clone()).collect();
        for id in ids {
            let (s, e) = self.item_span(&id).unwrap();
            if s >= a - EPS && e <= b + EPS {
                self.remove_item(&id)?;
                self.note(format!("-{id} (overwritten)"));
            } else if s < a && e > b {
                // split around the range
                self.split_item(&id, b)?;
                self.trim_end_to(&id, a)?;
            } else if s < a {
                self.trim_end_to(&id, a)?;
            } else {
                self.trim_start_to(&id, b)?;
            }
        }
        Ok(())
    }

    /// Shifts items at/after `at` on a track right by `d` (splitting one spanning `at`).
    pub fn insert_space(&mut self, ci: usize, tid: &str, at: f64, d: f64) -> Result<()> {
        let spanning: Option<String> = self.p.comps[ci].track(tid).unwrap().items.iter().find(|i| i.start < at - EPS && i.end() > at + EPS).map(|i| i.id.clone());
        if let Some(id) = spanning {
            self.split_item(&id, at)?;
        }
        let t = self.p.comps[ci].track_mut(tid).unwrap();
        for it in t.items.iter_mut() {
            if it.start >= at - EPS {
                it.start += d;
            }
        }
        Ok(())
    }

    pub fn remove_item(&mut self, id: &str) -> Result<Item> {
        let (c, t, i) = self.relocate(id)?;
        let it = self.p.comps[c].tracks[t].items.remove(i);
        // detach references
        for comp in self.p.comps.iter_mut() {
            for tr in comp.tracks.iter_mut() {
                for other in tr.items.iter_mut() {
                    if other.parent.as_deref() == Some(id) {
                        other.parent = None;
                    }
                    if other.matte.as_ref().map(|m| m.src == id).unwrap_or(false) {
                        other.matte = None;
                    }
                }
            }
        }
        Ok(it)
    }

    /// Max source duration available for an item (None = unlimited).
    pub fn src_limit(&self, it: &Item) -> Option<f64> {
        if it.props.get("hold").map(|a| a.v.bool()).unwrap_or(false) {
            return None;
        }
        match it.kind {
            ItemKind::Clip => {
                let m = self.p.media(it.src.as_deref()?)?;
                if m.is_still() {
                    None
                } else {
                    Some(m.dur)
                }
            }
            _ => None,
        }
    }

    /// Splits an item at timeline time `t`. Returns the new right-hand id.
    pub fn split_item(&mut self, id: &str, t: f64) -> Result<String> {
        let l = self.relocate(id)?;
        let fps = self.p.comps[l.0].fps;
        let it = self.item_at(l).clone();
        if t <= it.start + 0.5 / fps || t >= it.end() - 0.5 / fps {
            bail!("{} doesn't span {} ({}-{})", id, fmt_t(t), fmt_t(it.start), fmt_t(it.end()));
        }
        let off = t - it.start;
        let new_id = self.p.next_item_id(it.kind);
        let mut right = it.clone();
        right.id = new_id.clone();
        right.name = None;
        right.start = t;
        right.dur = it.dur - off;
        let sp = it.speed();
        if matches!(it.kind, ItemKind::Clip | ItemKind::Comp) {
            if it.reversed() {
                // left part plays later source
            } else {
                right.src_in = it.src_in + off * sp;
            }
        }
        shift_item_keys(&mut right, -off);
        right.tin = None;
        right.anims.retain(|a| a.kind != "in");
        right.props.remove("fadeIn");
        let left = self.item_mut_at(l);
        left.dur = off;
        if it.reversed() {
            left.src_in = it.src_in + right.dur * sp;
        }
        left.tout = None;
        left.anims.retain(|a| a.kind != "out");
        left.props.remove("fadeOut");
        let t_idx = l.1;
        let tr = &mut self.p.comps[l.0].tracks[t_idx];
        tr.items.push(right);
        tr.sort();
        Ok(new_id)
    }

    pub fn trim_end_to(&mut self, id: &str, end: f64) -> Result<()> {
        let l = self.relocate(id)?;
        let fps = self.p.comps[l.0].fps;
        let it = self.item_mut_at(l);
        let nd = end - it.start;
        if nd < 1.0 / fps - 1e-9 {
            bail!("{id}: duration would be < 1 frame");
        }
        it.dur = nd;
        Ok(())
    }

    pub fn trim_start_to(&mut self, id: &str, start: f64) -> Result<()> {
        let l = self.relocate(id)?;
        let fps = self.p.comps[l.0].fps;
        let it = self.item_at(l).clone();
        let delta = start - it.start;
        let nd = it.dur - delta;
        if nd < 1.0 / fps - 1e-9 {
            bail!("{id}: duration would be < 1 frame");
        }
        let sp = it.speed();
        let mut new_in = it.src_in;
        if matches!(it.kind, ItemKind::Clip | ItemKind::Comp) && !it.reversed() {
            new_in = it.src_in + delta * sp;
            if new_in < -EPS && !self.is_still(&it) {
                bail!("{id}: can't extend before the media start (needs {}s of handle)", fmt_t(-new_in));
            }
            new_in = new_in.max(0.0);
        }
        let m = self.item_mut_at(l);
        m.start = start;
        m.dur = nd;
        m.src_in = new_in;
        shift_item_keys(m, -delta);
        Ok(())
    }

    pub fn is_still(&self, it: &Item) -> bool {
        if it.props.get("hold").map(|a| a.v.bool()).unwrap_or(false) {
            return true;
        }
        match it.kind {
            ItemKind::Clip => it.src.as_deref().and_then(|s| self.p.media(s)).map(|m| m.is_still()).unwrap_or(false),
            ItemKind::Comp => false,
            _ => true,
        }
    }

    /// Clamps an item's duration to its available source; returns true if clamped.
    pub fn clamp_to_source(&mut self, id: &str) -> Result<bool> {
        let l = self.relocate(id)?;
        let it = self.item_at(l).clone();
        if let Some(limit) = self.src_limit(&it) {
            let sp = it.speed();
            let avail = (limit - it.src_in) / sp;
            if avail < 1.0 / self.p.comps[l.0].fps {
                bail!("{id}: in-point {} is past the media end ({}s)", fmt_t(it.src_in), fmt_t(limit));
            }
            if it.dur > avail + 1e-3 {
                self.item_mut_at(l).dur = avail;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Shifts items on a track that start at/after `from` by `d`.
    pub fn ripple(&mut self, ci: usize, tid: &str, from: f64, d: f64, all_tracks: bool) {
        let comp = &mut self.p.comps[ci];
        for t in comp.tracks.iter_mut() {
            if !(all_tracks || t.id == tid) || t.locked {
                continue;
            }
            for it in t.items.iter_mut() {
                if it.start >= from - EPS {
                    it.start = (it.start + d).max(0.0);
                }
            }
            t.sort();
        }
    }

    pub fn new_item(&mut self, kind: ItemKind, start: f64, dur: f64) -> Item {
        let id = self.p.next_item_id(kind);
        Item::new(id, kind, start, dur)
    }

    /// Applies key=value props to an item (schema validated).
    pub fn apply_props(&mut self, id: &str, kvs: &[(String, String)]) -> Result<()> {
        let l = self.relocate(id)?;
        let cx = self.vctx_of(l.0);
        let kind = self.item_at(l).kind;
        for (k, v) in kvs {
            let p = schema::item_prop(kind, k).ok_or_else(|| {
                let names = schema::item_props(kind).iter().chain(schema::LAYER.iter()).map(|p| p.name);
                let hint = schema::suggest(names, k).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
                anyhow!("unknown property '{k}' for {}.{hint} (help props)", kind.name())
            })?;
            let val = schema::clamp_val(p, schema::parse_val(p, v, cx)?);
            self.item_mut_at(l).props.insert(p.name.to_string(), Anim::new(val));
        }
        Ok(())
    }

    // ------------------------------------------------------------ dispatch

    pub fn cmd(&mut self, c: &Cmd) -> Result<()> {
        let mut a = A::new(c);
        match c.verb.as_str() {
            "import" => self.c_import(&mut a),
            "tag" => self.c_tag(&mut a, true),
            "untag" => self.c_tag(&mut a, false),
            "note" | "describe" => self.c_media_note(&mut a),
            "relink" => self.c_relink(&mut a),
            "unimport" => self.c_unimport(&mut a),
            "comp" | "seq" | "sequence" => self.c_comp(&mut a),
            "track" => self.c_track(&mut a),
            "add" | "place" | "overlay" => self.c_add(&mut a, false),
            "append" => self.c_add(&mut a, true),
            "insert" => {
                a.pos.push("insert".into());
                self.c_add(&mut a, false)
            }
            "text" | "title-text" => self.c_text(&mut a),
            "shape" => self.c_shape(&mut a),
            "solid" | "color-matte" | "bg" => self.c_solid(&mut a),
            "adjust" | "adj" | "adjustment" => self.c_adjust(&mut a),
            "del" | "delete" | "rm" | "remove" => self.c_del(&mut a),
            "move" | "mv" => self.c_move(&mut a),
            "trim" => self.c_trim(&mut a),
            "cut" | "split" | "razor" => self.c_cut(&mut a),
            "slip" => self.c_slip(&mut a),
            "slide" => self.c_slide(&mut a),
            "speed" => self.c_speed(&mut a),
            "reverse" => self.c_reverse(&mut a),
            "freeze" => self.c_freeze(&mut a),
            "dup" | "duplicate" | "copy" => self.c_dup(&mut a),
            "replace" => self.c_replace(&mut a),
            "close" | "closegaps" => self.c_close(&mut a),
            "gap" => self.c_gap(&mut a),
            "detach" | "unlink" => self.c_detach(&mut a),
            "fit" => self.c_fit(&mut a),
            "set" => self.c_set(&mut a),
            "kf" | "key" | "keys" => self.c_kf(&mut a, 0),
            "kf+" => self.c_kf(&mut a, 1),
            "kf-" => self.c_kf(&mut a, 2),
            "expr" | "expression" => self.c_expr(&mut a),
            "anim" | "animate" => self.c_anim(&mut a),
            "fx" | "effect" => self.c_fx(&mut a, false),
            "afx" => self.c_fx(&mut a, true),
            "tr" | "transition" => self.c_tr(&mut a),
            "fade" => self.c_fade(&mut a),
            "mask" => self.c_mask(&mut a),
            "matte" | "trackmatte" => self.c_matte(&mut a),
            "parent" => self.c_parent(&mut a),
            "color" | "grade" => self.c_color(&mut a),
            "lut" => self.c_lut(&mut a),
            "marker" | "mark" => self.c_marker(&mut a),
            "master" => self.c_master(&mut a),
            "name" | "rename" => self.c_name(&mut a),
            "duck" => self.c_duck(&mut a),
            "precomp" | "nest" => self.c_precomp(&mut a),
            "autocut" => self.c_autocut(&mut a),
            "captions" | "subtitles" | "subs" => self.c_captions(&mut a),
            "montage" | "beatcut" => self.c_montage(&mut a),
            "beats" => self.c_beats(&mut a),
            "reframe" => self.c_reframe(&mut a),
            "title" | "template" | "mogrt" => self.c_title(&mut a),
            "keep" | "roughcut" => self.c_keep(&mut a),
            "null" => self.c_null(&mut a, false),
            "camera" | "cam" => self.c_null(&mut a, true),
            "roll" | "rolling" => self.c_roll(&mut a),
            "lift" => self.c_lift(&mut a, false),
            "extract" => self.c_lift(&mut a, true),
            "swap" | "rearrange" => self.c_swap(&mut a),
            "paste" | "pasteattr" | "attributes" => self.c_paste(&mut a),
            "stagger" => self.c_stagger(&mut a),
            "align" => self.c_align(&mut a),
            "nudge" => self.c_nudge(&mut a),
            "distribute" => self.c_distribute(&mut a),
            "ease" | "easyease" => self.c_ease(&mut a),
            "workarea" | "inout" => self.c_workarea(&mut a),
            "sfx" | "sound-effect" | "soundfx" => self.c_sfx(&mut a),
            "speedramp" | "ramp" | "timeremap" => self.c_speedramp(&mut a),
            "punchin" | "punch-in" | "punchins" => self.c_punchin(&mut a),
            "jcut" => self.c_splitedit(&mut a, true),
            "lcut" => self.c_splitedit(&mut a, false),
            "brand" | "brandkit" => self.c_brand(&mut a),
            "sound" | "essentialsound" => self.c_sound(&mut a),
            "undo" | "redo" => bail!("undo/redo must be sent on their own (or use the project tool)"),
            v => {
                let verbs = VERBS.iter().copied();
                let hint = schema::suggest(verbs, v).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
                bail!("unknown command '{v}'.{hint} (help edit)")
            }
        }?;
        // anything left unconsumed is an error (catches typos)
        let left = a.rest();
        if !left.is_empty() {
            let k: Vec<String> = left.iter().map(|(k, _)| k.clone()).collect();
            bail!("unused argument(s): {}", k.join(", "));
        }
        if !a.pos.is_empty() {
            bail!("unexpected argument(s): {}", a.pos.join(" "));
        }
        Ok(())
    }

    // ------------------------------------------------------------ media / comps / tracks

    fn c_import(&mut self, a: &mut A) -> Result<()> {
        let alias = a.take(&["as", "name"]);
        let tags: Vec<String> = a.take(&["tag", "tags"]).map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
        let paths: Vec<String> = a.pos.drain(..).collect();
        if paths.is_empty() {
            bail!("import <path> [path...] [as=name] [tag=a,b]");
        }
        let single = paths.len() == 1;
        for p in paths {
            let id = self.import_path(&p, if single { alias.clone() } else { None })?;
            if let Some(m) = self.p.media.iter_mut().find(|m| m.id == id) {
                for t in &tags {
                    if !m.tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                        m.tags.push(t.clone());
                    }
                }
            }
            self.created.push(id);
        }
        Ok(())
    }

    fn c_comp(&mut self, a: &mut A) -> Result<()> {
        let sub = a.shift().unwrap_or_else(|| "new".into()).to_ascii_lowercase();
        match sub.as_str() {
            "new" | "add" => {
                let (mut w, mut h, mut fps) = (self.comp().w, self.comp().h, self.comp().fps);
                let mut name = None;
                if let Some(like) = a.take(&["like"]) {
                    let c = self.p.comp(&like).ok_or_else(|| anyhow!("unknown comp '{like}'"))?;
                    (w, h, fps) = (c.w, c.h, c.fps);
                }
                for p in a.pos.drain(..).collect::<Vec<_>>() {
                    if let Some((ww, hh)) = parse_size(&p) {
                        (w, h) = (ww, hh);
                    } else if let Some(f) = p.to_ascii_lowercase().strip_suffix("fps").and_then(|f| f.parse::<f64>().ok()) {
                        fps = f;
                    } else if let Ok(f) = p.parse::<f64>() {
                        fps = f;
                    } else {
                        name = Some(p);
                    }
                }
                if let Some(v) = a.take(&["w", "width"]) {
                    w = v.parse()?;
                }
                if let Some(v) = a.take(&["h", "height"]) {
                    h = v.parse()?;
                }
                if let Some(v) = a.take(&["fps", "rate"]) {
                    fps = v.parse()?;
                }
                if let Some(v) = a.take(&["size", "res"]) {
                    (w, h) = parse_size(&v).ok_or_else(|| anyhow!("bad size '{v}'"))?;
                }
                let name = a.take(&["name", "as"]).or(name).unwrap_or_else(|| format!("Comp {}", self.p.comps.len() + 1));
                if w < 16 || h < 16 || w > 8192 || h > 8192 || w % 2 == 1 || h % 2 == 1 {
                    bail!("comp size must be even and between 16 and 8192 px");
                }
                let id = self.p.next_comp_id();
                let mut c = Comp::new(&id, &name, w, h, fps);
                if let Some(d) = a.take(&["dur", "duration"]) {
                    c.dur = Some(self.time(&d)?);
                }
                if let Some(bg) = a.take(&["bg"]) {
                    c.bg = crate::model::value::Color::parse(&bg)?;
                }
                self.p.comps.push(c);
                self.note(format!("+{id} '{name}' {w}x{h}@{} (active)", fmt_num(fps)));
                self.comp = id.clone();
                self.created.push(id);
            }
            "use" | "open" | "select" => {
                let k = a.shift().ok_or_else(|| anyhow!("comp use <id>"))?;
                let id = self.p.comp(&k).ok_or_else(|| anyhow!("unknown comp '{k}'"))?.id.clone();
                self.comp = id.clone();
                self.note(format!("active comp {id}"));
            }
            "main" => {
                let k = a.shift().unwrap_or_else(|| self.comp.clone());
                let id = self.p.comp(&k).ok_or_else(|| anyhow!("unknown comp '{k}'"))?.id.clone();
                self.p.main = id.clone();
                self.note(format!("main comp {id}"));
            }
            "set" => {
                let k = a.shift().unwrap_or_else(|| self.comp.clone());
                let id = self.p.comp(&k).ok_or_else(|| anyhow!("unknown comp '{k}'"))?.id.clone();
                let mut changes = vec![];
                let kvs = a.rest();
                let cpos = self.p.comps.iter().position(|c| c.id == id).unwrap();
                for (key, v) in kvs {
                    let c = &mut self.p.comps[cpos];
                    match key.to_ascii_lowercase().as_str() {
                        "w" | "width" => c.w = v.parse()?,
                        "h" | "height" => c.h = v.parse()?,
                        "size" | "res" => {
                            let (w, h) = parse_size(&v).ok_or_else(|| anyhow!("bad size '{v}'"))?;
                            c.w = w;
                            c.h = h;
                        }
                        "fps" | "rate" => c.fps = v.parse()?,
                        "name" => c.name = v.clone(),
                        "bg" => c.bg = crate::model::value::Color::parse(&v)?,
                        "shutter" => c.shutter = v.parse()?,
                        "work" | "workarea" => {
                            c.work = match v.to_ascii_lowercase().as_str() {
                                "none" | "off" | "clear" => None,
                                _ => {
                                    let (x, y) = v.split_once('-').ok_or_else(|| anyhow!("work must look like 2-8 (seconds) or none"))?;
                                    let (x, y): (f64, f64) = (x.trim().parse()?, y.trim().parse()?);
                                    if y <= x || x < 0.0 {
                                        bail!("work area needs 0 <= in < out");
                                    }
                                    Some((x, y))
                                }
                            }
                        }
                        "dur" | "duration" => {
                            c.dur = if v == "auto" || v == "none" {
                                None
                            } else {
                                let t = v.parse::<f64>().ok();
                                Some(match t {
                                    Some(t) => t,
                                    None => bail!("dur must be seconds or auto"),
                                })
                            }
                        }
                        other => bail!("unknown comp setting '{other}' (w h size fps name bg dur shutter work)"),
                    }
                    changes.push(key);
                }
                self.note(format!("~{id} {}", changes.join(",")));
            }
            "del" | "delete" | "rm" => {
                let k = a.shift().ok_or_else(|| anyhow!("comp del <id>"))?;
                let id = self.p.comp(&k).ok_or_else(|| anyhow!("unknown comp '{k}'"))?.id.clone();
                if id == self.p.main {
                    bail!("can't delete the main comp");
                }
                if self.p.comps.iter().any(|c| c.items().any(|i| i.kind == ItemKind::Comp && i.src.as_deref() == Some(&id))) {
                    bail!("{id} is nested in another comp; delete those items first");
                }
                self.p.comps.retain(|c| c.id != id);
                if self.comp == id {
                    self.comp = self.p.main.clone();
                }
                self.note(format!("-{id}"));
            }
            "dup" | "duplicate" => {
                let k = a.shift().unwrap_or_else(|| self.comp.clone());
                let src = self.p.comp(&k).ok_or_else(|| anyhow!("unknown comp '{k}'"))?.clone();
                let id = self.p.next_comp_id();
                let mut c = src.clone();
                c.id = id.clone();
                c.name = a.take(&["name", "as"]).unwrap_or_else(|| format!("{} copy", src.name));
                // fresh item ids
                for t in c.tracks.iter_mut() {
                    for it in t.items.iter_mut() {
                        self.p.seq += 1;
                        it.id = format!("{}{}", it.kind.prefix(), self.p.seq);
                        it.parent = None;
                        it.matte = None;
                        it.name = None;
                    }
                }
                self.note(format!("+{id} '{}' (copy of {})", c.name, src.id));
                self.p.comps.push(c);
                self.created.push(id);
            }
            other => bail!("comp {other}? use: comp new|use|main|set|del|dup"),
        }
        Ok(())
    }

    fn c_track(&mut self, a: &mut A) -> Result<()> {
        let sub = a.shift().ok_or_else(|| anyhow!("track add|set|del|duck ..."))?.to_ascii_lowercase();
        let ci = self.ci();
        match sub.as_str() {
            "add" | "new" => {
                let k = a.shift().unwrap_or_else(|| "V".into()).to_ascii_uppercase();
                let kind = if k.starts_with('A') { TrackKind::Audio } else { TrackKind::Video };
                let n: usize = a.shift().map(|s| s.parse()).transpose()?.unwrap_or(1);
                for _ in 0..n {
                    let idx = self.p.comps[ci].track_count(kind) + 1;
                    let id = self.p.comps[ci].ensure_track(kind, idx);
                    self.note(format!("+{id}"));
                }
            }
            "set" => {
                let tid = a.shift().ok_or_else(|| anyhow!("track set <V1|A1> key=value"))?;
                if self.p.comps[ci].track(&tid).is_none() {
                    let (k, i) = parse_track_spec(&tid).ok_or_else(|| anyhow!("bad track '{tid}'"))?;
                    self.p.comps[ci].ensure_track(k, i);
                }
                let kvs = a.rest();
                let t = self.p.comps[ci].track_mut(&tid).unwrap();
                for (k, v) in &kvs {
                    let b = || !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off");
                    match k.to_ascii_lowercase().as_str() {
                        "hidden" | "hide" => t.hidden = b(),
                        "locked" | "lock" => t.locked = b(),
                        "muted" | "mute" => t.muted = b(),
                        "solo" => t.solo = b(),
                        "vol" | "volume" | "gain" => t.vol = v.trim_end_matches("dB").trim_end_matches("db").parse()?,
                        "pan" => t.pan = v.parse::<f64>()?.clamp(-1.0, 1.0),
                        "name" => t.name = Some(v.clone()),
                        other => bail!("unknown track setting '{other}' (hidden locked mute solo vol pan name)"),
                    }
                }
                let id = t.id.clone();
                self.note(format!("~{id}"));
            }
            "del" | "delete" | "rm" => {
                let tid = a.shift().ok_or_else(|| anyhow!("track del <id>"))?;
                let force = a.flag("force");
                let t = self.p.comps[ci].track(&tid).ok_or_else(|| anyhow!("no track {tid}"))?;
                if !t.items.is_empty() && !force {
                    bail!("track {tid} has {} item(s); add 'force' to delete them too", t.items.len());
                }
                let tid = t.id.clone();
                self.p.comps[ci].tracks.retain(|t| t.id != tid);
                self.note(format!("-{tid}"));
            }
            "duck" => {
                let tid = a.shift().ok_or_else(|| anyhow!("track duck <A1> by=<A2> [amount=-12]"))?;
                a.pos.insert(0, tid);
                self.c_duck(a)?;
            }
            other => bail!("track {other}? use: track add|set|del|duck"),
        }
        Ok(())
    }

    fn c_duck(&mut self, a: &mut A) -> Result<()> {
        let tid = a.shift().ok_or_else(|| anyhow!("duck <music track> by=<voice track> [amount=-12] [attack=20] [release=400]"))?;
        let by = a.take(&["by", "under", "source", "with"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("duck needs by=<track>"))?;
        let ci = self.ci();
        if by.eq_ignore_ascii_case("none") || by.eq_ignore_ascii_case("off") {
            let t = self.p.comps[ci].track_mut(&tid).ok_or_else(|| anyhow!("no track {tid}"))?;
            t.duck = None;
            self.note(format!("~{tid} duck off"));
            return Ok(());
        }
        let by_id = self.p.comps[ci].track(&by).ok_or_else(|| anyhow!("no track {by}"))?.id.clone();
        let amount: f64 = a.take(&["amount", "db", "gain"]).map(|s| s.trim_end_matches("dB").trim_end_matches("db").parse()).transpose()?.unwrap_or(-12.0);
        let attack: f64 = a.take(&["attack"]).map(|s| s.trim_end_matches("ms").parse()).transpose()?.unwrap_or(20.0);
        let release: f64 = a.take(&["release"]).map(|s| s.trim_end_matches("ms").parse()).transpose()?.unwrap_or(400.0);
        let t = self.p.comps[ci].track_mut(&tid).ok_or_else(|| anyhow!("no track {tid}"))?;
        if t.id == by_id {
            bail!("a track can't duck itself");
        }
        t.duck = Some(Duck { by: by_id.clone(), amount: -amount.abs(), attack, release });
        let id = t.id.clone();
        self.note(format!("~{id} ducks {}dB under {by_id}", fmt_num(-amount.abs())));
        Ok(())
    }

    // ------------------------------------------------------------ placement

    /// Parses `m1[12-25.5]` style source ranges.
    fn split_src_range(&self, s: &str) -> Result<(String, Option<f64>, Option<f64>)> {
        if let Some(i) = s.find('[') {
            if s.ends_with(']') {
                let key = s[..i].to_string();
                let inner = &s[i + 1..s.len() - 1];
                let (x, y) = inner.split_once('-').ok_or_else(|| anyhow!("range must look like {key}[in-out]"))?;
                let a = if x.trim().is_empty() { None } else { Some(self.time(x)?) };
                let b = if y.trim().is_empty() { None } else { Some(self.time(y)?) };
                return Ok((key, a, b));
            }
        }
        Ok((s.to_string(), None, None))
    }

    fn c_add(&mut self, a: &mut A, append: bool) -> Result<()> {
        let mode = place_mode(a)?;
        let mut track_spec = a.take(&["track", "t"]);
        let mut srcs = vec![];
        for p in a.pos.drain(..).collect::<Vec<_>>() {
            if parse_track_spec(&p).is_some() && track_spec.is_none() && self.p.media(&p).is_none() {
                track_spec = Some(p);
            } else {
                srcs.push(p);
            }
        }
        if srcs.is_empty() {
            bail!("add <media|comp|path>[in-out] [V1] [at=] [in=] [out=] [dur=] [speed=] [props...]");
        }
        let at_s = a.take(&["at", "start"]);
        let after = a.take(&["after"]);
        let in_s = a.take(&["in", "from"]);
        let out_s = a.take(&["out", "to"]);
        let dur_s = a.take(&["dur", "duration", "len"]);
        let speed_s = a.take(&["speed", "spd"]);
        let alias = a.take(&["as", "name"]);
        let props = a.rest();
        let multi = srcs.len() > 1;
        let mut cursor: Option<f64> = None;
        let mut seq_track: Option<String> = None;
        for raw in srcs.iter() {
            let (key, r_in, r_out) = self.split_src_range(raw)?;
            // comp or media?
            let comp_src = self.p.comp(&key).map(|c| c.id.clone()).filter(|_| self.p.media(&key).is_none());
            let (kind, src_id, natural, is_audio) = if let Some(cid) = comp_src {
                if cid == self.comp || self.p.comp_contains(&cid, &self.comp, 0) {
                    bail!("can't nest {cid} inside itself");
                }
                let d = self.p.comp(&cid).unwrap().duration();
                (ItemKind::Comp, cid, if d > 0.0 { Some(d) } else { None }, false)
            } else {
                let mid = self.media_or_import(&key)?;
                let m = self.p.media(&mid).unwrap().clone();
                (ItemKind::Clip, mid, if m.is_still() { None } else { Some(m.dur) }, m.kind == MediaKind::Audio)
            };
            let speed = speed_s.as_deref().map(parse_speed).transpose()?.unwrap_or(1.0).abs();
            let src_in = match (r_in, &in_s) {
                (Some(v), _) => v,
                (None, Some(s)) => self.time(s)?,
                _ => 0.0,
            };
            if src_in < 0.0 {
                bail!("in-point can't be negative");
            }
            if let Some(nat) = natural {
                if src_in >= nat - 1e-3 {
                    bail!("in-point {} is past the end of {} ({}s)", fmt_t(src_in), key, fmt_t(nat));
                }
            }
            let out = match (r_out, &out_s) {
                (Some(v), _) => Some(v),
                (None, Some(s)) => Some(self.time(s)?),
                _ => None,
            };
            let mut dur = if let Some(d) = &dur_s {
                self.time(d)?
            } else if let Some(o) = out {
                if o <= src_in {
                    bail!("out ({}) must be after in ({})", fmt_t(o), fmt_t(src_in));
                }
                (o - src_in) / speed
            } else if let Some(nat) = natural {
                (nat - src_in) / speed
            } else {
                5.0
            };
            if let Some(nat) = natural {
                let avail = (nat - src_in) / speed;
                if dur > avail + 1e-3 {
                    self.warn(format!("{key}: only {}s available after in={}, clamped", fmt_t(avail), fmt_t(src_in)));
                    dur = avail;
                }
            }
            let kind_track = if is_audio { TrackKind::Audio } else { TrackKind::Video };
            // start time
            let explicit_track = track_spec.clone();
            let start = if let Some(c) = cursor {
                c
            } else if let Some(s) = &at_s {
                self.time(s)?
            } else if let Some(af) = &after {
                self.item_span(af).ok_or_else(|| anyhow!("unknown item '{af}'"))?.1
            } else {
                // append to end of target track
                let tid = explicit_track.clone().unwrap_or_else(|| if is_audio { "A1".into() } else { "V1".into() });
                self.comp().track(&tid).map(|t| t.end()).unwrap_or(0.0)
            };
            let mut it = self.new_item(kind, start, dur);
            it.src = Some(src_id.clone());
            it.src_in = src_in;
            if speed != 1.0 {
                it.props.insert("speed".into(), Anim::new(Val::N(speed)));
            }
            if let Some(al) = &alias {
                if !multi {
                    it.name = Some(al.clone());
                }
            }
            let id = it.id.clone();
            let tid = match (explicit_track, &seq_track) {
                (Some(t), _) => t,
                (None, Some(t)) => t.clone(),
                (None, None) if at_s.is_some() => self.pick_track(kind_track, start, start + dur, 1),
                (None, None) => {
                    if is_audio {
                        "A1".into()
                    } else {
                        "V1".into()
                    }
                }
            };
            let tid = self.place(it, &tid, mode)?;
            if !props.is_empty() {
                self.apply_props(&id, &props)?;
            }
            self.note(format!("+{id} {} {} {} {}-{}{}", if kind == ItemKind::Comp { "comp" } else { "clip" }, src_id, tid, fmt_t(start), fmt_t(start + dur), if src_in > 0.0 { format!(" in={}", fmt_t(src_in)) } else { String::new() }));
            self.created.push(id);
            if multi || append {
                cursor = Some(start + dur);
                seq_track = Some(tid);
            }
        }
        Ok(())
    }

    pub(crate) fn overlay_start_dur(&mut self, a: &mut A, def_dur: f64) -> Result<(f64, f64)> {
        let start = match (a.take(&["at", "start"]), a.take(&["after"])) {
            (Some(s), _) => self.time(&s)?,
            (None, Some(af)) => self.item_span(&af).ok_or_else(|| anyhow!("unknown item '{af}'"))?.1,
            _ => 0.0,
        };
        let dur = if let Some(e) = a.take(&["end", "until"]) {
            self.time(&e)? - start
        } else if let Some(d) = a.take(&["dur", "duration", "len"]) {
            self.time(&d)?
        } else {
            def_dur
        };
        if dur <= 0.0 {
            bail!("duration must be positive");
        }
        Ok((start, dur))
    }

    pub(crate) fn place_overlay(&mut self, it: Item, track: Option<String>) -> Result<String> {
        let tid = match track {
            Some(t) => t,
            None => self.pick_track_above(TrackKind::Video, it.start, it.end()),
        };
        self.place(it, &tid, Mode::Strict)
    }

    pub(crate) fn take_track(&mut self, a: &mut A) -> Option<String> {
        if let Some(t) = a.take(&["track", "t"]) {
            return Some(t);
        }
        if let Some(i) = a.pos.iter().position(|p| parse_track_spec(p).is_some()) {
            return Some(a.pos.remove(i));
        }
        None
    }

    fn c_text(&mut self, a: &mut A) -> Result<()> {
        let track = self.take_track(a);
        let content = a.take(&["text", "content"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("text '<content>' [at=] [dur=] [size=] [color=] [enter=preset] [exit=preset]"))?;
        let (start, dur) = self.overlay_start_dur(a, 3.0)?;
        let alias = a.take(&["as", "name"]);
        let enter = a.take(&["enter", "anim", "in"]);
        let exit = a.take(&["exit", "outro", "out"]);
        let style = a.take(&["style", "preset"]);
        let mut it = self.new_item(ItemKind::Text, start, dur);
        it.text = Some(content.replace("\\n", "\n"));
        it.name = alias;
        let id = it.id.clone();
        // style defaults, then the brand kit, then what was asked
        let mut props = text_style(style.as_deref())?;
        let asked = a.rest();
        let mut brand = asked.clone();
        let headline = !matches!(style.as_deref().map(|s| s.to_ascii_lowercase()).as_deref(), Some("caption" | "subtitle" | "minimal"));
        self.brand_text_props(&mut brand, headline);
        props.extend(brand.into_iter().skip(asked.len()));
        props.extend(asked);
        let tid = self.place_overlay(it, track)?;
        self.apply_props(&id, &props)?;
        let n_chars = content.chars().filter(|c| !c.is_whitespace()).count();
        for (preset, _is_exit) in [(enter, false), (exit, true)] {
            if let Some(p) = preset {
                self.add_anim_preset(&id, &p, &super::presets::AnimOpts::default(), n_chars)?;
            }
        }
        let shown: String = content.chars().take(40).collect();
        self.note(format!("+{id} text '{}' {tid} {}-{}", shown, fmt_t(start), fmt_t(start + dur)));
        self.created.push(id);
        Ok(())
    }

    pub fn add_anim_preset(&mut self, id: &str, preset: &str, o: &super::presets::AnimOpts, n_chars: usize) -> Result<()> {
        let l = self.relocate(id)?;
        let cx = self.vctx_of(l.0);
        let is_text = self.item_at(l).kind == ItemKind::Text;
        if is_text && super::presets::is_text_anim(preset) {
            let dur = self.item_at(l).dur;
            let (kind, props) = super::presets::text_animator(preset, dur, n_chars, o)?;
            let it = self.item_mut_at(l);
            let aid = it.next_sub("x");
            it.anims.push(super::presets::new_animator(aid, kind, props));
            return Ok(());
        }
        if super::presets::is_layer_anim(preset) {
            let it = self.item_mut_at(l);
            return super::presets::apply_layer_anim(it, preset, o, cx);
        }
        let mut names: Vec<&str> = vec![];
        for (n, _) in super::presets::LAYER_ENTER.iter().chain(super::presets::LAYER_EXIT).chain(super::presets::LAYER_LOOP).chain(super::presets::LAYER_ONESHOT) {
            names.push(n);
        }
        if is_text {
            names.extend(super::presets::TEXT_ANIMS.iter().map(|(n, _)| *n));
        }
        let hint = schema::suggest(names.into_iter(), preset).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
        bail!("unknown animation '{preset}'.{hint} (help anim)")
    }

    fn c_shape(&mut self, a: &mut A) -> Result<()> {
        let track = self.take_track(a);
        let first = a.shift().ok_or_else(|| anyhow!("shape <rect|ellipse|polygon|star|path|line|arrow> [props] | shape add <item> <kind> [props]"))?;
        // `shape add g3 ellipse ...` adds to an existing shape layer
        let (target, kind_raw) = if first.eq_ignore_ascii_case("add") {
            let t = a.shift().ok_or_else(|| anyhow!("shape add <item> <kind>"))?;
            (Some(self.item_id(&t)?), a.shift().ok_or_else(|| anyhow!("shape add <item> <kind>"))?)
        } else {
            (None, first)
        };
        let kind = schema::normalize_shape_kind(&kind_raw).ok_or_else(|| anyhow!("unknown shape '{kind_raw}' (rect, ellipse, circle, polygon, star, path, line, arrow)"))?;
        let mut shape_props = crate::model::project::Props::new();
        let cx = self.vctx();
        let mut layer_kv = vec![];
        let (start, dur) = if target.is_none() { self.overlay_start_dur(a, 3.0)? } else { (0.0, 0.0) };
        let alias = a.take(&["as", "name"]);
        let enter = a.take(&["enter", "anim"]);
        let exit = a.take(&["exit"]);
        for (k, v) in a.rest() {
            let sk = schema::shape_props(kind).and_then(|l| schema::find(l, &k)).or_else(|| schema::find(schema::SHAPE_COMMON, &k));
            // `size=` shorthand for w/h
            if k == "size" && matches!(kind, "rect" | "ellipse") {
                let parts: Vec<&str> = v.split(['x', ',']).collect();
                let w: f64 = parts[0].parse()?;
                let h: f64 = parts.get(1).map(|s| s.parse()).transpose()?.unwrap_or(w);
                shape_props.insert("w".into(), Anim::new(Val::N(w)));
                shape_props.insert("h".into(), Anim::new(Val::N(h)));
                continue;
            }
            match sk {
                Some(p) if !(target.is_none() && matches!(p.name, "rot" | "scale" | "opacity") && schema::find(schema::LAYER, &k).is_some()) => {
                    shape_props.insert(p.name.to_string(), Anim::new(schema::clamp_val(p, schema::parse_val(p, &v, cx)?)));
                }
                _ => layer_kv.push((k, v)),
            }
        }
        if target.is_none() && !shape_props.contains_key("fill") {
            if let Some(c) = self.p.brand.accent.as_deref().and_then(|c| Color::parse(c).ok()) {
                shape_props.insert("fill".into(), Anim::new(Val::C(c)));
            }
        }
        if kind_raw.eq_ignore_ascii_case("circle") {
            if let Some(w) = shape_props.get("w").cloned() {
                shape_props.entry("h".into()).or_insert(w);
            }
        }
        if kind_raw.eq_ignore_ascii_case("triangle") {
            shape_props.entry("points".into()).or_insert(Anim::new(Val::N(3.0)));
        }
        if kind_raw.eq_ignore_ascii_case("square") {
            if let Some(w) = shape_props.get("w").cloned() {
                shape_props.entry("h".into()).or_insert(w);
            } else {
                shape_props.insert("w".into(), Anim::new(Val::N(300.0)));
                shape_props.insert("h".into(), Anim::new(Val::N(300.0)));
            }
        }
        if matches!(kind, "line" | "arrow") && !shape_props.contains_key("strokeW") {
            shape_props.insert("strokeW".into(), Anim::new(Val::N(8.0)));
            if !shape_props.contains_key("stroke") {
                let fill = shape_props.get("fill").map(|f| f.v.clone()).unwrap_or(Val::C(crate::model::value::Color::WHITE));
                shape_props.insert("stroke".into(), Anim::new(fill));
            }
            shape_props.insert("fill".into(), Anim::new(Val::C(crate::model::value::Color::TRANSPARENT)));
        }
        if let Some(tid) = target {
            let l = self.relocate(&tid)?;
            if self.item_at(l).kind != ItemKind::Shape {
                bail!("{tid} is not a shape layer");
            }
            let it = self.item_mut_at(l);
            let sid = it.next_sub("s");
            it.shapes.push(Effect { id: sid.clone(), kind: kind.to_string(), on: true, props: shape_props });
            if !layer_kv.is_empty() {
                bail!("unknown shape propert(ies): {}", layer_kv.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", "));
            }
            self.note(format!("+{tid}.{sid} {kind}"));
            return Ok(());
        }
        let mut it = self.new_item(ItemKind::Shape, start, dur);
        it.name = alias;
        let sid = it.next_sub("s");
        it.shapes.push(Effect { id: sid, kind: kind.to_string(), on: true, props: shape_props });
        let id = it.id.clone();
        let tid = self.place_overlay(it, track)?;
        self.apply_props(&id, &layer_kv)?;
        for p in [enter, exit].into_iter().flatten() {
            self.add_anim_preset(&id, &p, &super::presets::AnimOpts::default(), 0)?;
        }
        self.note(format!("+{id} shape {kind} {tid} {}-{}", fmt_t(start), fmt_t(start + dur)));
        self.created.push(id);
        Ok(())
    }

    fn c_solid(&mut self, a: &mut A) -> Result<()> {
        let track = self.take_track(a);
        let color = a.take(&["color", "c", "fill"]).or_else(|| a.shift()).unwrap_or_else(|| "#000000".into());
        let (start, dur) = self.overlay_start_dur(a, 3.0)?;
        let alias = a.take(&["as", "name"]);
        let mut it = self.new_item(ItemKind::Solid, start, dur);
        it.name = alias;
        let id = it.id.clone();
        let tid = match track {
            Some(t) => self.place(it, &t, Mode::Strict)?,
            None => self.place_overlay(it, None)?,
        };
        let mut kv = vec![("color".to_string(), color)];
        kv.extend(a.rest());
        self.apply_props(&id, &kv)?;
        self.note(format!("+{id} solid {tid} {}-{}", fmt_t(start), fmt_t(start + dur)));
        self.created.push(id);
        Ok(())
    }

    fn c_adjust(&mut self, a: &mut A) -> Result<()> {
        let track = self.take_track(a);
        let comp_dur = self.comp().duration();
        let (start, dur) = self.overlay_start_dur(a, if comp_dur > 0.0 { comp_dur } else { 5.0 })?;
        let alias = a.take(&["as", "name"]);
        let fx = a.take(&["fx"]);
        let mut it = self.new_item(ItemKind::Adjust, start, dur);
        it.name = alias;
        let id = it.id.clone();
        let tid = self.place_overlay(it, track)?;
        let kv = a.rest();
        self.apply_props(&id, &kv)?;
        if let Some(fx) = fx {
            for name in fx.split(',') {
                self.add_effect(&id, name, &[], false)?;
            }
        }
        self.note(format!("+{id} adjust {tid} {}-{}", fmt_t(start), fmt_t(start + dur)));
        self.created.push(id);
        Ok(())
    }

    // ------------------------------------------------------------ editing

    fn c_del(&mut self, a: &mut A) -> Result<()> {
        let ripple_all = matches!(a.take(&["ripple"]).as_deref(), Some("all") | Some("sync"));
        let ripple = ripple_all || a.flag("ripple");
        let spec = a.pos.drain(..).collect::<Vec<_>>().join(",");
        if spec.is_empty() {
            bail!("del <ids> [ripple|ripple=all]");
        }
        // sub-objects: c3.f1
        if !spec.contains(',') && spec.contains('.') && self.p.locate(&spec).is_none() {
            return self.del_sub(&spec);
        }
        let mut ids = self.targets(&spec)?;
        // delete right-to-left so ripples don't affect each other
        ids.sort_by(|x, y| {
            let sx = self.item_span(x).map(|s| s.0).unwrap_or(0.0);
            let sy = self.item_span(y).map(|s| s.0).unwrap_or(0.0);
            sy.partial_cmp(&sx).unwrap_or(std::cmp::Ordering::Equal)
        });
        ids.dedup();
        for id in ids {
            let (ci, ti, _) = self.relocate(&id)?;
            let tid = self.p.comps[ci].tracks[ti].id.clone();
            let it = self.remove_item(&id)?;
            if ripple {
                self.ripple(ci, &tid, it.end(), -it.dur, ripple_all);
            }
            self.note(format!("-{id}{}", if ripple { " (ripple)" } else { "" }));
        }
        Ok(())
    }

    fn c_move(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("move <ids> [at=T|by=±T] [track=V2] [mode=overwrite|insert]"))?;
        let ids = self.targets(&spec)?;
        let at = a.take(&["at", "to", "start"]);
        let by = a.take(&["by", "delta"]).or_else(|| a.shift());
        let track = a.take(&["track", "t"]).or_else(|| a.pos.iter().position(|p| parse_track_spec(p).is_some()).map(|i| a.pos.remove(i)));
        let mode = place_mode(a)?;
        if at.is_none() && by.is_none() && track.is_none() {
            bail!("move needs at=, by= or track=");
        }
        // compute delta from the earliest item when at= is used
        let first_start = ids.iter().filter_map(|i| self.item_span(i)).map(|s| s.0).fold(f64::INFINITY, f64::min);
        let delta = if let Some(t) = &at {
            self.time(t)? - first_start
        } else if let Some(b) = &by {
            let neg = b.starts_with('-');
            let v = self.time(b.trim_start_matches(['+', '-']))?;
            if neg {
                -v
            } else {
                v
            }
        } else {
            0.0
        };
        // remove all first, then re-place (so moving a group past itself works)
        let mut removed = vec![];
        for id in &ids {
            let (ci, ti, _) = self.relocate(id)?;
            let tid = self.p.comps[ci].tracks[ti].id.clone();
            if ci != self.ci() {
                bail!("{id} is in another comp (comp use {} first)", self.p.comps[ci].id);
            }
            let it = self.take_item(id)?;
            removed.push((tid, it));
        }
        for (tid, mut it) in removed {
            it.start += delta;
            if it.start < -EPS {
                bail!("{} would start before 0", it.id);
            }
            it.start = it.start.max(0.0);
            let dest = track.clone().unwrap_or(tid);
            let id = it.id.clone();
            let (s, e) = (it.start, it.end());
            let dest = self.place(it, &dest, mode)?;
            self.note(format!("~{id} {dest} {}-{}", fmt_t(s), fmt_t(e)));
        }
        Ok(())
    }

    fn c_trim(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("trim <id> [start=T|+d] [end=T|-d] [dur=D] [in=src] [ripple]"))?;
        let ripple = a.flag("ripple");
        let ids = self.targets(&spec)?;
        let start_s = a.take(&["start", "head", "left"]);
        let end_s = a.take(&["end", "tail", "right"]);
        let dur_s = a.take(&["dur", "duration"]);
        let in_s = a.take(&["in", "src"]);
        if start_s.is_none() && end_s.is_none() && dur_s.is_none() && in_s.is_none() {
            bail!("trim needs start=, end=, dur= or in=");
        }
        for id in ids {
            let l = self.relocate(&id)?;
            let it = self.item_at(l).clone();
            let tid = self.p.comps[l.0].tracks[l.1].id.clone();
            let old_end = it.end();
            if let Some(s) = &start_s {
                let ns = rel_or_abs(s, it.start, |x| self.time(x))?;
                // check room on the left (a ripple trim keeps the clip's start where it is)
                if ns < it.start && !ripple {
                    let t = &self.p.comps[l.0].tracks[l.1];
                    if !t.is_free(ns, it.start, Some(&id)) {
                        bail!("{id}: no room to extend left (item in the way)");
                    }
                }
                self.trim_start_to(&id, ns)?;
                if ripple {
                    // ripple trim: the head changes, the clip stays put and the rest of the track follows
                    let l2 = self.relocate(&id)?;
                    self.item_mut_at(l2).start = it.start;
                }
            }
            if let Some(s) = &in_s {
                let l = self.relocate(&id)?;
                let cur = self.item_at(l).src_in;
                let v = rel_or_abs(s, cur, |x| self.time(x))?;
                if v < 0.0 {
                    bail!("{id}: in-point can't be negative");
                }
                self.item_mut_at(l).src_in = v;
            }
            let l = self.relocate(&id)?;
            let cur = self.item_at(l).clone();
            let new_end = if let Some(e) = &end_s {
                Some(rel_or_abs(e, cur.end(), |x| self.time(x))?)
            } else if let Some(d) = &dur_s {
                Some(cur.start + rel_or_abs(d, cur.dur, |x| self.time(x))?)
            } else {
                None
            };
            if let Some(ne) = new_end {
                if ne > cur.end() && !ripple {
                    let t = &self.p.comps[l.0].tracks[l.1];
                    let ov = t.overlapping(cur.end(), ne, Some(&id));
                    if !ov.is_empty() {
                        bail!("{id}: extending to {} overlaps {} (add ripple)", fmt_t(ne), ov[0].id);
                    }
                }
                self.trim_end_to(&id, ne)?;
            }
            if self.clamp_to_source(&id)? {
                self.warn(format!("{id}: clamped to available media"));
            }
            let l = self.relocate(&id)?;
            let it2 = self.item_at(l).clone();
            if ripple {
                let d = it2.end() - old_end;
                if d.abs() > EPS {
                    // shift items after the old end (excluding this one)
                    let comp = &mut self.p.comps[l.0];
                    let t = comp.track_mut(&tid).unwrap();
                    for o in t.items.iter_mut() {
                        if o.id != id && o.start >= old_end - EPS {
                            o.start += d;
                        }
                    }
                    t.sort();
                }
            }
            self.note(format!("~{id} {}-{}{}", fmt_t(it2.start), fmt_t(it2.end()), if it2.src_in > 0.0 { format!(" in={}", fmt_t(it2.src_in)) } else { String::new() }));
        }
        Ok(())
    }

    fn c_cut(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("cut <id|V1|all> at=T [T2 ...]"))?;
        let mut times: Vec<f64> = vec![];
        if let Some(t) = a.take(&["at", "t"]) {
            for p in t.split(',') {
                times.push(self.time(p)?);
            }
        }
        for p in a.pos.drain(..).collect::<Vec<_>>() {
            times.push(self.time(&p)?);
        }
        if times.is_empty() {
            bail!("cut needs at=T");
        }
        times.sort_by(|x, y| y.partial_cmp(x).unwrap());
        let ids = self.targets(&spec)?;
        let single = ids.len() == 1 && self.p.locate(&spec).is_some();
        let mut made = 0;
        for t in times {
            for id in &ids {
                // after earlier splits an id may now refer to the left piece; find the piece containing t
                let (s, e) = match self.item_span(id) {
                    Some(x) => x,
                    None => continue,
                };
                let fps = self.fps();
                if t > s + 0.5 / fps && t < e - 0.5 / fps {
                    let nid = self.split_item(id, t)?;
                    self.note(format!("+{nid} (cut {id} @{})", fmt_t(t)));
                    self.created.push(nid);
                    made += 1;
                } else if single {
                    // maybe the time falls into a piece created by a later cut
                    let tr_items: Vec<String> = {
                        let (ci, ti, _) = self.relocate(id)?;
                        self.p.comps[ci].tracks[ti].items.iter().filter(|i| i.contains(t)).map(|i| i.id.clone()).collect()
                    };
                    if let Some(other) = tr_items.into_iter().find(|o| self.created.contains(o)) {
                        let nid = self.split_item(&other, t)?;
                        self.note(format!("+{nid} (cut {other} @{})", fmt_t(t)));
                        self.created.push(nid);
                        made += 1;
                    } else {
                        bail!("{id} ({}-{}) doesn't span {}", fmt_t(s), fmt_t(e), fmt_t(t));
                    }
                }
            }
        }
        if made == 0 {
            self.warn("nothing to cut at that time");
        }
        Ok(())
    }

    fn c_slip(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("slip <id> <±seconds>"))?)?;
        let d = a.take(&["by"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("slip <id> <±seconds>"))?;
        let neg = d.starts_with('-');
        let v = self.time(d.trim_start_matches(['+', '-']))? * if neg { -1.0 } else { 1.0 };
        let l = self.relocate(&id)?;
        let it = self.item_at(l).clone();
        let ni = it.src_in + v * it.speed();
        if ni < 0.0 {
            bail!("{id}: slip would move the in-point before 0");
        }
        if let Some(lim) = self.src_limit(&it) {
            if ni + it.dur * it.speed() > lim + 1e-3 {
                bail!("{id}: slip runs past the media end");
            }
        }
        self.item_mut_at(l).src_in = ni;
        self.note(format!("~{id} in={}", fmt_t(ni)));
        Ok(())
    }

    fn c_slide(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("slide <id> <±seconds>"))?)?;
        let d = a.take(&["by"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("slide <id> <±seconds>"))?;
        let neg = d.starts_with('-');
        let v = self.time(d.trim_start_matches(['+', '-']))? * if neg { -1.0 } else { 1.0 };
        let (ci, ti, _) = self.relocate(&id)?;
        let it = self.item_at(self.relocate(&id)?).clone();
        let t = &self.p.comps[ci].tracks[ti];
        let prev = t.items.iter().filter(|o| (o.end() - it.start).abs() < 1e-3).map(|o| o.id.clone()).next();
        let next = t.items.iter().filter(|o| (o.start - it.end()).abs() < 1e-3).map(|o| o.id.clone()).next();
        if let Some(p) = &prev {
            let (ps, _) = self.item_span(p).unwrap();
            if it.start + v <= ps + 1.0 / self.fps() {
                bail!("slide too far left");
            }
            self.trim_end_to(p, it.start + v)?;
        } else if it.start + v < 0.0 {
            bail!("slide before 0");
        }
        if let Some(n) = &next {
            let (_, ne) = self.item_span(n).unwrap();
            if it.end() + v >= ne - 1.0 / self.fps() {
                bail!("slide too far right");
            }
            self.trim_start_to(n, it.end() + v)?;
        }
        let l = self.relocate(&id)?;
        self.item_mut_at(l).start += v;
        self.p.comps[ci].tracks[ti].sort();
        self.note(format!("~{id} slid {}", fmt_num(v)));
        Ok(())
    }

    fn c_speed(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("speed <ids> <factor> [ripple]"))?;
        let ripple = a.flag("ripple");
        let f = a.take(&["x", "factor", "speed"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("speed <ids> <factor>"))?;
        let interp = a.take(&["interp"]);
        let f = parse_speed(&f)?;
        for id in self.targets(&spec)? {
            self.set_speed(&id, f, ripple)?;
            if let Some(i) = &interp {
                self.apply_props(&id, &[("interp".into(), i.clone())])?;
            }
        }
        Ok(())
    }

    pub fn set_speed(&mut self, id: &str, f: f64, ripple: bool) -> Result<()> {
        let l = self.relocate(id)?;
        let it = self.item_at(l).clone();
        if !matches!(it.kind, ItemKind::Clip | ItemKind::Comp) {
            bail!("{id}: speed applies to clips and nested comps");
        }
        let old = it.speed();
        let nd = it.dur * old / f.abs();
        let tid = self.p.comps[l.0].tracks[l.1].id.clone();
        if nd > it.dur && !ripple {
            let t = &self.p.comps[l.0].tracks[l.1];
            let ov = t.overlapping(it.end(), it.start + nd, Some(id));
            if !ov.is_empty() {
                bail!("{id}: slowing to {}x makes it {}s long and overlaps {} (add ripple)", fmt_num(f), fmt_t(nd), ov[0].id);
            }
        }
        {
            let m = self.item_mut_at(l);
            m.props.insert("speed".into(), Anim::new(Val::N(f.abs())));
            if f < 0.0 {
                m.props.insert("reverse".into(), Anim::new(Val::B(true)));
            }
            m.dur = nd;
            // keep keyframes proportional
            let ratio = old / f.abs();
            scale_item_keys(m, ratio);
        }
        if ripple {
            let d = nd - it.dur;
            let comp = &mut self.p.comps[l.0];
            let t = comp.track_mut(&tid).unwrap();
            for o in t.items.iter_mut() {
                if o.id != id && o.start >= it.end() - EPS {
                    o.start += d;
                }
            }
            t.sort();
        }
        self.note(format!("~{id} speed {}x dur={}", fmt_num(f.abs()), fmt_t(nd)));
        Ok(())
    }

    fn c_reverse(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("reverse <ids> [off]"))?;
        let off = a.flag("off") || a.flag("false");
        for id in self.targets(&spec)? {
            let l = self.relocate(&id)?;
            let m = self.item_mut_at(l);
            if m.kind != ItemKind::Clip {
                bail!("{id}: reverse applies to media clips");
            }
            m.props.insert("reverse".into(), Anim::new(Val::B(!off)));
            self.note(format!("~{id} reverse={}", !off));
        }
        Ok(())
    }

    fn c_freeze(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("freeze <id> at=T [dur=2]"))?)?;
        let at = a.take(&["at", "t"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("freeze needs at=T"))?;
        let t = self.time(&at)?;
        let d = a.take(&["dur", "duration"]).map(|s| self.time(&s)).transpose()?.unwrap_or(2.0);
        let l = self.relocate(&id)?;
        let it = self.item_at(l).clone();
        if it.kind != ItemKind::Clip {
            bail!("{id}: freeze applies to media clips");
        }
        let tid = self.p.comps[l.0].tracks[l.1].id.clone();
        let src_t = it.src_in + (t - it.start) * it.speed();
        let ci = l.0;
        // make room: split + ripple
        self.insert_space(ci, &tid, t, d)?;
        let mut f = self.new_item(ItemKind::Clip, t, d);
        f.src = it.src.clone();
        f.src_in = src_t;
        f.props.insert("hold".into(), Anim::new(Val::B(true)));
        for k in ["pos", "scale", "rot", "opacity", "anchor", "fit"] {
            if let Some(v) = it.props.get(k) {
                if !v.is_animated() {
                    f.props.insert(k.into(), v.clone());
                }
            }
        }
        f.fx = it.fx.clone();
        f.props.insert("mute".into(), Anim::new(Val::B(true)));
        let fid = f.id.clone();
        let t_mut = self.p.comps[ci].track_mut(&tid).unwrap();
        t_mut.items.push(f);
        t_mut.sort();
        self.note(format!("+{fid} freeze frame of {id} @{} for {}s", fmt_t(src_t), fmt_t(d)));
        self.created.push(fid);
        Ok(())
    }

    fn c_dup(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("dup <id> [at=T] [track=V2] [as=name]"))?)?;
        let at = a.take(&["at", "start"]);
        let track = a.take(&["track", "t"]).or_else(|| a.pos.iter().position(|p| parse_track_spec(p).is_some()).map(|i| a.pos.remove(i)));
        let alias = a.take(&["as", "name"]);
        let l = self.relocate(&id)?;
        let src = self.item_at(l).clone();
        let tid0 = self.p.comps[l.0].tracks[l.1].id.clone();
        let mut it = src.clone();
        it.id = self.p.next_item_id(src.kind);
        it.name = alias;
        it.start = match at {
            Some(s) => self.time(&s)?,
            None => src.end(),
        };
        let nid = it.id.clone();
        let kind = self.track_kind_for(&it);
        let dest = match track {
            Some(t) => t,
            None => {
                if self.comp().track(&tid0).map(|t| t.is_free(it.start, it.end(), None)).unwrap_or(false) {
                    tid0
                } else {
                    self.pick_track(kind, it.start, it.end(), 1)
                }
            }
        };
        let (s, e) = (it.start, it.end());
        let dest = self.place(it, &dest, Mode::Strict)?;
        self.note(format!("+{nid} (dup {id}) {dest} {}-{}", fmt_t(s), fmt_t(e)));
        self.created.push(nid);
        Ok(())
    }

    fn c_replace(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("replace <id> <media> [in=]"))?)?;
        let mkey = a.take(&["media", "with", "src"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("replace <id> <media>"))?;
        let mid = self.media_or_import(&mkey)?;
        let in_t = a.take(&["in"]).map(|s| self.time(&s)).transpose()?.unwrap_or(0.0);
        let l = self.relocate(&id)?;
        {
            let m = self.item_mut_at(l);
            if m.kind != ItemKind::Clip {
                bail!("{id}: replace applies to media clips");
            }
            m.src = Some(mid.clone());
            m.src_in = in_t;
            m.props.remove("hold");
        }
        if self.clamp_to_source(&id)? {
            self.warn(format!("{id}: new media is shorter, clamped"));
        }
        self.note(format!("~{id} src={mid}"));
        Ok(())
    }

    fn c_close(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("close <V1|A1|all> (removes gaps)"))?;
        let ci = self.ci();
        let tids: Vec<String> = if spec.eq_ignore_ascii_case("all") { self.comp().tracks.iter().map(|t| t.id.clone()).collect() } else { vec![self.comp().track(&spec).ok_or_else(|| anyhow!("no track {spec}"))?.id.clone()] };
        for tid in tids {
            let t = self.p.comps[ci].track_mut(&tid).unwrap();
            t.sort();
            let mut cur = 0.0;
            let mut moved = 0;
            for it in t.items.iter_mut() {
                if (it.start - cur).abs() > EPS {
                    it.start = cur;
                    moved += 1;
                }
                cur = it.end();
            }
            self.note(format!("~{tid} closed gaps ({moved} moved)"));
        }
        Ok(())
    }

    fn c_gap(&mut self, a: &mut A) -> Result<()> {
        let tid = a.shift().ok_or_else(|| anyhow!("gap <track|all> at=T dur=D"))?;
        let at = self.time(&a.take(&["at"]).ok_or_else(|| anyhow!("gap needs at="))?)?;
        let d = self.time(&a.take(&["dur", "duration"]).ok_or_else(|| anyhow!("gap needs dur="))?)?;
        let ci = self.ci();
        let tids: Vec<String> = if tid.eq_ignore_ascii_case("all") { self.comp().tracks.iter().map(|t| t.id.clone()).collect() } else { vec![tid] };
        for t in tids {
            self.insert_space(ci, &t, at, d)?;
            self.note(format!("~{t} gap {}s @{}", fmt_t(d), fmt_t(at)));
        }
        Ok(())
    }

    pub(crate) fn c_detach(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("detach <ids>"))?;
        for id in self.targets(&spec)? {
            let l = self.relocate(&id)?;
            let it = self.item_at(l).clone();
            let m = it.src.as_deref().and_then(|s| self.p.media(s)).cloned();
            let Some(m) = m.filter(|m| m.has_audio && m.has_video) else {
                bail!("{id} has no linked audio to detach");
            };
            let mut ac = self.new_item(ItemKind::Clip, it.start, it.dur);
            ac.src = Some(m.id.clone());
            ac.src_in = it.src_in;
            for k in ["speed", "reverse", "vol", "pan", "fadeIn", "fadeOut"] {
                if let Some(v) = it.props.get(k) {
                    ac.props.insert(k.into(), v.clone());
                }
            }
            ac.afx = it.afx.clone();
            let aid = ac.id.clone();
            let tid = self.pick_track(TrackKind::Audio, it.start, it.end(), 1);
            self.place(ac, &tid, Mode::Strict)?;
            let l = self.relocate(&id)?;
            let v = self.item_mut_at(l);
            v.props.insert("mute".into(), Anim::new(Val::B(true)));
            v.afx.clear();
            self.note(format!("+{aid} audio of {id} on {tid}"));
            self.created.push(aid);
        }
        Ok(())
    }

    fn c_fit(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("fit <id> dur=T [ripple]"))?)?;
        let ripple = a.flag("ripple");
        let d = a.take(&["dur", "to"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("fit needs dur="))?;
        let target = self.time(&d)?;
        let l = self.relocate(&id)?;
        let it = self.item_at(l).clone();
        let f = it.speed() * it.dur / target;
        self.set_speed(&id, f, ripple)
    }

    fn c_name(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("name <id> <name>"))?)?;
        let n = a.shift().ok_or_else(|| anyhow!("name <id> <name>"))?;
        if parse_track_spec(&n).is_some() || n.starts_with('$') {
            bail!("'{n}' can't be used as a name");
        }
        let l = self.relocate(&id)?;
        self.item_mut_at(l).name = if n == "-" { None } else { Some(n.clone()) };
        self.note(format!("~{id} name={n}"));
        Ok(())
    }

    fn c_marker(&mut self, a: &mut A) -> Result<()> {
        let first = a.shift().unwrap_or_else(|| "add".into());
        let ci = self.ci();
        match first.to_ascii_lowercase().as_str() {
            "del" | "rm" | "delete" => {
                let k = a.shift().ok_or_else(|| anyhow!("marker del <time|name>"))?;
                let before = self.p.comps[ci].markers.len();
                let t = self.time(&k).ok();
                self.p.comps[ci].markers.retain(|m| !(m.name.eq_ignore_ascii_case(&k) || t.map(|t| (m.t - t).abs() < 1e-3).unwrap_or(false)));
                self.note(format!("-{} marker(s)", before - self.p.comps[ci].markers.len()));
            }
            "clear" => {
                self.p.comps[ci].markers.clear();
                self.note("markers cleared");
            }
            "set" | "edit" => {
                let k = a.shift().ok_or_else(|| anyhow!("marker set <time|name> [name=] [t=] [color=] [dur=] [note=]"))?;
                let i = self.find_marker(&k)?;
                let mut m = self.p.comps[ci].markers[i].clone();
                self.marker_fields(&mut m, a.rest())?;
                let s = format!("~marker {} {}", fmt_t(m.t), m.name);
                let ms = &mut self.p.comps[ci].markers;
                ms[i] = m;
                ms.sort_by(|x, y| x.t.partial_cmp(&y.t).unwrap());
                self.note(s);
            }
            other => {
                let tstr = if other == "add" { a.take(&["at", "t"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("marker add <time> [name]"))? } else { first.clone() };
                let t = self.time(&tstr)?;
                let name = a.take(&["name"]).or_else(|| a.shift()).unwrap_or_default();
                let mut mk = Marker::new(t, name.clone());
                self.marker_fields(&mut mk, a.rest())?;
                let m = &mut self.p.comps[ci].markers;
                m.push(mk);
                m.sort_by(|x, y| x.t.partial_cmp(&y.t).unwrap());
                self.note(format!("+marker {} {}", fmt_t(t), name));
            }
        }
        Ok(())
    }

    fn c_master(&mut self, a: &mut A) -> Result<()> {
        let ci = self.ci();
        for (k, v) in a.rest() {
            let m = &mut self.p.comps[ci].master;
            match k.to_ascii_lowercase().as_str() {
                "vol" | "volume" | "gain" => m.vol = v.trim_end_matches("dB").trim_end_matches("db").parse()?,
                "lufs" | "loudness" | "normalize" => {
                    m.lufs = if v == "off" || v == "none" { None } else { Some(v.trim_end_matches("lufs").trim_end_matches("LUFS").parse()?) }
                }
                "limiter" | "limit" => m.limiter = !matches!(v.as_str(), "0" | "false" | "off"),
                other => bail!("unknown master setting '{other}' (vol lufs limiter)"),
            }
        }
        let m = &self.p.comps[ci].master;
        let s = format!("~master vol={} lufs={} limiter={}", fmt_num(m.vol), m.lufs.map(fmt_num).unwrap_or_else(|| "off".into()), m.limiter);
        self.note(s);
        Ok(())
    }
}

pub const VERBS: &[&str] = &[
    "import", "comp", "track", "add", "append", "insert", "text", "shape", "solid", "adjust", "del", "move", "trim", "cut", "slip", "slide", "speed", "reverse",
    "freeze", "dup", "replace", "close", "gap", "detach", "fit", "set", "kf", "kf+", "kf-", "expr", "anim", "fx", "afx", "tr", "fade", "mask", "matte", "parent",
    "color", "lut", "marker", "master", "name", "duck", "precomp", "autocut", "captions", "montage", "beats", "reframe", "title", "keep", "null", "camera", "roll",
    "lift", "extract", "swap", "paste", "stagger", "align", "nudge", "distribute", "ease", "workarea", "sound", "sfx", "speedramp", "punchin", "jcut", "lcut", "brand", "undo", "redo",
];

/// How a placed clip treats clips already there: `overwrite` / `insert` (or `ripple`) as flags or `mode=`.
fn place_mode(a: &mut A) -> Result<Mode> {
    if a.flag("overwrite") {
        return Ok(Mode::Overwrite);
    }
    if a.flag("insert") || a.flag("ripple") {
        return Ok(Mode::Insert);
    }
    Ok(match a.take(&["mode"]).map(|m| m.to_ascii_lowercase()).as_deref() {
        Some("overwrite") => Mode::Overwrite,
        Some("insert") | Some("ripple") => Mode::Insert,
        Some("strict") | None => Mode::Strict,
        Some(o) => bail!("mode must be strict|overwrite|insert, not {o}"),
    })
}

pub fn same_path(a: &str, b: &str) -> bool {
    let n = |s: &str| s.replace('\\', "/").to_ascii_lowercase();
    n(a) == n(b)
}

pub fn kind_s(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::Image => "image",
        MediaKind::Svg => "svg",
    }
}

pub fn media_summary(m: &Media) -> String {
    let mut s = String::new();
    if m.is_visual() {
        s.push_str(&format!("{}x{}", m.w, m.h));
    }
    if m.kind == MediaKind::Video {
        s.push_str(&format!(" {}fps", fmt_num((m.fps * 100.0).round() / 100.0)));
    }
    if !m.is_still() {
        s.push_str(&format!(" {}s", fmt_t(m.dur)));
    }
    if m.kind == MediaKind::Video && !m.has_audio {
        s.push_str(" noaudio");
    }
    if m.alpha {
        s.push_str(" alpha");
    }
    s.trim().to_string()
}

pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let l = s.to_ascii_lowercase();
    let named = match l.as_str() {
        "vertical" | "9:16" | "portrait" | "reels" | "tiktok" | "shorts" => Some((1080, 1920)),
        "square" | "1:1" => Some((1080, 1080)),
        "4:5" | "instagram" => Some((1080, 1350)),
        "16:9" | "1080p" | "hd" | "landscape" | "fhd" => Some((1920, 1080)),
        "720p" => Some((1280, 720)),
        "1440p" | "2k" => Some((2560, 1440)),
        "4k" | "uhd" | "2160p" => Some((3840, 2160)),
        "21:9" | "cinema" => Some((2560, 1080)),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let (w, h) = l.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// `+0.5` / `-1` are relative to `cur`; otherwise absolute via `abs`.
pub fn rel_or_abs(s: &str, cur: f64, abs: impl Fn(&str) -> Result<f64>) -> Result<f64> {
    let s = s.trim();
    if let Some(r) = s.strip_prefix('+') {
        return Ok(cur + abs(r)?);
    }
    if let Some(r) = s.strip_prefix('-') {
        return Ok(cur - abs(r)?);
    }
    abs(s)
}

fn each_anim(it: &mut Item, f: &mut dyn FnMut(&mut Anim)) {
    for a in it.props.values_mut() {
        f(a);
    }
    for list in [&mut it.fx, &mut it.masks, &mut it.shapes, &mut it.anims, &mut it.afx] {
        for e in list.iter_mut() {
            for a in e.props.values_mut() {
                f(a);
            }
        }
    }
}

pub fn shift_item_keys(it: &mut Item, dt: f64) {
    each_anim(it, &mut |a| a.shift(dt));
}

pub fn scale_item_keys(it: &mut Item, ratio: f64) {
    if (ratio - 1.0).abs() < 1e-9 {
        return;
    }
    each_anim(it, &mut |a| {
        for k in a.k.iter_mut() {
            k.t *= ratio;
        }
    });
}

/// Caption/title style presets -> text props.
pub fn text_style(style: Option<&str>) -> Result<Vec<(String, String)>> {
    let kv = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    Ok(match style.map(|s| s.to_ascii_lowercase()) {
        None => vec![],
        Some(s) => match s.as_str() {
            "title" | "headline" => kv(&[("size", "120"), ("weight", "black"), ("tracking", "-2")]),
            "subtitle" => kv(&[("size", "54"), ("weight", "medium"), ("color", "#ffffffcc")]),
            "caption" | "tiktok" | "bold" => kv(&[("size", "72"), ("weight", "black"), ("stroke", "#000000"), ("strokeW", "8"), ("caps", "1")]),
            "box" | "boxed" => kv(&[("size", "56"), ("weight", "bold"), ("bg", "#000000b3"), ("bgPad", "18")]),
            "neon" => kv(&[("size", "110"), ("weight", "bold"), ("color", "#ffffff"), ("shadow", "#ff2bd6"), ("shadowY", "0"), ("shadowBlur", "28")]),
            "minimal" => kv(&[("size", "64"), ("weight", "regular"), ("tracking", "6"), ("caps", "1")]),
            "outline" => kv(&[("size", "120"), ("weight", "black"), ("color", "none"), ("stroke", "#ffffff"), ("strokeW", "4")]),
            "gradient" => kv(&[("size", "120"), ("weight", "black"), ("color", "#ffd60a"), ("color2", "#ff2d55")]),
            "shadow" => kv(&[("size", "96"), ("weight", "bold"), ("shadow", "#000000aa")]),
            "lowerthird" => kv(&[("size", "52"), ("weight", "bold"), ("align", "left")]),
            other => bail!("unknown text style '{other}' (title subtitle caption box neon minimal outline gradient shadow lowerthird)"),
        },
    })
}
