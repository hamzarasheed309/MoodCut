//! Output: export presets, the render loop (compositor -> FFmpeg encoder via a
//! raw RGBA pipe, audio mixed in the same FFmpeg process), live preview
//! streaming (fragmented MP4), and still images / contact sheets.

use super::audio;
use super::comp::{out_size, render_frame, RenderCtx};
use super::decode::FrameServer;
use super::ffmpeg::Ff;
use super::text::{self, TextStyle};
use crate::model::project::Project;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use resvg::tiny_skia::{FillRule, Paint, Pixmap, Transform};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

pub struct Preset {
    pub name: String,
    pub ext: &'static str,
    pub video: Option<Vec<String>>,
    pub audio: Option<Vec<String>>,
    pub alpha: bool,
    pub gif: bool,
    pub seq: bool,
}

pub const PRESETS: &[(&str, &str)] = &[
    ("mp4", "H.264 MP4, high quality (default)"),
    ("youtube", "H.264 MP4 tuned for YouTube upload"),
    ("social", "H.264 MP4 for TikTok/Reels/Shorts (aliases: tiktok reels shorts instagram)"),
    ("draft", "fast low-quality H.264 preview"),
    ("hevc", "H.265/HEVC MP4 (smaller files)"),
    ("av1", "AV1 MP4 (SVT-AV1)"),
    ("prores", "ProRes 422 HQ .mov (editing master)"),
    ("prores4444", "ProRes 4444 .mov with alpha"),
    ("webm", "VP9 + Opus WebM"),
    ("webm-alpha", "VP9 WebM with transparency"),
    ("mov-alpha", "QuickTime Animation (qtrle) .mov with alpha"),
    ("gif", "animated GIF (palette optimized)"),
    ("png", "PNG image sequence with alpha"),
    ("mp3", "audio only MP3"),
    ("wav", "audio only WAV (PCM 16-bit)"),
    ("m4a", "audio only AAC"),
    ("flac", "audio only FLAC"),
    ("srt", "SubRip captions from the caption track (no video)"),
    ("vtt", "WebVTT captions from the caption track"),
    ("chapters", "YouTube chapter list from markers (.txt)"),
    ("xml", "FCP7 XML timeline for Premiere Pro / DaVinci Resolve / Final Cut 7 (original media)"),
    ("edl", "CMX3600 EDL of the V1 cut"),
];

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn h264(ff: &Ff, hw: &str, crf: u32, speed: &str) -> Vec<String> {
    let hw_list = ff.hw_encoders();
    let pick = match hw {
        "auto" | "on" | "gpu" => hw_list.first().cloned(),
        "off" | "" | "cpu" | "none" => None,
        other => {
            let want = if other.starts_with("h264_") { other.to_string() } else { format!("h264_{other}") };
            hw_list.iter().find(|e| **e == want).cloned()
        }
    };
    if let Some(enc) = pick {
        let q = (crf + 1).to_string();
        return match enc.as_str() {
            "h264_nvenc" => sv(&["-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", &q, "-b:v", "0", "-pix_fmt", "yuv420p"]),
            "h264_qsv" => sv(&["-c:v", "h264_qsv", "-global_quality", &q, "-pix_fmt", "nv12"]),
            "h264_amf" => sv(&["-c:v", "h264_amf", "-quality", "quality", "-rc", "cqp", "-qp_i", &q, "-qp_p", &q, "-pix_fmt", "yuv420p"]),
            "h264_videotoolbox" => sv(&["-c:v", "h264_videotoolbox", "-q:v", "65", "-pix_fmt", "yuv420p"]),
            _ => sv(&["-c:v", &enc, "-pix_fmt", "yuv420p"]),
        };
    }
    if ff.has_encoder("libx264") {
        sv(&["-c:v", "libx264", "-preset", speed, "-crf", &crf.to_string(), "-pix_fmt", "yuv420p", "-profile:v", "high"])
    } else if ff.has_encoder("libopenh264") {
        sv(&["-c:v", "libopenh264", "-b:v", "12M", "-pix_fmt", "yuv420p"])
    } else {
        sv(&["-c:v", "mpeg4", "-q:v", "3", "-pix_fmt", "yuv420p"])
    }
}

