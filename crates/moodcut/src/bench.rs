//! `moodcut bench` without a project: fixed synthetic scenes (no media is
//! needed; clips come from FFmpeg's lavfi sources), so numbers from different
//! builds and machines compare. CI keeps the output as an artifact: a baseline
//! before any speed claim, and an alarm when something gets slower.

use crate::engine::Engine;
use crate::render::{audio, comp, decode::FrameServer};
use anyhow::{Context, Result};
use std::fmt::Write;
use std::time::Instant;

/// (name, what it measures, script). Every scene is 1920x1080@30, 2 s.
const SCENES: &[(&str, &str, &str)] = &[
    ("solid", "one gradient layer, no effects", "solid #305080 at=0 dur=2 color2=#101828 gradType=linear as=bg"),
    ("grade1", "one LUT-baked color effect", "solid #305080 at=0 dur=2 color2=#101828 gradType=linear as=bg\nfx bg lumetri exposure=0.3 contrast=12 sat=110"),
    (
        "grade5",
        "a five-effect grade",
        "solid #305080 at=0 dur=2 color2=#101828 gradType=linear as=bg\nfx bg lumetri exposure=0.3 contrast=12 sat=110\nfx bg curves master='0,0 0.5,0.58 1,1'\nfx bg colorwheels shadowsL=-8 highlightsL=6 midtonesL=4\nfx bg vibrance amount=0.3\nfx bg channelmixer rg=10",
    ),
    ("blur", "full-frame blur radius 30", "solid #305080 at=0 dur=2 color2=#101828 gradType=linear as=bg\nfx bg blur radius=30"),
    (
        "window10",
        "blur radius 30 inside a window covering ~10% of the frame",
        "solid #305080 at=0 dur=2 color2=#101828 gradType=linear as=bg\nmask bg ellipse w=0.36 h=0.36 window\nfx bg blur radius=30 mask=m1",
    ),
    (
        "mask3",
        "one static feathered mask used by three effects",
        "solid #305080 at=0 dur=2 color2=#101828 gradType=linear as=bg\nmask bg ellipse w=0.6 h=0.6 feather=80 window\nfx bg lumetri exposure=0.4 mask=m1\nfx bg sharpen mask=m1\nfx bg vibrance amount=0.3 mask=m1",
    ),
    (
        "animmask",
        "an animated feathered cut mask",
        "solid #305080 at=0 dur=2 color2=#101828 gradType=linear as=bg\nmask bg ellipse w=0.4 h=0.4 feather=60\nkf bg.m1 x 0:0.3 2:0.7",
    ),
    ("text", "an animated title", "text 'BENCHMARK' at=0 dur=2 size=160 weight=900 enter=trackIn as=title"),
];

const CLIP_SCENES: &[(&str, &str, &str)] = &[
    ("clip", "a decoded 1080p clip", "add src V1 at=0 dur=2"),
    ("clipgrade", "a decoded clip with a three-effect grade", "add src V1 at=0 dur=2 as=c\nfx c lumetri exposure=0.2 contrast=10\nfx c curves master='0,0 0.5,0.55 1,1'\nfx c vibrance amount=0.3"),
];

fn time_frames(eng: &Engine, frames: &FrameServer, t1: f64) -> Result<(u64, f64, f64)> {
    let (p, _, active, _) = eng.snapshot();
    let fps = p.comp(&active).map(|c| c.fps).unwrap_or(30.0);
    let rc = comp::RenderCtx { p: &p, frames, draft: false, probe: None };
    // warm-up frame: decoders start, caches fill
    let _ = comp::render_frame(&rc, &active, 0.0, 1.0)?;
    let n = (t1 * fps).round().max(1.0) as u64;
    let start = Instant::now();
    let mut worst = 0.0f64;
    for i in 0..n {
        let f0 = Instant::now();
        let _ = comp::render_frame(&rc, &active, i as f64 / fps, 1.0)?;
        worst = worst.max(f0.elapsed().as_secs_f64() * 1000.0);
    }
    Ok((n, start.elapsed().as_secs_f64(), worst))
}

