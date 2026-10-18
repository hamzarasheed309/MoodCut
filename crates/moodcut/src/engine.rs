//! The Engine owns the open project and is shared by the MCP server, the HTTP
//! API and the GUI. Every edit goes through `edit()` (MoodCut Script), which is
//! transactional, undoable, autosaved and broadcast to listeners.

use crate::analyze;
use crate::model::project::{Media, Project};
use crate::ops::{self, Env};
use crate::render::comp::{render_frame, RenderCtx};
use crate::render::decode::FrameServer;
use crate::render::encode::{self, Progress, RenderOpts, RenderResult};
use crate::render::ffmpeg::Ff;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;

const MAX_UNDO: usize = 200;

pub struct State {
    pub p: Project,
    pub path: Option<PathBuf>,
    pub rev: u64,
    pub active: String,
    pub undo: Vec<(Project, String)>,
    pub redo: Vec<(Project, String)>,
    pub dirty: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub kind: String,
    pub rev: u64,
    pub source: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub rev: u64,
    pub source: String,
    pub script: String,
    pub result: String,
    pub ok: bool,
}

pub struct Job {
    pub id: String,
    pub opts: RenderOpts,
    pub prog: Progress,
    pub started: Instant,
    pub result: Mutex<Option<Result<RenderResult, String>>>,
}

impl Job {
    pub fn status(&self) -> Value {
        let done = self.prog.done.load(Ordering::Relaxed);
        let total = self.prog.total.load(Ordering::Relaxed).max(1);
        let el = self.started.elapsed().as_secs_f64();
        let res = self.result.lock();
        let (state, extra) = match &*res {
            None => ("running", json!({})),
            Some(Ok(r)) => ("done", json!({"out": r.out, "bytes": r.bytes, "secs": (r.secs * 10.0).round() / 10.0, "render_fps": (r.fps * 10.0).round() / 10.0, "size": format!("{}x{}", r.w, r.h)})),
            Some(Err(e)) => ("error", json!({"error": e})),
        };
        let pct = (done as f64 / total as f64 * 100.0).min(100.0);
        let eta = if done > 0 && state == "running" { el / done as f64 * (total - done.min(total)) as f64 } else { 0.0 };
        let mut v = json!({"id": self.id, "state": state, "pct": (pct * 10.0).round() / 10.0, "frames": format!("{done}/{total}"), "eta_s": eta.round(), "preset": self.opts.preset, "out": self.opts.out.to_string_lossy()});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        v
    }
}

pub struct Engine {
    pub ff: Arc<Ff>,
    pub st: RwLock<State>,
    pub events: broadcast::Sender<Event>,
    pub frames: Arc<FrameServer>,
    pub jobs: Mutex<Vec<Arc<Job>>>,
    pub log: Mutex<VecDeque<LogEntry>>,
    analysis: Mutex<HashMap<String, Value>>,
    /// When set, file access from remote clients is limited to these roots.
    pub roots: RwLock<Option<Vec<PathBuf>>>,
}

pub struct EditOutcome {
    pub log: Vec<String>,
    pub rev: u64,
    pub ops: usize,
}

struct EnvImpl<'a> {
    eng: &'a Engine,
}

impl<'a> Env for EnvImpl<'a> {
    fn probe(&self, path: &str) -> Result<Media> {
        self.eng.check_path(path)?;
        self.eng.ff.probe(path)
    }
    fn analyze(&self, p: &Project, media: &str, kind: &str, opts: &BTreeMap<String, String>) -> Result<Value> {
        let m = p.media(media).ok_or_else(|| anyhow!("unknown media {media}"))?;
        self.eng.analysis_for(m, kind, opts)
    }
}

impl Engine {
    pub fn new(ff: Ff) -> Arc<Engine> {
        let ff = Arc::new(ff);
        let p = Project::new("Untitled", 1920, 1080, 30.0);
        let active = p.main.clone();
        let (tx, _) = broadcast::channel(256);
        Arc::new(Engine {
            frames: Arc::new(FrameServer::new(ff.clone(), false)),
            ff,
            st: RwLock::new(State { p, path: None, rev: 1, active, undo: vec![], redo: vec![], dirty: false }),
            events: tx,
            jobs: Mutex::new(vec![]),
            log: Mutex::new(VecDeque::new()),
            analysis: Mutex::new(HashMap::new()),
            roots: RwLock::new(None),
        })
    }

