//! Cloning and speaking as queued tasks: one at a time, with progress, errors explained in plain
//! words with what to do about them, and the `voice` MCP tool built on the same tasks, so the
//! editor and AI apps see and control the same work.

use super::worker::{self, Report, WorkerFailure};
use super::*;

/// Reference audio scanned for its best part when no end is given.
const SCAN_SECS: f64 = 30.0;

pub(super) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(super) fn slug(s: &str, max: usize) -> String {
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
pub(super) fn id_slug(s: &str) -> String {
    let s: String = slug(s, 24).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "voice".into()
    } else {
        s
    }
}

pub(super) fn first_words(text: &str, n: usize) -> String {
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

pub(super) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v[k].as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn transcript_of(req: &Value) -> Option<String> {
    str_of(req, "transcript").or_else(|| str_of(req, "ref_text")).map(str::to_string)
}

pub(super) fn list_voices(root: &Path) -> Vec<Value> {
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

/// A voice by id or by name (any case).
pub(super) fn resolve_voice(root: &Path, key: &str) -> Result<(String, Value)> {
    let key = key.trim();
    if key.is_empty() {
        bail!("pick a voice");
    }
    if valid_id(key) {
        let dir = voices_dir(root).join(key);
        if dir.join("prompt.pt").is_file() {
            if let Some(v) = std::fs::read_to_string(dir.join("voice.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                return Ok((key.to_string(), v));
            }
        }
    }
    let all = list_voices(root);
    if let Some(v) = all.iter().find(|v| v["name"].as_str().is_some_and(|n| n.trim().eq_ignore_ascii_case(key))) {
        return Ok((v["id"].as_str().unwrap_or("").to_string(), v.clone()));
    }
    if all.is_empty() {
        bail!("voice '{key}' not found: no voice is cloned yet");
    }
    bail!("voice '{key}' not found. Voices: {}", all.iter().map(|v| format!("{} ({})", v["id"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "))
}

// ---------------------------------------------------------------- errors people can act on

/// An error as the editor shows it: what happened, what to do, which button helps, and the
/// technical details for "Details".
pub(super) struct Explained {
    pub message: String,
    pub hint: Option<String>,
    pub fix: Option<&'static str>,
    pub detail: String,
}

pub(super) fn explain(e: &anyhow::Error) -> Explained {
    let raw = format!("{e:#}");
    let detail = match e.downcast_ref::<WorkerFailure>() {
        Some(w) if !w.detail.trim().is_empty() => format!("{raw}\n\n{}", w.detail.trim()),
        _ => raw.clone(),
    };
    let t = detail.to_ascii_lowercase();
    let has = |ps: &[&str]| ps.iter().any(|p| t.contains(p));
    let x = |m: &str, h: &str, fix: Option<&'static str>| Explained { message: m.to_string(), hint: (!h.is_empty()).then(|| h.to_string()), fix, detail: detail.clone() };
    if raw.trim() == "cancelled" || raw.ends_with(": cancelled") {
        return x("Cancelled.", "", None);
    }
    if has(&["out of memory", "outofmemory"]) {
        return x(
            "Your graphics card ran out of memory.",
            "Close other apps that use the GPU (games, video players, other AI tools) and press Retry. A 5-10 second reference and shorter paragraphs need less memory.",
            Some("retry"),
        );
    }
    if has(&["no_speech"]) {
        return x(
            "No clear speech was found in the reference.",
            "Pick a part where one person speaks clearly for 3 to 10 seconds, without music (mark In and Out in the Source monitor), then clone again.",
            None,
        );
    }
    if has(&["setup:"]) {
        return x("OmniVoice isn't set up.", "Open the AI Voice panel and choose Download, or Locate a model you already have.", None);
    }
    if has(&["load_timeout"]) {
        return x(
            "OmniVoice took too long to load.",
            "The model is read from its drive; an external or sleeping drive can be slow. Press Retry; it loads faster the second time.",
            Some("retry"),
        );
    }
    if has(&["no space left", "os error 112", "disk full", "not enough space"]) {
        return x("The drive is full.", "Free some space on the OmniVoice drive and the project's drive, then press Retry.", Some("retry"));
    }
    if has(&["no module named", "modulenotfounderror", "dll load failed", "importerror", "cannot import name"]) {
        return x(
            "Part of OmniVoice's Python setup is missing or broken.",
            "Press Repair: it fixes the setup in a few minutes without downloading the model again.",
            Some("repair"),
        );
    }
    if worker::launcher_broken(&detail) {
        return x(
            "OmniVoice's Python setup can't start.",
            "Its folder may have been moved, or files are missing. Press Repair: it fixes the setup without downloading the model again.",
            Some("repair"),
        );
    }
    if has(&["could not resolve", "failed to resolve", "name resolution", "connection refused", "connection reset", "connection aborted", "timed out", "maxretryerror", "max retries", "temporary failure", "network is unreachable", "curl: (6)", "curl: (7)", "curl: (28)", "sslerror", "ssl: ", "httperror", "too many requests"]) {
        return x(
            "MoodCut couldn't download what it needs.",
            "Check the internet connection and press Retry. Cloning with a typed transcript needs no download.",
            Some("retry"),
        );
    }
    if has(&["cuda error", "no kernel image", "cudnn", "cublas", "nvidia driver", "device-side assert"]) {
        return x(
            "The graphics card reported an error.",
            "Press Retry. If it happens again, update the NVIDIA driver, or Unload and retry.",
            Some("retry"),
        );
    }
    if has(&["cutting the reference", "invalid data found", "does not contain any stream"]) {
        return x(
            "MoodCut couldn't read audio from that file.",
            "Use another file or a clip from the project. Voice notes (WhatsApp .opus/.ogg), MP3, WAV, M4A and videos work.",
            None,
        );
    }
    if has(&["stopped unexpectedly", "stopped while loading", "didn't load"]) {
        return x("OmniVoice stopped unexpectedly.", "Press Retry. If it happens again, press Repair, or open Details to see why.", Some("retry"));
    }
    // our own messages are already written for people: keep them, without internal codes
    let first = raw.lines().next().unwrap_or("").trim().to_string();
    let mut m: String = first.chars().take(300).collect();
    if let Some(c) = m.get(0..1) {
        m = format!("{}{}", c.to_uppercase(), &m[1..]);
    }
    if !m.ends_with('.') && !m.ends_with('?') && !m.ends_with('!') {
        m.push('.');
    }
    x(&m, "Press Retry. If it keeps failing, open Details.", Some("retry"))
}

// ---------------------------------------------------------------- the task queue

/// Checks a request and queues it; problems a person can fix at once are errors right away.
pub(super) fn submit(eng: &Arc<Engine>, kind: &'static str, req: &Value) -> Result<u64> {
    let f = found();
    if !f.ready() {
        bail!("OmniVoice isn't set up yet: open AI Voice and choose Download, or Locate a model you already have");
    }
    let root = f.root.clone().ok_or_else(|| anyhow!("OmniVoice has no folder for voices yet: run its setup once"))?;
    let label = match kind {
        "clone" => {
            let name = str_of(req, "name").ok_or_else(|| anyhow!("give the voice a name"))?;
            if req["consent"] != true {
                bail!("cloning needs the speaker's permission: confirm it only when it is their voice or they agreed");
            }
            if str_of(req, "media").is_none() && str_of(req, "path").is_none() {
                bail!("pick the reference audio: a clip from the project or a file");
            }
            format!("Clone voice \"{name}\"")
        }
        "retranscribe" => {
            let (_, v) = resolve_voice(&root, req["voice"].as_str().unwrap_or(""))?;
            if transcript_of(req).is_none() {
                bail!("type the exact words said in the voice's reference");
            }
            format!("Fix the transcript of \"{}\"", v["name"].as_str().unwrap_or("voice"))
        }
        _ => {
            let (_, v) = resolve_voice(&root, req["voice"].as_str().unwrap_or(""))?;
            let text = req["text"].as_str().unwrap_or("").trim();
            if text.is_empty() {
                bail!("write the script to speak");
            }
            format!("Speak \"{}\" ({})", first_words(text, 8), v["name"].as_str().unwrap_or("voice"))
        }
    };
    Ok(enqueue(eng.clone(), kind, label, req.clone()))
}

/// Runs a finished or failed task again with the same settings.
pub(super) fn retry(eng: &Arc<Engine>, id: u64) -> Result<u64> {
    let (kind, req) = {
        let s = shared().lock();
        let t = s.tasks.iter().find(|t| t.id == id).ok_or_else(|| anyhow!("that task is gone"))?;
        (t.kind, t.req.clone())
    };
    submit(eng, kind, &req)
}

fn enqueue(eng: Arc<Engine>, kind: &'static str, label: String, req: Value) -> u64 {
    let (id, start) = {
        let mut s = shared().lock();
        s.next_task += 1;
        let id = s.next_task;
        s.tasks.push(Task { id, kind, label, state: "queued", status: "waiting for the task before it".into(), pct: None, error: None, hint: None, fix: None, detail: None, result: None, secs: 0.0, req, started: None });
        // keep the list short: drop the oldest finished ones
        while s.tasks.len() > 30 {
            match s.tasks.iter().position(|t| t.state == "done" || t.state == "error") {
                Some(i) => {
                    s.tasks.remove(i);
                }
                None => break,
            }
        }
        (id, !std::mem::replace(&mut s.runner, true))
    };
    if start {
        std::thread::spawn(move || run_tasks(eng));
    }
    id
}

fn run_tasks(eng: Arc<Engine>) {
    loop {
        let next = {
            let mut s = shared().lock();
            match s.tasks.iter_mut().find(|t| t.state == "queued") {
                Some(t) => {
                    t.state = "running";
                    t.status = "starting".into();
                    t.started = Some(Instant::now());
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
        // no fraction (loading the model, a one-time update, a repair): the editor shows a
        // moving bar with the stage and the seconds, not a number that seems stuck
        let report = move |pct: Option<f64>, m: String| {
            if let Some(t) = shared().lock().tasks.iter_mut().find(|t| t.id == id) {
                t.status = m;
                t.pct = pct.map(|p| (p.clamp(0.0, 1.0) * 1000.0).round() / 1000.0);
            }
        };
        let f = found();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
            "clone" => clone_voice(&eng, &f, &req, &report),
            "retranscribe" => retranscribe(&f, &req, &report),
            _ => generate(&eng, &f, &req, &report),
        }))
        .unwrap_or_else(|p| Err(anyhow!("internal error: {}", crate::engine::panic_message(&*p))));
        let text = {
            let mut s = shared().lock();
            let Some(t) = s.tasks.iter_mut().find(|t| t.id == id) else { continue };
            t.secs = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;
            t.started = None;
            match r {
                Ok(v) => {
                    t.state = "done";
                    t.pct = Some(1.0);
                    t.status = v["text"].as_str().unwrap_or("done").to_string();
                    t.result = Some(v);
                }
                Err(e) => {
                    let x = explain(&e);
                    t.state = "error";
                    t.status = if x.message == "Cancelled." { "cancelled".into() } else { "failed".into() };
                    t.error = Some(x.message);
                    t.hint = x.hint;
                    t.fix = x.fix;
                    t.detail = Some(x.detail);
                }
            }
            format!("{}: {}", t.label, t.error.clone().unwrap_or_else(|| t.status.clone()))
        };
        notify(&eng, text);
    }
}

// ---------------------------------------------------------------- clone

/// Clones a voice: cuts the reference (a project clip or a file; without an end, the best 3-12 s
/// of speech in the first 30 s is picked at pauses), cleans it, transcribes it when no words are
/// typed, encodes it once and keeps it as `voices/<id>/` for every later generation.
fn clone_voice(eng: &Engine, f: &Found, req: &Value, report: Report) -> Result<Value> {
    let root = f.root.clone().ok_or_else(|| anyhow!("SETUP: no OmniVoice folder"))?;
    let name = str_of(req, "name").ok_or_else(|| anyhow!("give the voice a name"))?.to_string();
    if req["consent"] != true {
        bail!("cloning needs the speaker's permission");
    }
    let (src, src_label) = match (str_of(req, "media"), str_of(req, "path")) {
        (Some(id), _) => {
            let m = eng.media(id).ok_or_else(|| anyhow!("unknown media {id}"))?;
            if !m.has_audio {
                bail!("{id} ({}) has no audio", m.name);
            }
            (m.path.clone(), format!("{} {}", m.id, m.name))
        }
        (None, Some(p)) => {
            let label = Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string());
            (p.to_string(), label)
        }
        _ => bail!("pick the reference audio: a clip from the project or a file"),
    };
    eng.check_path(&src)?;
    if !Path::new(&src).is_file() {
        bail!("{src} wasn't found");
    }
    let typed = transcript_of(req);
    let start = req["start"].as_f64().unwrap_or(0.0).max(0.0);
    let end = req["end"].as_f64().filter(|e| *e > start);
    // typed words match the part the person marked; otherwise the best part is picked
    let auto = end.is_none() && typed.is_none();
    let len = match end {
        Some(e) => (e - start).min(MAX_REF_SECS),
        None if typed.is_some() => MAX_REF_SECS,
        None => SCAN_SECS,
    };
    if len < 1.0 {
        bail!("the reference is too short: use 3 to 10 seconds of clear speech");
    }
    let id = format!("{}-{}", id_slug(&name), format!("{:x}", now_secs()).chars().rev().take(5).collect::<String>());
    let dir = voices_dir(&root).join(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let result = (|| -> Result<Value> {
        report(Some(0.01), "preparing the reference audio".into());
        let cut = dir.join("source.wav");
        // rumble below the voice goes; noise reduction only when asked (it can dull a clean voice)
        let filters = if req["denoise"] == true { "highpass=f=60,afftdn=nf=-25" } else { "highpass=f=60" };
        let (ss, t) = (format!("{start:.3}"), format!("{len:.3}"));
        let mut args: Vec<String> = ["-y", "-ss", ss.as_str(), "-t", t.as_str(), "-i", src.as_str(), "-vn", "-sn", "-ac", "1", "-ar", "24000", "-af", filters, "-c:a", "pcm_s16le"].iter().map(|s| s.to_string()).collect();
        args.push(cut.to_string_lossy().to_string());
        eng.ff.run_capture(&args).context("cutting the reference audio")?;
        // 24 kHz 16-bit mono: 48 KB a second
        let secs = file_len(&cut).saturating_sub(44) as f64 / 48_000.0;
        if secs < 1.0 {
            bail!("NO_SPEECH: the reference has no usable audio at {start:.1} s");
        }
        let scaled = |p: Option<f64>, m: String| report(p.map(|p| 0.05 + 0.93 * p), m);
        let r = worker::request(f, json!({"op": "clone", "ref_audio": cut, "ref_text": typed, "out": dir.join("prompt.pt"), "auto_window": auto, "ref_out": dir.join("ref.wav")}), &scaled)?;
        let _ = std::fs::remove_file(&cut);
        let used0 = start + r["used"][0].as_f64().unwrap_or(0.0);
        let dur = r["ref_s"].as_f64().unwrap_or(secs);
        let ref_text = r["ref_text"].as_str().map(str::to_string).or(typed.clone()).unwrap_or_default();
        let v = json!({
            "name": name, "ref_text": ref_text, "source": src_label, "source_path": src,
            "start": (used0 * 100.0).round() / 100.0, "dur": (dur * 100.0).round() / 100.0,
            "auto": auto, "typed": typed.is_some(), "denoise": req["denoise"] == true,
            "created": now_secs(), "model": REPO,
        });
        std::fs::write(dir.join("voice.json"), serde_json::to_string_pretty(&v)?)?;
        let how = if auto { format!("the best {dur:.1} s (from {used0:.1} s)") } else { format!("{dur:.1} s") };
        Ok(json!({"voice": id, "ref_text": ref_text, "text": format!("voice \"{name}\" cloned from {how} of {src_label}")}))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// Encodes a voice again with corrected words: the transcript has to match the reference for a
/// faithful clone, and Whisper sometimes mishears names.
fn retranscribe(f: &Found, req: &Value, report: Report) -> Result<Value> {
    let root = f.root.clone().ok_or_else(|| anyhow!("SETUP: no OmniVoice folder"))?;
    let (id, mut v) = resolve_voice(&root, req["voice"].as_str().unwrap_or(""))?;
    let text = transcript_of(req).ok_or_else(|| anyhow!("type the exact words said in the reference"))?;
    let dir = voices_dir(&root).join(&id);
    let reference = dir.join("ref.wav");
    if !reference.is_file() {
        bail!("this voice's reference audio is missing: clone it again");
    }
    let tmp = dir.join("prompt.new.pt");
    let r = worker::request(f, json!({"op": "clone", "ref_audio": reference, "ref_text": text, "out": tmp, "auto_window": false}), report)?;
    std::fs::rename(&tmp, dir.join("prompt.pt")).context("saving the voice")?;
    v["ref_text"] = json!(r["ref_text"].as_str().unwrap_or(&text));
    v["typed"] = json!(true);
    std::fs::write(dir.join("voice.json"), serde_json::to_string_pretty(&v)?)?;
    let name = v["name"].as_str().unwrap_or(&id).to_string();
    Ok(json!({"voice": id, "ref_text": v["ref_text"], "text": format!("voice \"{name}\" encoded again with the corrected transcript")}))
}

// ---------------------------------------------------------------- speak

/// Speaks a script with a cloned voice: one clip per paragraph (or one for all), in 1-3 takes,
/// written next to the project's graphics, then imported (tag `voice`, the script and seed as the
/// note) and optionally placed at a time, all as one undoable edit.
fn generate(eng: &Engine, f: &Found, req: &Value, report: Report) -> Result<Value> {
    let root = f.root.clone().ok_or_else(|| anyhow!("SETUP: no OmniVoice folder"))?;
    let (vid, voice) = resolve_voice(&root, req["voice"].as_str().unwrap_or(""))?;
    let vname = voice["name"].as_str().unwrap_or(&vid).to_string();
    let text = req["text"].as_str().unwrap_or("").replace("\r\n", "\n");
    let parts: Vec<String> = if req["split"] != false {
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
    let takes = req["takes"].as_u64().unwrap_or(1).clamp(1, 3) as usize;
    let steps = req["steps"].as_u64().filter(|s| (4..=64).contains(s)).unwrap_or(match req["quality"].as_str() {
        Some("fast") => 16,
        Some("best") => 48,
        _ => 32,
    });
    let base_seed = req["seed"].as_u64().unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos() as u64 % 1_000_000).unwrap_or(1));
    let language = str_of(req, "language").filter(|l| !l.eq_ignore_ascii_case("auto"));
    let speed = req["speed"].as_f64().filter(|s| (0.5..=2.0).contains(s) && (*s - 1.0).abs() > 1e-3);
    let loudness = match &req["loudness"] {
        Value::Bool(false) => None,
        Value::Number(n) => n.as_f64(),
        _ => Some(-19.0),
    };
    let numbers = req["numbers"] != false;
    let at = match &req["at"] {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if s.trim().eq_ignore_ascii_case("playhead") => Some(eng.ui().playhead.unwrap_or(0.0)),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|t| *t >= 0.0);
    // next to the project's graphics and sound effects (the data folder until it is saved)
    let gdir = crate::gfx::graphics_dir(eng.st.read().path.as_deref());
    let dir = gdir.parent().map(|d| d.join("voice")).unwrap_or_else(|| gdir.join("voice"));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let prompt = voices_dir(&root).join(&vid).join("prompt.pt");
    struct Out {
        path: PathBuf,
        dur: f64,
        text: String,
        take: usize,
        seed: u64,
    }
    let total = (parts.len() * takes) as f64;
    let mut outs: Vec<Out> = vec![];
    for (i, p) in parts.iter().enumerate() {
        for k in 0..takes {
            let n = (i * takes + k) as f64;
            let what = match (parts.len() > 1, takes > 1) {
                (true, true) => format!("part {} of {}, take {}: ", i + 1, parts.len(), k + 1),
                (true, false) => format!("part {} of {}: ", i + 1, parts.len()),
                (false, true) => format!("take {} of {takes}: ", k + 1),
                _ => String::new(),
            };
            let step = |pp: Option<f64>, m: String| report(pp.map(|pp| (n + pp) / total * 0.97), format!("{what}{m}"));
            let seed = base_seed + k as u64;
            let stem = format!("{}-{}{}", slug(&vname, 20), slug(&first_words(p, 5), 32), if takes > 1 { format!("-take{}", k + 1) } else { String::new() });
            let out = unique_path(&dir, &stem, "wav");
            let r = worker::request(
                f,
                json!({"op": "generate", "voice": prompt, "text": p, "language": language, "speed": speed, "num_step": steps, "seed": seed, "numbers": numbers, "loudness": loudness, "out": out}),
                &step,
            )?;
            outs.push(Out { path: out, dur: r["dur"].as_f64().unwrap_or(0.0), text: p.clone(), take: k + 1, seed });
        }
    }
    report(Some(0.98), "adding to the project".into());
    let q = crate::ops::lex::quote;
    let mut t = at;
    let mut lines = vec![];
    for o in &outs {
        let file = o.path.to_string_lossy().to_string();
        let take = if takes > 1 { format!(" (take {})", o.take) } else { String::new() };
        lines.push(format!("import {} tag=voice as={}", q(&file), q(&format!("{vname}: {}{take}", first_words(&o.text, 6)))));
        let note: String = format!("AI voice ({vname}, OmniVoice, seed {}): {}", o.seed, o.text).chars().take(600).collect();
        lines.push(format!("note $ {}", q(&note)));
        // the first take of each part goes on the timeline; other takes wait in the Project panel
        if let (Some(at), 1) = (t, o.take) {
            lines.push(format!("add {} at={:.3}", q(&file), at));
            t = Some(at + o.dur + 0.25);
        }
    }
    eng.edit(&lines.join("\n"), "gui")?;
    let p = eng.st.read().p.clone();
    let mut items = vec![];
    for o in &outs {
        let media = p.media.iter().find(|m| crate::ops::exec::same_path(&m.path, &o.path.to_string_lossy())).map(|m| m.id.clone());
        // the clip just placed from it: the newest one using that media
        let clip = match (&media, at.is_some() && o.take == 1) {
            (Some(mid), true) => p.comps.iter().flat_map(|c| c.items()).filter(|it| it.src.as_deref() == Some(mid.as_str())).map(|it| it.id.clone()).max_by_key(|id| id.get(1..).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0)),
            _ => None,
        };
        items.push(json!({"media": media, "clip": clip, "file": o.path, "dur": o.dur, "take": o.take, "seed": o.seed, "text": o.text}));
    }
    let ids: Vec<String> = items.iter().filter_map(|i| i["media"].as_str().map(str::to_string)).collect();
    let spoken: f64 = outs.iter().filter(|o| o.take == 1).map(|o| o.dur).sum();
    let what = if takes > 1 { format!("{} clips ({} parts × {takes} takes)", outs.len(), parts.len()) } else { format!("{} clip{}", outs.len(), if outs.len() == 1 { "" } else { "s" }) };
    Ok(json!({
        "media": ids,
        "items": items,
        "dur": (spoken * 100.0).round() / 100.0,
        "text": format!("{what}, {spoken:.1} s, in the project{}{}", if ids.is_empty() { String::new() } else { format!(": {}", ids.join(", ")) }, if at.is_some() { " and on the timeline" } else { "" }),
    }))
}

// ---------------------------------------------------------------- the MCP tool

fn status_text() -> String {
    let f = found();
    let (install, tasks) = {
        let s = shared().lock();
        (s.install.clone(), s.tasks.clone())
    };
    let mut out = vec![];
    if install.running {
        out.push(format!("Setting up OmniVoice in {}: {}{}", install.root, install.status, install.pct.map(|p| format!(" ({p:.0}%)")).unwrap_or_default()));
    } else if let Some(e) = &install.error {
        out.push(format!("Setup failed: {e}{}", install.hint.as_ref().map(|h| format!(" {h}")).unwrap_or_default()));
    }
    match f.state() {
        "ready" => {
            let w = worker::summary();
            let gpu = gpu().map(|g| format!(", GPU {}", g.0)).unwrap_or_default();
            let loaded = if w["loaded"] == true { format!("loaded on {}", w["device"].as_str().unwrap_or("the GPU")) } else { "loads on first use (15-60 s)".into() };
            out.push(format!("OmniVoice ready: model {}{gpu}; {loaded}.", f.model.as_ref().map(|p| p.display().to_string()).unwrap_or_default()));
        }
        "runtime" => out.push(format!(
            "OmniVoice model found at {} but its Python runtime isn't set up: with the person's OK, voice action=setup drive=<drive> (about 6.5 GB).",
            f.model.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
        )),
        _ => out.push("OmniVoice isn't set up. Ask the person first, then voice action=setup drive=D downloads the model and Python with PyTorch (about 10 GB) to that drive; or they can Locate an existing model in the AI Voice panel.".into()),
    }
    if let Some(root) = f.root.as_deref() {
        let voices = list_voices(root);
        if voices.is_empty() {
            out.push("No voices yet: voice action=clone name=... media=m3 consent=true (only with the speaker's permission).".into());
        } else {
            out.push("Voices:".into());
            for v in voices {
                out.push(format!(
                    "  {} '{}' ({} s of {}): \"{}\"",
                    v["id"].as_str().unwrap_or(""),
                    v["name"].as_str().unwrap_or(""),
                    v["dur"].as_f64().map(|d| format!("{d:.1}")).unwrap_or_default(),
                    v["source"].as_str().unwrap_or("?"),
                    first_words(v["ref_text"].as_str().unwrap_or(""), 16)
                ));
            }
        }
    }
    let recent: Vec<&Task> = tasks.iter().rev().take(5).collect();
    if !recent.is_empty() {
        out.push("Recent tasks:".into());
        for t in recent {
            let state = match t.state {
                "running" => format!("running {}%: {}", t.pct.map(|p| (p * 100.0).round() as i64).unwrap_or(0), t.status),
                "error" => format!("failed: {}{}", t.error.clone().unwrap_or_default(), t.hint.as_ref().map(|h| format!(" {h}")).unwrap_or_default()),
                "done" => t.status.clone(),
                other => other.to_string(),
            };
            out.push(format!("  #{} {}: {state}", t.id, t.label));
        }
    }
    out.join("\n")
}

fn done_text(t: &Task) -> String {
    let r = t.result.clone().unwrap_or(Value::Null);
    let mut s = format!("{} ({:.1} s)", r["text"].as_str().unwrap_or("done"), t.secs);
    if let Some(rt) = r["ref_text"].as_str() {
        s.push_str(&format!("\nvoice id: {}\ntranscript: \"{rt}\"\nIf a word is wrong, fix it for a better clone: voice action=retranscribe voice={} transcript='...'", r["voice"].as_str().unwrap_or(""), r["voice"].as_str().unwrap_or("")));
    }
    if let Some(items) = r["items"].as_array() {
        for it in items {
            s.push_str(&format!(
                "\n+{} {:.2}s take {} seed {}{}",
                it["media"].as_str().unwrap_or("?"),
                it["dur"].as_f64().unwrap_or(0.0),
                it["take"],
                it["seed"],
                it["clip"].as_str().map(|c| format!(" placed as {c}")).unwrap_or_default()
            ));
        }
        s.push_str("\nListen before using it: preview media=<id> shows the waveform; regenerate a take with the same seed= to keep it, another seed for a new reading.");
    }
    s
}

/// Waits for a task (when asked) and answers with its result, or the explained error.
fn finish(id: u64, wait: bool) -> Result<String> {
    if !wait {
        return Ok(format!("task #{id} queued: follow it with voice action=status"));
    }
    let deadline = Instant::now() + Duration::from_secs(1200);
    loop {
        let t = shared().lock().tasks.iter().find(|t| t.id == id).cloned();
        let Some(t) = t else { bail!("task #{id} is gone") };
        match t.state {
            "done" => return Ok(done_text(&t)),
            "error" => bail!("{}{}", t.error.clone().unwrap_or_else(|| "failed".into()), t.hint.as_ref().map(|h| format!(" {h}")).unwrap_or_default()),
            _ if Instant::now() > deadline => return Ok(format!("task #{id} is still running ({}): check with voice action=status", t.status)),
            _ => std::thread::sleep(Duration::from_millis(300)),
        }
    }
}

/// The `voice` MCP tool: the same setup, voices and tasks the AI Voice panel shows.
pub fn mcp_tool(eng: &Arc<Engine>, a: &Value) -> Result<String> {
    let action = a["action"].as_str().unwrap_or("status");
    let wait = a["wait"].as_bool().unwrap_or(true);
    match action {
        "status" | "voices" => Ok(status_text()),
        "speak" | "generate" | "say" => {
            let id = submit(eng, "generate", a)?;
            finish(id, wait)
        }
        "clone" => {
            let id = submit(eng, "clone", a)?;
            finish(id, wait)
        }
        "retranscribe" | "transcript" => {
            let id = submit(eng, "retranscribe", a)?;
            finish(id, wait)
        }
        "rename" | "delete" => {
            let root = found().root.ok_or_else(|| anyhow!("no voices yet"))?;
            let (id, v) = resolve_voice(&root, a["voice"].as_str().unwrap_or(""))?;
            let name = v["name"].as_str().unwrap_or(&id).to_string();
            if action == "delete" {
                super::api(eng, &json!({"action": "delete_voice", "id": id}))?;
                Ok(format!("voice '{name}' ({id}) deleted; clips already generated stay"))
            } else {
                let new = str_of(a, "name").ok_or_else(|| anyhow!("rename needs name="))?;
                super::api(eng, &json!({"action": "rename_voice", "id": id, "name": new}))?;
                Ok(format!("voice {id} renamed '{new}'"))
            }
        }
        "setup" | "install" => {
            super::api(eng, &json!({"action": "install", "drive": a["drive"], "folder": a["folder"]}))?;
            Ok(format!("{}\nSetup runs in the background (the AI Voice panel shows it too); check with voice action=status.", status_text()))
        }
        "repair" => {
            super::api(eng, &json!({"action": "repair"}))?;
            Ok("repairing OmniVoice's setup in the background (no model download); check with voice action=status".into())
        }
        "cancel" => {
            super::api(eng, &json!({"action": "cancel", "id": a["id"]}))?;
            super::api(eng, &json!({"action": "cancel_install"}))?;
            Ok("cancelled what was running".into())
        }
        "unload" => Ok(if worker::unload() { "model unloaded; GPU memory is free".into() } else { "it is busy: cancel the task first".into() }),
        other => bail!("unknown action '{other}': status, voices, speak, clone, retranscribe, rename, delete, setup, repair, cancel, unload"),
    }
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
        assert!(valid_id("hamza-3f2a1") && !valid_id("../x") && !valid_id(""));
    }

    #[test]
    fn errors_say_what_to_do() {
        let oom: anyhow::Error = WorkerFailure { message: "CUDA out of memory. Tried to allocate 12.00 MiB".into(), etype: "OutOfMemoryError".into(), detail: String::new() }.into();
        let x = explain(&oom);
        assert_eq!((x.message.as_str(), x.fix), ("Your graphics card ran out of memory.", Some("retry")));
        assert!(x.detail.contains("Tried to allocate"), "the details keep the original");
        let moved = anyhow!("OmniVoice stopped while loading: error: uv trampoline failed to spawn Python child process\n  Caused by: entity not found (os error 2)");
        assert_eq!(explain(&moved).fix, Some("repair"));
        let missing: anyhow::Error = WorkerFailure { message: "No module named 'omnivoice'".into(), etype: "ModuleNotFoundError".into(), detail: String::new() }.into();
        assert_eq!(explain(&missing).fix, Some("repair"));
        assert_eq!(explain(&anyhow!("NO_SPEECH: Whisper heard no words")).message, "No clear speech was found in the reference.");
        assert_eq!(explain(&anyhow!("cancelled")).message, "Cancelled.");
        let net = anyhow!("HTTPSConnectionPool(host='huggingface.co'): Max retries exceeded");
        assert_eq!(explain(&net).message, "MoodCut couldn't download what it needs.");
        let ours = explain(&anyhow!("unknown media m9"));
        assert_eq!(ours.message, "Unknown media m9.");
    }
}
