//! The queue behind AI Voice and AI Video: each request is a task with a stage, a fraction
//! (the provider's own progress when it reports one, otherwise an estimate from the model's
//! usual time, flagged `estimated`), the time it took, a cancel that also stops the job at the
//! provider, retry with the same settings, and failures explained in plain words with what to
//! do and which button helps. The editor polls the list; AI apps read the same tasks through
//! the `voice` and `video` tools.

use super::http::ApiError;
use super::keys;
use crate::engine::Engine;
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// What a task runs: the engine, its request, and where to report progress.
pub type Work = fn(&Arc<Engine>, &Value, &Progress) -> Result<Value>;

#[derive(Clone, Serialize)]
pub struct Task {
    pub id: u64,
    /// "voice" or "video": tasks in a lane share its limit of jobs at once.
    pub lane: &'static str,
    /// speak, clone, video
    pub kind: &'static str,
    pub provider: &'static str,
    pub label: String,
    /// queued, running, done or error
    pub state: &'static str,
    /// What is happening now (or the result once done).
    pub status: String,
    /// 0..1 while running, when it is known or estimated.
    pub pct: Option<f64>,
    /// `pct` and `eta` are estimated from the model's usual time (the provider reports none).
    pub estimated: bool,
    /// Seconds left, when it can be told.
    pub eta: Option<f64>,
    /// Credits or characters it costs, when the provider says.
    pub cost: Option<String>,
    /// A failure explained: what happened, what to do, which button helps (retry, keys, billing).
    pub error: Option<String>,
    pub hint: Option<String>,
    pub fix: Option<&'static str>,
    pub link: Option<String>,
    pub detail: Option<String>,
    pub result: Option<Value>,
    /// Seconds it took (counting while it runs).
    pub secs: f64,
    #[serde(skip)]
    pub req: Value,
    #[serde(skip)]
    started: Option<Instant>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
    #[serde(skip)]
    work: Work,
}

#[derive(Default)]
struct Queue {
    list: Vec<Task>,
    next: u64,
}

fn queue() -> &'static Mutex<Queue> {
    static Q: OnceLock<Mutex<Queue>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(Queue::default()))
}

/// Jobs at once per lane: speech is quick and goes in order; video jobs take minutes and the
/// providers run several in parallel.
fn capacity(lane: &str) -> usize {
    if lane == "video" {
        3
    } else {
        1
    }
}

/// A running task's handle on its progress and its cancel flag.
pub struct Progress {
    id: u64,
    pub cancel: Arc<AtomicBool>,
}

