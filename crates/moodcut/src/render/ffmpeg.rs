//! FFmpeg discovery, capability detection and media probing.

use crate::model::project::{Media, MediaKind};
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Ff {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub version: String,
    pub encoders: HashSet<String>,
    pub filters: HashSet<String>,
    hw_cache: Mutex<Option<Vec<String>>>,
}

/// Creates a Command that never pops up a console window on Windows.
pub fn command(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn candidates() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Ok(p) = std::env::var("MOODCUT_FFMPEG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            if let Some(d) = p.parent() {
                v.push(d.to_path_buf());
            }
        } else {
            v.push(p.clone());
            v.push(p.join("bin"));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            v.push(d.to_path_buf());
            v.push(d.join("ffmpeg"));
            v.push(d.join("ffmpeg").join("bin"));
        }
    }
    if let Some(home) = crate::config::data_dir() {
        v.push(home.join("ffmpeg").join("bin"));
        v.push(home.join("ffmpeg"));
    }
    if let Ok(path) = std::env::var("PATH") {
        for p in std::env::split_paths(&path) {
            v.push(p);
        }
    }
    if cfg!(windows) {
        for base in ["C:\\ffmpeg\\bin", "C:\\Program Files\\ffmpeg\\bin", "C:\\ProgramData\\chocolatey\\bin"] {
            v.push(PathBuf::from(base));
        }
        if let Ok(la) = std::env::var("LOCALAPPDATA") {
            let la = PathBuf::from(la);
            v.push(la.join("Microsoft").join("WinGet").join("Links"));
            // winget package folders (Gyan.FFmpeg / BtbN)
            if let Ok(rd) = std::fs::read_dir(la.join("Microsoft").join("WinGet").join("Packages")) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().to_ascii_lowercase();
                    if n.contains("ffmpeg") {
                        if let Ok(sub) = std::fs::read_dir(e.path()) {
                            for s in sub.flatten() {
                                v.push(s.path().join("bin"));
                            }
                        }
                    }
                }
            }
        }
        if let Ok(up) = std::env::var("USERPROFILE") {
            v.push(PathBuf::from(&up).join("scoop").join("shims"));
        }
    } else {
        for base in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin", "/snap/bin"] {
            v.push(PathBuf::from(base));
        }
    }
    v
}

pub const MISSING: &str = "FFmpeg not found. Install it (Windows: `winget install Gyan.FFmpeg`, or run `moodcut setup`), \
or set MOODCUT_FFMPEG to the folder containing ffmpeg and ffprobe.";

impl Ff {
    /// A placeholder when FFmpeg isn't installed: graphics-only work (scripts,
    /// text, shapes, HTML stills, frames) still runs; media operations report
    /// how to install it.
    pub fn missing() -> Ff {
        Ff { ffmpeg: PathBuf::new(), ffprobe: PathBuf::new(), version: "not installed".into(), encoders: HashSet::new(), filters: HashSet::new(), hw_cache: Mutex::new(None) }
    }

    pub fn available(&self) -> bool {
        !self.ffmpeg.as_os_str().is_empty()
    }

    /// Errors with install instructions when FFmpeg is missing.
    pub fn need(&self) -> Result<()> {
        if self.available() {
            Ok(())
        } else {
            bail!("{MISSING}")
        }
    }

    pub fn find() -> Result<Ff> {
        if std::env::var("MOODCUT_FFMPEG").is_ok_and(|v| v.eq_ignore_ascii_case("none")) {
            bail!("{MISSING}"); // lets you run (and test) without FFmpeg
        }
        for dir in candidates() {
            let ff = dir.join(exe_name("ffmpeg"));
            let fp = dir.join(exe_name("ffprobe"));
            if ff.is_file() && fp.is_file() {
                if let Ok(ver) = Self::version_of(&ff) {
                    let mut f = Ff { ffmpeg: ff, ffprobe: fp, version: ver, encoders: HashSet::new(), filters: HashSet::new(), hw_cache: Mutex::new(None) };
                    f.encoders = f.list("-encoders");
                    f.filters = f.list("-filters");
                    return Ok(f);
                }
            }
        }
        bail!("{MISSING}")
    }