    fn emit(&self, kind: &str, rev: u64, source: &str, text: String) {
        let _ = self.events.send(Event { kind: kind.into(), rev, source: source.into(), text });
    }

    pub fn snapshot(&self) -> (Project, u64, String, Option<PathBuf>) {
        let s = self.st.read();
        (s.p.clone(), s.rev, s.active.clone(), s.path.clone())
    }

    /// Rejects paths outside the allowed roots (only when roots are configured).
    /// Works for paths that don't exist yet (save / render targets).
    pub fn check_path(&self, path: &str) -> Result<()> {
        let roots = self.roots.read();
        let Some(roots) = roots.as_ref() else { return Ok(()) };
        let p = resolve_path(Path::new(path));
        if roots.iter().any(|r| p.starts_with(r)) {
            Ok(())
        } else {
            bail!("access to {path} is not allowed (outside the configured media folders)")
        }
    }

    pub fn edit(&self, script: &str, source: &str) -> Result<EditOutcome> {
        let trimmed = script.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("undo").filter(|r| r.trim().chars().all(|c| c.is_ascii_digit())) {
            let n = rest.trim().parse::<usize>().unwrap_or(1).max(1);
            return self.undo(n, source);
        }
        if let Some(rest) = lower.strip_prefix("redo").filter(|r| r.trim().chars().all(|c| c.is_ascii_digit())) {
            let n = rest.trim().parse::<usize>().unwrap_or(1).max(1);
            return self.redo(n, source);
        }
        let (mut work, active) = {
            let s = self.st.read();
            (s.p.clone(), s.active.clone())
        };
        let env = EnvImpl { eng: self };
        let res = ops::run(&mut work, &env, &active, script);
        match res {
            Err(e) => {
                let msg = e.to_string();
                self.push_log(LogEntry { rev: self.st.read().rev, source: source.into(), script: script.into(), result: msg.clone(), ok: false });
                bail!("{msg}\n(no changes applied)")
            }
            Ok(out) => {
                let rev;
                {
                    let mut s = self.st.write();
                    let old = std::mem::replace(&mut s.p, work);
                    s.undo.push((old, first_line(script)));
                    if s.undo.len() > MAX_UNDO {
                        s.undo.remove(0);
                    }
                    s.redo.clear();
                    s.rev += 1;
                    s.active = out.comp.clone();
                    s.dirty = true;
                    rev = s.rev;
                }
                self.autosave();
                let summary = out.log.join("\n");
                self.push_log(LogEntry { rev, source: source.into(), script: script.into(), result: summary.clone(), ok: true });
                self.emit("changed", rev, source, first_line(script));
                Ok(EditOutcome { log: out.log, rev, ops: out.ops })
            }
        }
    }

    fn push_log(&self, e: LogEntry) {
        let mut l = self.log.lock();
        l.push_back(e);
        while l.len() > 300 {
            l.pop_front();
        }
    }

    pub fn undo(&self, n: usize, source: &str) -> Result<EditOutcome> {
        let mut done = vec![];
        let rev;
        {
            let mut s = self.st.write();
            for _ in 0..n {
                let Some((prev, label)) = s.undo.pop() else { break };
                let cur = std::mem::replace(&mut s.p, prev);
                s.redo.push((cur, label.clone()));
                done.push(format!("undid: {label}"));
            }
            if done.is_empty() {
                bail!("nothing to undo");
            }
            if s.p.comp(&s.active).is_none() {
                s.active = s.p.main.clone();
            }
            s.rev += 1;
            s.dirty = true;
            rev = s.rev;
        }
        self.autosave();
        self.emit("changed", rev, source, "undo".into());
        Ok(EditOutcome { log: done, rev, ops: 1 })
    }

    pub fn redo(&self, n: usize, source: &str) -> Result<EditOutcome> {
        let mut done = vec![];
        let rev;
        {
            let mut s = self.st.write();
            for _ in 0..n {
                let Some((next, label)) = s.redo.pop() else { break };
                let cur = std::mem::replace(&mut s.p, next);
                s.undo.push((cur, label.clone()));
                done.push(format!("redid: {label}"));
            }
            if done.is_empty() {
                bail!("nothing to redo");
            }
            if s.p.comp(&s.active).is_none() {
                s.active = s.p.main.clone();
            }
            s.rev += 1;
            s.dirty = true;
            rev = s.rev;
        }
        self.autosave();
        self.emit("changed", rev, source, "redo".into());
        Ok(EditOutcome { log: done, rev, ops: 1 })
    }