pub fn resolve_preset(name: &str, ff: &Ff, hw: &str) -> Result<Preset> {
    let l = name.to_ascii_lowercase().replace('_', "-");
    let aac = |br: &str| Some(sv(&["-c:a", "aac", "-b:a", br]));
    let p = |name: &str, ext, video, audio, alpha| Preset { name: name.into(), ext, video, audio, alpha, gif: false, seq: false };
    Ok(match l.as_str() {
        "mp4" | "h264" | "default" | "master" => {
            let mut v = h264(ff, hw, 18, "medium");
            v.extend(sv(&["-movflags", "+faststart"]));
            p("mp4", "mp4", Some(v), aac("192k"), false)
        }
        "youtube" | "yt" => {
            let mut v = h264(ff, hw, 17, "medium");
            v.extend(sv(&["-movflags", "+faststart", "-bf", "2", "-g", "30"]));
            p("youtube", "mp4", Some(v), aac("256k"), false)
        }
        "social" | "tiktok" | "reels" | "shorts" | "instagram" | "ig" => {
            let mut v = h264(ff, hw, 19, "medium");
            v.extend(sv(&["-maxrate", "20M", "-bufsize", "40M", "-movflags", "+faststart"]));
            p("social", "mp4", Some(v), aac("192k"), false)
        }
        "draft" | "proxy" | "preview" => {
            let mut v = h264(ff, hw, 26, "ultrafast");
            v.extend(sv(&["-movflags", "+faststart"]));
            p("draft", "mp4", Some(v), aac("128k"), false)
        }
        "hevc" | "h265" | "x265" => {
            let v = if hw != "off" && ff.hw_encoders().iter().any(|e| e == "h264_nvenc") && ff.has_encoder("hevc_nvenc") {
                sv(&["-c:v", "hevc_nvenc", "-preset", "p5", "-cq", "23", "-b:v", "0", "-pix_fmt", "yuv420p", "-tag:v", "hvc1"])
            } else if ff.has_encoder("libx265") {
                sv(&["-c:v", "libx265", "-preset", "medium", "-crf", "22", "-pix_fmt", "yuv420p", "-tag:v", "hvc1"])
            } else {
                bail!("this FFmpeg build has no HEVC encoder")
            };
            let mut v = v;
            v.extend(sv(&["-movflags", "+faststart"]));
            p("hevc", "mp4", Some(v), aac("192k"), false)
        }
        "av1" => {
            let v = if ff.has_encoder("libsvtav1") {
                sv(&["-c:v", "libsvtav1", "-crf", "32", "-preset", "8", "-pix_fmt", "yuv420p"])
            } else if ff.has_encoder("libaom-av1") {
                sv(&["-c:v", "libaom-av1", "-crf", "32", "-b:v", "0", "-cpu-used", "6", "-row-mt", "1", "-pix_fmt", "yuv420p"])
            } else if let Some(hw) = ["av1_nvenc", "av1_qsv", "av1_amf"].into_iter().find(|e| ff.hw_encoders_av1().iter().any(|h| h == e)) {
                sv(&["-c:v", hw, "-b:v", "6M", "-pix_fmt", "yuv420p"])
            } else {
                bail!("this FFmpeg build has no AV1 encoder; use preset=hevc, or install a full FFmpeg build and point MOODCUT_FFMPEG at it")
            };
            p("av1", "mp4", Some(v), aac("192k"), false)
        }
        "prores" | "prores422" => p("prores", "mov", Some(sv(&["-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le", "-vendor", "apl0"])), Some(sv(&["-c:a", "pcm_s16le"])), false),
        "prores4444" | "alpha" => p("prores4444", "mov", Some(sv(&["-c:v", "prores_ks", "-profile:v", "4", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16", "-vendor", "apl0"])), Some(sv(&["-c:a", "pcm_s16le"])), true),
        "webm" | "vp9" => p("webm", "webm", Some(sv(&["-c:v", "libvpx-vp9", "-crf", "32", "-b:v", "0", "-row-mt", "1", "-deadline", "good", "-cpu-used", "4", "-pix_fmt", "yuv420p"])), Some(sv(&["-c:a", "libopus", "-b:a", "128k"])), false),
        "webm-alpha" | "vp9-alpha" => p("webm-alpha", "webm", Some(sv(&["-c:v", "libvpx-vp9", "-crf", "30", "-b:v", "0", "-row-mt", "1", "-pix_fmt", "yuva420p", "-auto-alt-ref", "0"])), Some(sv(&["-c:a", "libopus", "-b:a", "128k"])), true),
        "mov-alpha" | "qtrle" | "animation" => p("mov-alpha", "mov", Some(sv(&["-c:v", "qtrle", "-pix_fmt", "argb"])), Some(sv(&["-c:a", "pcm_s16le"])), true),
        "gif" => Preset { name: "gif".into(), ext: "gif", video: Some(vec![]), audio: None, alpha: false, gif: true, seq: false },
        "png" | "png-seq" | "pngseq" | "frames" => Preset { name: "png".into(), ext: "png", video: Some(sv(&["-c:v", "png"])), audio: None, alpha: true, gif: false, seq: true },
        "mp3" => p("mp3", "mp3", None, Some(sv(&["-c:a", "libmp3lame", "-q:a", "2"])), false),
        "wav" => p("wav", "wav", None, Some(sv(&["-c:a", "pcm_s16le"])), false),
        "m4a" | "aac" => p("m4a", "m4a", None, aac("256k"), false),
        "flac" => p("flac", "flac", None, Some(sv(&["-c:a", "flac"])), false),
        other => bail!("unknown preset '{other}' (presets: {})", PRESETS.iter().map(|p| p.0).collect::<Vec<_>>().join(" ")),
    })
}

#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub stage: Mutex<String>,
}

#[derive(Clone, Debug)]
pub struct RenderOpts {
    pub comp: String,
    pub t0: f64,
    pub t1: f64,
    pub preset: String,
    pub out: PathBuf,
    pub scale: f32,
    pub hw: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RenderResult {
    pub out: String,
    pub frames: u64,
    pub secs: f64,
    pub bytes: u64,
    pub fps: f64,
    pub w: u32,
    pub h: u32,
}

fn spawn_stderr(child: &mut std::process::Child) -> std::thread::JoinHandle<String> {
    let mut err = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    })
}

fn tail(s: &str) -> String {
    let l: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    l[l.len().saturating_sub(6)..].join(" | ")
}

/// Renders a comp range to a file.
pub fn render_file(ff: &Arc<Ff>, p: &Project, o: &RenderOpts, prog: &Progress) -> Result<RenderResult> {
    let started = Instant::now();
    ff.need()?;
    let comp = p.comp(&o.comp).ok_or_else(|| anyhow!("no comp {}", o.comp))?;
    let preset = resolve_preset(&o.preset, ff, &o.hw)?;
    let t0 = o.t0.max(0.0);
    let t1 = if o.t1 > t0 { o.t1 } else { comp.duration() };
    if t1 - t0 < 1e-3 {
        bail!("nothing to render: the range is empty (is the timeline empty?)");
    }
    let mut out = o.out.clone();
    if out.extension().is_none() {
        out.set_extension(preset.ext);
    }
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
    }
    let fps = comp.fps;
    let (w, h) = out_size(comp, o.scale.clamp(0.05, 4.0));
    let n = ((t1 - t0) * fps).round().max(1.0) as u64;
    prog.total.store(if preset.video.is_some() { n } else { 1 }, Ordering::Relaxed);
    *prog.stage.lock() = "rendering".into();
    let has_video = preset.video.is_some();
    let first_audio = if has_video { 1 } else { 0 };
    let ap = if preset.audio.is_some() { Some(audio::plan(p, ff, &o.comp, t0, t1, first_audio)) } else { None };
    let mut c = ff.cmd();
    c.arg("-y");
    if has_video {
        c.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &format!("{fps}"), "-i", "pipe:0"]);
    }
    let mut graph = vec![];
    if let Some(ap) = &ap {
        c.args(audio::input_args(ap));
        graph.push(ap.graph.clone());
    }
    let mut vmap = "0:v".to_string();
    if preset.gif {
        let gf = fps.min(15.0);
        let gw = w.min(720);
        graph.push(format!("[0:v]fps={gf},scale={gw}:-2:flags=lanczos,split[g1][g2];[g1]palettegen=stats_mode=diff[pal];[g2][pal]paletteuse=dither=bayer:bayer_scale=4[vout]"));
        vmap = "[vout]".into();
    }
    if !graph.is_empty() {
        c.args(["-filter_complex", &graph.join(";")]);
    }
    if has_video {
        c.args(["-map", &vmap]);
        c.args(preset.video.clone().unwrap());
    }
    if let Some(ap) = &ap {
        c.args(["-map", &format!("[{}]", ap.label)]);
        c.args(preset.audio.clone().unwrap());
    }
    let out_path = if preset.seq {
        let stem = out.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "frame".into());
        let dir = out.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        dir.join(format!("{stem}_%05d.png"))
    } else {
        out.clone()
    };
    c.arg(&out_path);
    c.stdin(if has_video { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = c.spawn().context("starting ffmpeg encoder")?;
    let err_t = spawn_stderr(&mut child);
    if has_video {
        let mut stdin = child.stdin.take().unwrap();
        let (tx, rx) = sync_channel::<Vec<u8>>(3);
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            for buf in rx {
                stdin.write_all(&buf)?;
            }
            Ok(())
        });
        let frames = FrameServer::new(ff.clone(), false);
        let rc = RenderCtx { p, frames: &frames, draft: false };
        let mut send_err = None;
        for i in 0..n {
            if prog.cancel.load(Ordering::Relaxed) {
                break;
            }
            let t = t0 + i as f64 / fps;
            let pm = render_frame(&rc, &o.comp, t, o.scale)?;
            let data = if preset.alpha { pm.take_demultiplied() } else { pm.take() };
            if tx.send(data).is_err() {
                send_err = Some(());
                break;
            }
            prog.done.store(i + 1, Ordering::Relaxed);
        }
        drop(tx);
        let _ = writer.join();
        if send_err.is_some() {
            let _ = child.wait();
            let e = err_t.join().unwrap_or_default();
            bail!("encoder stopped early: {}", tail(&e));
        }
    }
    let st = child.wait()?;
    let e = err_t.join().unwrap_or_default();
    if prog.cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&out);
        bail!("render cancelled");
    }
    if !st.success() {
        bail!("ffmpeg failed: {}", tail(&e));
    }
    prog.done.store(prog.total.load(Ordering::Relaxed), Ordering::Relaxed);
    *prog.stage.lock() = "done".into();
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let secs = started.elapsed().as_secs_f64();
    Ok(RenderResult { out: out.to_string_lossy().to_string(), frames: n, secs, bytes, fps: n as f64 / secs.max(1e-3), w, h })
}

