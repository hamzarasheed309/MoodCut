//! MoodCut — AI-native FFmpeg video editor & motion graphics engine.
//!
//! One executable:
//!   moodcut gui        open the editor (also serves MCP over HTTP)
//!   moodcut mcp        MCP over stdio (Claude Desktop / Claude Code / Cursor ...)
//!   moodcut serve      headless HTTP server (GUI + REST + MCP streamable HTTP)
//!   moodcut render     render a .mcut project from the command line
//!   moodcut run        apply a MoodCut Script to a project file
//!   moodcut doctor     check FFmpeg, encoders, GPU, fonts, speech-to-text

pub mod analyze;
pub mod brand;
pub mod cache;
pub mod check;
pub mod clients;
pub mod config;
pub mod engine;
pub mod export;
pub mod gfx;
pub mod library;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod projfile;
pub mod render;
pub mod server;
pub mod setup;
pub mod sfx;
pub mod shell;
pub mod tunnel;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "moodcut", version, about = "MoodCut — AI-native video editor & motion graphics engine (FFmpeg + MCP)", after_help = "A Moodbook Studio project — https://moodbook.uk")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the editor window (and serve MCP over HTTP for ChatGPT/Claude).
    Gui {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Project to open
        project: Option<String>,
    },
    /// Headless server: GUI + REST + MCP (streamable HTTP at /mcp).
    Serve {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Require this bearer token for /mcp and the API (auto-generated with --public).
        #[arg(long)]
        token: Option<String>,
        /// Listen on all interfaces and require a token (for tunnels / ChatGPT connectors).
        #[arg(long)]
        public: bool,
        /// Limit file access to these folders (repeatable).
        #[arg(long = "allow")]
        allow: Vec<String>,
        project: Option<String>,
    },
    /// Make MoodCut reachable from claude.ai / ChatGPT on the web: starts the editor plus an HTTPS
    /// tunnel and prints a secret connector URL to paste into the AI's connector settings.
    Connect {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Your own public https address (named Cloudflare tunnel, Tailscale Funnel, reverse proxy)
        /// that forwards to this port, instead of a temporary quick tunnel.
        #[arg(long)]
        url: Option<String>,
        /// Make a new secret (old connector URLs stop working).
        #[arg(long)]
        new_token: bool,
        /// Don't open the editor window.
        #[arg(long)]
        no_window: bool,
        project: Option<String>,
    },
    /// MCP over stdio. Attaches to a running MoodCut if one is listening, otherwise runs embedded.
    Mcp {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Don't start the local GUI/HTTP server alongside stdio.
        #[arg(long)]
        no_http: bool,
    },
    /// Render a project file.
    Render {
        project: String,
        #[arg(short, long)]
        out: Option<String>,
        #[arg(short, long, default_value = "mp4")]
        preset: String,
        /// Time range a-b in seconds, or `work` for the work area
        #[arg(long)]
        range: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        scale: f32,
        #[arg(long)]
        comp: Option<String>,
        /// GPU encoder: auto (the best GPU's) | off (CPU) | nvenc | qsv | amf
        #[arg(long, default_value = "auto")]
        hw: String,
    },
    /// Apply a MoodCut Script (inline or @file) to a project and save it.
    Run {
        project: String,
        script: String,
        /// Create the project if it doesn't exist (WxH@fps, e.g. 1920x1080@30)
        #[arg(long)]
        create: Option<String>,
    },
    /// Render a single frame to an image.
    Frame {
        project: String,
        #[arg(short, long, default_value_t = 0.0)]
        t: f64,
        #[arg(short, long, default_value = "frame.png")]
        out: String,
        #[arg(long, default_value_t = 1280)]
        width: u32,
    },
    /// Render an HTML/CSS/JS motion graphic to video with alpha (or a still), optionally into a project.
    Graphic {
        /// The .html file (full page or body fragment).
        html: String,
        /// Output file: .mov (PNG with alpha) or .png for a still. Not needed with --project.
        #[arg(short, long)]
        out: Option<String>,
        /// Seconds (0 = still image).
        #[arg(long, default_value_t = 5.0)]
        dur: f64,
        #[arg(long, default_value = "1920x1080")]
        size: String,
        #[arg(long, default_value_t = 30.0)]
        fps: f64,
        /// Keep the page background instead of transparency.
        #[arg(long)]
        opaque: bool,
        /// For a still (--dur 0): the animation time to capture, in seconds.
        #[arg(long)]
        at: Option<f64>,
        /// Import into this project and save it.
        #[arg(long)]
        project: Option<String>,
        /// With --project: also place it, e.g. "V3 at=2".
        #[arg(long)]
        place: Option<String>,
        /// key=value for {{key}} placeholders (repeatable).
        #[arg(long = "var")]
        vars: Vec<String>,
    },
    /// Quality-check an edit (black gaps, title-safe, contrast, loudness, ...) and optionally fix it.
    Check {
        project: String,
        /// web (default) | youtube | social | podcast | broadcast
        #[arg(long)]
        target: Option<String>,
        /// Apply the proposed fixes and save.
        #[arg(long)]
        fix: bool,
        /// Skip measuring the audio mix.
        #[arg(long)]
        no_audio: bool,
        /// Expected duration in seconds.
        #[arg(long)]
        dur: Option<f64>,
        /// Expected size, e.g. 1080x1920.
        #[arg(long)]
        size: Option<String>,
        /// Exit with status 1 when errors are found (for scripts/CI).
        #[arg(long)]
        strict: bool,
    },
    /// Check FFmpeg, encoders, GPU, fonts and speech-to-text.
    Doctor,
    /// Measure compositor speed (no encoding).
    #[command(hide = true)]
    Bench {
        project: String,
        #[arg(long, default_value = "0-5")]
        range: String,
        #[arg(long, default_value_t = 1.0)]
        scale: f32,
    },
    /// Print the MoodCut Script reference as Markdown.
    #[command(hide = true)]
    Docs,
    /// Regenerate the logo, icons and installer artwork from the brand definition (brand.rs).
    #[command(hide = true)]
    Brand {
        /// The repository folder.
        #[arg(default_value = ".")]
        root: String,
    },
    /// Install a portable FFmpeg (no admin rights needed) and check the setup.
    Setup {
        /// Download the portable FFmpeg even if one is already installed (to update it)
        #[arg(long)]
        force: bool,
        /// Install from an FFmpeg build archive you already have (.zip / .tar.xz) instead of downloading.
        #[arg(long)]
        from: Option<String>,
        /// With --from: the folder for ffmpeg and ffprobe (default: the data folder).
        #[arg(long)]
        dest: Option<String>,
    },
    /// Connect MoodCut to the AI apps on this computer: Claude Desktop, Claude Code, Cursor,
    /// VS Code, Windsurf, LM Studio, Gemini CLI, Codex. No names = every app that's installed.
    InstallAi {
        /// Apps to connect (claude, claude-code, cursor, vscode, windsurf, lmstudio, gemini, codex, all).
        apps: Vec<String>,
        /// Show which apps are installed and connected.
        #[arg(long)]
        list: bool,
        /// No summary (for installers).
        #[arg(long)]
        quiet: bool,
    },
    /// Remove MoodCut from the AI apps' settings (all of them when no names are given).
    UninstallAi {
        apps: Vec<String>,
        /// Also every other profile on this PC that MoodCut connected (the Windows uninstaller).
        #[arg(long, hide = true)]
        all_profiles: bool,
        /// Leave entries that point at another MoodCut executable alone.
        #[arg(long, hide = true)]
        only_this_exe: bool,
    },
    /// Register MoodCut in Claude Desktop (same as `install-ai claude`).
    InstallClaude,
}

