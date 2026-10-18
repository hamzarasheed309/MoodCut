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
pub mod config;
pub mod engine;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod render;
pub mod server;
pub mod setup;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "moodcut", version, about = "MoodCut — AI-native video editor & motion graphics engine (FFmpeg + MCP)")]
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
        /// Time range a-b in seconds
        #[arg(long)]
        range: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        scale: f32,
        #[arg(long)]
        comp: Option<String>,
        /// Hardware encoder: off | auto | nvenc | qsv | amf
        #[arg(long, default_value = "off")]
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
    /// Install FFmpeg (Windows: winget) and optional extras.
    Setup,
    /// Register MoodCut in Claude Desktop's MCP config.
    InstallClaude,
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn load_engine() -> Result<std::sync::Arc<engine::Engine>> {
    let ff = render::ffmpeg::Ff::find()?;
    Ok(engine::Engine::new(ff))
}

fn parse_range(r: &str) -> Result<(f64, f64)> {
    let (a, b) = r.split_once('-').context("range must be a-b")?;
    Ok((a.trim().parse()?, b.trim().parse()?))
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.unwrap_or(Cmd::Gui { port: 7878, project: None }) {
        Cmd::Gui { port, project } => {
            setup::detach_console_if_owned();
            let eng = load_engine()?;
            if let Some(p) = project {
                eng.open(&p)?;
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                let opts = server::ServeOpts { host: "127.0.0.1".into(), port, token: None, open_window: true };
                server::serve(eng, opts).await
            })
        }
        Cmd::Serve { port, host, token, public, allow, project } => {
            let eng = load_engine()?;
            if let Some(p) = project {
                eng.open(&p)?;
            }
            if !allow.is_empty() {
                let roots: Vec<PathBuf> = allow.iter().map(|a| engine::resolve_path(std::path::Path::new(a))).collect();
                *eng.roots.write() = Some(roots);
            }
            let (host, token) = if public { ("0.0.0.0".to_string(), Some(token.unwrap_or_else(server::random_token))) } else { (host, token) };
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move { server::serve(eng, server::ServeOpts { host, port, token, open_window: false }).await })
        }
        Cmd::Mcp { port, no_http } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(mcp::stdio::run(port, !no_http))
        }
        Cmd::Render { project, out, preset, range, scale, comp, hw } => {
            let eng = load_engine()?;
            eng.open(&project)?;
            let (t0, t1) = match range {
                Some(r) => parse_range(&r)?,
                None => (0.0, 0.0),
            };
            let opts = render::encode::RenderOpts { comp: comp.unwrap_or_default(), t0, t1, preset, out: out.map(PathBuf::from).unwrap_or_default(), scale, hw };
            let job = eng.start_render(opts)?;
            let mut last = String::new();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let st = job.status();
                let line = format!("\r{} {}% {} eta {}s   ", st["state"].as_str().unwrap_or(""), st["pct"], st["frames"].as_str().unwrap_or(""), st["eta_s"]);
                if line != last {
                    eprint!("{line}");
                    last = line;
                }
                if st["state"] != "running" {
                    eprintln!();
                    if st["state"] == "error" {
                        bail!("{}", st["error"].as_str().unwrap_or("render failed"));
                    }
                    println!("{}", serde_json::to_string_pretty(&st)?);
                    break;
                }
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
        Cmd::Doctor => setup::doctor(),
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
        Cmd::Setup => setup::setup(),
        Cmd::InstallClaude => setup::install_claude(),
    }
}
