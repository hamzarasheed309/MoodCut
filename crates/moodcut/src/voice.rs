//! AI voice: clone a voice from a few seconds of audio and speak scripts with it, using
//! OmniVoice (`k2-fsa/OmniVoice`, a 0.6B zero-shot text-to-speech model) run on this computer.
//!
//! MoodCut itself stays one Rust binary. OmniVoice is Python (PyTorch), so it runs in an
//! environment of its own that MoodCut sets up in a folder on a drive the person picks
//! (`<drive>\MoodCut\OmniVoice` by default; the model and PyTorch need about 10 GB). `uv`
//! installs Python and the packages there, so nothing goes into the system Python or onto the
//! system drive. The layout of that folder (the "root"):
//!
//! ```text
//! model/                 the model from huggingface.co/k2-fsa/OmniVoice (about 3.3 GB)
//! tools/uv(.exe)         uv, which installs Python and the packages
//! python/  env/          uv-managed Python and the environment (PyTorch, omnivoice)
//! hf/                    HF_HOME: Whisper, downloaded by OmniVoice the first time a voice is
//!                        cloned without a transcript of its reference audio
//! voices/<id>/           cloned voices: ref.wav, prompt.pt (the encoded voice), voice.json
//! omnivoice_worker.py    the worker MoodCut talks to (assets/omnivoice_worker.py)
//! ```
//!
//! The model is looked for automatically: the saved location, `MOODCUT_OMNIVOICE_MODEL`,
//! `<drive>\MoodCut\OmniVoice` and other usual folders on every fixed drive, and the Hugging
//! Face caches. When it isn't found, the editor offers to locate it or download it.
//!
//! The worker loads the model once and stays up while it is used (it is stopped after ten idle
//! minutes, which frees the GPU memory). Cloning and generating run as queued tasks, one at a
//! time; generated speech is written next to the project's graphics and imported into the
//! project as one undoable edit (tag `voice`, the script as the note, optionally placed at the
//! playhead).

use crate::engine::Engine;
use crate::render::ffmpeg::command;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const REPO: &str = "k2-fsa/OmniVoice";
const WORKER_PY: &str = include_str!("../assets/omnivoice_worker.py");
const WORKER_FILE: &str = "omnivoice_worker.py";
/// Under the drive the person picks.
pub const DEFAULT_SUBDIR: &str = if cfg!(windows) { "MoodCut\\OmniVoice" } else { "MoodCut/OmniVoice" };
const PYTHON_VERSION: &str = "3.12";
/// The PyTorch OmniVoice is tested with (its own uv constraint).
const TORCH: [&str; 2] = ["torch==2.8.0", "torchaudio==2.8.0"];
/// Approximate download sizes, for the editor and the free-space check.
const MODEL_BYTES: u64 = 3_270_000_000;
const RUNTIME_BYTES: u64 = 6_500_000_000;
const WHISPER_BYTES: u64 = 1_620_000_000;
/// The worker is stopped after this long without work (frees the GPU memory).
const IDLE_UNLOAD: Duration = Duration::from_secs(600);
/// Reference audio longer than this is cut (3-10 s works best).
const MAX_REF_SECS: f64 = 20.0;

/// The files of the model repo: (path, approximate size, required).
const MODEL_FILES: &[(&str, u64, bool)] = &[
    ("config.json", 2_240, true),
    ("tokenizer_config.json", 533, true),
    ("tokenizer.json", 11_400_000, true),
    ("chat_template.jinja", 4_170, false),
    ("README.md", 9_420, false),
    ("audio_tokenizer/config.json", 2_530, true),
    ("audio_tokenizer/preprocessor_config.json", 206, true),
    ("audio_tokenizer/LICENSE", 9_170, false),
    ("audio_tokenizer/model.safetensors", 806_000_000, true),
    ("model.safetensors", 2_450_000_000, true),
];

// ---------------------------------------------------------------- locations

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn env_python(root: &Path) -> PathBuf {
    if cfg!(windows) {
        root.join("env").join("Scripts").join("python.exe")
    } else {
        root.join("env").join("bin").join("python")
    }
}

fn uv_path(root: &Path) -> PathBuf {
    root.join("tools").join(exe("uv"))
}

/// Written once the environment imported omnivoice successfully (holds the versions).
fn ready_marker(root: &Path) -> PathBuf {
    root.join("env").join(".moodcut-ready")
}

fn voices_dir(root: &Path) -> PathBuf {
    root.join("voices")
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// A complete OmniVoice model folder (the Hugging Face repo, with its audio tokenizer).
pub fn is_model_dir(d: &Path) -> bool {
    d.join("config.json").is_file() && d.join("tokenizer.json").is_file() && file_len(&d.join("model.safetensors")) > (1 << 30) && file_len(&d.join("audio_tokenizer").join("model.safetensors")) > (100 << 20)
}

/// The model folder in what the person pointed at: the model itself, an install folder (its
/// `model/`), or a Hugging Face cache (`models--k2-fsa--OmniVoice` or a `hub` folder).
fn model_in(p: &Path) -> Option<PathBuf> {
    if is_model_dir(p) {
        return Some(p.to_path_buf());
    }
    for c in [p.join("model"), p.join("OmniVoice"), p.join("OmniVoice").join("model")] {
        if is_model_dir(&c) {
            return Some(c);
        }
    }
    let cache = if p.join("snapshots").is_dir() { p.to_path_buf() } else { p.join("models--k2-fsa--OmniVoice") };
    newest_snapshot(&cache)
}

/// The newest complete snapshot in a Hugging Face cache folder of the repo.
fn newest_snapshot(repo_cache: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(repo_cache.join("snapshots")).ok()?;
    let mut snaps: Vec<(SystemTime, PathBuf)> = rd.flatten().map(|e| e.path()).filter(|p| is_model_dir(p)).map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), p)).collect();
    snaps.sort();
    snaps.pop().map(|(_, p)| p)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok().map(PathBuf::from)
}

/// Fixed drives on Windows (`C:\`, `D:\`, ...); the home folder elsewhere.
fn search_bases() -> Vec<PathBuf> {
    if cfg!(windows) {
        drives().into_iter().filter(|d| !d.removable).map(|d| PathBuf::from(d.root)).collect()
    } else {
        home_dir().into_iter().collect()
    }
}

/// Where an install folder made by MoodCut would be on each drive.
fn candidate_roots() -> Vec<PathBuf> {
    search_bases().into_iter().map(|b| b.join(DEFAULT_SUBDIR)).collect()
}

