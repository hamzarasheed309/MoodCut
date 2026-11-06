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
use anyhow::{anyhow, bail, Result};
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
    /// Where the project autosaves while it has no file of its own (a new file for
    /// each untitled project, so starting another never overwrites unsaved work).
    pub autosave: PathBuf,
    /// Revision at which this project was opened or created (the editor restores
    /// the saved playhead when it changes).
    pub opened: u64,
    /// The playhead saved in the file that was opened.
    pub restore: Option<f64>,
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
            // e.g. "rendering", or the GPU encoder failed and it's encoding on the CPU
            None => ("running", json!({"stage": *self.prog.stage.lock(), "cancelling": self.prog.cancel.load(Ordering::Relaxed)})),
            Some(Ok(r)) => ("done", json!({"out": r.out, "bytes": r.bytes, "secs": (r.secs * 10.0).round() / 10.0, "render_fps": (r.fps * 10.0).round() / 10.0, "size": format!("{}x{}", r.w, r.h)})),
            Some(Err(e)) => ("error", json!({"error": e})),
        };
        let pct = (done as f64 / total as f64 * 100.0).min(100.0);
        let eta = if done > 0 && state == "running" { el / done as f64 * (total - done.min(total)) as f64 } else { 0.0 };
        if let Some(Ok(r)) = &*res {
            if r.frames == 0 {
                // a sidecar export (captions, chapters, XML, EDL): no frames
                return json!({"id": self.id, "state": state, "preset": self.opts.preset, "out": r.out, "bytes": r.bytes, "note": *self.prog.stage.lock()});
            }
        }
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
    /// Preview proxies of heavy footage (built in the background).
    pub proxies: Arc<crate::render::proxy::Proxies>,
    pub jobs: Mutex<Vec<Arc<Job>>>,
    pub log: Mutex<VecDeque<LogEntry>>,
    analysis: Mutex<HashMap<String, Value>>,
    /// When set, file access from remote clients is limited to these roots.
    pub roots: RwLock<Option<Vec<PathBuf>>>,
    /// Probe results by path|size|mtime (also cached on disk).
    pub(crate) probes: Mutex<HashMap<String, Media>>,
    /// Library thumbnails / filmstrip frames.
    pub(crate) thumbs: Mutex<HashMap<String, Arc<resvg::tiny_skia::Pixmap>>>,
    /// Audio peak data for waveforms, by path|size|mtime.
    pub(crate) peaks: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    /// The editor window's selection and playhead, sent over its WebSocket.
    ui: RwLock<ops::Ui>,
    /// Editor windows connected right now (the UI state only counts while one is).
    pub gui_clients: std::sync::atomic::AtomicUsize,
    /// One project-file write at a time (autosaves come from every thread that edits).
    save_lock: Mutex<()>,
    /// The newest revision on disk, so a slow autosave never overwrites a newer one.
    saved_rev: Mutex<u64>,
    /// The editor's last playhead (kept after it disconnects; saved with the project).
    playhead: Mutex<Option<f64>>,
    /// Why the last autosave failed, so the editor hears about each problem once.
    autosave_err: Mutex<Option<String>>,
    /// A problem from startup for the editor to show, e.g. the project it was asked
    /// to open is damaged (cleared once a project opens).
    pub notice: Mutex<Option<String>>,
}

/// The message of a caught panic.
pub fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into())
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
        self.eng.probe_cached(path)
    }
    fn analyze(&self, p: &Project, media: &str, kind: &str, opts: &BTreeMap<String, String>) -> Result<Value> {
        let m = p.media(media).ok_or_else(|| anyhow!("unknown media {media}"))?;
        self.eng.analysis_for(m, kind, opts)
    }
    fn ui(&self) -> ops::Ui {
        self.eng.ui()
    }
    fn sfx(&self, kind: &str, dur: f64, pitch: f64, seed: u64) -> Result<Media> {
        let k = crate::sfx::kind(kind).ok_or_else(|| anyhow!("unknown sound effect '{kind}'"))?;
        // next to the project's graphics (or in the data folder until it is saved)
        let gdir = crate::gfx::graphics_dir(self.eng.st.read().path.as_deref());
        let dir = gdir.parent().map(|d| d.join("sfx")).unwrap_or_else(|| gdir.join("sfx"));
        let path = dir.join(crate::sfx::file_name(k, dur, pitch, seed));
        if !path.exists() {
            crate::sfx::write_wav(&path, &crate::sfx::synth(k, dur, pitch, seed))?;
        }
        self.eng.probe_cached(&path.to_string_lossy())
    }
}