fn main() {
    install_crash_log();
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// Every panic (caught or not) is also appended to `crash.log` in the data folder,
/// with its location and a backtrace: the editor window has no console to show it.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        use std::io::Write;
        let f = config::ensure_data_dir().join("crash.log");
        // keep it small: start over past 1 MB
        if std::fs::metadata(&f).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::remove_file(&f);
        }
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let entry = format!("--- {} UTC  MoodCut {VERSION}  thread '{thread}'\n{info}\n{}\n", render::encode::chrono_like(), std::backtrace::Backtrace::force_capture());
        if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
            let _ = fh.write_all(entry.as_bytes());
        }
    }));
}

/// Opens the project the editor was started with; a damaged or missing file must
/// not keep the editor from starting, so the problem is shown in it instead.
fn open_at_start(eng: &engine::Engine, path: &str) {
    if let Err(e) = eng.open(path) {
        let msg = format!("{e:#}");
        eprintln!("moodcut: {msg}");
        *eng.notice.lock() = Some(msg);
    }
}

/// Starts the engine; without FFmpeg it still runs graphics-only work and
/// media operations explain how to install it.
fn load_engine() -> Result<std::sync::Arc<engine::Engine>> {
    let ff = render::ffmpeg::Ff::find().unwrap_or_else(|e| {
        eprintln!("warning: {e}");
        render::ffmpeg::Ff::missing()
    });
    Ok(engine::Engine::new(ff))
}