/// Usual places for a model downloaded by hand or by other tools.
fn candidate_models() -> Vec<PathBuf> {
    let mut out = vec![];
    for b in search_bases() {
        for rel in ["OmniVoice", "models/OmniVoice", "models/k2-fsa/OmniVoice", "AI/OmniVoice", "AI/models/OmniVoice", "AI Models/OmniVoice", "MoodCut/OmniVoice/hf/hub"] {
            out.push(b.join(rel));
        }
    }
    if let Some(h) = home_dir() {
        for rel in ["OmniVoice", "models/OmniVoice", "Downloads/OmniVoice", "Documents/OmniVoice"] {
            out.push(h.join(rel));
        }
        out.push(h.join(".cache").join("huggingface").join("hub"));
    }
    for var in ["HF_HUB_CACHE", "HUGGINGFACE_HUB_CACHE"] {
        if let Ok(v) = std::env::var(var) {
            out.push(PathBuf::from(v));
        }
    }
    if let Ok(v) = std::env::var("HF_HOME") {
        out.push(PathBuf::from(v).join("hub"));
    }
    out
}

// ---------------------------------------------------------------- drives

#[derive(Clone, Debug, Serialize)]
pub struct Drive {
    pub root: String,
    pub label: String,
    /// Bytes free (None when unknown).
    pub free: Option<u64>,
    pub total: Option<u64>,
    pub removable: bool,
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Drives someone can install to: fixed and removable ones (no network, CD or RAM drives).
#[cfg(windows)]
pub fn drives() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    let mask = unsafe { GetLogicalDrives() };
    let mut out = vec![];
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let w = wide(&root);
        // DRIVE_REMOVABLE = 2, DRIVE_FIXED = 3
        let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
        if kind != 2 && kind != 3 {
            continue;
        }
        let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
        // fails for an empty card reader: nothing to install to
        if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut total_free) } == 0 {
            continue;
        }
        let mut name = [0u16; 261];
        let ok = unsafe { GetVolumeInformationW(w.as_ptr(), name.as_mut_ptr(), name.len() as u32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0) } != 0;
        let label = if ok { String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(0)]) } else { String::new() };
        out.push(Drive { root, label, free: Some(free), total: Some(total), removable: kind == 2 });
    }
    out
}

#[cfg(not(windows))]
pub fn drives() -> Vec<Drive> {
    home_dir().map(|h| Drive { root: h.to_string_lossy().to_string(), label: "Home".into(), free: free_space(&h), total: None, removable: false }).into_iter().collect()
}

/// Free bytes on the drive holding `p` (or the nearest existing folder above it).
#[cfg(windows)]
pub fn free_space(p: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut d = p.to_path_buf();
    while !d.exists() {
        d = d.parent()?.to_path_buf();
    }
    let w = wide(&d.to_string_lossy());
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    (unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut total_free) } != 0).then_some(free)
}

#[cfg(not(windows))]
pub fn free_space(p: &Path) -> Option<u64> {
    let mut d = p.to_path_buf();
    while !d.exists() {
        d = d.parent()?.to_path_buf();
    }
    // `df -Pk <dir>`: the 4th column of the 2nd line is the available KiB
    let out = command(Path::new("df")).arg("-Pk").arg(&d).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse::<u64>().ok().map(|k| k * 1024)
}

// ---------------------------------------------------------------- detection

#[derive(Clone, Debug, Default, Serialize)]
pub struct Found {
    /// The install folder (environment, voices, Whisper cache).
    pub root: Option<PathBuf>,
    pub model: Option<PathBuf>,
    /// A Python with omnivoice, when the environment is ready.
    pub python: Option<PathBuf>,
    /// What the environment reported when it was set up (torch, device, gpu, omnivoice).
    pub runtime: Option<Value>,
}

impl Found {
    fn ready(&self) -> bool {
        self.model.is_some() && self.python.is_some()
    }
    fn state(&self) -> &'static str {
        match (self.model.is_some(), self.python.is_some()) {
            (true, true) => "ready",
            (true, false) => "runtime",
            (false, true) => "model",
            (false, false) => "missing",
        }
    }
}

fn saved() -> (Option<PathBuf>, Option<PathBuf>) {
    let s = crate::config::settings();
    let get = |k: &str| s["omnivoice"][k].as_str().filter(|x| !x.is_empty()).map(PathBuf::from);
    (get("root"), get("model"))
}

fn save(root: Option<&Path>, model: Option<&Path>) {
    let s = |p: Option<&Path>| p.map(|p| Value::String(p.to_string_lossy().to_string())).unwrap_or(Value::Null);
    crate::config::save_setting("omnivoice", json!({"root": s(root), "model": s(model)}));
}

fn runtime_of(root: &Path) -> Option<Value> {
    if !env_python(root).is_file() {
        return None;
    }
    let text = std::fs::read_to_string(ready_marker(root)).ok()?;
    serde_json::from_str(&text).ok().or(Some(json!({})))
}

/// Finds the model and the environment; remembers what it found in the settings.
pub fn detect() -> Found {
    let (saved_root, saved_model) = saved();
    let mut root = std::env::var("MOODCUT_OMNIVOICE_HOME").ok().map(PathBuf::from).or(saved_root.clone());
    let mut model = std::env::var("MOODCUT_OMNIVOICE_MODEL").ok().and_then(|m| model_in(Path::new(&m))).or_else(|| saved_model.clone().filter(|m| is_model_dir(m)));
    if model.is_none() {
        model = root.as_ref().and_then(|r| model_in(r));
    }
    // an install folder from an earlier setup (another drive, settings reset)
    if root.is_none() || model.is_none() {
        for c in candidate_roots() {
            let has_model = is_model_dir(&c.join("model"));
            let has_env = env_python(&c).is_file();
            if root.is_none() && (has_model || has_env) {
                root = Some(c.clone());
            }
            if model.is_none() && has_model {
                model = Some(c.join("model"));
            }
        }
    }
    if model.is_none() {
        model = candidate_models().iter().find_map(|c| model_in(c));
    }
    let mut python = None;
    let mut runtime = None;
    if let Some(r) = &root {
        if let Some(rt) = runtime_of(r) {
            python = Some(env_python(r));
            runtime = Some(rt);
        }
    }
    // a Python of one's own that has omnivoice installed
    if python.is_none() {
        if let Some(p) = std::env::var("MOODCUT_OMNIVOICE_PYTHON").ok().map(PathBuf::from).filter(|p| p.is_file()) {
            python = Some(p);
            runtime = Some(json!({"python": "MOODCUT_OMNIVOICE_PYTHON"}));
        }
    }
    if (root.clone(), model.clone()) != (saved_root, saved_model) && (root.is_some() || model.is_some()) {
        save(root.as_deref(), model.as_deref());
    }
    Found { root, model, python, runtime }
}

// ---------------------------------------------------------------- shared state

