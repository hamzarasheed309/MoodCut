//! The OmniVoice worker process. It starts on first use (repairing an environment whose folder
//! was moved, and adding packages newer MoodCut versions use), answers one request at a time
//! with progress messages, and is stopped after ten idle minutes, which frees the GPU memory.

use super::*;

/// Progress for a task: (fraction 0..1 when known, what is happening).
pub(super) type Report<'a> = &'a dyn Fn(Option<f64>, String);

/// A request that failed in the worker (or the worker itself), with what it said, so the error
/// can be explained in plain words and its details shown on request.
#[derive(Debug)]
pub(super) struct WorkerFailure {
    pub message: String,
    pub etype: String,
    pub detail: String,
}

impl std::fmt::Display for WorkerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.etype.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{} ({})", self.message, self.etype)
        }
    }
}

impl std::error::Error for WorkerFailure {}

pub(super) fn worker_command(py: &Path, root: &Path) -> Command {
    // below-normal priority: the editor and Windows stay responsive while it speaks
    let mut c = crate::render::ffmpeg::background_command(py);
    c.env("HF_HOME", root.join("hf"))
        .env("HF_HUB_DISABLE_TELEMETRY", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("PYTHONUNBUFFERED", "1")
        .env("TOKENIZERS_PARALLELISM", "false")
        // less fragmentation on small GPUs (6 GB laptop cards)
        .env("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")
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
    vram: Value,
}

impl Worker {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
    /// The last lines Python wrote to stderr (a traceback ends with the exception).
    fn stderr_tail(&self, n: usize) -> String {
        let e = self.err.lock();
        let lines: Vec<&str> = e.iter().map(|s| s.trim_end()).filter(|s| !s.trim().is_empty() && !s.contains("Redirects are currently not supported")).collect();
        lines[lines.len().saturating_sub(n)..].join("\n")
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Slot(Mutex<Option<Worker>>);

fn slot() -> &'static Slot {
    static W: OnceLock<Slot> = OnceLock::new();
    W.get_or_init(|| {
        // unload after a while without work, so the GPU memory comes back
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_secs(30));
            if let Some(mut g) = slot().0.try_lock() {
                if g.as_ref().is_some_and(|w| w.last.elapsed() > IDLE_UNLOAD) {
                    *g = None;
                }
            }
        });
        Slot(Mutex::new(None))
    })
}

/// What the editor shows: whether a model is loaded, on which device, and its GPU memory.
pub(super) fn summary() -> Value {
    match slot().0.try_lock() {
        Some(mut g) => {
            let alive = g.as_mut().is_some_and(|w| w.alive());
            match g.as_ref() {
                Some(w) if alive => json!({"loaded": true, "device": w.device, "idle_s": w.last.elapsed().as_secs(), "vram": w.vram}),
                _ => json!({"loaded": false}),
            }
        }
        // busy with a request
        None => json!({"loaded": true, "busy": true}),
    }
}

/// Stops the worker (waits for a request that is running).
pub(super) fn stop() {
    *slot().0.lock() = None;
}

/// Stops the worker unless it is busy; true when it is stopped.
pub(super) fn unload() -> bool {
    match slot().0.try_lock() {
        Some(mut w) => {
            *w = None;
            true
        }
        None => false,
    }
}