    pub fn history(&self) -> Vec<String> {
        let s = self.st.read();
        s.undo.iter().rev().take(30).map(|(_, l)| l.clone()).collect()
    }

    pub fn new_project(&self, name: &str, w: u32, h: u32, fps: f64, path: Option<PathBuf>) -> Result<String> {
        let p = Project::new(name, w, h, fps);
        let active = p.main.clone();
        let rev;
        {
            let mut s = self.st.write();
            let old = std::mem::replace(&mut s.p, p);
            s.undo.push((old, "new project".into()));
            s.redo.clear();
            s.path = path;
            s.active = active;
            s.rev += 1;
            s.dirty = true;
            rev = s.rev;
        }
        self.frames.clear();
        if self.st.read().path.is_some() {
            self.save(None)?;
        }
        self.emit("changed", rev, "project", "new".into());
        Ok(format!("new project '{name}' {w}x{h}@{fps} (main comp s1)"))
    }

    pub fn open(&self, path: &str) -> Result<String> {
        self.check_path(path)?;
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        let mut p: Project = serde_json::from_str(&text).with_context(|| format!("parsing {path}"))?;
        // relink media moved alongside the project
        let dir = Path::new(path).parent().map(|d| d.to_path_buf());
        let mut missing = vec![];
        for m in p.media.iter_mut() {
            if !Path::new(&m.path).exists() {
                if let Some(d) = &dir {
                    let cand = d.join(&m.name);
                    if cand.exists() {
                        m.path = cand.to_string_lossy().to_string();
                        continue;
                    }
                }
                missing.push(m.name.clone());
            }
        }
        let active = p.main.clone();
        let name = p.name.clone();
        let rev;
        {
            let mut s = self.st.write();
            let old = std::mem::replace(&mut s.p, p);
            s.undo.push((old, "open".into()));
            s.redo.clear();
            s.path = Some(PathBuf::from(path));
            s.active = active;
            s.rev += 1;
            s.dirty = false;
            rev = s.rev;
        }
        self.frames.clear();
        self.emit("changed", rev, "project", "open".into());
        let mut msg = format!("opened '{name}' from {path}");
        if !missing.is_empty() {
            msg.push_str(&format!("\n! missing media: {}", missing.join(", ")));
        }
        Ok(msg)
    }

    pub fn save(&self, path: Option<&str>) -> Result<String> {
        let (p, cur) = {
            let s = self.st.read();
            (s.p.clone(), s.path.clone())
        };
        let target = match path {
            Some(x) => {
                let mut pb = PathBuf::from(x);
                if pb.extension().is_none() {
                    pb.set_extension("mcut");
                }
                pb
            }
            None => cur.ok_or_else(|| anyhow!("project has no file yet; save with a path (e.g. C:/Videos/MyEdit.mcut)"))?,
        };
        self.check_path(&target.to_string_lossy())?;
        if let Some(d) = target.parent() {
            if !d.as_os_str().is_empty() {
                std::fs::create_dir_all(d)?;
            }
        }
        let json = serde_json::to_string_pretty(&p)?;
        let tmp = target.with_extension("mcut.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &target)?;
        {
            let mut s = self.st.write();
            s.path = Some(target.clone());
            s.dirty = false;
        }
        Ok(format!("saved {}", target.display()))
    }

    fn autosave(&self) {
        let (p, path) = {
            let s = self.st.read();
            (s.p.clone(), s.path.clone())
        };
        let target = match path {
            Some(pb) => pb,
            None => crate::config::autosave_dir().join("untitled.mcut"),
        };
        if let Ok(json) = serde_json::to_string_pretty(&p) {
            let tmp = target.with_extension("mcut.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &target);
                if self.st.read().path.is_some() {
                    self.st.write().dirty = false;
                }
            }
        }
    }

    /// Imports files, folders (media inside) and simple globs (*.mp4).
    pub fn import(&self, paths: &[String], source: &str) -> Result<EditOutcome> {
        let mut files = vec![];
        for p in paths {
            let pb = PathBuf::from(p);
            if pb.is_dir() {
                let mut v: Vec<PathBuf> = std::fs::read_dir(&pb)?.flatten().map(|e| e.path()).filter(|x| is_media(x)).collect();
                v.sort();
                files.extend(v);
            } else if p.contains('*') {
                let (dir, pat) = match pb.parent() {
                    Some(d) if !d.as_os_str().is_empty() => (d.to_path_buf(), pb.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()),
                    _ => (PathBuf::from("."), p.clone()),
                };
                let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|x| glob_match(&pat, &x.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())).collect();
                v.sort();
                files.extend(v);
            } else {
                files.push(pb);
            }
        }
        if files.is_empty() {
            bail!("no media files found");
        }
        let script = format!("import {}", files.iter().map(|f| crate::ops::lex::quote(&f.to_string_lossy())).collect::<Vec<_>>().join(" "));
        self.edit(&script, source)
    }