    fn version_of(exe: &Path) -> Result<String> {
        let out = command(exe).arg("-version").stdout(Stdio::piped()).stderr(Stdio::null()).output()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let line = s.lines().next().unwrap_or("").to_string();
        if !line.starts_with("ffmpeg version") {
            bail!("not ffmpeg");
        }
        Ok(line.split_whitespace().nth(2).unwrap_or("?").to_string())
    }

    fn list(&self, flag: &str) -> HashSet<String> {
        let out = command(&self.ffmpeg).args(["-hide_banner", flag]).stdout(Stdio::piped()).stderr(Stdio::null()).output();
        let mut set = HashSet::new();
        if let Ok(o) = out {
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                let parts: Vec<&str> = l.split_whitespace().collect();
                if parts.len() >= 2 && parts[0].len() >= 3 && !parts[0].contains('=') {
                    set.insert(parts[1].to_string());
                }
            }
        }
        set
    }

    pub fn has_encoder(&self, e: &str) -> bool {
        self.encoders.contains(e)
    }

    pub fn has_filter(&self, f: &str) -> bool {
        self.filters.contains(f)
    }

    pub fn cmd(&self) -> Command {
        let mut c = command(&self.ffmpeg);
        c.args(["-hide_banner", "-nostdin", "-loglevel", "error"]);
        c
    }

    /// Hardware H.264 encoders that actually work on this machine (tested once).
    pub fn hw_encoders(&self) -> Vec<String> {
        let mut g = self.hw_cache.lock();
        if let Some(v) = &*g {
            return v.clone();
        }
        let mut ok = vec![];
        for enc in ["h264_nvenc", "h264_qsv", "h264_amf", "h264_videotoolbox", "h264_vaapi"] {
            if !self.has_encoder(enc) {
                continue;
            }
            let st = self
                .cmd()
                .args(["-f", "lavfi", "-i", "color=c=black:s=256x256:d=0.2", "-frames:v", "3", "-c:v", enc, "-f", "null", "-"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if st.map(|s| s.success()).unwrap_or(false) {
                ok.push(enc.to_string());
            }
        }
        *g = Some(ok.clone());
        ok
    }

    /// Probes a media file.
    pub fn probe(&self, path: &str) -> Result<Media> {
        let p = Path::new(path);
        if !p.exists() {
            bail!("file not found: {path}");
        }
        let meta = std::fs::metadata(p)?;
        if meta.is_dir() {
            bail!("{path} is a folder (import files, or use the media tool with a folder to import all media in it)");
        }
        let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let mtime = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let abs = std::fs::canonicalize(p).map(|p| strip_unc(&p)).unwrap_or_else(|_| path.to_string());
        if ext == "svg" {
            let data = std::fs::read(p)?;
            let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).map_err(|e| anyhow!("bad svg: {e}"))?;
            let sz = tree.size();
            return Ok(Media {
                id: String::new(),
                path: abs,
                name,
                kind: MediaKind::Svg,
                dur: 0.0,
                w: sz.width().round() as u32,
                h: sz.height().round() as u32,
                fps: 0.0,
                has_video: true,
                has_audio: false,
                alpha: true,
                channels: 0,
                rate: 0,
                codec: "svg".into(),
                size: meta.len(),
                mtime,
                gfx: None,
                tags: vec![],
                note: String::new(),
            });
        }
        self.need()?;
        let out = command(&self.ffprobe)
            .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", path])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("running ffprobe")?;
        if !out.status.success() {
            bail!("ffprobe failed for {path}: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let j: serde_json::Value = serde_json::from_slice(&out.stdout)?;
        let streams = j["streams"].as_array().cloned().unwrap_or_default();
        let fmt = &j["format"];
        let fmt_name = fmt["format_name"].as_str().unwrap_or("");
        let fdur = fmt["duration"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let v = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0);
        let a = streams.iter().find(|s| s["codec_type"] == "audio");
        let mut m = Media {
            id: String::new(),
            path: abs,
            name,
            kind: MediaKind::Video,
            dur: fdur,
            w: 0,
            h: 0,
            fps: 0.0,
            has_video: v.is_some(),
            has_audio: a.is_some(),
            alpha: false,
            channels: 0,
            rate: 0,
            codec: String::new(),
            size: meta.len(),
            mtime,
            gfx: None,
            tags: vec![],
            note: String::new(),
        };
        if let Some(a) = a {
            m.channels = a["channels"].as_u64().unwrap_or(2) as u32;
            m.rate = a["sample_rate"].as_str().and_then(|s| s.parse().ok()).unwrap_or(48000);
            if m.dur == 0.0 {
                m.dur = a["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            }
        }
        match v {
            None => {
                if a.is_none() {
                    bail!("{path} has no audio or video streams");
                }
                m.kind = MediaKind::Audio;
                m.codec = a.unwrap()["codec_name"].as_str().unwrap_or("").to_string();
            }
            Some(v) => {
                m.w = v["width"].as_u64().unwrap_or(0) as u32;
                m.h = v["height"].as_u64().unwrap_or(0) as u32;
                m.codec = v["codec_name"].as_str().unwrap_or("").to_string();
                let pix = v["pix_fmt"].as_str().unwrap_or("");
                m.alpha = pix.starts_with("yuva") || pix.starts_with("rgba") || pix.starts_with("bgra") || pix.starts_with("argb") || pix.starts_with("abgr") || pix.starts_with("ya") || pix.starts_with("gbrap") || pix == "pal8" || pix == "rgba64be" || pix == "rgba64le";
                m.fps = parse_rate(v["avg_frame_rate"].as_str().unwrap_or("0/0")).filter(|f| *f > 0.0 && *f < 1000.0).or_else(|| parse_rate(v["r_frame_rate"].as_str().unwrap_or("0/0"))).unwrap_or(30.0);
                let rot = v["side_data_list"]
                    .as_array()
                    .and_then(|l| l.iter().find_map(|sd| sd["rotation"].as_f64()))
                    .or_else(|| v["tags"]["rotate"].as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0);
                if (rot.abs() - 90.0).abs() < 1.0 || (rot.abs() - 270.0).abs() < 1.0 {
                    std::mem::swap(&mut m.w, &mut m.h);
                }
                let nb_frames = v["nb_frames"].as_str().and_then(|s| s.parse::<u64>().ok());
                let is_image_fmt = fmt_name.contains("image2") || fmt_name.ends_with("_pipe") || matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "avif" | "jxl" | "exr" | "tga");
                let single = nb_frames.map(|n| n <= 1).unwrap_or(true) && fdur < 0.05 + 1.0 / m.fps.max(1.0);
                if is_image_fmt && (single || fmt_name.contains("image2") || fmt_name.ends_with("_pipe")) && !(ext == "webp" && nb_frames.unwrap_or(1) > 1) {
                    m.kind = MediaKind::Image;
                    m.dur = 0.0;
                    m.has_audio = false;
                    if matches!(ext.as_str(), "png" | "webp" | "tif" | "tiff" | "tga") {
                        m.alpha = true;
                    }
                } else if m.dur <= 0.0 {
                    m.dur = v["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                }
            }
        }
        if m.kind != MediaKind::Image && m.dur <= 0.0 {
            bail!("couldn't determine the duration of {path}");
        }
        Ok(m)
    }

    /// Runs ffmpeg with args, capturing stderr. Returns (stdout, stderr).
    pub fn run_capture(&self, args: &[String]) -> Result<(Vec<u8>, String)> {
        self.need()?;
        let out = command(&self.ffmpeg).args(["-hide_banner", "-nostdin"]).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() {
            let tail: Vec<&str> = err.lines().rev().take(6).collect();
            bail!("ffmpeg failed: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | "));
        }
        Ok((out.stdout, err))
    }
}

pub fn parse_rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let n: f64 = n.parse().ok()?;
    let d: f64 = d.parse().ok()?;
    if d == 0.0 {
        None
    } else {
        Some(n / d)
    }
}

/// Removes the `\\?\` prefix Windows' canonicalize adds.
pub fn strip_unc(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").map(|x| x.to_string()).unwrap_or(s)
}

/// Escapes a value for use inside an FFmpeg filtergraph option.
pub fn esc_filter(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.replace('\\', "/").chars() {
        match c {
            ':' | '\'' | ',' | ';' | '[' | ']' | '=' => {
                o.push('\\');
                o.push(c);
            }
            _ => o.push(c),
        }
    }
    o
}