/// Streams a fragmented-MP4 preview from t0 to the end of the comp.
pub fn preview_stream(ff: Arc<Ff>, p: Project, frames: Arc<FrameServer>, comp: String, t0: f64, scale: f32, cancel: Arc<AtomicBool>) -> Result<Receiver<Vec<u8>>> {
    ff.need()?;
    let c0 = p.comp(&comp).ok_or_else(|| anyhow!("no comp {comp}"))?;
    let t1 = c0.duration();
    if t1 <= t0 {
        bail!("nothing to play after {t0}s");
    }
    let fps = c0.fps;
    let (w, h) = out_size(c0, scale);
    let ap = audio::plan_preview(&p, &ff, &comp, t0, t1, 1);
    let mut c = ff.cmd();
    c.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r", &format!("{fps}"), "-i", "pipe:0"]);
    c.args(audio::input_args(&ap));
    c.args(["-filter_complex", &ap.graph, "-map", "0:v", "-map", &format!("[{}]", ap.label)]);
    let enc = if ff.has_encoder("libx264") { sv(&["-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-crf", "26"]) } else { sv(&["-c:v", "mpeg4", "-q:v", "5"]) };
    c.args(enc);
    c.args(["-g", &format!("{}", fps.round() as i64), "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k", "-f", "mp4", "-movflags", "frag_keyframe+empty_moov+default_base_moof", "pipe:1"]);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().context("starting preview encoder")?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = sync_channel::<Vec<u8>>(16);
    let cancel_r = cancel.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 1 << 16];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    if tx.send(buf[..k].to_vec()).is_err() {
                        cancel_r.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    });
    std::thread::spawn(move || {
        let rc = RenderCtx { p: &p, frames: &frames, draft: true };
        let n = ((t1 - t0) * fps).round() as u64;
        for i in 0..n {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let t = t0 + i as f64 / fps;
            let Ok(pm) = render_frame(&rc, &comp, t, scale) else { break };
            if stdin.write_all(pm.data()).is_err() {
                break;
            }
        }
        drop(stdin);
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
        }
        let _ = child.wait();
    });
    Ok(rx)
}

