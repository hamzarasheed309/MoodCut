//! Environment checks and installers: `doctor`, `setup`, `install-claude`,
//! plus Windows console handling for the GUI mode.

use crate::render::ffmpeg::{command, Ff};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Stdio;

/// In GUI mode on Windows, hide the console window if we own it (double-click launch).
pub fn detach_console_if_owned() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
        let mut pids = [0u32; 4];
        let n = GetConsoleProcessList(pids.as_mut_ptr(), 4);
        if n <= 1 {
            FreeConsole();
        }
    }
}

pub fn doctor() -> Result<()> {
    println!("MoodCut {}", crate::VERSION);
    match Ff::find() {
        Ok(ff) => {
            println!("✔ FFmpeg {} at {}", ff.version(), ff.ffmpeg_path().display());
            for (enc, what) in [("libx264", "H.264"), ("libx265", "HEVC"), ("libvpx-vp9", "VP9"), ("prores_ks", "ProRes"), ("aac", "AAC"), ("libmp3lame", "MP3"), ("libopus", "Opus")] {
                println!("  {} {what} ({enc})", if ff.has_encoder(enc) { "✔" } else { "✘" });
            }
            match ["libsvtav1", "libaom-av1"].into_iter().find(|e| ff.has_encoder(e)) {
                Some(e) => println!("  ✔ AV1 ({e})"),
                None => println!("  – AV1 (not in this FFmpeg build; the av1 preset needs a full build or a GPU AV1 encoder)"),
            }
            for (f, what) in [("rubberband", "pitch shifting"), ("sidechaincompress", "auto ducking"), ("loudnorm", "loudness normalization"), ("whisper", "built-in speech-to-text (FFmpeg 8+)"), ("minterpolate", "optical-flow slow motion")] {
                println!("  {} {what} ({f})", if ff.has_filter(f) { "✔" } else { "–" });
            }
            let hw = ff.hw_encoders();
            if hw.is_empty() {
                println!("  – no working GPU encoder (CPU encoding is used)");
            } else {
                println!("  ✔ GPU encoders: {} (use hw=auto when rendering)", hw.join(", "));
            }
        }
        Err(e) => println!("✘ {e}"),
    }
    let fams = crate::render::text::families();
    println!("✔ {} font families available (Inter bundled)", fams.len());
    let has_whisper = ["whisper-cli", "whisper-cpp", "whisper"].iter().any(|b| which(b).is_some());
    println!("{} speech-to-text CLI (whisper.cpp or openai-whisper){}", if has_whisper { "✔" } else { "–" }, if has_whisper { "" } else { " — optional, needed for captions/transcripts" });
    if let Some(d) = crate::config::data_dir() {
        println!("  data dir: {}", d.display());
    }
    println!("\nClaude Desktop: run `moodcut install-claude`. claude.ai / ChatGPT on the web: run `moodcut connect`.");
    Ok(())
}

pub fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::var("PATH").ok().and_then(|p| std::env::split_paths(&p).map(|d| d.join(&exe)).find(|p| p.is_file()))
}

/// curl / tar: on PATH, or Windows 10+'s built-in copies (AI clients may start
/// MoodCut with a minimal environment).
pub fn sys_tool(name: &str) -> Option<PathBuf> {
    which(name).or_else(|| {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        Some(PathBuf::from(root).join("System32").join(format!("{name}.exe"))).filter(|p| cfg!(windows) && p.is_file())
    })
}

/// Portable FFmpeg builds for this platform, best first.
fn ffmpeg_downloads() -> Vec<&'static str> {
    if cfg!(all(windows, target_arch = "aarch64")) {
        vec!["https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-winarm64-gpl.zip"]
    } else if cfg!(windows) {
        vec!["https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip", "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"]
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        vec!["https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linuxarm64-gpl.tar.xz"]
    } else if cfg!(target_os = "linux") {
        vec!["https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"]
    } else {
        vec![]
    }
}

