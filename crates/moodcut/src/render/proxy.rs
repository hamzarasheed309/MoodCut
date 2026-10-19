//! Proxies: small, quick-to-decode copies of heavy footage, made in the
//! background and used only for previews (exports always read the originals),
//! the way Premiere Pro's ingest proxies and "Toggle Proxies" work.
//!
//! 4K, HEVC/AV1/VP9 and high-frame-rate files are slow to decode and seek, so
//! scrubbing and playback stall on the CPU (or load the GPU when hardware
//! decoding is used). A proxy is H.264 at up to 1280 px with a short GOP, no
//! B-frames and `fastdecode`: any frame is a few cheap decodes away. Proxies
//! are built one at a time, at low priority with two threads, so editing stays
//! smooth while they are made. They live in the data folder's cache, named by
//! the source's path, size and date, so a changed file gets a new proxy.

use super::ffmpeg::{background_command, Ff};
use crate::model::project::{Media, MediaKind};
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Longest edge of a proxy.
pub const PROXY_EDGE: u32 = 1280;

/// Which footage gets a proxy without being asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Off,
    /// heavy footage only (4K, HEVC/AV1/VP9, 50+ fps at 1080p and up)
    Auto,
    /// every video
    All,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s.to_ascii_lowercase().as_str() {
            "off" | "none" | "manual" => Mode::Off,
            "auto" | "heavy" => Mode::Auto,
            "all" | "on" | "always" => Mode::All,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Auto => "auto",
            Mode::All => "all",
        }
    }
}

#[derive(Clone, Debug)]
pub enum State {
    Queued,
    Building(f32),
    Ready { path: String, w: u32, h: u32 },
    Failed(String),
    /// Checked: there is no proxy (remembered so previews don't look on disk every frame).
    Absent,
}

struct Job {
    key: String,
    media: Media,
}

pub struct Proxies {
    ff: Arc<Ff>,
    dir: PathBuf,
    /// Previews use proxies (Premiere's "Toggle Proxies").
    enabled: AtomicBool,
    mode: RwLock<Mode>,
    state: Mutex<HashMap<String, State>>,
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    worker: OnceLock<()>,
    cancel: AtomicBool,
    /// Bumped whenever what previews show may change (a proxy finished, the toggle flipped).
    gen: AtomicU64,
    on_change: OnceLock<Box<dyn Fn(String) + Send + Sync>>,
}

fn key(m: &Media) -> String {
    format!("{}|{}|{}", m.path, m.size, m.mtime)
}

/// Video that a proxy can stand in for (not graphics with alpha, not stills).
pub fn eligible(m: &Media) -> bool {
    m.kind == MediaKind::Video && m.has_video && !m.alpha && m.gfx.is_none() && m.w > 0 && m.h > 0 && m.dur > 0.5
}

/// Footage that is expensive to decode at preview sizes.
pub fn heavy(m: &Media) -> bool {
    let px = m.w as u64 * m.h as u64;
    let codec = m.codec.to_ascii_lowercase();
    let hard_codec = ["hevc", "h265", "av1", "vp9", "prores", "dnxhd", "cfhd", "mjpeg2000", "jpeg2000"].iter().any(|c| codec.contains(c));
    px > 1920 * 1088 || (hard_codec && px >= 1280 * 720) || (m.fps > 50.0 && px >= 1920 * 1080)
}

/// Proxy frame size: the long edge capped at `PROXY_EDGE`, even dimensions.
pub fn proxy_size(m: &Media) -> (u32, u32) {
    let long = m.w.max(m.h).max(1) as f64;
    let k = (PROXY_EDGE as f64 / long).min(1.0);
    let ev = |v: u32| (((v as f64 * k).round() as u32) / 2 * 2).max(2);
    (ev(m.w), ev(m.h))
}

impl Proxies {
    pub fn new(ff: Arc<Ff>) -> Arc<Proxies> {
        let s = crate::config::settings();
        let mode = s["proxies"]["mode"].as_str().and_then(Mode::parse).unwrap_or(Mode::Auto);
        let enabled = s["proxies"]["enabled"].as_bool().unwrap_or(true);
        Arc::new(Proxies {
            ff,
            dir: crate::config::cache_dir().join("proxies"),
            enabled: AtomicBool::new(enabled),
            mode: RwLock::new(mode),
            state: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            worker: OnceLock::new(),
            cancel: AtomicBool::new(false),
            gen: AtomicU64::new(0),
            on_change: OnceLock::new(),
        })
    }

    /// Called with a short message whenever a proxy finishes or settings change.
    pub fn set_on_change(&self, f: Box<dyn Fn(String) + Send + Sync>) {
        let _ = self.on_change.set(f);
    }

    fn changed(&self, msg: String) {
        self.gen.fetch_add(1, Ordering::Relaxed);
        if let Some(f) = self.on_change.get() {
            f(msg);
        }
    }