/// Python that can't start: its launcher points at a folder that was moved, or files are gone.
pub(super) fn launcher_broken(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["trampoline", "no python at", "entity not found", "cannot find the file", "cannot find the path", "os error 2)", "os error 3)", "program not found"].iter().any(|p| t.contains(p))
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

/// Runs a uv command for a repair or an update; its last lines become the error.
fn run_uv(cmd: &mut Command, what: &str) -> Result<()> {
    let out = cmd.stdin(Stdio::null()).output().with_context(|| format!("{what}: couldn't start uv"))?;
    if !out.status.success() {
        bail!("{what} failed: {}", last_lines(&String::from_utf8_lossy(&out.stderr), 3));
    }
    Ok(())
}

/// Recreates the environment's launchers next to its Python (after the folder was moved or the
/// managed Python went missing). Packages stay; nothing large is downloaded.
pub(super) fn repair_launchers(root: &Path, report: Report) -> Result<()> {
    report(None, "repairing OmniVoice's Python setup".into());
    let uv = ensure_uv(root)?;
    run_uv(uv_command(root, &uv).args(["python", "install", PYTHON_VERSION, "--no-bin", "--no-registry"]), "installing Python")?;
    run_uv(uv_command(root, &uv).arg("venv").arg(root.join("env")).args(["--python", PYTHON_VERSION, "--allow-existing"]), "repairing the environment")?;
    Ok(())
}

/// Packages newer MoodCut versions use (num2words), added once to an older setup. Optional:
/// without them numbers just aren't spelled out.
fn ensure_current(f: &Found, report: Report) {
    let Some(root) = f.root.as_deref() else { return };
    let py = env_python(root);
    if f.python.as_deref() != Some(py.as_path()) {
        return; // someone's own Python (MOODCUT_OMNIVOICE_PYTHON)
    }
    let Some(mut marker) = runtime_of(root) else { return };
    if marker["moodcut_rev"].as_u64().unwrap_or(1) >= RUNTIME_REV {
        return;
    }
    report(None, "updating OmniVoice's packages (one time, a few seconds)".into());
    let r = ensure_uv(root).and_then(|uv| run_uv(uv_command(root, &uv).args(["pip", "install", "--python"]).arg(&py).args(EXTRAS), "adding packages"));
    if r.is_ok() {
        marker["moodcut_rev"] = json!(RUNTIME_REV);
        let _ = std::fs::write(ready_marker(root), serde_json::to_string_pretty(&marker).unwrap_or_default());
    }
}

fn spawn(f: &Found, report: Report) -> Result<Worker> {
    let py = f.python.clone().ok_or_else(|| anyhow!("SETUP: OmniVoice's Python environment isn't set up"))?;
    let model = f.model.clone().ok_or_else(|| anyhow!("SETUP: the OmniVoice model wasn't found"))?;
    let root = f.root.clone().unwrap_or_else(|| crate::config::ensure_data_dir().join("omnivoice"));
    std::fs::create_dir_all(&root)?;
    write_worker(&root)?;
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
            if e.len() > 80 {
                e.pop_front();
            }
        }
    });
    let mut w = Worker { child, stdin, rx, err, key: format!("{}|{}", py.display(), model.display()), device: String::new(), last: Instant::now(), next_id: 1, vram: Value::Null };
    // loading 3 GB from a hard disk and starting CUDA can take minutes
    let t0 = Instant::now();
    let mut stage = "loading the OmniVoice model".to_string();
    loop {
        if CANCEL_TASK.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        match w.rx.recv_timeout(Duration::from_millis(400)) {
            Ok(v) if v["ready"] == true => {
                w.device = v["device"].as_str().unwrap_or("").to_string();
                w.vram = json!({"used_gb": v["vram_used_gb"], "total_gb": v["vram_total_gb"]});
                return Ok(w);
            }
            Ok(v) if v["ready"] == false => {
                let detail = format!("{}\n{}", v["trace"].as_str().unwrap_or(""), w.stderr_tail(12));
                return Err(WorkerFailure { message: format!("OmniVoice didn't load: {}", v["error"].as_str().unwrap_or("unknown error")), etype: v["etype"].as_str().unwrap_or("").into(), detail }.into());
            }
            Ok(v) => {
                if let Some(s) = v["stage"].as_str() {
                    stage = s.to_string();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) if t0.elapsed() < Duration::from_secs(900) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("LOAD_TIMEOUT: OmniVoice took too long to load"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(300));
                return Err(WorkerFailure { message: "OmniVoice stopped while loading".into(), etype: String::new(), detail: w.stderr_tail(20) }.into());
            }
        }
        report(None, format!("{stage} · {} s (15-60 s the first time)", t0.elapsed().as_secs()));
    }
}