/// Composites a premultiplied pixmap over black and encodes JPEG.
pub fn encode_jpeg(pm: &Pixmap, quality: u8) -> Result<Vec<u8>> {
    let mut rgb = Vec::with_capacity((pm.width() * pm.height() * 3) as usize);
    for px in pm.data().chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(10, 100));
    image::ImageEncoder::write_image(enc, &rgb, pm.width(), pm.height(), image::ExtendedColorType::Rgb8)?;
    Ok(out)
}

pub fn encode_png(pm: &Pixmap) -> Result<Vec<u8>> {
    pm.encode_png().map_err(|e| anyhow!("png: {e}"))
}

/// Draws a small time label onto a pixmap.
fn label(pm: &mut Pixmap, text_s: &str, x: f32, y: f32, size: f32) {
    let st = TextStyle { family: "Inter".into(), weight: 700.0, italic: false, size, align: 0, line_h: 1.1, tracking: 0.0, box_w: 0.0, caps: false };
    let lay = text::layout(text_s, &st);
    let Some(path) = &lay.combined else { return };
    let pad = size * 0.3;
    if let Some(bg) = resvg::tiny_skia::Rect::from_xywh(x, y, lay.w + pad * 2.0, lay.h + pad) {
        let mut p = Paint::default();
        p.set_color_rgba8(0, 0, 0, 170);
        pm.fill_rect(bg, &p, Transform::identity(), None);
    }
    let mut p = Paint { anti_alias: true, ..Default::default() };
    p.set_color_rgba8(255, 255, 255, 255);
    pm.fill_path(path, &p, FillRule::Winding, Transform::from_translate(x + pad, y + pad * 0.5), None);
}