impl Progress {
    fn update(&self, f: impl FnOnce(&mut Task)) {
        if let Some(t) = queue().lock().list.iter_mut().find(|t| t.id == self.id) {
            f(t);
        }
    }
    /// A stage with a known fraction (or none: the editor shows a moving bar).
    pub fn set(&self, pct: Option<f64>, status: impl Into<String>) {
        let status = status.into();
        self.update(|t| {
            t.status = status;
            t.pct = pct.map(|p| (p.clamp(0.0, 1.0) * 1000.0).round() / 1000.0);
            t.estimated = false;
            t.eta = None;
        });
    }
    /// A fraction the provider reported, with the seconds left worked out from it.
    pub fn real(&self, pct: f64, eta: Option<f64>, status: impl Into<String>) {
        let status = status.into();
        self.update(|t| {
            t.status = status;
            t.pct = Some((pct.clamp(0.0, 1.0) * 1000.0).round() / 1000.0);
            t.estimated = false;
            t.eta = eta.map(|e| e.max(0.0).round());
        });
    }
    /// A fraction estimated from the usual time, with the seconds probably left.
    pub fn estimate(&self, pct: f64, eta: Option<f64>, status: impl Into<String>) {
        let status = status.into();
        self.update(|t| {
            t.status = status;
            t.pct = Some((pct.clamp(0.0, 1.0) * 1000.0).round() / 1000.0);
            t.estimated = true;
            t.eta = eta.map(|e| e.max(0.0).round());
        });
    }
    pub fn cost(&self, text: impl Into<String>) {
        let text = text.into();
        self.update(|t| t.cost = Some(text));
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// Waits, waking at once on cancel.
    pub fn sleep(&self, d: Duration) -> Result<()> {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if self.cancelled() {
                bail!("cancelled");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if self.cancelled() {
            bail!("cancelled");
        }
        Ok(())
    }
}

/// Queues a task; it starts when its lane has room.
pub fn submit(eng: &Arc<Engine>, lane: &'static str, kind: &'static str, provider: &'static str, label: String, req: Value, work: Work) -> u64 {
    let cancel = Arc::new(AtomicBool::new(false));
    let id = {
        let mut q = queue().lock();
        q.next += 1;
        let id = q.next;
        q.list.push(Task {
            id,
            lane,
            kind,
            provider,
            label,
            state: "queued",
            status: "waiting for the jobs before it".into(),
            pct: None,
            estimated: false,
            eta: None,
            cost: None,
            error: None,
            hint: None,
            fix: None,
            link: None,
            detail: None,
            result: None,
            secs: 0.0,
            req,
            started: None,
            cancel: cancel.clone(),
            work,
        });
        // keep the list short: drop the oldest finished ones
        while q.list.len() > 40 {
            match q.list.iter().position(|t| t.state == "done" || t.state == "error") {
                Some(i) => {
                    q.list.remove(i);
                }
                None => break,
            }
        }
        id
    };
    let eng = eng.clone();
    std::thread::spawn(move || run(eng, id, cancel));
    id
}

fn run(eng: Arc<Engine>, id: u64, cancel: Arc<AtomicBool>) {
    // wait for room in the lane, first come first served
    let (lane, req, work) = loop {
        {
            let mut q = queue().lock();
            let Some(me) = q.list.iter().find(|t| t.id == id) else { return };
            let lane = me.lane;
            if me.state != "queued" {
                return;
            }
            if cancel.load(Ordering::Relaxed) {
                if let Some(t) = q.list.iter_mut().find(|t| t.id == id) {
                    t.state = "error";
                    t.status = "cancelled".into();
                    t.error = Some("Cancelled.".into());
                }
                drop(q);
                super::notify(&eng, lane, "cancelled");
                return;
            }
            let running = q.list.iter().filter(|t| t.lane == lane && t.state == "running").count();
            let first = q.list.iter().find(|t| t.lane == lane && t.state == "queued").map(|t| t.id) == Some(id);
            if running < capacity(lane) && first {
                let t = q.list.iter_mut().find(|t| t.id == id).expect("found above");
                t.state = "running";
                t.status = "starting".into();
                t.started = Some(Instant::now());
                break (t.lane, t.req.clone(), t.work);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    super::notify(&eng, lane, "started");
    let p = Progress { id, cancel: cancel.clone() };
    let t0 = Instant::now();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&eng, &req, &p))).unwrap_or_else(|e| Err(anyhow!("internal error: {}", crate::engine::panic_message(&*e))));
    let text = {
        let mut q = queue().lock();
        let Some(t) = q.list.iter_mut().find(|t| t.id == id) else { return };
        t.secs = (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        t.started = None;
        t.eta = None;
        match r {
            Ok(v) => {
                t.state = "done";
                t.pct = Some(1.0);
                t.estimated = false;
                t.status = v["text"].as_str().unwrap_or("done").to_string();
                t.result = Some(v);
            }
            Err(e) => {
                let x = if cancel.load(Ordering::Relaxed) { explain(&anyhow!("cancelled")) } else { explain(&e) };
                t.state = "error";
                t.status = if x.message == "Cancelled." { "cancelled".into() } else { "failed".into() };
                t.error = Some(x.message);
                t.hint = x.hint;
                t.fix = x.fix;
                t.link = x.link;
                t.detail = Some(x.detail);
            }
        }
        // "done <label>: <result>" or "failed <label>: <what to do>", for the editor's toasts
        format!("{} {}: {}", if t.state == "done" { "done" } else { "failed" }, t.label, t.error.clone().unwrap_or_else(|| t.status.clone()))
    };
    super::notify(&eng, lane, text);
}

/// Tasks of a lane (oldest first), with the running seconds filled in.
pub fn list(lane: &str) -> Vec<Task> {
    let mut out: Vec<Task> = queue().lock().list.iter().filter(|t| t.lane == lane).cloned().collect();
    for t in out.iter_mut() {
        if let Some(st) = t.started {
            t.secs = (st.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        }
    }
    out
}

pub fn get(id: u64) -> Option<Task> {
    list_all().into_iter().find(|t| t.id == id)
}

fn list_all() -> Vec<Task> {
    let mut out: Vec<Task> = queue().lock().list.clone();
    for t in out.iter_mut() {
        if let Some(st) = t.started {
            t.secs = (st.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        }
    }
    out
}

/// Cancels one task (or every task of the lane); running ones also stop at the provider.
pub fn cancel(lane: &str, id: Option<u64>) -> usize {
    let q = queue().lock();
    let mut n = 0;
    for t in q.list.iter().filter(|t| t.lane == lane && id.map_or(true, |i| i == t.id)) {
        if t.state == "queued" || t.state == "running" {
            t.cancel.store(true, Ordering::Relaxed);
            n += 1;
        }
    }
    n
}

/// Runs a finished or failed task again with the same settings.
pub fn retry(eng: &Arc<Engine>, lane: &str, id: u64) -> Result<u64> {
    let t = queue().lock().list.iter().find(|t| t.id == id && t.lane == lane).cloned().ok_or_else(|| anyhow!("that task is gone"))?;
    if t.state == "queued" || t.state == "running" {
        bail!("task #{id} is still running");
    }
    let mut req = t.req.clone();
    // a resumed job starts over at the provider
    if let Some(o) = req.as_object_mut() {
        o.remove("resume");
    }
    Ok(submit(eng, t.lane, t.kind, t.provider, t.label.clone(), req, t.work))
}

/// Forgets finished tasks of a lane.
pub fn clear(lane: &str) {
    queue().lock().list.retain(|t| t.lane != lane || t.state == "queued" || t.state == "running");
}

/// Waits for a task (up to `max`) and returns it; still running after that is not an error.
pub fn wait(id: u64, max: Duration) -> Result<Task> {
    let end = Instant::now() + max;
    loop {
        let t = get(id).ok_or_else(|| anyhow!("task #{id} is gone"))?;
        if t.state == "done" || t.state == "error" || Instant::now() > end {
            return Ok(t);
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

/// One line for AI apps: a task's state, progress and outcome.
pub fn line(t: &Task) -> String {
    let state = match t.state {
        "running" => {
            let pct = t.pct.map(|p| format!("{}{}% ", if t.estimated { "~" } else { "" }, (p * 100.0).round() as i64)).unwrap_or_default();
            let eta = t.eta.filter(|e| *e > 0.0).map(|e| format!(", about {} left", clock(e))).unwrap_or_default();
            format!("running {pct}({}{eta}): {}", clock(t.secs), t.status)
        }
        "error" => format!("failed: {}{}", t.error.clone().unwrap_or_default(), t.hint.as_ref().map(|h| format!(" {h}")).unwrap_or_default()),
        "done" => format!("done in {}: {}", clock(t.secs), t.status),
        other => format!("{other}: {}", t.status),
    };
    format!("#{} {}: {state}{}", t.id, t.label, t.cost.as_ref().map(|c| format!(" [{c}]")).unwrap_or_default())
}

pub fn clock(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    if s >= 60 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

// ---------------------------------------------------------------- errors people can act on

/// An error as the editor shows it: what happened, what to do, which button helps, a link, and
/// the technical details for "Details".
pub struct Explained {
    pub message: String,
    pub hint: Option<String>,
    /// retry, keys (open AI Services), billing (open the provider's billing page)
    pub fix: Option<&'static str>,
    pub link: Option<String>,
    pub detail: String,
}

pub fn explain(e: &anyhow::Error) -> Explained {
    let raw = format!("{e:#}");
    let api = e.downcast_ref::<ApiError>();
    let detail = match api {
        Some(a) if !a.body.trim().is_empty() => format!("{raw}\n\n{}", a.body.trim()),
        _ => raw.clone(),
    };
    let lower = detail.to_ascii_lowercase();
    let has = |ps: &[&str]| ps.iter().any(|p| lower.contains(p));
    let x = |m: String, h: String, fix: Option<&'static str>, link: Option<String>| Explained { message: m, hint: (!h.is_empty()).then_some(h), fix, link, detail: detail.clone() };
    if raw.trim() == "cancelled" || raw.ends_with(": cancelled") {
        return x("Cancelled.".into(), String::new(), None, None);
    }
    // which service it was about: the error's own, or one named in the message
    let pid = api.map(|a| a.provider).or_else(|| keys::PROVIDERS.iter().find(|p| raw.contains(p.name) || raw.contains(&format!("NO_KEY {}", p.id))).map(|p| p.id));
    let name = pid.map(keys::name).unwrap_or("The AI service");
    let prov = pid.and_then(keys::provider);
    let keys_url = prov.map(|p| p.keys_url.to_string());
    let billing = prov.map(|p| p.billing_url.to_string());
    if raw.contains("NO_KEY") {
        return x(format!("{name} isn't connected yet."), format!("Add your {name} API key in AI Services (it is stored encrypted on this computer)."), Some("keys"), keys_url);
    }
    if let Some(a) = api {
        let code = a.code.to_ascii_lowercase();
        let c = |ps: &[&str]| ps.iter().any(|p| code.contains(p));
        let msg = a.message.trim().trim_end_matches('.').to_string();
        let said = if msg.is_empty() { String::new() } else { format!(" {name} said: \"{msg}\".") };
        if a.status == 401 || c(&["invalid_api_key", "missing_api_key", "unauthorized", "authentication", "invalid_authorization"]) {
            return x(format!("{name} didn't accept the API key."), format!("Paste a valid key in AI Services (it may have been revoked or copied incompletely).{said}"), Some("keys"), keys_url);
        }
        if a.status == 402 || c(&["insufficient_credits", "quota_exceeded", "payment_required", "insufficient_balance", "credits"]) || has(&["enough credits", "insufficient credits", "out of credits", "exhausted balance", "insufficient balance", "credit balance"]) {
            return x(format!("Your {name} account is out of credits."), format!("Add credits or upgrade the plan, then press Retry.{said}"), Some("billing"), billing);
        }
        if c(&["can_not_use_instant_voice_cloning", "feature_not_available", "subscription_required"]) || has(&["instant voice cloning", "upgrade your subscription", "not available on your current plan"]) {
            return x(format!("Your {name} plan doesn't include this."), format!("Voice cloning needs a paid {name} plan (Starter or higher). Library voices work on every plan.{said}"), Some("billing"), billing);
        }
        if c(&["voice_limit", "voice_slots"]) || has(&["voice limit", "maximum amount of custom voices"]) {
            return x(format!("Your {name} plan has no free voice slot."), "Delete a cloned voice you no longer use (in the Voices list), or upgrade the plan.".into(), Some("billing"), billing);
        }
        if a.status == 403 || c(&["insufficient_permissions", "missing_permissions", "forbidden", "access_denied"]) {
            return x(format!("The {name} key isn't allowed to do this."), format!("Create a key with full access (or this permission) and paste it in AI Services.{said}"), Some("keys"), keys_url);
        }
        if a.status == 429 || c(&["rate_limit", "concurrent", "too_many", "system_busy", "daily_generation_limit"]) {
            return x(format!("{name} is busy, or your plan's limit of jobs at once was reached."), format!("Wait a minute (or let the other jobs finish) and press Retry.{said}"), Some("retry"), None);
        }
        if a.status >= 500 {
            return x(format!("{name} is having trouble right now."), "This is on their side. Press Retry in a minute.".into(), Some("retry"), None);
        }
        if c(&["text_too_long"]) {
            return x("The script is too long for this model.".into(), "Split it into paragraphs (a blank line starts a new clip) or pick a model that takes more text.".into(), None, None);
        }
        if c(&["voice_not_found"]) || (a.status == 404 && has(&["voice"])) {
            return x("That voice doesn't exist anymore.".into(), "Refresh the Voices list and pick another voice.".into(), None, None);
        }
        if has(&["safety", "moderation", "content policy", "nsfw", "inappropriate", "public figure"]) {
            return x(format!("{name}'s content filter blocked this request."), "Change the prompt or the image (no real public figures, logos or explicit content) and try again. Blocked requests may still be charged.".into(), None, None);
        }
        if a.status == 400 || a.status == 422 || a.status == 404 {
            let what = if msg.is_empty() { "a setting".to_string() } else { format!("a setting: {msg}") };
            return x(format!("{name} rejected {what}."), "Change that setting (or the model) and try again.".into(), None, None);
        }
    }
    if has(&["safety.", "safety_", "content moderation", "moderation", "nsfw", "content policy", "content_policy"]) {
        return x(
            format!("{name}'s content filter blocked this request."),
            "Change the prompt or the image (no real public figures, logos, watermarks or explicit content) and try again. Blocked requests may still be charged.".into(),
            None,
            None,
        );
    }
    if has(&["asset.invalid", "image is too small", "invalid image", "unsupported image"]) {
        return x(format!("{name} couldn't use that image."), "Use a larger, clean image (no logos or text overlays) in JPEG or PNG.".into(), None, None);
    }
    if has(&["third_party.unavailable", "internal.bad_output", "input_preprocessing.internal"]) {
        return x(format!("{name} couldn't finish this video."), "Their model had a problem; press Retry (maybe in a minute). Prompts that ask for on-screen text or include logos fail more often.".into(), Some("retry"), None);
    }
    if has(&["couldn't reach", "could not resolve", "failed to connect", "connection refused", "connection reset", "timed out", "operation timed out", "network is unreachable", "ssl", "schannel"]) {
        return x(format!("MoodCut couldn't reach {name}."), "Check the internet connection (and any VPN, proxy or firewall), then press Retry.".into(), Some("retry"), None);
    }
    if has(&["no space left", "os error 112", "disk full", "not enough space"]) {
        return x("The drive is full.".into(), "Free some space on the project's drive, then press Retry.".into(), Some("retry"), None);
    }
    if has(&["ffmpeg failed", "invalid data found", "does not contain any stream", "ffmpeg isn't"]) {
        return x("MoodCut couldn't read or convert that media.".into(), "Use another file or clip (MP3, WAV, M4A, MP4, MOV, JPEG and PNG work). Details has FFmpeg's message.".into(), None, None);
    }
    if has(&["downloading the result"]) {
        return x(format!("The result from {name} couldn't be downloaded."), "Press Retry. The provider keeps results for a day or two.".into(), Some("retry"), None);
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
    x(m, "Press Retry. If it keeps failing, open Details.".into(), Some("retry"), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(provider: &'static str, status: u16, code: &str, message: &str) -> anyhow::Error {
        ApiError { provider, status, code: code.into(), message: message.into(), body: String::new() }.into()
    }

    #[test]
    fn errors_say_what_to_do() {
        let x = explain(&api("elevenlabs", 401, "invalid_api_key", "Invalid API key"));
        assert_eq!((x.message.as_str(), x.fix), ("ElevenLabs didn't accept the API key.", Some("keys")));
        assert!(x.link.unwrap().contains("elevenlabs.io"));
        let x = explain(&api("runway", 400, "", "You do not have enough credits to run this task."));
        assert_eq!((x.message.as_str(), x.fix), ("Your Runway account is out of credits.", Some("billing")));
        assert_eq!(explain(&api("elevenlabs", 402, "insufficient_credits", "")).fix, Some("billing"));
        assert_eq!(explain(&api("fal", 429, "", "Too many requests")).fix, Some("retry"));
        assert_eq!(explain(&api("fal", 503, "", "")).message, "fal.ai is having trouble right now.");
        assert_eq!(explain(&api("elevenlabs", 403, "can_not_use_instant_voice_cloning", "")).message, "Your ElevenLabs plan doesn't include this.");
        let x = explain(&api("runway", 400, "", "ratio must be one of 1280:720"));
        assert_eq!(x.message, "Runway rejected a setting: ratio must be one of 1280:720.");
        assert_eq!(explain(&anyhow!("NO_KEY fal: fal.ai isn't connected")).fix, Some("keys"));
        assert_eq!(explain(&anyhow!("Runway: SAFETY.INPUT.TEXT")).message, "Runway's content filter blocked this request.");
        assert_eq!(explain(&anyhow!("SAFETY.OUTPUT.VIDEO")).message, "The AI service's content filter blocked this request.");
        assert_eq!(explain(&anyhow!("couldn't reach Runway: curl: (6) Could not resolve host")).message, "MoodCut couldn't reach Runway.");
        assert_eq!(explain(&anyhow!("cancelled")).message, "Cancelled.");
        assert_eq!(explain(&anyhow!("unknown media m9")).message, "Unknown media m9.");
    }

    #[test]
    fn clocks() {
        assert_eq!(clock(5.4), "5 s");
        assert_eq!(clock(75.0), "1:15");
    }
}