    fn analysis_key(m: &Media, kind: &str, opts: &BTreeMap<String, String>) -> String {
        let o: Vec<String> = opts.iter().filter(|(k, _)| *k != "src").map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}|{}|{}|{}|{}", m.path, m.size, m.mtime, kind, o.join(","))
    }

    /// Cached analysis (memory + disk).
    pub fn analysis_for(&self, m: &Media, kind: &str, opts: &BTreeMap<String, String>) -> Result<Value> {
        let kind = match kind {
            "silences" => "silence",
            "scene" | "shots" => "scenes",
            "tempo" | "bpm" => "beats",
            "transcribe" | "speech" | "stt" => "transcript",
            k => k,
        };
        let key = Self::analysis_key(m, kind, opts);
        if let Some(v) = self.analysis.lock().get(&key) {
            return Ok(v.clone());
        }
        let file = crate::config::cache_dir().join(format!("an_{:016x}.json", crate::render::eval::hash_str(&key)));
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                self.analysis.lock().insert(key, v.clone());
                return Ok(v);
            }
        }
        let v = analyze::run(&self.ff, &m.path, m.dur, kind, opts)?;
        let _ = std::fs::write(&file, serde_json::to_string(&v).unwrap_or_default());
        self.analysis.lock().insert(key, v.clone());
        Ok(v)
    }

    /// Runs analysis for a media id or item id (item maps to its media) and records it in the project.
    pub fn analyze(&self, target: &str, kind: &str, opts: &BTreeMap<String, String>) -> Result<(String, Value)> {
        let (m, kind_s) = {
            let s = self.st.read();
            let mid = if let Some(m) = s.p.media(target) {
                m.id.clone()
            } else if let Some(it) = s.p.item(target) {
                it.src.clone().filter(|x| s.p.media(x).is_some()).ok_or_else(|| anyhow!("{target} has no media"))?
            } else {
                bail!("unknown media or item '{target}'")
            };
            (s.p.media(&mid).unwrap().clone(), kind.to_string())
        };
        let v = self.analysis_for(&m, &kind_s, opts)?;
        {
            let mut s = self.st.write();
            let key = format!("{}:{}", m.id, kind_s);
            let small = if kind_s == "transcript" { json!({"segments": v["segments"].as_array().map(|a| a.len()).unwrap_or(0)}) } else { json!(true) };
            s.p.analysis.insert(key, small);
        }
        Ok((m.id, v))
    }

    pub fn frame(&self, comp: Option<&str>, t: f64, width: u32, draft: bool) -> Result<resvg::tiny_skia::Pixmap> {
        let (p, _, active, _) = self.snapshot();
        let cid = comp.map(|c| c.to_string()).unwrap_or(active);
        let c = p.comp(&cid).ok_or_else(|| anyhow!("no comp {cid}"))?;
        let scale = (width.clamp(16, 7680) as f32 / c.w as f32).min(4.0);
        let rc = RenderCtx { p: &p, frames: &self.frames, draft };
        render_frame(&rc, &c.id.clone(), t, scale)
    }

    pub fn contact_sheet(&self, comp: Option<&str>, times: &[f64], thumb_w: u32, cols: usize) -> Result<resvg::tiny_skia::Pixmap> {
        let (p, _, active, _) = self.snapshot();
        let cid = comp.map(|c| c.to_string()).unwrap_or(active);
        let id = p.comp(&cid).ok_or_else(|| anyhow!("no comp {cid}"))?.id.clone();
        let rc = RenderCtx { p: &p, frames: &self.frames, draft: true };
        encode::contact_sheet(&rc, &id, times, thumb_w, cols)
    }

    pub fn start_render(self: &Arc<Self>, mut opts: RenderOpts) -> Result<Arc<Job>> {
        let (p, _, active, path) = self.snapshot();
        if opts.comp.is_empty() {
            opts.comp = active;
        }
        let comp = p.comp(&opts.comp).ok_or_else(|| anyhow!("no comp {}", opts.comp))?;
        opts.comp = comp.id.clone();
        if comp.duration() <= 0.0 {
            bail!("the timeline is empty; add media before rendering");
        }
        if opts.out.as_os_str().is_empty() {
            let pr = encode::resolve_preset(&opts.preset, &self.ff, &opts.hw)?;
            opts.out = encode::default_out(path.as_deref(), &p.name, pr.ext);
        } else {
            self.check_path(&opts.out.to_string_lossy())?;
        }
        let id = format!("r{}", self.jobs.lock().len() + 1);
        let job = Arc::new(Job { id: id.clone(), opts: opts.clone(), prog: Progress::default(), started: Instant::now(), result: Mutex::new(None) });
        self.jobs.lock().push(job.clone());
        let eng = self.clone();
        let j2 = job.clone();
        std::thread::spawn(move || {
            let r = encode::render_file(&eng.ff, &p, &j2.opts, &j2.prog);
            let txt = match &r {
                Ok(x) => format!("render {} done: {}", j2.id, x.out),
                Err(e) => format!("render {} failed: {e:#}", j2.id),
            };
            *j2.result.lock() = Some(r.map_err(|e| format!("{e:#}")));
            let rev = eng.st.read().rev;
            eng.emit("job", rev, "render", txt);
        });
        self.emit("job", self.st.read().rev, "render", format!("render {id} started"));
        Ok(job)
    }

    pub fn job(&self, id: &str) -> Option<Arc<Job>> {
        self.jobs.lock().iter().find(|j| j.id == id).cloned()
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let n = s.lines().filter(|l| !l.trim().is_empty()).count();
    let mut out: String = l.chars().take(80).collect();
    if n > 1 {
        out.push_str(&format!(" (+{} lines)", n - 1));
    }
    out
}