fn parse_range(r: &str) -> Result<(f64, f64)> {
    let (a, b) = r.split_once('-').context("range must be a-b")?;
    Ok((a.trim().parse()?, b.trim().parse()?))
}

/// The editor window onto the server at `port`: MoodCut's own native window where there is one
/// (Windows), which ends the process when it is closed; else a browser app window.
fn show_window(port: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}");
    let e = shell::run(&url, port);
    if cfg!(windows) {
        shell::report_start_error(&format!("MoodCut couldn't open its Windows app window.\n\n{e:#}\n\nInstall or repair the Microsoft Edge WebView2 Runtime, then try again."));
        return Err(e.context("couldn't open the native MoodCut window"));
    }
    server::open_window(&url);
    Ok(())
}

/// A second launch: bring the editor that is already open to the front. A server without a
/// window of its own (started by an AI app or `moodcut serve`) gets one here.
fn show_editor(port: u16) -> Result<()> {
    shell::allow_foreground();
    let native = server::local_request(port, "POST", "/api/shell", Some(r#"{"action":"focus"}"#), 1500)
        .and_then(|(_, body)| serde_json::from_str::<serde_json::Value>(&body).ok())
        .is_some_and(|v| v["native"] == true);
    if !native {
        show_window(port)?;
    }
    Ok(())
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.unwrap_or(Cmd::Gui { port: 7878, project: None }) {
        Cmd::Gui { port, project } => {
            setup::detach_console_if_owned();
            // one editor at a time: a second launch (or a double-clicked .mcut) goes to the open one
            if let Some((p, _)) = server::find_instance(port) {
                if let Some(path) = &project {
                    let abs = std::fs::canonicalize(path).map(|a| render::ffmpeg::strip_unc(&a)).unwrap_or_else(|_| path.clone());
                    server::local_request(p, "POST", "/api/project", Some(&serde_json::json!({"action": "open", "path": abs, "source": "launch"}).to_string()), 3000);
                }
                show_editor(p)?;
                return Ok(());
            }
            let port = server::free_port(port);
            let project_path = project.as_ref().map(|p| std::fs::canonicalize(p).map(|a| render::ffmpeg::strip_unc(&a)).unwrap_or_else(|_| p.clone()));
            let eng = load_engine()?;
            if let Some(p) = project {
                open_at_start(&eng, &p);
            }
            // the engine and its server run on a background thread; this one is the window's
            let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
            std::thread::Builder::new().name("server".into()).spawn(move || {
                let r = tokio::runtime::Runtime::new().map_err(anyhow::Error::from).and_then(|rt| {
                    rt.block_on(async move {
                        let opts = server::ServeOpts { host: "127.0.0.1".into(), port, token: None, open_window: false, tunnel: false, public_url: None };
                        server::serve(eng, opts).await
                    })
                });
                let _ = tx.send(r);
            })?;
            // wait until it answers (or has already failed)
            let t0 = std::time::Instant::now();
            let ended = loop {
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(r) => break Some(r),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => bail!("the editor's server stopped unexpectedly"),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if server::running_instance(port).is_some() {
                            break None;
                        }
                        if t0.elapsed() > std::time::Duration::from_secs(20) {
                            bail!("the editor's server didn't start within 20 seconds");
                        }
                    }
                }
            };
            if let Some(r) = ended {
                // started at the same moment as another MoodCut (two files opened at once):
                // the other one got the port, so hand this file over to it
                if let Err(e) = &r {
                    if e.to_string().contains("can't listen") {
                        std::thread::sleep(std::time::Duration::from_millis(400));
                        if let Some((p, _)) = server::find_instance(port) {
                            if let Some(path) = &project_path {
                                server::local_request(p, "POST", "/api/project", Some(&serde_json::json!({"action": "open", "path": path, "source": "launch"}).to_string()), 3000);
                            }
                            show_editor(p)?;
                            return Ok(());
                        }
                    }
                }
                return r;
            }
            // closing the window quits MoodCut, like any desktop editor
            show_window(port)?;
            rx.recv().unwrap_or(Ok(()))
        }
        Cmd::Serve { port, host, token, public, allow, project } => {
            let eng = load_engine()?;
            if let Some(p) = project {
                open_at_start(&eng, &p);
            }
            if !allow.is_empty() {
                let roots: Vec<PathBuf> = allow.iter().map(|a| engine::resolve_path(std::path::Path::new(a))).collect();
                *eng.roots.write() = Some(roots);
            }
            let (host, token) = if public { ("0.0.0.0".to_string(), Some(token.unwrap_or_else(server::random_token))) } else { (host, token) };
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move { server::serve(eng, server::ServeOpts { host, port, token, open_window: false, tunnel: false, public_url: None }).await })
        }
        Cmd::Connect { port, url, new_token, no_window, project } => {
            if new_token {
                let _ = std::fs::remove_file(config::ensure_data_dir().join("remote-token"));
            }
            // the editor is already open: give it the web link instead of starting a second one
            if let Some((p, _)) = server::find_instance(port) {
                if new_token {
                    eprintln!("note: the open editor keeps its current secret until it restarts");
                }
                let body = match &url {
                    Some(u) => serde_json::json!({"action": "url", "url": u}),
                    None => serde_json::json!({"action": "start"}),
                };
                eprintln!("MoodCut is already open (port {p}); {} …", if url.is_some() { "setting its public address" } else { "starting a web link for it" });
                // the first start may download cloudflared and wait up to 45 s for the tunnel
                let (_, text) = server::local_request(p, "POST", "/api/tunnel", Some(&body.to_string()), 15000).ok_or_else(|| anyhow::anyhow!("the open MoodCut didn't answer"))?;
                let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                let connector = v["connector"].as_str().ok_or_else(|| anyhow::anyhow!("{}", v["error"].as_str().unwrap_or("couldn't start the web link")))?;
                println!("{}", tunnel::instructions(connector));
                if tunnel::copy_to_clipboard(connector) {
                    eprintln!("(connector URL copied to the clipboard)");
                }
                if !no_window {
                    server::open_window(&format!("http://127.0.0.1:{p}"));
                }
                return Ok(());
            }
            let port = server::free_port(port);
            let eng = load_engine()?;
            if let Some(p) = project {
                open_at_start(&eng, &p);
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                let opts = server::ServeOpts { host: "127.0.0.1".into(), port, token: None, open_window: !no_window, tunnel: url.is_none(), public_url: url };
                server::serve(eng, opts).await
            })
        }
        Cmd::Mcp { port, no_http } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(mcp::stdio::run(port, !no_http))
        }
        Cmd::Render { project, out, preset, range, scale, comp, hw } => {
            let eng = load_engine()?;
            eng.open(&project)?;
            let comp = comp.unwrap_or_default();
            let (t0, t1) = match range {
                Some(r) => eng.render_range(&comp, &r)?,
                None => (0.0, 0.0),
            };
            let opts = render::encode::RenderOpts { comp, t0, t1, preset, out: out.map(PathBuf::from).unwrap_or_default(), scale, hw };
            let job = eng.start_render(opts)?;
            let mut last = String::new();
            loop {
                let st = job.status();
                if st["state"] == "running" {
                    let line = format!("\r{} {}% {} eta {}s   ", st["state"].as_str().unwrap_or(""), st["pct"], st["frames"].as_str().unwrap_or(""), st["eta_s"]);
                    if line != last {
                        eprint!("{line}");
                        last = line;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    continue;
                }
                if !last.is_empty() {
                    eprintln!();
                }
                if st["state"] == "error" {
                    bail!("{}", st["error"].as_str().unwrap_or("render failed"));
                }
                println!("{}", serde_json::to_string_pretty(&st)?);
                break;
            }
            Ok(())
        }
        Cmd::Run { project, script, create } => {
            let eng = load_engine()?;
            if std::path::Path::new(&project).exists() {
                eng.open(&project)?;
            } else {
                let spec = create.unwrap_or_else(|| "1920x1080@30".into());
                let (size, fps) = spec.split_once('@').unwrap_or((spec.as_str(), "30"));
                let (w, h) = ops::exec::parse_size(size).context("bad --create size")?;
                let name = std::path::Path::new(&project).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
                eng.new_project(&name, w, h, fps.parse()?, Some(PathBuf::from(&project)))?;
            }
            let text = if let Some(f) = script.strip_prefix('@') { std::fs::read_to_string(f)? } else { script };
            let out = eng.edit(&text, "cli")?;
            for l in &out.log {
                println!("{l}");
            }
            println!("{}", eng.save(Some(&project))?);
            Ok(())
        }
        Cmd::Frame { project, t, out, width } => {
            let eng = load_engine()?;
            eng.open(&project)?;
            let pm = eng.frame(None, t, width, false)?;
            let bytes = if out.ends_with(".jpg") || out.ends_with(".jpeg") { render::encode::encode_jpeg(&pm, 90)? } else { render::encode::encode_png(&pm)? };
            std::fs::write(&out, bytes)?;
            println!("wrote {out} ({}x{})", pm.width(), pm.height());
            Ok(())
        }
        Cmd::Graphic { html, out, dur, size, fps, opaque, at, project, place, vars } => {
            let src = std::fs::read_to_string(&html).with_context(|| format!("reading {html}"))?;
            let (w, h) = ops::exec::parse_size(&size).ok_or_else(|| anyhow::anyhow!("bad size {size}"))?;
            let vars: std::collections::BTreeMap<String, String> = vars.iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
            match project {
                Some(proj) => {
                    let eng = load_engine()?;
                    if std::path::Path::new(&proj).exists() {
                        eng.open(&proj)?;
                    } else {
                        eng.new_project("Untitled", w, h, fps, Some(PathBuf::from(&proj)))?;
                    }
                    let name = std::path::Path::new(&html).file_stem().map(|s| s.to_string_lossy().to_string());
                    let req = gfx::GraphicReq { html: Some(src), name, dur: Some(dur), size: Some((w, h)), fps: Some(fps), alpha: Some(!opaque), vars, place, preview: 0, at, ..Default::default() };
                    let o = eng.graphic(req, "cli")?;
                    println!("{}", o.text);
                    println!("{}", eng.save(None)?);
                }
                None => {
                    let ff = if dur > 0.0 { Some(render::ffmpeg::Ff::find()?) } else { render::ffmpeg::Ff::find().ok() };
                    let out = PathBuf::from(out.unwrap_or_else(|| format!("{}.{}", html.trim_end_matches(".html"), if dur > 0.0 { "mov" } else { "png" })));
                    let page = gfx::build_page(&src, &vars, &gfx::PageOpts { w, h, fps, dur, extra_css: "", base: "", preview: false });
                    let dir = std::path::Path::new(&html).parent().map(|d| d.to_path_buf()).filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from("."));
                    let spec = gfx::RenderSpec { html: &page.html, dir: &dir, media: Default::default(), w, h, fps, dur, alpha: !opaque, vars: &page.vars, keep: (0..6).map(|i| (i as f64 + 0.5) * dur / 6.0).collect(), still_at: at.unwrap_or(0.0) };
                    let r = gfx::render(ff.as_ref(), &spec, &out, None)?;
                    println!("wrote {} ({} frames in {:.1}s)", out.display(), r.frames, r.secs);
                    if r.empty {
                        println!("! nothing visible was rendered");
                    }
                    for e in r.errors {
                        println!("! {e}");
                    }
                    if !page.missing.is_empty() {
                        println!("! unset fields (rendered empty): {} (pass --var name=value)", page.missing.join(", "));
                    }
                }
            }
            Ok(())
        }
        Cmd::Check { project, target, fix, no_audio, dur, size, strict } => {
            let eng = load_engine()?;
            eng.open(&project)?;
            let o = check::CheckOpts {
                target,
                fix,
                audio: !no_audio,
                expect: check::Expect { dur, size: size.as_deref().and_then(ops::exec::parse_size), ..Default::default() },
                ..Default::default()
            };
            let r = eng.check(&o, "cli")?;
            println!("{}", r.text);
            if fix {
                println!("{}", eng.save(None)?);
            }
            if strict && r.text.lines().any(|l| l.starts_with('E') && l.chars().nth(1).is_some_and(|c| c.is_ascii_digit())) {
                std::process::exit(1);
            }
            Ok(())
        }
        Cmd::Doctor => setup::doctor(),
        Cmd::Brand { root } => {
            for f in brand::generate(std::path::Path::new(&root))? {
                println!("wrote {f}");
            }
            Ok(())
        }
        Cmd::Docs => {
            print!("{}", mcp::docs::reference_markdown());
            Ok(())
        }
        Cmd::Bench { project, range, scale } => {
            let eng = load_engine()?;
            eng.open(&project)?;
            let (t0, t1) = parse_range(&range)?;
            let (p, _, active, _) = eng.snapshot();
            let fps = p.comp(&active).map(|c| c.fps).unwrap_or(30.0);
            let frames = render::decode::FrameServer::new(eng.ff.clone(), false);
            let rc = render::comp::RenderCtx { p: &p, frames: &frames, draft: false };
            let n = ((t1 - t0) * fps).round() as u64;
            let start = std::time::Instant::now();
            let mut worst = (0.0f64, 0.0f64);
            for i in 0..n {
                let t = t0 + i as f64 / fps;
                let f0 = std::time::Instant::now();
                let _ = render::comp::render_frame(&rc, &active, t, scale)?;
                let ms = f0.elapsed().as_secs_f64() * 1000.0;
                if ms > worst.0 {
                    worst = (ms, t);
                }
            }
            let el = start.elapsed().as_secs_f64();
            println!("{n} frames in {el:.2}s = {:.1} fps (worst {:.0} ms at t={:.2})", n as f64 / el, worst.0, worst.1);
            Ok(())
        }
        Cmd::Setup { force, from, dest } => setup::setup(force, from.as_deref(), dest.as_deref()),
        Cmd::InstallAi { apps, list, quiet } => {
            if list {
                setup::list_ai()
            } else {
                setup::install_ai(&apps, quiet)
            }
        }
        Cmd::UninstallAi { apps, all_profiles, only_this_exe } => {
            if all_profiles && apps.is_empty() {
                setup::uninstall_ai_all_profiles()
            } else {
                setup::uninstall_ai(&apps, only_this_exe)
            }
        }
        Cmd::InstallClaude => setup::install_ai(&["claude".to_string()], false),
    }
}