fn scene(eng: &Engine, frames: &FrameServer, out: &mut String, name: &str, what: &str, script: &str, prelude: &str) -> Result<()> {
    eng.new_project("bench", 1920, 1080, 30.0, None)?;
    if !prelude.is_empty() {
        eng.edit(prelude, "bench").with_context(|| format!("bench scene {name} (setup)"))?;
    }
    eng.edit(script, "bench").with_context(|| format!("bench scene {name}"))?;
    let (n, secs, worst) = time_frames(eng, frames, 2.0)?;
    let _ = writeln!(out, "{name:<10} {:>7.1} fps {:>7.1} ms/frame  worst {:>6.1} ms  ({what})", n as f64 / secs, secs * 1000.0 / n as f64, worst);
    Ok(())
}

/// Runs every scene and the audio benchmarks; returns the report.
pub fn run(eng: &Engine) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, "MoodCut {} bench, {} threads", crate::VERSION, rayon::current_num_threads());
    let frames = FrameServer::new(eng.ff.clone(), false);
    for (name, what, script) in SCENES {
        scene(eng, &frames, &mut out, name, what, script, "")?;
    }
    if !eng.ff.available() {
        let _ = writeln!(out, "(no FFmpeg: clip and audio benchmarks skipped)");
        return Ok(out);
    }
    let dir = std::env::temp_dir().join(format!("moodcut-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let res = media_benches(eng, &frames, &dir, &mut out);
    let _ = std::fs::remove_dir_all(&dir);
    res?;
    Ok(out)
}

fn media_benches(eng: &Engine, frames: &FrameServer, dir: &std::path::Path, out: &mut String) -> Result<()> {
    let video = dir.join("src.mp4");
    let tone = dir.join("tone.wav");
    let s = |p: &std::path::Path| p.to_string_lossy().to_string();
    let args = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    eng.ff.run_capture(&args(&["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=30:d=3", "-pix_fmt", "yuv420p", "-c:v", "mpeg4", "-q:v", "4", &s(&video)]))?;
    eng.ff.run_capture(&args(&["-y", "-v", "error", "-f", "lavfi", "-i", "sine=f=330:d=1.5,aformat=channel_layouts=stereo", &s(&tone)]))?;
    let prelude = format!("import '{}' as=src", s(&video));
    for (name, what, script) in CLIP_SCENES {
        scene(eng, frames, out, name, what, script, &prelude)?;
    }
    // audio: a 200-clip timeline (each clip with a gain ramp), as a long edit has
    eng.new_project("bench", 1920, 1080, 30.0, None)?;
    let mut script = format!("import '{}' as=tone\n", s(&tone));
    for i in 0..200 {
        let _ = writeln!(script, "add tone A1 at={i} dur=1 as=tn{i}\nkf tn{i} vol 0:-12 0.5:0 1:-6");
    }
    eng.edit(&script, "bench").context("bench audio timeline")?;
    let (p, _, active, _) = eng.snapshot();
    let dur = p.comp(&active).map(|c| c.duration()).unwrap_or(200.0);
    let t0 = Instant::now();
    let plan = audio::plan(&p, &eng.ff, &active, 0.0, dur, 0);
    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut a = audio::input_args(&plan);
    let (gargs, _graph_file) = eng.ff.graph_args(&plan.graph)?;
    a.extend(gargs);
    a.extend(args(&["-map", &format!("[{}]", plan.label), "-f", "null", "-"]));
    let t1 = Instant::now();
    eng.ff.run_capture(&a).context("bench audio render")?;
    let _ = writeln!(
        out,
        "audio200   graph {:>6} chars built in {:.1} ms, {} inputs, rendered {:.0} s of audio in {:.2} s",
        plan.graph.len(),
        build_ms,
        plan.inputs.len(),
        dur,
        t1.elapsed().as_secs_f64()
    );
    Ok(())
}