pub fn is_media(p: &Path) -> bool {
    let e = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(
        e.as_str(),
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "mts" | "m2ts" | "mxf" | "wmv" | "flv" | "3gp" | "mpg" | "mpeg" | "ts" | "gif" | "mp3" | "wav" | "aac" | "m4a" | "flac" | "ogg" | "opus" | "wma" | "aif" | "aiff" | "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "svg" | "heic" | "avif" | "exr" | "tga"
    )
}

pub fn glob_match(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.to_ascii_lowercase().chars().collect();
    let n: Vec<char> = name.to_ascii_lowercase().chars().collect();
    fn rec(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => rec(&p[1..], n) || (!n.is_empty() && rec(p, &n[1..])),
            (Some('?'), Some(_)) => rec(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => rec(&p[1..], &n[1..]),
            _ => false,
        }
    }
    rec(&p, &n)
}

/// Canonical form of a path that may not exist yet: the longest existing prefix
/// is canonicalized and the rest is applied lexically, so `allowed/../x` can't
/// slip past a `starts_with` check.
pub fn resolve_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let comps: Vec<Component> = abs.components().collect();
    for i in (1..=comps.len()).rev() {
        let prefix: PathBuf = comps[..i].iter().collect();
        if let Ok(mut out) = std::fs::canonicalize(&prefix) {
            for c in &comps[i..] {
                match c {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::Normal(n) => out.push(n),
                    _ => {}
                }
            }
            return out;
        }
    }
    abs
}

#[cfg(test)]
mod tests {
    use super::resolve_path;

    #[test]
    fn resolve_path_handles_missing_parts_and_dotdot() {
        let base = std::env::temp_dir().join("moodcut-resolve-test");
        std::fs::create_dir_all(base.join("allowed")).unwrap();
        let root = std::fs::canonicalize(base.join("allowed")).unwrap();
        assert!(resolve_path(&base.join("allowed/new/dir/out.mp4")).starts_with(&root));
        assert!(!resolve_path(&base.join("allowed/missing/../../secret.txt")).starts_with(&root));
        assert!(!resolve_path(&base.join("allowed/../secret.txt")).starts_with(&root));
    }
}