#[derive(Clone, Debug, Default, Serialize)]
struct Install {
    running: bool,
    root: String,
    step: String,
    status: String,
    /// 0-100 for downloads with a known size.
    pct: Option<f64>,
    log: VecDeque<String>,
    error: Option<String>,
    done: bool,
}

#[derive(Clone, Debug, Serialize)]
struct Task {
    id: u64,
    kind: &'static str,
    label: String,
    state: &'static str,
    status: String,
    error: Option<String>,
    result: Option<Value>,
    secs: f64,
    #[serde(skip)]
    req: Value,
}

#[derive(Default)]
struct Shared {
    found: Option<Found>,
    install: Install,
    tasks: Vec<Task>,
    next_task: u64,
    runner: bool,
}

fn shared() -> &'static Mutex<Shared> {
    static S: OnceLock<Mutex<Shared>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Shared::default()))
}

static CANCEL_INSTALL: AtomicBool = AtomicBool::new(false);
static CANCEL_TASK: AtomicBool = AtomicBool::new(false);

fn found() -> Found {
    if let Some(f) = shared().lock().found.clone() {
        return f;
    }
    let f = detect();
    shared().lock().found = Some(f.clone());
    f
}

fn rescan() -> Found {
    let f = detect();
    shared().lock().found = Some(f.clone());
    f
}

fn notify(eng: &Engine, text: impl Into<String>) {
    let rev = eng.st.read().rev;
    eng.emit("voice", rev, "voice", text.into());
}

fn log_line(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let mut s = shared().lock();
    s.install.status = line.chars().take(200).collect();
    s.install.log.push_back(s.install.status.clone());
    while s.install.log.len() > 40 {
        s.install.log.pop_front();
    }
}

fn set_step(step: &str, pct: Option<f64>) {
    let mut s = shared().lock();
    s.install.step = step.to_string();
    s.install.pct = pct;
    s.install.status = step.to_string();
    s.install.log.push_back(format!("— {step}"));
}

fn set_status(status: String, pct: Option<f64>) {
    let mut s = shared().lock();
    s.install.status = status;
    s.install.pct = pct;
}

// ---------------------------------------------------------------- the editor's API

/// `GET/POST /api/voice`: `{action: status|scan|drives|locate|install|cancel_install|clone|
/// generate|cancel|delete_voice|clear|unload}`.
pub fn api(eng: &Arc<Engine>, a: &Value) -> Result<Value> {
    let action = a["action"].as_str().unwrap_or("status");
    match action {
        "status" => {}
        "scan" => {
            rescan();
        }
        "drives" => {
            let f = found();
            let suggested = f.root.as_ref().map(|r| r.to_string_lossy().to_string());
            return Ok(json!({"ok": true, "drives": drives(), "subdir": DEFAULT_SUBDIR, "suggested": suggested, "need": need_bytes(&f, None)}));
        }
        "locate" => {
            let path = a["path"].as_str().map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| anyhow!("path required"))?;
            locate(Path::new(path))?;
        }
        "install" => {
            let root = match (a["folder"].as_str().filter(|s| !s.trim().is_empty()), a["drive"].as_str().filter(|s| !s.trim().is_empty())) {
                (Some(f), _) => PathBuf::from(f.trim()),
                (None, Some(d)) => PathBuf::from(d.trim()).join(DEFAULT_SUBDIR),
                (None, None) => found().root.ok_or_else(|| anyhow!("pick a drive to install OmniVoice on"))?,
            };
            start_install(eng.clone(), root)?;
        }
        "cancel_install" => CANCEL_INSTALL.store(true, Ordering::Relaxed),
        "clone" | "generate" => {
            let f = found();
            if !f.ready() {
                bail!("OmniVoice isn't set up yet");
            }
            let label = if action == "clone" {
                format!("Clone voice \"{}\"", a["name"].as_str().unwrap_or("voice").trim())
            } else {
                let text = a["text"].as_str().unwrap_or("").trim();
                format!("Speak \"{}\"", first_words(text, 8))
            };
            enqueue(eng.clone(), if action == "clone" { "clone" } else { "generate" }, label, a.clone());
        }
        "cancel" => {
            let id = a["id"].as_u64();
            let mut s = shared().lock();
            for t in s.tasks.iter_mut() {
                if id.map_or(true, |i| i == t.id) {
                    match t.state {
                        "queued" => {
                            t.state = "error";
                            t.error = Some("cancelled".into());
                        }
                        "running" => CANCEL_TASK.store(true, Ordering::Relaxed),
                        _ => {}
                    }
                }
            }
        }
        "clear" => shared().lock().tasks.retain(|t| t.state == "queued" || t.state == "running"),
        "delete_voice" => {
            let id = a["id"].as_str().unwrap_or("");
            let root = found().root.ok_or_else(|| anyhow!("no OmniVoice folder"))?;
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                bail!("bad voice id");
            }
            let dir = voices_dir(&root).join(id);
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir).with_context(|| format!("deleting {}", dir.display()))?;
            }
        }
        "rename_voice" => {
            let root = found().root.ok_or_else(|| anyhow!("no OmniVoice folder"))?;
            let id = a["id"].as_str().unwrap_or("");
            let name = a["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| anyhow!("name required"))?;
            let mut v = load_voice(&root, id)?;
            v["name"] = json!(name);
            std::fs::write(voices_dir(&root).join(id).join("voice.json"), serde_json::to_string_pretty(&v)?)?;
        }
        "unload" => {
            // not while it is speaking (that is what Cancel is for)
            if let Some(mut w) = worker_slot().0.try_lock() {
                *w = None;
            }
        }
        other => bail!("unknown action '{other}'"),
    }
    Ok(status())
}

fn status() -> Value {
    let f = found();
    let (install, tasks) = {
        let s = shared().lock();
        (s.install.clone(), s.tasks.clone())
    };
    let voices = f.root.as_deref().map(list_voices).unwrap_or_default();
    let worker = worker_slot().summary();
    json!({
        "ok": true,
        "state": f.state(),
        "root": f.root,
        "model": f.model,
        "python": f.python,
        "runtime": f.runtime,
        "gpu": gpu().map(|g| g.0.clone()),
        "worker": worker,
        "install": install,
        "tasks": tasks,
        "voices": voices,
        "repo": REPO,
        "subdir": DEFAULT_SUBDIR,
        "sizes": {"model": MODEL_BYTES, "runtime": RUNTIME_BYTES, "whisper": WHISPER_BYTES},
    })
}