/// Downloads a portable FFmpeg into `<data dir>/ffmpeg/bin`, where MoodCut
/// looks first: no admin rights, no PATH change, works immediately.
/// `progress` gets short status lines ("downloading 42 MB").
pub fn install_ffmpeg(progress: &(dyn Fn(String) + Sync)) -> Result<PathBuf> {
    let urls = ffmpeg_downloads();
    if urls.is_empty() {
        bail!("no portable FFmpeg for this platform; install it with your package manager (macOS: brew install ffmpeg)");
    }
    let curl = sys_tool("curl").context("curl is needed to download FFmpeg (built into Windows 10 and later)")?;
    let tar = sys_tool("tar").context("tar is needed to unpack FFmpeg (built into Windows 10 and later)")?;
    let home = crate::config::ensure_data_dir();
    let work = home.join("ffmpeg-download");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let mut last_err = String::new();
    for url in urls {
        let name = url.rsplit('/').next().unwrap_or("ffmpeg.zip");
        let archive = work.join(name);
        progress(format!("downloading {name}"));
        let mut child = command(&curl).args(["-L", "--fail", "--silent", "--show-error", "--retry", "2", "-o"]).arg(&archive).arg(url).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("running curl")?;
        let st = loop {
            if let Some(st) = child.try_wait()? {
                break st;
            }
            let mb = std::fs::metadata(&archive).map(|m| m.len() / 1_000_000).unwrap_or(0);
            progress(format!("downloading {name}: {mb} MB"));
            std::thread::sleep(std::time::Duration::from_millis(500));
        };
        if !st.success() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut e, &mut err);
            }
            last_err = format!("{name}: {}", err.trim());
            continue;
        }
        progress(format!("unpacking {name}"));
        let out = work.join("x");
        std::fs::create_dir_all(&out)?;
        let st = command(&tar).arg("-xf").arg(&archive).arg("-C").arg(&out).stdout(Stdio::null()).stderr(Stdio::piped()).status().context("running tar")?;
        let _ = std::fs::remove_file(&archive);
        if !st.success() {
            last_err = format!("couldn't unpack {name}");
            continue;
        }
        let Some(bin) = find_ffmpeg_bin(&out, 0) else {
            last_err = format!("{name} has no ffmpeg/ffprobe");
            continue;
        };
        let dest = home.join("ffmpeg").join("bin");
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(home.join("ffmpeg"))?;
        if std::fs::rename(&bin, &dest).is_err() {
            // different volume or locked: copy the files instead
            std::fs::create_dir_all(&dest)?;
            for e in std::fs::read_dir(&bin)?.flatten() {
                if e.path().is_file() {
                    std::fs::copy(e.path(), dest.join(e.file_name()))?;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&work);
        let ff = Ff::detect().context("FFmpeg was unpacked but doesn't run")?;
        progress(format!("FFmpeg {} ready", ff.version()));
        return Ok(dest);
    }
    let _ = std::fs::remove_dir_all(&work);
    bail!("couldn't download FFmpeg ({last_err})")
}

/// The folder in an unpacked archive that holds ffmpeg and ffprobe.
fn find_ffmpeg_bin(dir: &std::path::Path, depth: usize) -> Option<PathBuf> {
    let exe = |n: &str| if cfg!(windows) { format!("{n}.exe") } else { n.to_string() };
    if dir.join(exe("ffmpeg")).is_file() && dir.join(exe("ffprobe")).is_file() {
        return Some(dir.to_path_buf());
    }
    if depth > 3 {
        return None;
    }
    std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.path().is_dir()).find_map(|e| find_ffmpeg_bin(&e.path(), depth + 1))
}

pub fn setup(force: bool) -> Result<()> {
    if !force && Ff::find().is_ok() {
        println!("FFmpeg is already installed.");
        return doctor();
    }
    if cfg!(target_os = "macos") {
        println!("Install FFmpeg with: brew install ffmpeg");
        return Ok(());
    }
    println!("Installing a portable FFmpeg into {} …", crate::config::ensure_data_dir().join("ffmpeg").display());
    let last = std::sync::Mutex::new(String::new());
    let shown = |s: String| {
        let mut l = last.lock().unwrap();
        if *l != s {
            use std::io::Write;
            eprint!("\r{s}          ");
            let _ = std::io::stderr().flush();
            *l = s;
        }
    };
    match install_ffmpeg(&shown) {
        Ok(dir) => {
            eprintln!();
            println!("Installed to {}", dir.display());
            doctor()
        }
        Err(e) if cfg!(windows) => {
            eprintln!();
            println!("{e:#}\nTrying winget (Gyan.FFmpeg) instead…");
            let st = command(std::path::Path::new("winget"))
                .args(["install", "--id", "Gyan.FFmpeg", "-e", "--accept-source-agreements", "--accept-package-agreements"])
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .context("running winget (install App Installer from the Microsoft Store if missing)")?;
            if !st.success() {
                bail!("winget failed; download FFmpeg from https://www.gyan.dev/ffmpeg/builds/ and set MOODCUT_FFMPEG to its bin folder");
            }
            println!("Done. Open a new terminal so PATH updates, then run `moodcut doctor`.");
            Ok(())
        }
        Err(e) => {
            eprintln!();
            Err(e.context("install FFmpeg with your package manager instead, e.g.: sudo apt install ffmpeg"))
        }
    }
}

pub fn claude_config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join("Claude").join("claude_desktop_config.json"))
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library/Application Support/Claude/claude_desktop_config.json"))
    } else {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config/Claude/claude_desktop_config.json"))
    }
}

pub fn install_claude() -> Result<()> {
    let path = claude_config_path().context("can't locate the Claude Desktop config folder")?;
    let exe = std::env::current_exe()?;
    let exe_s = crate::render::ffmpeg::strip_unc(&exe);
    let mut cfg: serde_json::Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| serde_json::json!({}));
    if !cfg.is_object() {
        cfg = serde_json::json!({});
    }
    let servers = cfg.as_object_mut().unwrap().entry("mcpServers").or_insert_with(|| serde_json::json!({}));
    servers["moodcut"] = serde_json::json!({"command": exe_s, "args": ["mcp"]});
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg)?)?;
    println!("Registered MoodCut in {}", path.display());
    println!("Restart Claude Desktop. Open http://127.0.0.1:7878 to watch edits live.");
    println!("Claude Code: claude mcp add moodcut -- \"{}\" mcp", exe_s);
    Ok(())
}