/// Starts the worker; a Python that can't start because its folder moved is repaired once.
fn start(f: &Found, report: Report) -> Result<Worker> {
    ensure_current(f, report);
    match spawn(f, report) {
        Ok(w) => Ok(w),
        Err(e) => {
            let text = format!("{e:#} {}", e.downcast_ref::<WorkerFailure>().map(|w| w.detail.as_str()).unwrap_or(""));
            let ours = f.root.as_deref().filter(|r| f.python.as_deref() == Some(env_python(r).as_path()));
            match ours {
                Some(root) if launcher_broken(&text) && !CANCEL_TASK.load(Ordering::Relaxed) => {
                    repair_launchers(root, report).with_context(|| format!("{e:#}"))?;
                    spawn(f, report)
                }
                _ => Err(e),
            }
        }
    }
}

/// Sends one request to the worker (starting it when needed) and waits for its reply, passing
/// the worker's progress messages on.
pub(super) fn request(f: &Found, mut req: Value, report: Report) -> Result<Value> {
    let mut slot = slot().0.lock();
    let key = format!("{}|{}", f.python.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), f.model.as_ref().map(|p| p.display().to_string()).unwrap_or_default());
    if !slot.as_mut().is_some_and(|w| w.key == key && w.alive()) {
        *slot = None;
        *slot = Some(start(f, report)?);
    }
    let w = slot.as_mut().expect("started");
    let id = w.next_id;
    w.next_id += 1;
    req["id"] = json!(id);
    let line = format!("{req}\n");
    if w.stdin.write_all(line.as_bytes()).and_then(|_| w.stdin.flush()).is_err() {
        let detail = w.stderr_tail(20);
        *slot = None;
        return Err(WorkerFailure { message: "OmniVoice stopped unexpectedly".into(), etype: String::new(), detail }.into());
    }
    loop {
        if CANCEL_TASK.load(Ordering::Relaxed) {
            // the only way to stop a generation midway
            *slot = None;
            bail!("cancelled");
        }
        match w.rx.recv_timeout(Duration::from_millis(400)) {
            Ok(v) if v["id"] == json!(id) && v.get("ok").is_some() => {
                w.last = Instant::now();
                if v.get("vram_used_gb").is_some() {
                    w.vram = json!({"used_gb": v["vram_used_gb"], "total_gb": v["vram_total_gb"], "peak_gb": v["peak_gb"]});
                }
                if v["ok"] == true {
                    return Ok(v);
                }
                return Err(WorkerFailure { message: v["error"].as_str().unwrap_or("OmniVoice failed").to_string(), etype: v["etype"].as_str().unwrap_or("").to_string(), detail: v["trace"].as_str().unwrap_or("").to_string() }.into());
            }
            Ok(v) if v["id"] == json!(id) => {
                if let Some(p) = v["progress"].as_f64() {
                    report(Some(p), v["stage"].as_str().unwrap_or("working").to_string());
                }
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(300));
                let detail = w.stderr_tail(20);
                *slot = None;
                return Err(WorkerFailure { message: "OmniVoice stopped unexpectedly".into(), etype: String::new(), detail }.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_environment_is_recognized() {
        assert!(launcher_broken("error: uv trampoline failed to spawn Python child process\n  Caused by: entity not found (os error 2)"));
        assert!(launcher_broken("No Python at '\"C:\\MoodCut\\OmniVoice\\python\\cpython-3.12\\python.exe'"));
        assert!(launcher_broken("starting D:\\x\\python.exe: The system cannot find the file specified. (os error 2)"));
        assert!(!launcher_broken("CUDA out of memory. Tried to allocate 12.00 MiB"));
        assert!(!launcher_broken("ModuleNotFoundError: No module named 'omnivoice'"));
    }

    #[test]
    fn failures_keep_the_worker_details() {
        let e: anyhow::Error = WorkerFailure { message: "boom".into(), etype: "ValueError".into(), detail: "Traceback...".into() }.into();
        assert_eq!(format!("{e:#}"), "boom (ValueError)");
        assert_eq!(e.downcast_ref::<WorkerFailure>().map(|w| w.detail.as_str()), Some("Traceback..."));
    }
}