/// The model (and possibly an install folder) the person pointed at.
fn locate(p: &Path) -> Result<()> {
    if !p.exists() {
        bail!("{} doesn't exist", p.display());
    }
    let (mut root, _) = saved();
    let model = model_in(p);
    // an install folder (environment and voices) counts too, with or without its model
    let install_root = [p.to_path_buf(), p.join(DEFAULT_SUBDIR)].into_iter().find(|c| env_python(c).is_file() || voices_dir(c).is_dir());
    if model.is_none() && install_root.is_none() {
        bail!("no OmniVoice model in {}: pick the folder that has config.json, model.safetensors and audio_tokenizer (from huggingface.co/{REPO})", p.display());
    }
    if install_root.is_some() {
        root = install_root;
    }
    let model = model.or_else(|| saved().1.filter(|m| is_model_dir(m)));
    save(root.as_deref(), model.as_deref());
    rescan();
    Ok(())
}

// ---------------------------------------------------------------- install

/// Bytes still to download to set up in `root` (model and environment).
fn need_bytes(f: &Found, root: Option<&Path>) -> u64 {
    let mut need = 0;
    if f.model.is_none() {
        let dir = root.map(|r| r.join("model"));
        need += MODEL_FILES.iter().map(|(rel, size, _)| if dir.as_ref().is_some_and(|d| d.join(rel).is_file()) { 0 } else { *size }).sum::<u64>();
    }
    if !root.map(|r| runtime_of(r).is_some()).unwrap_or(f.python.is_some()) {
        need += RUNTIME_BYTES;
    }
    need
}

fn start_install(eng: Arc<Engine>, root: PathBuf) -> Result<()> {
    if !root.is_absolute() {
        bail!("pick a full folder path, like D:\\{DEFAULT_SUBDIR}");
    }
    {
        let mut s = shared().lock();
        if s.install.running {
            bail!("OmniVoice is already being set up in {}", s.install.root);
        }
        s.install = Install { running: true, root: root.to_string_lossy().to_string(), step: "starting".into(), status: "starting".into(), ..Default::default() };
    }
    CANCEL_INSTALL.store(false, Ordering::Relaxed);
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| install(&eng, &root))).unwrap_or_else(|p| Err(anyhow!("internal error: {}", crate::engine::panic_message(&*p))));
        rescan();
        let msg = {
            let mut s = shared().lock();
            s.install.running = false;
            s.install.pct = None;
            match &r {
                Ok(()) => {
                    s.install.done = true;
                    s.install.step = "ready".into();
                    s.install.status = "OmniVoice is ready".into();
                }
                Err(e) => {
                    let e = if CANCEL_INSTALL.load(Ordering::Relaxed) { "cancelled: press Install to continue where it stopped".to_string() } else { format!("{e:#}") };
                    s.install.error = Some(e);
                }
            }
            s.install.status.clone()
        };
        notify(&eng, msg);
    });
    Ok(())
}

fn install(eng: &Engine, root: &Path) -> Result<()> {
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let before = rescan();
    // a model found elsewhere is used where it is; otherwise it goes into the install folder
    let model_dir = before.model.clone().unwrap_or_else(|| root.join("model"));
    save(Some(root), before.model.as_deref());
    let need = need_bytes(&before, Some(root));
    if let Some(free) = free_space(root) {
        // PyTorch unpacks next to its download, and Whisper comes later: keep some room
        let want = need + (2 << 30);
        if free < want {
            bail!(
                "{} has {} free; OmniVoice needs about {} there. Pick another drive or free some space",
                root.display(),
                crate::library::fmt_bytes(free),
                crate::library::fmt_bytes(want)
            );
        }
    }
    notify(eng, "setting up OmniVoice");
    if !is_model_dir(&model_dir) {
        download_model(&model_dir)?;
        save(Some(root), Some(&model_dir));
    }
    if runtime_of(root).is_none() {
        setup_runtime(root)?;
    }
    write_worker(root)?;
    // the environment changed under a running worker: start a new one next time
    stop_worker();
    // the uv cache: archives already linked into the environment
    let _ = std::fs::remove_dir_all(root.join("cache"));
    save(Some(root), Some(&model_dir));
    Ok(())
}

fn hf_endpoint() -> String {
    std::env::var("HF_ENDPOINT").ok().filter(|e| !e.trim().is_empty()).unwrap_or_else(|| "https://huggingface.co".into()).trim_end_matches('/').to_string()
}

fn download_model(dir: &Path) -> Result<()> {
    let total: u64 = MODEL_FILES.iter().map(|f| f.1).sum();
    let base = hf_endpoint();
    let mut done_bytes: u64 = MODEL_FILES.iter().filter(|(rel, _, _)| dir.join(rel).is_file()).map(|f| f.1).sum();
    set_step(&format!("Downloading the OmniVoice model ({})", crate::library::fmt_bytes(total)), Some(done_bytes as f64 / total as f64 * 100.0));
    for (rel, size, required) in MODEL_FILES {
        let dest = dir.join(rel);
        if dest.is_file() {
            continue;
        }
        let url = format!("{base}/{REPO}/resolve/main/{rel}?download=true");
        let start = done_bytes;
        let r = download(&url, &dest, &|got| {
            let pct = ((start + got.min(*size)) as f64 / total as f64 * 100.0).min(100.0);
            set_status(format!("model: {rel} {} of {}", crate::library::fmt_bytes(got), crate::library::fmt_bytes(*size)), Some(pct));
        });
        match r {
            Ok(()) => done_bytes += size,
            Err(e) if !required && !CANCEL_INSTALL.load(Ordering::Relaxed) => log_line(&format!("skipped {rel}: {e:#}")),
            Err(e) => return Err(e.context(format!("downloading {rel} from {base}/{REPO}"))),
        }
    }
    if !is_model_dir(dir) {
        bail!("the model in {} is incomplete after downloading; press Install to try again", dir.display());
    }
    log_line("model downloaded");
    Ok(())
}

/// Downloads with curl into `<dest>.part` (resumed when it exists) and renames when complete.
fn download(url: &str, dest: &Path, progress: &dyn Fn(u64)) -> Result<()> {
    let curl = crate::setup::sys_tool("curl").context("curl is needed to download (built into Windows 10 and later)")?;
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    for attempt in 0..2 {
        let mut c = command(&curl);
        c.args(["-L", "--fail", "--silent", "--show-error", "--retry", "3", "--retry-delay", "3", "--connect-timeout", "30"]);
        if attempt == 0 && part.is_file() {
            c.args(["-C", "-"]);
        }
        let mut child = c.arg("-o").arg(&part).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("running curl")?;
        let st = loop {
            if let Some(st) = child.try_wait()? {
                break st;
            }
            if CANCEL_INSTALL.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("cancelled");
            }
            progress(file_len(&part));
            std::thread::sleep(Duration::from_millis(500));
        };
        if st.success() {
            std::fs::rename(&part, dest).with_context(|| format!("saving {}", dest.display()))?;
            return Ok(());
        }
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        // a partial file the server won't resume (it changed upstream): start over once
        if attempt == 0 && part.is_file() && (err.contains("416") || err.contains("range") || err.contains("(33)")) {
            let _ = std::fs::remove_file(&part);
            continue;
        }
        bail!("{}", err.trim().lines().last().unwrap_or("download failed"));
    }
    bail!("download failed")
}