impl Engine {
    pub fn new(ff: Ff) -> Arc<Engine> {
        let ff = Arc::new(ff);
        let p = Project::new("Untitled", 1920, 1080, 30.0);
        let active = p.main.clone();
        let (tx, _) = broadcast::channel(256);
        let proxies = crate::render::proxy::Proxies::new(ff.clone());
        let eng = Arc::new(Engine {
            frames: Arc::new(FrameServer::for_preview(ff.clone(), proxies.clone())),
            proxies,
            ff,
            st: RwLock::new(State { p, path: None, rev: 1, active, undo: vec![], redo: vec![], dirty: false, autosave: crate::projfile::new_untitled(), opened: 1, restore: None }),
            events: tx,
            jobs: Mutex::new(vec![]),
            log: Mutex::new(VecDeque::new()),
            analysis: Mutex::new(HashMap::new()),
            roots: RwLock::new(None),
            probes: Mutex::new(HashMap::new()),
            thumbs: Mutex::new(HashMap::new()),
            peaks: Mutex::new(HashMap::new()),
            ui: RwLock::new(ops::Ui::default()),
            gui_clients: std::sync::atomic::AtomicUsize::new(0),
            save_lock: Mutex::new(()),
            saved_rev: Mutex::new(1),
            playhead: Mutex::new(None),
            autosave_err: Mutex::new(None),
            notice: Mutex::new(None),
        });
        // a finished proxy changes what previews show: tell the editor
        let weak = Arc::downgrade(&eng);
        eng.proxies.set_on_change(Box::new(move |msg| {
            if let Some(e) = weak.upgrade() {
                let rev = e.st.read().rev;
                e.emit("proxy", rev, "proxy", msg);
            }
        }));
        eng
    }

    /// Changes whenever a preview of the same revision could look different
    /// (a proxy became ready, proxies were switched on or off): for frame caches.
    pub fn view_key(&self) -> (u64, u64) {
        (self.st.read().rev, self.proxies.generation())
    }

    /// One media item by id (cheaper than a whole snapshot).
    pub fn media(&self, id: &str) -> Option<Media> {
        self.st.read().p.media(id).cloned()
    }

