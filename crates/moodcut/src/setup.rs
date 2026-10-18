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
            println!("✔ FFmpeg {} at {}", ff.version, ff.ffmpeg.display());
            for (enc, what) in [("libx264", "H.264"), ("libx265", "HEVC"), ("libsvtav1", "AV1"), ("libvpx-vp9", "VP9"), ("prores_ks", "ProRes"), ("aac", "AAC"), ("libmp3lame", "MP3"), ("libopus", "Opus")] {
                println!("  {} {what} ({enc})", if ff.has_encoder(enc) { "✔" } else { "✘" });
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
    println!("\nClaude Desktop: run `moodcut install-claude`. ChatGPT: run `moodcut serve --public` behind an HTTPS tunnel (see README).");
    Ok(())
}

fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::var("PATH").ok().and_then(|p| std::env::split_paths(&p).map(|d| d.join(&exe)).find(|p| p.is_file()))
}

pub fn setup() -> Result<()> {
    if Ff::find().is_ok() {
        println!("FFmpeg is already installed.");
        return doctor();
    }
    if cfg!(windows) {
        println!("Installing FFmpeg with winget (Gyan.FFmpeg)…");
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
    } else if cfg!(target_os = "macos") {
        println!("Install FFmpeg with: brew install ffmpeg");
    } else {
        println!("Install FFmpeg with your package manager, e.g.: sudo apt install ffmpeg");
    }
    Ok(())
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