fn uv_asset() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "aarch64")) {
        Some("uv-aarch64-pc-windows-msvc.zip")
    } else if cfg!(windows) {
        Some("uv-x86_64-pc-windows-msvc.zip")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("uv-aarch64-apple-darwin.tar.gz")
    } else if cfg!(target_os = "macos") {
        Some("uv-x86_64-apple-darwin.tar.gz")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("uv-aarch64-unknown-linux-gnu.tar.gz")
    } else if cfg!(target_os = "linux") {
        Some("uv-x86_64-unknown-linux-gnu.tar.gz")
    } else {
        None
    }
}

/// uv in the install folder, downloaded from its GitHub releases when missing.
fn ensure_uv(root: &Path) -> Result<PathBuf> {
    let uv = uv_path(root);
    if uv.is_file() {
        return Ok(uv);
    }
    let asset = uv_asset().ok_or_else(|| anyhow!("no uv build for this platform: install Python 3.12 with omnivoice yourself and set MOODCUT_OMNIVOICE_PYTHON"))?;
    set_step("Downloading uv (Python installer)", None);
    let tools = root.join("tools");
    let archive = tools.join(asset);
    download(&format!("https://github.com/astral-sh/uv/releases/latest/download/{asset}"), &archive, &|got| set_status(format!("uv: {}", crate::library::fmt_bytes(got)), None))?;
    let tar = crate::setup::sys_tool("tar").context("tar is needed to unpack uv (built into Windows 10 and later)")?;
    let work = tools.join("unpack");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let out = command(&tar).arg("-xf").arg(&archive).arg("-C").arg(&work).stdout(Stdio::null()).stderr(Stdio::piped()).output().context("running tar")?;
    if !out.status.success() {
        bail!("couldn't unpack {asset}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let found = find_file(&work, &exe("uv"), 0).ok_or_else(|| anyhow!("{asset} has no uv"))?;
    std::fs::copy(&found, &uv).with_context(|| format!("copying uv to {}", uv.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_file(&archive);
    Ok(uv)
}

fn find_file(dir: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let rd: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    if let Some(f) = rd.iter().find(|p| p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy() == name)) {
        return Some(f.clone());
    }
    if depth > 3 {
        return None;
    }
    rd.iter().filter(|p| p.is_dir()).find_map(|p| find_file(p, name, depth + 1))
}

/// The NVIDIA GPU and its driver version, if there is one.
fn gpu() -> Option<&'static (String, f64)> {
    static G: OnceLock<Option<(String, f64)>> = OnceLock::new();
    G.get_or_init(|| {
        let smi = crate::setup::sys_tool("nvidia-smi")?;
        let out = command(&smi).args(["--query-gpu=name,driver_version", "--format=csv,noheader"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let (name, drv) = text.lines().next()?.split_once(',')?;
        let drv = drv.trim();
        let major: f64 = drv.split('.').next()?.parse().ok()?;
        Some((format!("{} (driver {drv})", name.trim()), major))
    })
    .as_ref()
}

/// The PyTorch wheel index for this computer: CUDA builds for NVIDIA GPUs, else CPU
/// (macOS gets Metal from the default index).
fn torch_index() -> Option<(&'static str, String)> {
    if cfg!(target_os = "macos") {
        return None;
    }
    match gpu() {
        Some((name, drv)) if *drv >= 570.0 => Some(("https://download.pytorch.org/whl/cu128", format!("CUDA 12.8 for {name}"))),
        Some((name, drv)) if *drv >= 528.0 => Some(("https://download.pytorch.org/whl/cu126", format!("CUDA 12.6 for {name}"))),
        Some((name, _)) => Some(("https://download.pytorch.org/whl/cpu", format!("CPU only: the driver of {name} is too old for CUDA 12; update it and run Install again for GPU speed"))),
        None => Some(("https://download.pytorch.org/whl/cpu", "CPU only (no NVIDIA GPU found; speech takes longer)".to_string())),
    }
}

fn uv_command(root: &Path, uv: &Path) -> Command {
    let mut c = command(uv);
    // everything on the chosen drive: Python, packages, uv's download cache
    c.env("UV_CACHE_DIR", root.join("cache"))
        .env("UV_PYTHON_INSTALL_DIR", root.join("python"))
        .env("UV_PYTHON_PREFERENCE", "only-managed")
        .env("UV_NO_PROGRESS", "1")
        .env("UV_HTTP_TIMEOUT", "300")
        .env("NO_COLOR", "1")
        .env_remove("VIRTUAL_ENV")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH");
    c
}

fn setup_runtime(root: &Path) -> Result<()> {
    let uv = ensure_uv(root)?;
    let py = env_python(root);
    set_step(&format!("Installing Python {PYTHON_VERSION}"), None);
    run_logged(uv_command(root, &uv).args(["python", "install", PYTHON_VERSION]), "installing Python", None)?;
    run_logged(uv_command(root, &uv).arg("venv").arg(root.join("env")).args(["--python", PYTHON_VERSION, "--allow-existing"]), "creating the environment", None)?;
    let mut torch = uv_command(root, &uv);
    torch.args(["pip", "install", "--python"]).arg(&py).args(TORCH);
    match torch_index() {
        Some((index, what)) => {
            set_step(&format!("Installing PyTorch, {what} (about 2.5 GB)"), None);
            torch.args(["--index-url", index]);
        }
        None => set_step("Installing PyTorch (about 1 GB)", None),
    }
    run_logged(&mut torch, "installing PyTorch", Some(&root.join("cache")))?;
    set_step("Installing OmniVoice", None);
    run_logged(uv_command(root, &uv).args(["pip", "install", "--python"]).arg(&py).args(["omnivoice", "soundfile"]), "installing omnivoice", Some(&root.join("cache")))?;
    set_step("Checking the installation", None);
    write_worker(root)?;
    let out = worker_command(&py, root).arg(root.join(WORKER_FILE)).arg("--check").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().context("starting Python")?;
    let reply = String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("@@MC ").and_then(|j| serde_json::from_str::<Value>(j).ok()));
    match reply {
        Some(v) if v["ok"] == true => {
            log_line(&format!("PyTorch {} on {}{}", v["torch"].as_str().unwrap_or("?"), v["device"].as_str().unwrap_or("?"), v["gpu"].as_str().map(|g| format!(" ({g})")).unwrap_or_default()));
            std::fs::write(ready_marker(root), serde_json::to_string_pretty(&v)?)?;
            Ok(())
        }
        other => {
            let err = String::from_utf8_lossy(&out.stderr).to_string();
            let why = other.and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no reply").to_string());
            bail!("OmniVoice was installed but doesn't start: {why}")
        }
    }
}

/// Runs a setup command, streaming its output into the install log. `watch` is a folder whose
/// growing size shows download progress (uv prints nothing while it downloads PyTorch).
fn run_logged(cmd: &mut Command, what: &str, watch: Option<&Path>) -> Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().with_context(|| format!("{what}: couldn't start"))?;
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let mut readers = vec![];
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)].into_iter().flatten() {
        let tail = tail.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(|l| l.ok()) {
                log_line(&line);
                let mut t = tail.lock();
                t.push_back(line);
                if t.len() > 12 {
                    t.pop_front();
                }
            }
        }));
    }
    let t0 = Instant::now();
    let base = watch.map(dir_size).unwrap_or(0);
    let mut last_watch = Instant::now();
    let st = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if CANCEL_INSTALL.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("cancelled");
        }
        if let Some(w) = watch {
            if last_watch.elapsed() > Duration::from_secs(3) {
                last_watch = Instant::now();
                let got = dir_size(w).saturating_sub(base);
                let el = t0.elapsed().as_secs();
                set_status(format!("{what}: {} downloaded, {}:{:02}", crate::library::fmt_bytes(got), el / 60, el % 60), None);
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    for r in readers {
        let _ = r.join();
    }
    if !st.success() {
        let t = tail.lock();
        let why: Vec<&str> = t.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        bail!("{what} failed: {}", why.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | "));
    }
    Ok(())
}

fn dir_size(d: &Path) -> u64 {
    fn walk(d: &Path, depth: usize) -> u64 {
        let Ok(rd) = std::fs::read_dir(d) else { return 0 };
        rd.flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() && depth < 12 => walk(&e.path(), depth + 1),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    walk(d, 0)
}

fn write_worker(root: &Path) -> Result<()> {
    let p = root.join(WORKER_FILE);
    if std::fs::read_to_string(&p).ok().as_deref() != Some(WORKER_PY) {
        std::fs::write(&p, WORKER_PY).with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- the worker process

fn worker_command(py: &Path, root: &Path) -> Command {
    let mut c = command(py);
    c.env("HF_HOME", root.join("hf"))
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("PYTHONUNBUFFERED", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .current_dir(root);
    c
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Value>,
    err: Arc<Mutex<VecDeque<String>>>,
    key: String,
    device: String,
    last: Instant,
    next_id: u64,
}

impl Worker {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
    fn stderr_tail(&self) -> String {
        let e = self.err.lock();
        let lines: Vec<&str> = e.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        // the exception line of a traceback says the most
        lines.iter().rev().find(|l| l.contains("Error") || l.contains("error")).or(lines.last()).map(|s| s.to_string()).unwrap_or_default()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct WorkerSlot(Mutex<Option<Worker>>);

impl WorkerSlot {
    /// What the editor shows: whether a model is loaded and on which device.
    fn summary(&self) -> Value {
        match self.0.try_lock() {
            Some(mut g) => {
                let alive = g.as_mut().is_some_and(|w| w.alive());
                match g.as_ref() {
                    Some(w) if alive => json!({"loaded": true, "device": w.device, "idle_s": w.last.elapsed().as_secs()}),
                    _ => json!({"loaded": false}),
                }
            }
            // busy with a request
            None => json!({"loaded": true, "busy": true}),
        }
    }
}

fn worker_slot() -> &'static WorkerSlot {
    static W: OnceLock<WorkerSlot> = OnceLock::new();
    W.get_or_init(|| {
        // unload after a while without work, so the GPU memory comes back
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_secs(30));
            if let Some(mut g) = worker_slot().0.try_lock() {
                if g.as_ref().is_some_and(|w| w.last.elapsed() > IDLE_UNLOAD) {
                    *g = None;
                }
            }
        });
        WorkerSlot(Mutex::new(None))
    })
}

pub fn stop_worker() {
    *worker_slot().0.lock() = None;
}

fn start_worker(f: &Found, status: &dyn Fn(String)) -> Result<Worker> {
    let py = f.python.clone().ok_or_else(|| anyhow!("OmniVoice's Python environment isn't set up"))?;
    let model = f.model.clone().ok_or_else(|| anyhow!("the OmniVoice model wasn't found"))?;
    let root = f.root.clone().unwrap_or_else(|| crate::config::ensure_data_dir().join("omnivoice"));
    std::fs::create_dir_all(&root)?;
    write_worker(&root)?;
    status("loading the OmniVoice model (takes a moment the first time)".into());
    let mut cmd = worker_command(&py, &root);
    cmd.arg(root.join(WORKER_FILE)).arg("--model").arg(&model);
    if let Ok(d) = std::env::var("MOODCUT_OMNIVOICE_DEVICE") {
        cmd.args(["--device", &d]);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().with_context(|| format!("starting {}", py.display()))?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            if let Some(v) = line.strip_prefix("@@MC ").and_then(|j| serde_json::from_str::<Value>(j).ok()) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let err = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let err2 = err.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            let mut e = err2.lock();
            e.push_back(line);
            if e.len() > 60 {
                e.pop_front();
            }
        }
    });
    let mut w = Worker { child, stdin, rx, err, key: format!("{}|{}", py.display(), model.display()), device: String::new(), last: Instant::now(), next_id: 1 };
    // loading 3 GB from a hard disk and starting CUDA can take minutes
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        if CANCEL_TASK.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        match w.rx.recv_timeout(Duration::from_millis(400)) {
            Ok(v) if v["ready"] == true => {
                w.device = v["device"].as_str().unwrap_or("").to_string();
                return Ok(w);
            }
            Ok(v) if v["ready"] == false => bail!("OmniVoice didn't load: {}", v["error"].as_str().unwrap_or("unknown error")),
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("OmniVoice took too long to load"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(300));
                bail!("OmniVoice stopped while loading: {}", w.stderr_tail());
            }
        }
    }
}