    pub(crate) fn emit(&self, kind: &str, rev: u64, source: &str, text: String) {
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
        // edits run on a copy, so even a bug in a command (a panic) leaves the project as it was
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ops::run(&mut work, &env, &active, script)));
        let res = match res {
            Ok(Ok(out)) => self.commit(work, Some(out.comp.clone()), script, source, &out.log).map(|rev| (out, rev)).map_err(|e| format!("{e:#}")),
            Ok(Err(e)) => Err(e.to_string()),
            Err(panic) => Err(format!("MoodCut hit an internal error running this edit ({}); the crash log has the details", panic_message(&*panic))),
        };
        match res {
            Err(msg) => {
                self.push_log(LogEntry { rev: self.st.read().rev, source: source.into(), script: script.into(), result: msg.clone(), ok: false });
                bail!("{msg}\n(no changes applied)")
            }
            Ok((out, rev)) => Ok(EditOutcome { log: out.log, rev, ops: out.ops }),
        }
    }

    /// Replaces the project with an edited copy as one undo step, then
    /// autosaves, logs and notifies listeners. Returns the new revision.
    /// Refuses (changing nothing) a copy whose numbers can't be saved.
    pub(crate) fn commit(&self, work: Project, active: Option<String>, label: &str, source: &str, log: &[String]) -> Result<u64> {
        // JSON can't hold NaN or infinity: such a value would make the saved project unreadable
        crate::projfile::check_numbers(&work).map_err(|e| anyhow!("this edit would store {e:#}, so nothing was changed (check the numbers in it)"))?;
        let rev;
        {
            let mut s = self.st.write();
            let old = std::mem::replace(&mut s.p, work);
            s.undo.push((old, first_line(label)));
            if s.undo.len() > MAX_UNDO {
                s.undo.remove(0);
            }
            s.redo.clear();
            s.rev += 1;
            if let Some(a) = active {
                s.active = a;
            }
            if s.p.comp(&s.active).is_none() {
                s.active = s.p.main.clone();
            }
            s.dirty = true;
            rev = s.rev;
        }
        self.autosave();
        self.push_log(LogEntry { rev, source: source.into(), script: label.into(), result: log.join("\n"), ok: true });
        self.emit("changed", rev, source, first_line(label));
        Ok(rev)
    }

    /// Runs script lines against a working copy (for multi-step transactions).
    pub(crate) fn run_ops(&self, work: &mut Project, active: &str, script: &str) -> Result<ops::exec::ExecOut> {
        let env = EnvImpl { eng: self };
        ops::run(work, &env, active, script).map_err(|e| anyhow!("{e}"))
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
        let sources;
        {
            let mut s = self.st.write();
            let before = crate::gfx::sources(&s.p);
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
            sources = (before, crate::gfx::sources(&s.p));
        }
        // a graphic's .html file holds the source the project has now
        crate::gfx::mirror_sources(&sources.0, &sources.1);
        self.autosave();
        self.emit("changed", rev, source, "undo".into());
        Ok(EditOutcome { log: done, rev, ops: 1 })
    }

    pub fn redo(&self, n: usize, source: &str) -> Result<EditOutcome> {
        let mut done = vec![];
        let rev;
        let sources;
        {
            let mut s = self.st.write();
            let before = crate::gfx::sources(&s.p);
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
            sources = (before, crate::gfx::sources(&s.p));
        }
        crate::gfx::mirror_sources(&sources.0, &sources.1);
        self.autosave();
        self.emit("changed", rev, source, "redo".into());
        Ok(EditOutcome { log: done, rev, ops: 1 })
    }

    pub fn history(&self) -> Vec<String> {
        let s = self.st.read();
        s.undo.iter().rev().take(30).map(|(_, l)| l.clone()).collect()
    }

    /// Undo labels (newest first) and redo labels (next first), for the History panel.
    pub fn history_both(&self, n: usize) -> (Vec<String>, Vec<String>) {
        let s = self.st.read();
        (s.undo.iter().rev().take(n).map(|(_, l)| l.clone()).collect(), s.redo.iter().rev().take(n).map(|(_, l)| l.clone()).collect())
    }

    /// A render range: `a-b` seconds, or `work` for the comp's work area.
    pub fn render_range(&self, comp: &str, r: &str) -> Result<(f64, f64)> {
        let r = r.trim();
        if matches!(r.to_ascii_lowercase().as_str(), "work" | "workarea" | "inout") {
            let s = self.st.read();
            let cid = if comp.is_empty() { s.active.clone() } else { comp.to_string() };
            let c = s.p.comp(&cid).ok_or_else(|| anyhow!("no comp {cid}"))?;
            return c.work.ok_or_else(|| anyhow!("{} has no work area (set one with `workarea <in> <out>`)", c.id));
        }
        let (x, y) = r.split_once('-').ok_or_else(|| anyhow!("range must be a-b (seconds) or work"))?;
        Ok((x.trim().parse()?, y.trim().parse()?))
    }

    /// What the person is doing in the editor (empty when no editor window is open).
    pub fn ui(&self) -> ops::Ui {
        if self.gui_clients.load(Ordering::Relaxed) == 0 {
            return ops::Ui::default();
        }
        self.ui.read().clone()
    }

    pub fn set_ui(&self, ui: ops::Ui) {
        if let Some(t) = ui.playhead.filter(|t| t.is_finite()) {
            *self.playhead.lock() = Some(t);
        }
        *self.ui.write() = ui;
    }

    /// A scope (waveform, parade, vectorscope, histogram, or `all` with the frame)
    /// of the frame at `t`, and its exposure / color summary.
    pub fn scope(&self, comp: Option<&str>, t: f64, kind: &str, w: u32, h: u32) -> Result<(resvg::tiny_skia::Pixmap, String)> {
        use crate::render::scopes;
        let frame = self.frame(comp, t, 640, true)?;
        let summary = scopes::summary(&frame);
        if kind != "all" {
            return Ok((scopes::render(&frame, kind, w, h)?, summary));
        }
        let (cw, ch) = (w / 2, h / 2);
        let mut sheet = resvg::tiny_skia::Pixmap::new(cw * 2, ch * 2).ok_or_else(|| anyhow!("bad scope size"))?;
        let k = (cw as f32 / frame.width() as f32).min(ch as f32 / frame.height() as f32);
        let mut cell = resvg::tiny_skia::Pixmap::new(cw, ch).unwrap();
        cell.fill(resvg::tiny_skia::Color::from_rgba8(12, 12, 16, 255));
        let (fw, fh) = (frame.width() as f32 * k, frame.height() as f32 * k);
        let m = resvg::tiny_skia::Transform::from_translate((cw as f32 - fw) / 2.0, (ch as f32 - fh) / 2.0).pre_scale(k, k);
        cell.draw_pixmap(0, 0, frame.as_ref(), &resvg::tiny_skia::PixmapPaint { quality: resvg::tiny_skia::FilterQuality::Bilinear, ..Default::default() }, m, None);
        let cells = [cell, scopes::render(&frame, "waveform", cw, ch)?, scopes::render(&frame, "vectorscope", cw, ch)?, scopes::render(&frame, "parade", cw, ch)?];
        for (i, c) in cells.iter().enumerate() {
            crate::render::pixel::blit(&mut sheet, c, (i as u32 % 2 * cw) as i32, (i as u32 / 2 * ch) as i32);
        }
        Ok((sheet, summary))
    }

    pub fn new_project(&self, name: &str, w: u32, h: u32, fps: f64, path: Option<PathBuf>) -> Result<String> {
        if !(fps.is_finite() && fps > 0.0 && fps <= 1000.0) {
            bail!("fps must be a number above 0 (e.g. 30)");
        }
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            bail!("the size must be between 1x1 and 16384x16384");
        }
        let p = Project::new(name, w, h, fps);
        let active = p.main.clone();
        let rev = {
            let mut s = self.st.write();
            s.p = p;
            // a new project starts a new history: undoing back into the previous project
            // would autosave that project over this one
            s.undo.clear();
            s.redo.clear();
            s.path = path;
            s.autosave = crate::projfile::new_untitled();
            s.active = active;
            s.rev += 1;
            s.dirty = false; // nothing to lose yet
            s.opened = s.rev;
            s.restore = None;
            s.rev
        };
        *self.playhead.lock() = None;
        *self.notice.lock() = None;
        self.frames.clear();
        let mut msg = format!("new project '{name}' {w}x{h}@{fps} (main comp s1)");
        if self.st.read().path.is_some() {
            if let Err(e) = self.save(None) {
                let bad = self.st.write().path.take();
                let shown = bad.map(|b| b.display().to_string()).unwrap_or_default();
                msg.push_str(&format!("\n! couldn't save it to {shown}: {e:#}. It autosaves as an untitled project until you save it somewhere else"));
            }
        }
        self.emit("changed", rev, "project", "new".into());
        Ok(msg)
    }

    pub fn open(&self, path: &str) -> Result<String> {
        self.check_path(path)?;
        let file = PathBuf::from(path);
        let loaded = crate::projfile::load(&file)?;
        Ok(self.install(loaded, Some(file), None))
    }

    /// Brings back an untitled project autosaved earlier (after a crash, or one that was
    /// left without saving). It stays untitled, autosaving into the same file, until saved.
    pub fn recover(&self, file: &str) -> Result<String> {
        let pb = PathBuf::from(file);
        if !crate::projfile::is_untitled_autosave(&pb) {
            bail!("only autosaved untitled projects can be recovered (project action=recover lists them); open project files with action=open");
        }
        let loaded = crate::projfile::load(&pb)?;
        Ok(self.install(loaded, None, Some(pb)))
    }

    /// Untitled projects autosaved earlier (newest first), not counting the open one.
    pub fn unsaved(&self) -> Vec<crate::projfile::Unsaved> {
        let cur = self.st.read().autosave.clone();
        crate::projfile::unsaved(&cur)
    }

    /// Makes a project read from disk the open one, with a fresh history. Returns
    /// what to tell the person or AI: what was repaired, relinked or is missing.
    fn install(&self, l: crate::projfile::Loaded, path: Option<PathBuf>, autosave: Option<PathBuf>) -> String {
        let crate::projfile::Loaded { project: p, notes, missing, session, changed } = l;
        let name = p.name.clone();
        let active = session.comp.as_deref().and_then(|c| p.comp(c)).map(|c| c.id.clone()).unwrap_or_else(|| p.main.clone());
        let untitled = path.is_none();
        let rev = {
            let mut s = self.st.write();
            s.p = p;
            // undoing into the previous project would autosave it over this one
            s.undo.clear();
            s.redo.clear();
            s.path = path.clone();
            s.autosave = autosave.unwrap_or_else(crate::projfile::new_untitled);
            s.active = active;
            s.rev += 1;
            // repaired or relinked: the file is behind until the next save
            s.dirty = changed || untitled;
            s.opened = s.rev;
            s.restore = session.playhead;
            s.rev
        };
        {
            let mut g = self.saved_rev.lock();
            *g = (*g).max(rev);
        }
        *self.playhead.lock() = session.playhead;
        *self.notice.lock() = None;
        *self.autosave_err.lock() = None;
        self.frames.clear();
        let media = self.st.read().p.media.clone();
        self.proxies.request(&media, false);
        self.emit("changed", rev, "project", if untitled { "recover" } else { "open" }.into());
        let mut msg = match &path {
            Some(f) => format!("opened '{name}' from {}", f.display()),
            None => format!("recovered '{name}': it's still untitled, so save it to keep it"),
        };
        for n in &notes {
            msg.push_str(&format!("\n! {n}"));
        }
        if !missing.is_empty() {
            let shown: Vec<&str> = missing.iter().take(12).map(|s| s.as_str()).collect();
            msg.push_str(&format!(
                "\n! missing media ({}): {}{}. Relink them with `relink <id> <new path>`; until then previews show them in red (offline) and exports stop",
                missing.len(),
                shown.join(", "),
                if missing.len() > 12 { ", ..." } else { "" }
            ));
        }
        msg
    }

    /// Where the editor is, saved with the project.
    fn session(&self, active: &str) -> crate::projfile::Session {
        crate::projfile::Session { comp: Some(active.to_string()), playhead: *self.playhead.lock() }
    }

    /// Saves to the project's file, or to `path` (Save As; `.mcut` is added when missing).
    pub fn save(&self, path: Option<&str>) -> Result<String> {
        let cur = self.st.read().path.clone();
        let target = match path.map(str::trim).filter(|x| !x.is_empty()) {
            Some(x) => {
                let pb = PathBuf::from(x);
                if pb.is_dir() {
                    bail!("{} is a folder: give a file name, e.g. {}", pb.display(), pb.join("MyEdit.mcut").display());
                }
                match pb.extension() {
                    Some(e) if e.eq_ignore_ascii_case("mcut") => pb,
                    _ => PathBuf::from(format!("{}.mcut", pb.display())),
                }
            }
            None => cur.clone().ok_or_else(|| anyhow!("the project has no file yet: save it with a path (e.g. C:/Videos/MyEdit.mcut)"))?,
        };
        self.check_path(&target.to_string_lossy())?;
        if crate::projfile::is_untitled_autosave(&target) {
            bail!("that's MoodCut's autosave folder: save the project somewhere of your own");
        }
        let _g = self.save_lock.lock();
        let (p, rev, active, untitled) = {
            let s = self.st.read();
            (s.p.clone(), s.rev, s.active.clone(), s.autosave.clone())
        };
        let text = crate::projfile::to_json(&p, Some(&target), &self.session(&active))?;
        crate::projfile::backup(&target);
        crate::projfile::write_atomic(&target, &text).map_err(|e| anyhow!("couldn't save the project: {e:#}"))?;
        let was_untitled = {
            let mut s = self.st.write();
            let was = s.path.is_none();
            s.path = Some(target.clone());
            if s.rev == rev {
                s.dirty = false;
            }
            was
        };
        {
            let mut g = self.saved_rev.lock();
            *g = (*g).max(rev);
        }
        // the work lives in its own file now: its untitled autosave isn't needed any more
        if was_untitled {
            let _ = std::fs::remove_file(&untitled);
        }
        *self.autosave_err.lock() = None;
        self.emit("saved", rev, "project", target.display().to_string());
        Ok(format!("saved {}", target.display()))
    }

    /// Writes the project to its file (or to its untitled autosave) after a change.
    /// A failure is reported to the editor once; the project then stays marked unsaved.
    fn autosave(&self) {
        let _g = self.save_lock.lock();
        let (p, path, untitled, rev, active) = {
            let s = self.st.read();
            (s.p.clone(), s.path.clone(), s.autosave.clone(), s.rev, s.active.clone())
        };
        if rev <= *self.saved_rev.lock() {
            return; // a newer revision is already on disk
        }
        let target = path.clone().unwrap_or_else(|| untitled.clone());
        let r = crate::projfile::to_json(&p, path.as_deref(), &self.session(&active)).and_then(|text| {
            if path.is_some() {
                crate::projfile::backup(&target);
            } else if !target.exists() {
                crate::projfile::prune_untitled(&target);
            }
            crate::projfile::write_atomic(&target, &text)
        });
        match r {
            Ok(()) => {
                {
                    let mut g = self.saved_rev.lock();
                    *g = (*g).max(rev);
                }
                if path.is_some() {
                    let mut s = self.st.write();
                    if s.rev == rev {
                        s.dirty = false;
                    }
                }
                if self.autosave_err.lock().take().is_some() {
                    self.emit("autosave", rev, "project", format!("autosave works again ({})", target.display()));
                }
            }
            Err(e) => {
                let msg = format!("autosave failed: {e:#}. Your changes are safe in MoodCut for now: save the project somewhere else (Save As) to keep them");
                let mut last = self.autosave_err.lock();
                if last.as_deref() != Some(msg.as_str()) {
                    *last = Some(msg.clone());
                    drop(last);
                    eprintln!("moodcut: {msg}");
                    self.emit("autosave", rev, "project", msg);
                }
            }
        }
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

    /// A finished analysis from the memory/disk cache, without computing it.
    pub fn cached_analysis(&self, m: &Media, kind: &str) -> Option<Value> {
        let key = Self::analysis_key(m, kind, &BTreeMap::new());
        if let Some(v) = self.analysis.lock().get(&key) {
            return Some(v.clone());
        }
        let file = crate::config::cache_dir().join(format!("an_{:016x}.json", crate::render::eval::hash_str(&key)));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
        self.analysis.lock().insert(key, v.clone());
        Some(v)
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
        if let Some((kind, ext)) = crate::export::sidecar(&opts.preset) {
            return self.export_sidecar(&p, opts, path.as_deref(), kind, ext);
        }
        // everything the export reads must be there: say so now, not halfway through
        let t1 = if opts.t1 > opts.t0 { opts.t1 } else { comp.duration() };
        let offline: Vec<String> = media_used(&p, &opts.comp, opts.t0.max(0.0), t1)
            .iter()
            .filter_map(|id| p.media(id))
            .filter(|m| !Path::new(&m.path).exists())
            .map(|m| format!("{} {} ({})", m.id, m.name, m.path))
            .collect();
        if !offline.is_empty() {
            let shown: Vec<&str> = offline.iter().take(8).map(|s| s.as_str()).collect();
            bail!(
                "can't export: {} media file{} used in this range {} missing: {}{}. Relink {} with `relink <id> <new path>` or remove {} clips",
                offline.len(),
                if offline.len() == 1 { "" } else { "s" },
                if offline.len() == 1 { "is" } else { "are" },
                shown.join("; "),
                if offline.len() > 8 { "; ..." } else { "" },
                if offline.len() == 1 { "it" } else { "them" },
                if offline.len() == 1 { "its" } else { "their" }
            );
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
        let spawned = std::thread::Builder::new().name(format!("render-{id}")).spawn(move || {
            // a bug while rendering must end the job with an error, never leave it "running"
            let run = |o: &RenderOpts| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| encode::render_file(&eng.ff, &p, o, &j2.prog))) {
                Ok(r) => r,
                Err(panic) => Err(anyhow!("MoodCut hit an internal error while rendering ({}); the export was stopped and the crash log has the details", panic_message(&*panic))),
            };
            let mut r = run(&j2.opts);
            // the GPU's encoder failed to start or quit: the CPU always works
            let gpu_failed = matches!(&r, Err(e) if format!("{e:#}").contains("GPU encoder failed"));
            if gpu_failed && encode::hw_allowed(&j2.opts.hw) && !j2.prog.cancel.load(Ordering::Relaxed) {
                *j2.prog.stage.lock() = "the GPU encoder failed: encoding on the CPU".into();
                j2.prog.done.store(0, Ordering::Relaxed);
                eng.emit("job", eng.st.read().rev, "render", format!("render {}: the GPU encoder failed, retrying on the CPU", j2.id));
                let mut cpu = j2.opts.clone();
                cpu.hw = "off".into();
                r = run(&cpu);
            }
            let txt = match &r {
                Ok(x) => format!("render {} done: {}", j2.id, x.out),
                Err(e) => format!("render {} failed: {e:#}", j2.id),
            };
            *j2.result.lock() = Some(r.map_err(|e| format!("{e:#}")));
            let rev = eng.st.read().rev;
            eng.emit("job", rev, "render", txt);
        });
        if let Err(e) = spawned {
            *job.result.lock() = Some(Err(format!("couldn't start the render: {e}")));
            bail!("couldn't start the render: {e}");
        }
        self.emit("job", self.st.read().rev, "render", format!("render {id} started"));
        Ok(job)
    }

    /// Stops a running export (its unfinished file is deleted).
    pub fn cancel_render(&self, id: &str) -> Result<String> {
        let j = self.job(id).ok_or_else(|| anyhow!("no render {id}"))?;
        if j.result.lock().is_some() {
            return Ok(format!("render {id} already finished"));
        }
        j.prog.cancel.store(true, Ordering::Relaxed);
        Ok(format!("cancelling render {id}"))
    }

    /// Caption files, chapters and interchange timelines: written at once, no encode.
    fn export_sidecar(&self, p: &Project, mut opts: RenderOpts, path: Option<&Path>, kind: &str, ext: &str) -> Result<Arc<Job>> {
        if opts.out.as_os_str().is_empty() {
            opts.out = encode::default_out(path, &p.name, ext);
        } else {
            self.check_path(&opts.out.to_string_lossy())?;
        }
        let t0 = Instant::now();
        let summary = crate::export::write(p, &opts.comp, kind, &opts.out)?;
        let c = p.comp(&opts.comp).unwrap();
        let out = opts.out.to_string_lossy().to_string();
        let bytes = std::fs::metadata(&opts.out).map(|m| m.len()).unwrap_or(0);
        let id = format!("r{}", self.jobs.lock().len() + 1);
        let r = RenderResult { out: out.clone(), frames: 0, secs: t0.elapsed().as_secs_f64(), bytes, fps: 0.0, w: c.w, h: c.h };
        let prog = Progress::default();
        prog.done.store(1, Ordering::Relaxed);
        prog.total.store(1, Ordering::Relaxed);
        *prog.stage.lock() = summary;
        let job = Arc::new(Job { id: id.clone(), opts, prog, started: t0, result: Mutex::new(Some(Ok(r))) });
        self.jobs.lock().push(job.clone());
        self.emit("job", self.st.read().rev, "render", format!("export {id} done: {out}"));
        Ok(job)
    }

    pub fn job(&self, id: &str) -> Option<Arc<Job>> {
        self.jobs.lock().iter().find(|j| j.id == id).cloned()
    }
}