/// Renders several frames tiled into one image (cheap for vision models).
pub fn contact_sheet(rc: &RenderCtx, comp_id: &str, times: &[f64], thumb_w: u32, cols: usize) -> Result<Pixmap> {
    let comp = rc.p.comp(comp_id).ok_or_else(|| anyhow!("no comp {comp_id}"))?;
    let scale = thumb_w as f32 / comp.w as f32;
    let (tw, th) = out_size(comp, scale);
    let cols = cols.max(1).min(times.len().max(1));
    let rows = times.len().div_ceil(cols);
    let gap = 4u32;
    let mut sheet = Pixmap::new(cols as u32 * (tw + gap) - gap, rows as u32 * (th + gap) - gap).ok_or_else(|| anyhow!("sheet too big"))?;
    sheet.fill(resvg::tiny_skia::Color::from_rgba8(24, 24, 24, 255));
    for (i, t) in times.iter().enumerate() {
        let mut pm = render_frame(rc, comp_id, *t, scale)?;
        label(&mut pm, &format!("{}s", crate::model::time::fmt_t(*t)), 3.0, 3.0, (th as f32 * 0.09).clamp(9.0, 22.0));
        let (cx, cy) = ((i % cols) as u32 * (tw + gap), (i / cols) as u32 * (th + gap));
        super::pixel::blit(&mut sheet, &pm, cx as i32, cy as i32);
    }
    Ok(sheet)
}