/// Sends one request to the worker (starting it when needed) and waits for its reply.
fn request(f: &Found, mut req: Value, status: &dyn Fn(String)) -> Result<Value> {
    let mut slot = worker_slot().0.lock();
    let key = format!("{}|{}", f.python.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), f.model.as_ref().map(|p| p.display().to_string()).unwrap_or_default());
    if !slot.as_mut().is_some_and(|w| w.key == key && w.alive()) {
        *slot = None;
        *slot = Some(start_worker(f, status)?);
    }
    let w = slot.as_mut().expect("started");
    let id = w.next_id;
    w.next_id += 1;
    req["id"] = json!(id);
    let line = format!("{req}\n");
    if w.stdin.write_all(line.as_bytes()).and_then(|_| w.stdin.flush()).is_err() {
        let why = w.stderr_tail();
        *slot = None;
        bail!("OmniVoice stopped: {why}");
    }
    loop {
        if CANCEL_TASK.load(Ordering::Relaxed) {
            // the only way to stop a generation midway
            *slot = None;
            bail!("cancelled");
        }
        match w.rx.recv_timeout(Duration::from_millis(400)) {
            Ok(v) if v["id"] == json!(id) => {
                w.last = Instant::now();
                if v["ok"] == true {
                    return Ok(v);
                }
                bail!("{}", v["error"].as_str().unwrap_or("OmniVoice failed"));
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(300));
                let why = w.stderr_tail();
                *slot = None;
                bail!("OmniVoice stopped: {why}");
            }
        }
    }
}