/// Media an export of `comp` from `t0` to `t1` reads, by id: enabled clips overlapping
/// the range (not on muted audio tracks, nor silent ones on hidden video tracks), and
/// everything in the comps they nest.
fn media_used(p: &Project, comp: &str, t0: f64, t1: f64) -> std::collections::BTreeSet<String> {
    use crate::model::project::{ItemKind, TrackKind};
    fn walk(p: &Project, comp: &str, range: Option<(f64, f64)>, depth: usize, out: &mut std::collections::BTreeSet<String>) {
        let Some(c) = p.comp(comp) else { return };
        if depth > 16 {
            return;
        }
        for t in c.tracks.iter().filter(|t| !(t.kind == TrackKind::Audio && t.muted)) {
            for it in t.items.iter().filter(|it| !it.off) {
                if range.is_some_and(|(a, b)| it.end() <= a || it.start >= b) {
                    continue;
                }
                match (it.kind, it.src.as_deref()) {
                    (ItemKind::Clip, Some(s)) => {
                        // a hidden video track still plays its clips' sound
                        if let Some(m) = p.media(s).filter(|m| !(t.kind == TrackKind::Video && t.hidden && !m.has_audio)) {
                            out.insert(m.id.clone());
                        }
                    }
                    (ItemKind::Comp, Some(s)) => walk(p, s, None, depth + 1, out),
                    _ => {}
                }
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(p, comp, Some((t0, t1)), 0, &mut out);
    out
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
    use super::{resolve_path, Engine};
    use crate::model::project::{Media, Project};
    use crate::ops::{self, Env};
    use crate::render::encode::RenderOpts;
    use crate::render::ffmpeg::Ff;
    use anyhow::{bail, Result};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Pretend media: every path probes as a 30 s 1080p clip (audio for .mp3/.wav).
    struct Mock;

    impl Env for Mock {
        fn probe(&self, path: &str) -> Result<Media> {
            let name = path.rsplit(['/', '\\']).next().unwrap().to_string();
            let audio = name.ends_with(".mp3") || name.ends_with(".wav");
            Ok(serde_json::from_value(json!({
                "id": "", "path": path, "name": name, "kind": if audio { "audio" } else { "video" }, "dur": 30.0, "w": 1920, "h": 1080, "fps": 30.0,
                "has_video": !audio, "has_audio": true, "channels": 2, "rate": 48000, "codec": "h264", "size": 1, "mtime": 1
            }))?)
        }
        fn analyze(&self, _: &Project, _: &str, _: &str, _: &BTreeMap<String, String>) -> Result<serde_json::Value> {
            bail!("no analysis in engine tests")
        }
        fn sfx(&self, kind: &str, dur: f64, pitch: f64, seed: u64) -> Result<Media> {
            let k = crate::sfx::kind(kind).unwrap();
            let mut m = self.probe(&format!("/sfx/{}", crate::sfx::file_name(k, dur, pitch, seed)))?;
            m.dur = dur;
            Ok(m)
        }
    }

    fn engine() -> Arc<Engine> {
        Engine::new(Ff::missing())
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("moodcut-engine-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A project using every kind of detail an edit can have.
    fn rich_project() -> Project {
        let mut p = Project::new("Everything", 1920, 1080, 30.0);
        for line in [
            "import /v/a.mp4 /v/b.mp4 /m/song.mp3",
            "append m1[0-5] m2[0-5]",
            "tr c1 c2 wipeleft 0.8",
            "fx c1 blur radius=4",
            "color c2 cinematic",
            "mask c1 ellipse invert",
            "kf c1 opacity 0:0 1:1@outBack 3:0.5",
            "kf c1 scale 0:1 2:1.5@anticipate",
            "expr c2 rot 'time*90'",
            "text 'Title' at=0 dur=3 as=ttl",
            "anim ttl slamIn",
            "solid #3366cc w=300 h=180 pos=320,180 at=0 dur=3 as=box",
            "matte box ttl alphainv",
            "null at=0 dur=5 as=ctrl",
            "parent ttl ctrl",
            "shape rect w=200 h=20 at=4 dur=2 as=dot",
            "precomp dot name=Badge",
            "add m3 A1 at=0 dur=10",
            "afx A1 eq",
            "afx master limiter",
            "master lufs=-14 limiter=1",
            "track add A",
            "track set A1 vol=-3 pan=0.2 name=Music",
            "duck A1 by=A2 amount=-10",
            "marker add 2 'Hook' color=#f00 dur=1 note='big moment'",
            "workarea 1 3",
            "brand bold accent=#ff2d55",
            "sfx whoosh at=2",
        ] {
            if let Err(e) = ops::run(&mut p, &Mock, "s1", line) {
                panic!("{line}: {e}");
            }
        }
        p
    }

    #[test]
    fn save_and_open_keep_every_detail() {
        let d = scratch("roundtrip");
        let file = d.join("everything.mcut");
        let p = rich_project();
        let a = engine();
        a.commit(p.clone(), None, "build", "test", &[]).unwrap();
        a.set_ui(ops::Ui { playhead: Some(2.5), ..Default::default() });
        assert!(a.save(Some(file.to_str().unwrap())).unwrap().starts_with("saved"));
        assert!(!a.st.read().dirty);
        let b = engine();
        let msg = b.open(file.to_str().unwrap()).unwrap();
        assert!(msg.contains("missing media"), "the pretend files don't exist: {msg}");
        let (q, _, active, path) = b.snapshot();
        assert_eq!(serde_json::to_value(&q).unwrap(), serde_json::to_value(&p).unwrap(), "the reopened project differs");
        assert_eq!(active, "s1");
        assert_eq!(path.as_deref(), Some(file.as_path()));
        assert_eq!(b.st.read().restore, Some(2.5), "the playhead comes back");
        // and again: saving what was opened writes the same project
        b.save(None).unwrap();
        let c = engine();
        c.open(file.to_str().unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&c.snapshot().0).unwrap(), serde_json::to_value(&p).unwrap());
    }

    #[test]
    fn undo_after_open_never_writes_the_previous_project_into_the_file() {
        let d = scratch("undo-open");
        let (fa, fb) = (d.join("a.mcut"), d.join("b.mcut"));
        let e = engine();
        e.new_project("A", 1920, 1080, 30.0, Some(fa.clone())).unwrap();
        e.edit("text 'only in A' at=0 dur=2", "test").unwrap();
        e.new_project("B", 1280, 720, 25.0, Some(fb.clone())).unwrap();
        let b_before = std::fs::read_to_string(&fb).unwrap();
        e.open(fa.to_str().unwrap()).unwrap();
        assert!(e.edit("undo", "test").is_err(), "opening starts a new history");
        e.open(fb.to_str().unwrap()).unwrap();
        assert!(e.edit("undo", "test").is_err());
        assert_eq!(std::fs::read_to_string(&fb).unwrap(), b_before);
        assert!(!std::fs::read_to_string(&fb).unwrap().contains("only in A"));
    }

    #[test]
    fn undo_and_redo_put_a_graphics_html_back() {
        let d = scratch("gfx-undo");
        let html = d.join("title.html");
        let e = engine();
        e.new_project("G", 1920, 1080, 30.0, Some(d.join("g.mcut"))).unwrap();
        let media = |src: &str| -> Media {
            let mut m = Mock.probe(d.join("title.mov").to_str().unwrap()).unwrap();
            m.id = "m1".into();
            m.gfx = Some(crate::gfx::Gfx { html: html.to_string_lossy().to_string(), w: 1920, h: 1080, fps: 30.0, dur: 4.0, alpha: true, src: src.into(), ..Default::default() });
            m
        };
        // what `graphic` does: commit the media, then write the page next to it
        let mut p = e.snapshot().0;
        p.media.push(media("<h1>{{t|One}}</h1>"));
        e.commit(p, None, "graphic title", "test", &[]).unwrap();
        std::fs::write(&html, "<h1>{{t|One}}</h1>").unwrap();
        let mut p = e.snapshot().0;
        p.media[0] = media("<h1>{{t|Two}}</h1>");
        e.commit(p, None, "graphic title", "test", &[]).unwrap();
        std::fs::write(&html, "<h1>{{t|Two}}</h1>").unwrap();

        e.edit("undo", "test").unwrap();
        assert_eq!(std::fs::read_to_string(&html).unwrap(), "<h1>{{t|One}}</h1>", "a re-render after undo must not bring the undone html back");
        e.edit("redo", "test").unwrap();
        assert_eq!(std::fs::read_to_string(&html).unwrap(), "<h1>{{t|Two}}</h1>");
        e.edit("text 'unrelated' at=0 dur=1", "test").unwrap();
        std::fs::write(&html, "edited in a text editor").unwrap();
        e.edit("undo", "test").unwrap();
        assert_eq!(std::fs::read_to_string(&html).unwrap(), "edited in a text editor", "undoing something else leaves the file alone");
    }

    #[test]
    fn an_edit_with_an_impossible_number_changes_nothing() {
        let e = engine();
        e.edit("text 'Hi' at=0 dur=2 as=hi", "test").unwrap();
        let (mut p, rev, ..) = e.snapshot();
        p.item_mut("hi").unwrap().start = f64::INFINITY;
        let err = e.commit(p, None, "bad", "test", &[]).unwrap_err().to_string();
        assert!(err.contains("invalid number") && err.contains("start"), "{err}");
        assert_eq!(e.snapshot().1, rev);
        assert_eq!(e.snapshot().0.item("hi").unwrap().start, 0.0);
    }

    #[test]
    fn untitled_projects_autosave_separately_and_can_be_recovered() {
        let d = scratch("recover");
        let e = engine();
        e.edit("text 'Lost work' at=0 dur=2", "test").unwrap();
        let first = e.st.read().autosave.clone();
        assert!(first.is_file(), "an untitled project autosaves");
        e.new_project("Second", 1920, 1080, 30.0, None).unwrap();
        e.edit("text 'Other work' at=0 dur=2", "test").unwrap();
        let second = e.st.read().autosave.clone();
        assert_ne!(first, second, "the second untitled project doesn't overwrite the first");
        assert!(e.unsaved().iter().any(|u| u.path == first.to_string_lossy()));
        let msg = e.recover(first.to_str().unwrap()).unwrap();
        assert!(msg.contains("recovered"), "{msg}");
        assert!(e.snapshot().0.comps[0].items().any(|i| i.text.as_deref() == Some("Lost work")));
        assert!(e.snapshot().3.is_none(), "still untitled");
        assert!(e.recover(d.join("x.mcut").to_str().unwrap()).is_err(), "only autosaves can be recovered");
        // saved for real: its autosave goes away
        e.save(Some(d.join("kept").to_str().unwrap())).unwrap();
        assert!(d.join("kept.mcut").is_file());
        assert!(!first.exists());
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn export_stops_before_starting_when_media_is_offline() {
        let e = engine();
        let mut p = Project::new("t", 1920, 1080, 30.0);
        ops::run(&mut p, &Mock, "s1", "import /nowhere/shot.mp4\nappend m1[0-4]").unwrap();
        e.commit(p, None, "build", "test", &[]).unwrap();
        let opts = RenderOpts { comp: String::new(), t0: 0.0, t1: 0.0, preset: "mp4".into(), out: PathBuf::new(), scale: 1.0, hw: "off".into() };
        let err = match e.start_render(opts) {
            Ok(_) => panic!("the export started with offline media"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("can't export") && err.contains("m1 shot.mp4") && err.contains("relink"), "{err}");
        assert!(e.jobs.lock().is_empty(), "no job was started");
    }

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
