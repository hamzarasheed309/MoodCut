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
    let apps: Vec<String> = crate::clients::statuses().into_iter().filter(|s| s.registered).map(|s| s.name.to_string()).collect();
    if apps.is_empty() {
        println!("\nAI apps: none connected yet. Run `moodcut install-ai` (Claude Desktop, Claude Code, Cursor, VS Code, ...); claude.ai / ChatGPT on the web: `moodcut connect`.");
    } else {
        println!("\nAI apps connected: {}. More: `moodcut install-ai --list`; claude.ai / ChatGPT on the web: `moodcut connect`.", apps.join(", "));
    }
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
        match install_ffmpeg_archive(&archive, &home.join("ffmpeg").join("bin"), progress) {
            Ok(dest) => {
                let _ = std::fs::remove_dir_all(&work);
                return Ok(dest);
            }
            Err(e) => last_err = format!("{e:#}"),
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    bail!("couldn't download FFmpeg ({last_err})")
}

/// Unpacks an FFmpeg build archive (.zip / .tar.xz, as downloaded by the
/// installer or by hand) and puts its ffmpeg + ffprobe into `dest`.
pub fn install_ffmpeg_archive(archive: &std::path::Path, dest: &std::path::Path, progress: &(dyn Fn(String) + Sync)) -> Result<PathBuf> {
    let tar = sys_tool("tar").context("tar is needed to unpack FFmpeg (built into Windows 10 and later)")?;
    let name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !archive.is_file() {
        bail!("{} not found", archive.display());
    }
    progress(format!("unpacking {name}"));
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| crate::config::ensure_data_dir().join("ffmpeg"));
    std::fs::create_dir_all(&parent).with_context(|| format!("creating {}", parent.display()))?;
    let out = parent.join(".unpack");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;
    let st = command(&tar).arg("-xf").arg(archive).arg("-C").arg(&out).stdout(Stdio::null()).stderr(Stdio::piped()).status().context("running tar")?;
    if !st.success() {
        let _ = std::fs::remove_dir_all(&out);
        bail!("couldn't unpack {name}");
    }
    let Some(bin) = find_ffmpeg_bin(&out, 0) else {
        let _ = std::fs::remove_dir_all(&out);
        bail!("{name} has no ffmpeg/ffprobe");
    };
    let _ = std::fs::remove_dir_all(dest);
    if std::fs::rename(&bin, dest).is_err() {
        // different volume or locked: copy the files instead
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(&bin)?.flatten() {
            if e.path().is_file() {
                std::fs::copy(e.path(), dest.join(e.file_name()))?;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    // check the unpacked copy itself (another FFmpeg may come first on PATH)
    let exe = dest.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
    let out = command(&exe).arg("-version").stdout(Stdio::piped()).stderr(Stdio::null()).output().context("FFmpeg was unpacked but doesn't run")?;
    let first = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string();
    if !out.status.success() || !first.starts_with("ffmpeg version") {
        bail!("FFmpeg was unpacked to {} but doesn't run", dest.display());
    }
    let version = first.split_whitespace().nth(2).unwrap_or("?").to_string();
    progress(format!("FFmpeg {version} ready"));
    Ok(dest.to_path_buf())
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

pub fn setup(force: bool, from: Option<&str>, dest: Option<&str>) -> Result<()> {
    if let Some(archive) = from {
        // the installer downloads FFmpeg with its own progress bar and hands it over
        let dest = dest.map(PathBuf::from).unwrap_or_else(|| crate::config::ensure_data_dir().join("ffmpeg").join("bin"));
        let dir = install_ffmpeg_archive(std::path::Path::new(archive), &dest, &|s: String| eprintln!("{s}"))?;
        println!("Installed FFmpeg to {}", dir.display());
        return Ok(());
    }
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

/// `moodcut install-ai [apps]`: registers MoodCut with the given AI apps, or
/// with every one found on this computer. `quiet` skips the summary (installer).
pub fn install_ai(ids: &[String], quiet: bool) -> Result<()> {
    ai_apply(ids, false, quiet)
}

/// `moodcut uninstall-ai [apps]`: removes MoodCut from the AI apps' settings.
pub fn uninstall_ai(ids: &[String]) -> Result<()> {
    ai_apply(ids, true, false)
}

fn ai_apply(ids: &[String], remove: bool, quiet: bool) -> Result<()> {
    use crate::clients;
    let targets: Vec<&'static clients::Client> = if ids.is_empty() || ids.iter().any(|i| i.eq_ignore_ascii_case("all")) {
        let all = ids.iter().any(|i| i.eq_ignore_ascii_case("all"));
        // removing: everywhere MoodCut is registered; adding: every app found (or all when asked)
        clients::CLIENTS.iter().filter(|c| { let st = clients::status(c); if remove { st.registered } else { all || st.installed } }).collect()
    } else {
        ids.iter()
            .map(|i| clients::find(i).ok_or_else(|| anyhow::anyhow!("unknown AI app '{i}' (known: {})", clients::CLIENTS.iter().map(|c| c.id).collect::<Vec<_>>().join(", "))))
            .collect::<Result<_>>()?
    };
    if targets.is_empty() {
        if !quiet {
            println!("{}", if remove { "MoodCut isn't registered with any AI app." } else { "No AI apps found on this computer. Install Claude Desktop, Cursor, VS Code... and run this again, or name one: moodcut install-ai claude" });
        }
        return Ok(());
    }
    let mut failed = 0;
    for c in targets {
        match clients::apply(c, remove) {
            Ok(line) => println!("✔ {line}"),
            Err(e) => {
                failed += 1;
                println!("✘ {}: {e:#}", c.name);
            }
        }
    }
    if !quiet && !remove {
        println!("\nEdits the AI makes show up live in the MoodCut window (open it any time). claude.ai / ChatGPT on the web: moodcut connect");
    }
    if failed > 0 && !quiet {
        bail!("{failed} app(s) couldn't be set up (see above)");
    }
    Ok(())
}

/// `moodcut install-ai --list`
pub fn list_ai() -> Result<()> {
    for st in crate::clients::statuses() {
        let mark = if st.registered && !st.stale { "✔" } else if st.installed { "○" } else { "–" };
        let what = match (st.installed, st.registered, st.stale) {
            (_, true, true) => "registered, but points at another MoodCut (run install-ai again)",
            (_, true, false) => "connected",
            (true, false, _) => "found, not connected",
            _ => "not found",
        };
        println!("{mark} {:<18} {:<12} {}  [{}]", st.name, st.id, what, st.config.unwrap_or_default());
    }
    Ok(())
}