// ---------------------------------------------------------------- tasks: clone and generate

fn enqueue(eng: Arc<Engine>, kind: &'static str, label: String, req: Value) {
    let start = {
        let mut s = shared().lock();
        s.next_task += 1;
        let id = s.next_task;
        s.tasks.push(Task { id, kind, label, state: "queued", status: "waiting".into(), error: None, result: None, secs: 0.0, req });
        // keep the list short: drop the oldest finished ones
        while s.tasks.len() > 30 {
            match s.tasks.iter().position(|t| t.state == "done" || t.state == "error") {
                Some(i) => {
                    s.tasks.remove(i);
                }
                None => break,
            }
        }
        !std::mem::replace(&mut s.runner, true)
    };
    if start {
        std::thread::spawn(move || run_tasks(eng));
    }
}

fn run_tasks(eng: Arc<Engine>) {
    loop {
        let next = {
            let mut s = shared().lock();
            match s.tasks.iter_mut().find(|t| t.state == "queued") {
                Some(t) => {
                    t.state = "running";
                    t.status = "starting".into();
                    Some((t.id, t.kind, t.req.clone()))
                }
                None => {
                    s.runner = false;
                    None
                }
            }
        };
        let Some((id, kind, req)) = next else { return };
        CANCEL_TASK.store(false, Ordering::Relaxed);
        notify(&eng, "voice task started");
        let t0 = Instant::now();
        let status = move |m: String| {
            if let Some(t) = shared().lock().tasks.iter_mut().find(|t| t.id == id) {
                t.status = m;
            }
        };
        let f = found();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| if kind == "clone" { clone_voice(&eng, &f, &req, &status) } else { generate(&eng, &f, &req, &status) }))
            .unwrap_or_else(|p| Err(anyhow!("internal error: {}", crate::engine::panic_message(&*p))));
        let text = {
            let mut s = shared().lock();
            let Some(t) = s.tasks.iter_mut().find(|t| t.id == id) else { continue };
            t.secs = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;
            match r {
                Ok(v) => {
                    t.state = "done";
                    t.status = v["text"].as_str().unwrap_or("done").to_string();
                    t.result = Some(v);
                }
                Err(e) => {
                    t.state = "error";
                    t.error = Some(format!("{e:#}"));
                    t.status = "failed".into();
                }
            }
            format!("{}: {}", t.label, t.error.clone().unwrap_or_else(|| t.status.clone()))
        };
        notify(&eng, text);
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn slug(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.chars().count() >= max {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "voice".into()
    } else {
        out
    }
}

/// Only ASCII in ids: they are folder names and travel in URLs.
fn id_slug(s: &str) -> String {
    let s: String = slug(s, 24).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "voice".into()
    } else {
        s
    }
}

fn first_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut s = words.iter().take(n).cloned().collect::<Vec<_>>().join(" ");
    if words.len() > n {
        s.push('…');
    }
    s
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    p
}

fn load_voice(root: &Path, id: &str) -> Result<Value> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("pick a voice");
    }
    let dir = voices_dir(root).join(id);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("voice.json")).with_context(|| format!("voice '{id}' not found"))?)?;
    if !dir.join("prompt.pt").is_file() {
        bail!("voice '{id}' is incomplete: clone it again");
    }
    Ok(v)
}

fn list_voices(root: &Path) -> Vec<Value> {
    let Ok(rd) = std::fs::read_dir(voices_dir(root)) else { return vec![] };
    let mut out: Vec<Value> = rd
        .flatten()
        .filter(|e| e.path().join("prompt.pt").is_file())
        .filter_map(|e| {
            let mut v: Value = serde_json::from_str(&std::fs::read_to_string(e.path().join("voice.json")).ok()?).ok()?;
            v["id"] = json!(e.file_name().to_string_lossy());
            Some(v)
        })
        .collect();
    out.sort_by(|a, b| b["created"].as_u64().cmp(&a["created"].as_u64()));
    out
}