    pub fn generation(&self) -> u64 {
        self.gen.load(Ordering::Relaxed)
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        if self.enabled.swap(on, Ordering::Relaxed) != on {
            crate::config::save_setting("proxies", serde_json::json!({"mode": self.mode().name(), "enabled": on}));
            self.changed(format!("proxies {}", if on { "on" } else { "off" }));
        }
    }

    pub fn mode(&self) -> Mode {
        *self.mode.read()
    }

    pub fn set_mode(&self, m: Mode) {
        *self.mode.write() = m;
        crate::config::save_setting("proxies", serde_json::json!({"mode": m.name(), "enabled": self.enabled()}));
    }

    fn file(&self, m: &Media) -> PathBuf {
        let (w, h) = proxy_size(m);
        self.dir.join(format!("{:016x}_{w}x{h}.mp4", crate::render::eval::hash_str(&key(m))))
    }

    /// The proxy's state, noticing proxies made in an earlier session. None = no proxy.
    pub fn state(&self, m: &Media) -> Option<State> {
        let k = key(m);
        if let Some(s) = self.state.lock().get(&k) {
            return (!matches!(s, State::Absent)).then(|| s.clone());
        }
        let f = self.file(m);
        let s = if eligible(m) && f.metadata().map(|x| x.len() > 1024).unwrap_or(false) {
            let (w, h) = proxy_size(m);
            State::Ready { path: f.to_string_lossy().to_string(), w, h }
        } else {
            State::Absent
        };
        self.state.lock().insert(k, s.clone());
        (!matches!(s, State::Absent)).then_some(s)
    }

    /// The file previews should decode for `m` at `dw` px wide: the proxy when
    /// proxies are on, one is ready and it's big enough (at most 1.5x upscaled).
    pub fn for_preview(&self, m: &Media, dw: u32) -> Option<String> {
        if !self.enabled() || !eligible(m) {
            return None;
        }
        match self.state(m)? {
            State::Ready { path, w, .. } if dw as f64 <= w as f64 * 1.5 => Some(path),
            _ => None,
        }
    }

    /// Queues proxies: `force` makes one for any eligible video, otherwise the mode decides.
    /// Returns how many were queued.
    pub fn request(self: &Arc<Self>, media: &[Media], force: bool) -> usize {
        let mode = self.mode();
        let mut n = 0;
        for m in media {
            if !eligible(m) || !(force || mode == Mode::All || (mode == Mode::Auto && heavy(m))) {
                continue;
            }
            match self.state(m) {
                Some(State::Ready { .. } | State::Queued | State::Building(_)) => continue,
                _ => {}
            }
            let k = key(m);
            self.state.lock().insert(k.clone(), State::Queued);
            self.queue.lock().push_back(Job { key: k, media: m.clone() });
            n += 1;
        }
        if n > 0 {
            self.cancel.store(false, Ordering::Relaxed);
            self.start_worker();
            self.wake.notify_one();
        }
        n
    }