/// One labelled thumbnail for `tile_sheet`.
pub struct Tile {
    pub label: String,
    pub img: Pixmap,
}

/// Tiles thumbnails into one image: each is fitted (aspect kept) and centered in
/// a `cell_w` x `cell_h` cell. `checker` shows transparency as a checkerboard.
pub fn tile_sheet(tiles: &[Tile], cell_w: u32, cell_h: u32, cols: usize, checker: bool) -> Result<Pixmap> {
    let cols = cols.max(1).min(tiles.len().max(1));
    let rows = tiles.len().div_ceil(cols).max(1);
    let gap = 4u32;
    let mut sheet = Pixmap::new(cols as u32 * (cell_w + gap) - gap, rows as u32 * (cell_h + gap) - gap).ok_or_else(|| anyhow!("sheet too big"))?;
    sheet.fill(resvg::tiny_skia::Color::from_rgba8(24, 24, 24, 255));
    for (i, t) in tiles.iter().enumerate() {
        let (cx, cy) = ((i % cols) as u32 * (cell_w + gap), (i / cols) as u32 * (cell_h + gap));
        let mut cell = Pixmap::new(cell_w, cell_h).ok_or_else(|| anyhow!("cell"))?;
        if checker {
            let sq = (cell_h / 12).max(6);
            for y in 0..cell_h {
                for x in 0..cell_w {
                    let v = if ((x / sq) + (y / sq)) % 2 == 0 { 92u8 } else { 64u8 };
                    let o = ((y * cell_w + x) * 4) as usize;
                    cell.data_mut()[o..o + 4].copy_from_slice(&[v, v, v, 255]);
                }
            }
        } else {
            cell.fill(resvg::tiny_skia::Color::from_rgba8(0, 0, 0, 255));
        }
        let (iw, ih) = (t.img.width().max(1) as f32, t.img.height().max(1) as f32);
        let k = (cell_w as f32 / iw).min(cell_h as f32 / ih);
        let (fw, fh) = (((iw * k).round() as u32).max(1), ((ih * k).round() as u32).max(1));
        let fitted = super::decode::resize_pixmap(&t.img, fw, fh);
        cell.draw_pixmap(((cell_w - fw) / 2) as i32, ((cell_h - fh) / 2) as i32, fitted.as_ref(), &resvg::tiny_skia::PixmapPaint::default(), Transform::identity(), None);
        if !t.label.is_empty() {
            label(&mut cell, &t.label, 3.0, 3.0, (cell_h as f32 * 0.09).clamp(9.0, 20.0));
        }
        super::pixel::blit(&mut sheet, &cell, cx as i32, cy as i32);
    }
    Ok(sheet)
}

/// Decodes PNG/JPEG bytes into a premultiplied pixmap.
pub fn decode_image(bytes: &[u8]) -> Result<Pixmap> {
    let img = image::load_from_memory(bytes)?.to_rgba8();
    let (w, h) = img.dimensions();
    super::decode::pixmap_from_rgba(img.into_raw(), w, h, true).ok_or_else(|| anyhow!("bad image"))
}

/// Default output path for a render.
pub fn default_out(project_path: Option<&Path>, name: &str, ext: &str) -> PathBuf {
    let base = project_path.and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| {
        dirs_videos().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    });
    let stamp = chrono_like();
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    base.join("MoodCut Renders").join(format!("{safe}_{stamp}.{ext}"))
}

fn dirs_videos() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var("USERPROFILE").ok().map(|u| PathBuf::from(u).join("Videos"))
    } else {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Videos"))
    }
}

fn chrono_like() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // yyyymmdd-hhmmss in UTC without a date crate
    let days = s / 86400;
    let secs = s % 86400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, m, d, secs / 3600, (secs / 60) % 60, secs % 60)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