/// Clones a voice: cuts the reference audio (a library clip's in/out, or a file), encodes it
/// with OmniVoice once and keeps it as `voices/<id>/prompt.pt` for every later generation.
fn clone_voice(eng: &Engine, f: &Found, req: &Value, status: &dyn Fn(String)) -> Result<Value> {
    let root = f.root.clone().ok_or_else(|| anyhow!("no OmniVoice folder to keep voices in: run Install once"))?;
    if req["consent"] != true {
        bail!("confirm that you have the speaker's permission to clone this voice");
    }
    let name = req["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| anyhow!("give the voice a name"))?.to_string();
    let (src, src_label) = match (req["media"].as_str().filter(|m| !m.is_empty()), req["path"].as_str().filter(|p| !p.trim().is_empty())) {
        (Some(id), _) => {
            let m = eng.media(id).ok_or_else(|| anyhow!("unknown media {id}"))?;
            if !m.has_audio {
                bail!("{id} ({}) has no audio", m.name);
            }
            (m.path.clone(), format!("{} {}", m.id, m.name))
        }
        (None, Some(p)) => (p.trim().to_string(), p.trim().to_string()),
        _ => bail!("pick the reference audio: a clip from the project or a file"),
    };
    eng.check_path(&src)?;
    if !Path::new(&src).is_file() {
        bail!("{src} not found");
    }
    let start = req["start"].as_f64().unwrap_or(0.0).max(0.0);
    let end = req["end"].as_f64().filter(|e| *e > start);
    let len = end.map(|e| e - start).unwrap_or(MAX_REF_SECS).min(MAX_REF_SECS);
    if len < 1.0 {
        bail!("the reference is too short: use 3 to 10 seconds of clear speech");
    }
    let id = format!("{}-{}", id_slug(&name), format!("{:x}", now_secs()).chars().rev().take(5).collect::<String>());
    let dir = voices_dir(&root).join(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let result = (|| -> Result<Value> {
        status("preparing the reference audio".into());
        let wav = dir.join("ref.wav");
        let (ss, t) = (format!("{start:.3}"), format!("{len:.3}"));
        let mut args: Vec<String> = ["-y", "-ss", ss.as_str(), "-t", t.as_str(), "-i", src.as_str(), "-vn", "-sn", "-ac", "1", "-ar", "24000", "-c:a", "pcm_s16le"].iter().map(|s| s.to_string()).collect();
        args.push(wav.to_string_lossy().to_string());
        eng.ff.run_capture(&args).context("cutting the reference audio")?;
        // 24 kHz 16-bit mono: 48 KB a second
        let secs = file_len(&wav).saturating_sub(44) as f64 / 48_000.0;
        if secs < 1.0 {
            bail!("the reference has no usable audio at {start:.1}s");
        }
        let given = req["ref_text"].as_str().map(str::trim).filter(|t| !t.is_empty());
        status(if given.is_some() { "encoding the voice".into() } else { "transcribing and encoding the voice (Whisper is downloaded the first time, about 1.6 GB)".into() });
        let r = request(f, json!({"op": "clone", "ref_audio": wav, "ref_text": given, "out": dir.join("prompt.pt")}), status)?;
        let v = json!({
            "name": name, "ref_text": r["ref_text"].as_str().unwrap_or(given.unwrap_or("")), "source": src_label,
            "start": start, "dur": (secs * 100.0).round() / 100.0, "created": now_secs(), "model": REPO,
        });
        std::fs::write(dir.join("voice.json"), serde_json::to_string_pretty(&v)?)?;
        Ok(json!({"voice": id, "text": format!("voice \"{name}\" cloned from {:.1}s of {src_label}", secs)}))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// Speaks a script with a cloned voice: one WAV per paragraph (or one for all), written next to
/// the project's graphics, then imported (tag `voice`) and optionally placed, as one edit.
fn generate(eng: &Engine, f: &Found, req: &Value, status: &dyn Fn(String)) -> Result<Value> {
    let root = f.root.clone().ok_or_else(|| anyhow!("no OmniVoice folder: run Install once"))?;
    let vid = req["voice"].as_str().unwrap_or("");
    let voice = load_voice(&root, vid)?;
    let vname = voice["name"].as_str().unwrap_or(vid).to_string();
    let text = req["text"].as_str().unwrap_or("").replace("\r\n", "\n");
    let parts: Vec<String> = if req["split"] == true {
        text.split("\n\n").map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|p| !p.is_empty()).collect()
    } else {
        let all = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if all.is_empty() {
            vec![]
        } else {
            vec![all]
        }
    };
    if parts.is_empty() {
        bail!("write the script to speak");
    }
    // next to the project's graphics and sound effects (the data folder until it is saved)
    let gdir = crate::gfx::graphics_dir(eng.st.read().path.as_deref());
    let dir = gdir.parent().map(|d| d.join("voice")).unwrap_or_else(|| gdir.join("voice"));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let prompt = voices_dir(&root).join(vid).join("prompt.pt");
    let language = req["language"].as_str().filter(|l| !l.is_empty() && *l != "auto");
    let speed = req["speed"].as_f64().filter(|s| (0.5..=2.0).contains(s) && (*s - 1.0).abs() > 1e-3);
    let steps = req["steps"].as_u64().filter(|s| (4..=64).contains(s));
    let mut outs: Vec<(PathBuf, f64, String)> = vec![];
    for (i, p) in parts.iter().enumerate() {
        status(if parts.len() > 1 { format!("speaking part {} of {}", i + 1, parts.len()) } else { "speaking".into() });
        let out = unique_path(&dir, &format!("{}-{}", slug(&vname, 20), slug(&first_words(p, 5), 32)), "wav");
        let r = request(f, json!({"op": "generate", "voice": prompt, "text": p, "language": language, "speed": speed, "num_step": steps, "out": out}), status)?;
        outs.push((out, r["dur"].as_f64().unwrap_or(0.0), p.clone()));
    }
    status("importing".into());
    let q = crate::ops::lex::quote;
    let mut at = req["at"].as_f64().filter(|t| *t >= 0.0);
    let mut lines = vec![];
    for (path, dur, p) in &outs {
        let file = path.to_string_lossy().to_string();
        lines.push(format!("import {} tag=voice as={}", q(&file), q(&format!("{vname}: {}", first_words(p, 6)))));
        let note: String = format!("AI voice ({vname}, OmniVoice): {p}").chars().take(600).collect();
        lines.push(format!("note $ {}", q(&note)));
        if let Some(t) = at {
            lines.push(format!("add {} at={:.3}", q(&file), t));
            at = Some(t + dur + 0.25);
        }
    }
    eng.edit(&lines.join("\n"), "gui")?;
    let p = eng.st.read().p.clone();
    let ids: Vec<String> = outs.iter().filter_map(|(path, _, _)| p.media.iter().find(|m| crate::ops::exec::same_path(&m.path, &path.to_string_lossy())).map(|m| m.id.clone())).collect();
    let total: f64 = outs.iter().map(|o| o.1).sum();
    Ok(json!({
        "media": ids,
        "files": outs.iter().map(|o| o.0.to_string_lossy().to_string()).collect::<Vec<_>>(),
        "dur": (total * 100.0).round() / 100.0,
        "text": format!("{} clip{} ({:.1}s) in the project{}", outs.len(), if outs.len() == 1 { "" } else { "s" }, total, if ids.is_empty() { String::new() } else { format!(": {}", ids.join(", ")) }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_ids() {
        assert_eq!(slug("Hello, World!", 40), "hello-world");
        assert_eq!(slug("  ", 40), "voice");
        assert_eq!(id_slug("Narrator (warm)"), "narrator-warm");
        assert!(id_slug("Ünïcödé").chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert_eq!(first_words("one two three four", 2), "one two…");
        assert_eq!(first_words("one two", 5), "one two");
    }

    #[test]
    fn a_model_folder_needs_its_weights_and_audio_tokenizer() {
        let d = std::env::temp_dir().join(format!("moodcut-omnivoice-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("audio_tokenizer")).unwrap();
        std::fs::write(d.join("config.json"), "{}").unwrap();
        std::fs::write(d.join("tokenizer.json"), "{}").unwrap();
        assert!(!is_model_dir(&d), "no weights");
        assert!(model_in(&d).is_none());
        // a Hugging Face cache without a complete snapshot isn't a model either
        std::fs::create_dir_all(d.join("models--k2-fsa--OmniVoice").join("snapshots").join("abc")).unwrap();
        assert!(model_in(&d).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn worker_script_is_embedded() {
        assert!(WORKER_PY.contains("create_voice_clone_prompt") && WORKER_PY.contains("@@MC "));
    }
}