    /// Stops queued and running proxy jobs.
    pub fn cancel_all(&self) {
        let mut q = self.queue.lock();
        let mut st = self.state.lock();
        for j in q.drain(..) {
            st.remove(&j.key);
        }
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Deletes the proxies of these media (they are rebuilt only when asked or by the mode).
    pub fn delete(&self, media: &[Media]) -> usize {
        let mut n = 0;
        for m in media {
            let f = self.file(m);
            if std::fs::remove_file(&f).is_ok() {
                n += 1;
            }
            self.state.lock().remove(&key(m));
        }
        if n > 0 {
            self.changed(format!("{n} prox{} deleted", if n == 1 { "y" } else { "ies" }));
        }
        n
    }

    /// One-line status for a set of media (e.g. "3 ready, 1 building 42%, 2 queued").
    pub fn summary(&self, media: &[Media]) -> String {
        let (mut ready, mut queued, mut failed, mut building) = (0, 0, 0, vec![]);
        for m in media.iter().filter(|m| eligible(m)) {
            match self.state(m) {
                Some(State::Ready { .. }) => ready += 1,
                Some(State::Queued) => queued += 1,
                Some(State::Building(p)) => building.push(format!("{} {:.0}%", m.id, p * 100.0)),
                Some(State::Failed(_)) => failed += 1,
                Some(State::Absent) | None => {}
            }
        }
        let mut parts = vec![format!("{ready} ready")];
        if !building.is_empty() {
            parts.push(format!("building {}", building.join(", ")));
        }
        if queued > 0 {
            parts.push(format!("{queued} queued"));
        }
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        format!("proxies {} (mode {}, {}): {}", if self.enabled() { "on" } else { "off" }, self.mode().name(), "previews only; exports use the originals", parts.join(", "))
    }

    /// Short per-media tag for listings: "proxy", "proxy 40%", "proxy queued", "".
    pub fn tag(&self, m: &Media) -> String {
        match self.state(m) {
            Some(State::Ready { .. }) => "proxy".into(),
            Some(State::Building(p)) => format!("proxy {:.0}%", p * 100.0),
            Some(State::Queued) => "proxy queued".into(),
            Some(State::Failed(_)) => "proxy failed".into(),
            Some(State::Absent) | None => String::new(),
        }
    }

    fn start_worker(self: &Arc<Self>) {
        let me = self.clone();
        self.worker.get_or_init(|| {
            std::thread::Builder::new().name("proxies".into()).spawn(move || me.work()).map(|_| ()).unwrap_or(())
        });
    }

    fn work(self: Arc<Self>) {
        loop {
            let job = {
                let mut q = self.queue.lock();
                while q.is_empty() {
                    self.wake.wait(&mut q);
                }
                q.pop_front().unwrap()
            };
            self.cancel.store(false, Ordering::Relaxed);
            let r = self.build(&job);
            let msg = match &r {
                Ok(_) => format!("proxy ready for {}", job.media.name),
                Err(e) => format!("proxy failed for {}: {e}", job.media.name),
            };
            let st = match r {
                Ok(s) => s,
                Err(e) if self.cancel.load(Ordering::Relaxed) => {
                    let _ = e;
                    self.state.lock().remove(&job.key);
                    continue;
                }
                Err(e) => State::Failed(e.to_string()),
            };
            self.state.lock().insert(job.key.clone(), st);
            self.changed(msg);
        }
    }

    fn build(&self, job: &Job) -> anyhow::Result<State> {
        let m = &job.media;
        self.ff.need()?;
        std::fs::create_dir_all(&self.dir)?;
        let out = self.file(m);
        let part = out.with_extension("part.mp4");
        let (w, h) = proxy_size(m);
        let codec: Vec<&str> = if self.ff.has_encoder("libx264") {
            vec!["-c:v", "libx264", "-preset", "veryfast", "-tune", "fastdecode", "-crf", "23", "-g", "15", "-bf", "0", "-pix_fmt", "yuv420p"]
        } else {
            vec!["-c:v", "mpeg4", "-q:v", "4", "-g", "15", "-bf", "0", "-pix_fmt", "yuv420p"]
        };
        let attempt = |timing: &[&str]| -> anyhow::Result<(bool, String)> {
            let mut c = background_command(&self.ff.ffmpeg_path());
            c.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"]).arg(&m.path);
            c.args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &format!("scale={w}:{h}:flags=bilinear")]);
            c.args(timing);
            c.args(&codec);
            c.args(["-threads", "2", "-movflags", "+faststart", "-progress", "pipe:1", "-nostats"]).arg(&part);
            c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = c.spawn()?;
            let mut err = child.stderr.take().unwrap();
            let err_t = std::thread::spawn(move || {
                let mut s = String::new();
                let _ = err.read_to_string(&mut s);
                s
            });
            let out = BufReader::new(child.stdout.take().unwrap());
            for line in out.lines().map_while(Result::ok) {
                if self.cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    break;
                }
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                    let p = (us / 1e6 / m.dur.max(0.1)).clamp(0.0, 0.99) as f32;
                    self.state.lock().insert(job.key.clone(), State::Building(p));
                }
            }
            let ok = child.wait()?.success();
            Ok((ok, err_t.join().unwrap_or_default()))
        };
        self.state.lock().insert(job.key.clone(), State::Building(0.0));
        // keep the source's timestamps (variable frame rate phone video stays in sync)
        let (mut ok, mut err) = attempt(&["-fps_mode", "passthrough"])?;
        if !ok && err.contains("fps_mode") {
            (ok, err) = attempt(&["-vsync", "passthrough"])?;
        }
        if self.cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&part);
            anyhow::bail!("cancelled");
        }
        if !ok || !Path::new(&part).is_file() {
            let _ = std::fs::remove_file(&part);
            let last = err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("ffmpeg failed").to_string();
            anyhow::bail!("{last}");
        }
        std::fs::rename(&part, &out)?;
        Ok(State::Ready { path: out.to_string_lossy().to_string(), w, h })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(w: u32, h: u32, codec: &str, fps: f64) -> Media {
        Media { kind: MediaKind::Video, has_video: true, w, h, codec: codec.into(), fps, dur: 10.0, path: "/x.mp4".into(), ..serde_json::from_str(r#"{"id":"m1","path":"","name":"x","kind":"video"}"#).unwrap() }
    }

    #[test]
    fn heavy_footage_and_sizes() {
        assert!(heavy(&media(3840, 2160, "h264", 30.0)));
        assert!(heavy(&media(1920, 1080, "hevc", 30.0)));
        assert!(heavy(&media(1920, 1080, "h264", 60.0)));
        assert!(!heavy(&media(1920, 1080, "h264", 30.0)));
        assert!(!heavy(&media(1280, 720, "h264", 60.0)));
        assert_eq!(proxy_size(&media(3840, 2160, "h264", 30.0)), (1280, 720));
        assert_eq!(proxy_size(&media(2160, 3840, "h264", 30.0)), (720, 1280));
        assert_eq!(proxy_size(&media(1280, 720, "hevc", 30.0)), (1280, 720));
        let mut alpha = media(3840, 2160, "prores", 30.0);
        alpha.alpha = true;
        assert!(!eligible(&alpha));
    }
}
