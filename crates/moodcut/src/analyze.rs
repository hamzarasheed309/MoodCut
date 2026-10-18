//! Media analysis: silence, scene cuts, loudness, black frames, image stats,
//! beat/tempo tracking (native DSP) and speech transcription (whisper.cpp /
//! openai-whisper CLI / FFmpeg 8 whisper filter / sidecar SRT/VTT/JSON).

use crate::render::ffmpeg::{command, Ff};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const KINDS: &[(&str, &str)] = &[
    ("silence", "silent ranges [start,end] (thresh=-35 dB, min=0.4 s) -> use with autocut"),
    ("scenes", "scene-cut times (thresh=0.3)"),
    ("loudness", "integrated LUFS, loudness range, true peak"),
    ("beats", "tempo (BPM) and beat times -> use with beats/montage"),
    ("transcript", "speech to text with word timings -> captions / text-based editing"),
    ("black", "black segments"),
    ("stats", "average brightness/saturation/contrast (exposure check)"),
];

fn ffargs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn parse_after(line: &str, key: &str) -> Option<f64> {
    let i = line.find(key)? + key.len();
    let rest = line[i..].trim_start();
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == 'e').collect();
    num.parse().ok()
}

pub fn silence(ff: &Ff, path: &str, thresh: f64, min: f64, dur: f64) -> Result<Value> {
    let (_, err) = ff.run_capture(&ffargs(&["-i", path, "-vn", "-sn", "-af", &format!("silencedetect=noise={thresh}dB:d={min}"), "-f", "null", "-"]))?;
    let mut out = vec![];
    let mut start: Option<f64> = None;
    for l in err.lines() {
        if let Some(s) = parse_after(l, "silence_start:") {
            start = Some(s.max(0.0));
        }
        if let Some(e) = parse_after(l, "silence_end:") {
            if let Some(s) = start.take() {
                out.push(json!([round3(s), round3(e)]));
            }
        }
    }
    if let Some(s) = start {
        out.push(json!([round3(s), round3(dur)]));
    }
    let total: f64 = out.iter().map(|r| r[1].as_f64().unwrap() - r[0].as_f64().unwrap()).sum();
    Ok(json!({"silences": out, "count": out.len(), "silent_total": round3(total), "thresh_db": thresh, "min": min}))
}

pub fn scenes(ff: &Ff, path: &str, thresh: f64) -> Result<Value> {
    let (_, err) = ff.run_capture(&ffargs(&["-i", path, "-an", "-sn", "-vf", &format!("scale=320:-2,select='gt(scene,{thresh})',showinfo"), "-f", "null", "-"]))?;
    let mut cuts = vec![];
    for l in err.lines() {
        if l.contains("Parsed_showinfo") {
            if let Some(t) = parse_after(l, "pts_time:") {
                cuts.push(round3(t));
            }
        }
    }
    Ok(json!({"cuts": cuts, "count": cuts.len(), "thresh": thresh}))
}

pub fn loudness(ff: &Ff, path: &str) -> Result<Value> {
    let (_, err) = ff.run_capture(&ffargs(&["-i", path, "-vn", "-sn", "-af", "ebur128=peak=true", "-f", "null", "-"]))?;
    let mut i = None;
    let mut lra = None;
    let mut tp = None;
    let summary = err.rfind("Summary:").map(|k| &err[k..]).unwrap_or(&err);
    for l in summary.lines() {
        let t = l.trim();
        if t.starts_with("I:") {
            i = parse_after(t, "I:");
        } else if t.starts_with("LRA:") {
            lra = parse_after(t, "LRA:");
        } else if t.starts_with("Peak:") {
            tp = parse_after(t, "Peak:");
        }
    }
    let i = i.ok_or_else(|| anyhow!("no audio loudness measured"))?;
    let hint = if i < -20.0 {
        "quiet: try master lufs=-14 for YouTube/social"
    } else if i > -10.0 {
        "very loud: consider master lufs=-14 limiter=1"
    } else {
        "ok"
    };
    Ok(json!({"lufs": i, "lra": lra, "true_peak": tp, "hint": hint}))
}

pub fn black(ff: &Ff, path: &str) -> Result<Value> {
    let (_, err) = ff.run_capture(&ffargs(&["-i", path, "-an", "-sn", "-vf", "scale=320:-2,blackdetect=d=0.1:pix_th=0.10", "-f", "null", "-"]))?;
    let mut out = vec![];
    for l in err.lines() {
        if let (Some(s), Some(e)) = (parse_after(l, "black_start:"), parse_after(l, "black_end:")) {
            out.push(json!([round3(s), round3(e)]));
        }
    }
    Ok(json!({"black": out}))
}

pub fn stats(ff: &Ff, path: &str, dur: f64) -> Result<Value> {
    let fps = if dur > 0.0 { (8.0 / dur).clamp(0.05, 4.0) } else { 1.0 };
    let (out, _) = ff.run_capture(&ffargs(&["-i", path, "-an", "-sn", "-vf", &format!("fps={fps},scale=320:-2,signalstats,metadata=print:file=-"), "-f", "null", "-"]))?;
    let s = String::from_utf8_lossy(&out);
    let mut acc: BTreeMap<&str, (f64, u32)> = BTreeMap::new();
    for l in s.lines() {
        for key in ["YAVG", "YMIN", "YMAX", "SATAVG", "HUEAVG", "YDIF"] {
            let k = format!("lavfi.signalstats.{key}=");
            if let Some(v) = l.strip_prefix(&k).and_then(|v| v.trim().parse::<f64>().ok()) {
                let e = acc.entry(key).or_insert((0.0, 0));
                e.0 += v;
                e.1 += 1;
            }
        }
    }
    let avg = |k: &str| acc.get(k).map(|(s, n)| round3(s / (*n).max(1) as f64)).unwrap_or(0.0);
    // values are 8-bit luma (16-235 nominal)
    let y = avg("YAVG");
    let sat = avg("SATAVG");
    let mut hints = vec![];
    if y < 70.0 {
        hints.push("dark: raise exposure (color <id> exposure=0.4) or shadows");
    } else if y > 180.0 {
        hints.push("bright: lower exposure or highlights");
    }
    if sat < 25.0 {
        hints.push("low saturation: try vibrance/sat");
    } else if sat > 90.0 {
        hints.push("very saturated");
    }
    if avg("YMAX") - avg("YMIN") < 120.0 {
        hints.push("flat contrast: try contrast=20");
    }
    Ok(json!({"luma_avg": y, "luma_min": avg("YMIN"), "luma_max": avg("YMAX"), "sat_avg": sat, "motion": avg("YDIF"), "hints": hints}))
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ---------------------------------------------------------------- beats

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ar, ai) = (re[i + k], im[i + k]);
                let (br, bi) = (re[i + k + len / 2], im[i + k + len / 2]);
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                re[i + k] = ar + tr;
                im[i + k] = ai + ti;
                re[i + k + len / 2] = ar - tr;
                im[i + k + len / 2] = ai - ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Onset envelope + tempo + DP beat tracking (Ellis 2007).
pub fn beats_from_samples(x: &[f32], sr: f32) -> (f64, Vec<f64>, Vec<f64>) {
    let n_fft = 1024;
    let hop = 128;
    if x.len() < n_fft * 4 {
        return (0.0, vec![], vec![]);
    }
    let win: Vec<f32> = (0..n_fft).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n_fft as f32).cos()).collect();
    let frames = (x.len() - n_fft) / hop;
    let mut prev = vec![0f32; n_fft / 2];
    let mut env = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut re: Vec<f32> = (0..n_fft).map(|i| x[f * hop + i] * win[i]).collect();
        let mut im = vec![0f32; n_fft];
        fft(&mut re, &mut im);
        let mut flux = 0.0;
        for k in 1..n_fft / 2 {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
            let lm = (1.0 + 100.0 * mag).ln();
            let d = lm - prev[k];
            if d > 0.0 {
                flux += d;
            }
            prev[k] = lm;
        }
        env.push(flux);
    }
    // high-pass (subtract local mean) and normalize
    let w = 32;
    let mut o: Vec<f32> = (0..env.len())
        .map(|i| {
            let a = i.saturating_sub(w);
            let b = (i + w).min(env.len() - 1);
            let m: f32 = env[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
            (env[i] - m).max(0.0)
        })
        .collect();
    let sd = (o.iter().map(|v| v * v).sum::<f32>() / o.len() as f32).sqrt().max(1e-9);
    for v in o.iter_mut() {
        *v /= sd;
    }
    let fps = sr / hop as f32;
    // tempo via autocorrelation with a log-gaussian prior around 120 BPM
    let min_lag = (fps * 60.0 / 200.0) as usize;
    let max_lag = (fps * 60.0 / 55.0) as usize;
    let mut best = (0.0f32, min_lag.max(1));
    for lag in min_lag.max(1)..max_lag.min(o.len() / 2) {
        let mut s = 0.0;
        for i in lag..o.len() {
            s += o[i] * o[i - lag];
        }
        s /= (o.len() - lag) as f32;
        let bpm = 60.0 * fps / lag as f32;
        let prior = (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp();
        let sc = s * prior;
        if sc > best.0 {
            best = (sc, lag);
        }
    }
    let period = best.1 as f32;
    let bpm = 60.0 * fps / period;
    // DP beat tracking
    let alpha = 100.0f32;
    let n = o.len();
    let mut score = o.clone();
    let mut back = vec![usize::MAX; n];
    for t in 0..n {
        let lo = (t as f32 - 2.0 * period).max(0.0) as usize;
        let hi = (t as f32 - 0.5 * period).max(0.0) as usize;
        let mut bv = f32::NEG_INFINITY;
        let mut bi = usize::MAX;
        for p in lo..hi.min(t) {
            let tau = (t - p) as f32;
            let pen = -alpha * (tau / period).ln().powi(2);
            let v = score[p] + pen;
            if v > bv {
                bv = v;
                bi = p;
            }
        }
        if bi != usize::MAX {
            score[t] = o[t] + bv;
            back[t] = bi;
        }
    }
    // start from the best score in the last period
    let tail_lo = n.saturating_sub(period as usize * 2);
    let mut t = (tail_lo..n).max_by(|a, b| score[*a].partial_cmp(&score[*b]).unwrap()).unwrap_or(n - 1);
    let mut bt = vec![];
    while t != usize::MAX {
        bt.push(t);
        t = back[t];
    }
    bt.reverse();
    let to_s = |f: usize| round3((f * hop + n_fft / 2) as f64 / sr as f64);
    let beats: Vec<f64> = bt.iter().map(|f| to_s(*f)).collect();
    // downbeats: phase with the strongest onsets among every-4th beats
    let mut best_phase = 0;
    let mut best_sum = f32::NEG_INFINITY;
    for ph in 0..4 {
        let s: f32 = bt.iter().skip(ph).step_by(4).map(|f| o[*f]).sum();
        if s > best_sum {
            best_sum = s;
            best_phase = ph;
        }
    }
    let downs: Vec<f64> = beats.iter().skip(best_phase).step_by(4).cloned().collect();
    // refine tempo by least-squares fit of beat time vs index (sub-frame accuracy)
    let mut bpm = bpm as f64;
    if beats.len() >= 8 {
        let n = beats.len() as f64;
        let mx = (n - 1.0) / 2.0;
        let my = beats.iter().sum::<f64>() / n;
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for (i, b) in beats.iter().enumerate() {
            let dx = i as f64 - mx;
            sxy += dx * (b - my);
            sxx += dx * dx;
        }
        let period = sxy / sxx.max(1e-9);
        if period > 0.2 && period < 1.5 {
            bpm = 60.0 / period;
        }
    }
    ((bpm * 10.0).round() / 10.0, beats, downs)
}

pub fn beats(ff: &Ff, path: &str) -> Result<Value> {
    let sr = 11025u32;
    let mut c = ff.cmd();
    c.args(["-i", path, "-vn", "-sn", "-ac", "1", "-ar", &sr.to_string(), "-f", "f32le", "pipe:1"]).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn()?;
    let mut buf = vec![];
    child.stdout.take().unwrap().read_to_end(&mut buf)?;
    child.wait()?;
    let samples: Vec<f32> = buf.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    if samples.is_empty() {
        bail!("no audio in {path}");
    }
    let (bpm, beats, downs) = beats_from_samples(&samples, sr as f32);
    Ok(json!({"bpm": bpm, "beats": beats, "downbeats": downs, "count": beats.len()}))
}

// ---------------------------------------------------------------- transcript

fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut dirs: Vec<PathBuf> = std::env::var("PATH").ok().map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(d) = crate::config::data_dir() {
        dirs.push(d.join("whisper"));
        dirs.push(d.join("bin"));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

fn whisper_model() -> Option<PathBuf> {
    if let Ok(m) = std::env::var("MOODCUT_WHISPER_MODEL") {
        let p = PathBuf::from(m);
        if p.is_file() {
            return Some(p);
        }
    }
    let d = crate::config::data_dir()?.join("models");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&d).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "bin").unwrap_or(false)).collect();
    v.sort();
    v.into_iter().next()
}

fn parse_tc(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let nums: Vec<f64> = parts.iter().map(|p| p.parse::<f64>()).collect::<std::result::Result<_, _>>().ok()?;
    Some(match nums.len() {
        3 => nums[0] * 3600.0 + nums[1] * 60.0 + nums[2],
        2 => nums[0] * 60.0 + nums[1],
        1 => nums[0],
        _ => return None,
    })
}

/// Parses SRT or VTT into segments.
pub fn parse_subs(text: &str) -> Vec<Value> {
    let mut segs = vec![];
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some((a, b)) = l.split_once("-->") {
            let s = parse_tc(a);
            let e = parse_tc(b.split_whitespace().next().unwrap_or(""));
            let mut txt = vec![];
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() {
                txt.push(lines[i].trim());
                i += 1;
            }
            if let (Some(s), Some(e)) = (s, e) {
                let t = txt.join(" ");
                let clean: String = strip_tags(&t);
                segs.push(json!({"s": round3(s), "e": round3(e), "text": clean}));
            }
        }
        i += 1;
    }
    segs
}

fn strip_tags(s: &str) -> String {
    let mut o = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => o.push(c),
            _ => {}
        }
    }
    o
}

fn wav16k(ff: &Ff, path: &str, out: &Path) -> Result<()> {
    ff.run_capture(&ffargs(&["-y", "-i", path, "-vn", "-sn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", &out.to_string_lossy()]))?;
    Ok(())
}

pub fn transcript(ff: &Ff, path: &str, opts: &BTreeMap<String, String>) -> Result<Value> {
    // 1) explicit or sidecar subtitle files
    let mut sidecars = vec![];
    if let Some(src) = opts.get("src") {
        sidecars.push(PathBuf::from(src));
    }
    let p = Path::new(path);
    for ext in ["srt", "vtt", "json"] {
        sidecars.push(p.with_extension(ext));
    }
    for sc in &sidecars {
        if !sc.is_file() || sc == p {
            continue;
        }
        let text = std::fs::read_to_string(sc)?;
        if sc.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(segs) = normalize_whisper_json(&v) {
                    return Ok(json!({"engine": "sidecar", "segments": segs}));
                }
            }
            continue;
        }
        let segs = parse_subs(&text);
        if !segs.is_empty() {
            return Ok(json!({"engine": "sidecar", "file": sc.to_string_lossy(), "segments": segs}));
        }
    }
    let lang = opts.get("lang").cloned().unwrap_or_else(|| "auto".into());
    let tmp = std::env::temp_dir().join(format!("moodcut_tx_{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    // 2) whisper.cpp CLI
    let model = opts.get("model").map(PathBuf::from).filter(|p| p.is_file()).or_else(whisper_model);
    let cpp = std::env::var("MOODCUT_WHISPER_BIN").ok().map(PathBuf::from).filter(|p| p.is_file()).or_else(|| which("whisper-cli")).or_else(|| which("whisper-cpp"));
    if let (Some(bin), Some(model)) = (cpp, model.clone()) {
        let wav = tmp.join("in.wav");
        wav16k(ff, path, &wav)?;
        let of = tmp.join("out");
        let st = command(&bin)
            .args(["-m", &model.to_string_lossy(), "-f", &wav.to_string_lossy(), "-l", &lang, "-oj", "-ml", "1", "-sow", "-of", &of.to_string_lossy()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("running whisper.cpp")?;
        if st.success() {
            let j: Value = serde_json::from_str(&std::fs::read_to_string(of.with_extension("json"))?)?;
            let words: Vec<(String, f64, f64)> = j["transcription"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|w| {
                            let t = w["text"].as_str()?.trim().to_string();
                            let s = w["offsets"]["from"].as_f64()? / 1000.0;
                            let e = w["offsets"]["to"].as_f64()? / 1000.0;
                            if t.is_empty() {
                                None
                            } else {
                                Some((t, s, e))
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = std::fs::remove_dir_all(&tmp);
            return Ok(json!({"engine": "whisper.cpp", "segments": words_to_segments(&words)}));
        }
    }
    // 3) openai-whisper python CLI
    if let Some(bin) = which("whisper") {
        let wav = tmp.join("in.wav");
        wav16k(ff, path, &wav)?;
        let mut c = command(&bin);
        c.args([wav.to_string_lossy().as_ref(), "--model", opts.get("model").map(|s| s.as_str()).unwrap_or("base"), "--output_format", "json", "--word_timestamps", "True", "--output_dir", &tmp.to_string_lossy()]);
        if lang != "auto" {
            c.args(["--language", &lang]);
        }
        let st = c.stdout(Stdio::null()).stderr(Stdio::null()).status();
        if st.map(|s| s.success()).unwrap_or(false) {
            let j: Value = serde_json::from_str(&std::fs::read_to_string(tmp.join("in.json"))?)?;
            let _ = std::fs::remove_dir_all(&tmp);
            if let Some(segs) = normalize_whisper_json(&j) {
                return Ok(json!({"engine": "openai-whisper", "segments": segs}));
            }
        }
    }
    // 4) FFmpeg >= 8 whisper filter
    if ff.has_filter("whisper") {
        if let Some(model) = model {
            let out = tmp.join("ff.json");
            let dest = crate::render::ffmpeg::esc_filter(&out.to_string_lossy());
            let m = crate::render::ffmpeg::esc_filter(&model.to_string_lossy());
            let r = ff.run_capture(&ffargs(&["-i", path, "-vn", "-af", &format!("aresample=16000,whisper=model={m}:language={lang}:queue=10:destination={dest}:format=json"), "-f", "null", "-"]));
            if r.is_ok() {
                let text = std::fs::read_to_string(&out).unwrap_or_default();
                let mut segs = vec![];
                for l in text.lines() {
                    if let Ok(v) = serde_json::from_str::<Value>(l) {
                        let s = v["start"].as_f64().unwrap_or(0.0);
                        let e = v["end"].as_f64().unwrap_or(s);
                        let (s, e) = if e > 10000.0 { (s / 1000.0, e / 1000.0) } else { (s, e) };
                        segs.push(json!({"s": round3(s), "e": round3(e), "text": v["text"].as_str().unwrap_or("").trim()}));
                    }
                }
                let _ = std::fs::remove_dir_all(&tmp);
                if !segs.is_empty() {
                    return Ok(json!({"engine": "ffmpeg-whisper", "segments": segs}));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    bail!(
        "no speech-to-text engine available. Options: (1) install whisper.cpp (`whisper-cli`) and put a ggml model in {} or set MOODCUT_WHISPER_MODEL; \
         (2) `pip install openai-whisper`; (3) put an .srt/.vtt next to the media or pass src=<file.srt>",
        crate::config::data_dir().map(|d| d.join("models").to_string_lossy().to_string()).unwrap_or_default()
    )
}

fn normalize_whisper_json(v: &Value) -> Option<Vec<Value>> {
    let segs = v["segments"].as_array()?;
    let mut out = vec![];
    for s in segs {
        let st = s["start"].as_f64().or_else(|| s["s"].as_f64())?;
        let en = s["end"].as_f64().or_else(|| s["e"].as_f64())?;
        let text = s["text"].as_str().unwrap_or("").trim().to_string();
        let words: Vec<Value> = s["words"]
            .as_array()
            .map(|ws| {
                ws.iter()
                    .filter_map(|w| {
                        let t = w["word"].as_str().or_else(|| w["w"].as_str())?.trim().to_string();
                        let a = w["start"].as_f64().or_else(|| w["s"].as_f64())?;
                        let b = w["end"].as_f64().or_else(|| w["e"].as_f64())?;
                        Some(json!({"w": t, "s": round3(a), "e": round3(b)}))
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(json!({"s": round3(st), "e": round3(en), "text": text, "words": words}));
    }
    Some(out)
}

/// Groups word timings into sentence-like segments.
pub fn words_to_segments(words: &[(String, f64, f64)]) -> Vec<Value> {
    let mut segs = vec![];
    let mut cur: Vec<&(String, f64, f64)> = vec![];
    for (i, w) in words.iter().enumerate() {
        let gap = if i > 0 { w.1 - words[i - 1].2 } else { 0.0 };
        if !cur.is_empty() && (gap > 0.8 || cur.len() >= 24) {
            segs.push(seg_json(&cur));
            cur.clear();
        }
        cur.push(w);
        if w.0.ends_with(['.', '?', '!']) {
            segs.push(seg_json(&cur));
            cur.clear();
        }
    }
    if !cur.is_empty() {
        segs.push(seg_json(&cur));
    }
    segs
}

fn seg_json(ws: &[&(String, f64, f64)]) -> Value {
    let text = ws.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ");
    json!({"s": round3(ws[0].1), "e": round3(ws.last().unwrap().2), "text": text, "words": ws.iter().map(|w| json!({"w": w.0, "s": round3(w.1), "e": round3(w.2)})).collect::<Vec<_>>()})
}

/// Runs an analysis by kind.
pub fn run(ff: &Ff, path: &str, dur: f64, kind: &str, opts: &BTreeMap<String, String>) -> Result<Value> {
    let f = |k: &str, d: f64| opts.get(k).and_then(|v| v.trim_end_matches("dB").trim_end_matches("db").parse::<f64>().ok()).unwrap_or(d);
    match kind {
        "silence" | "silences" => silence(ff, path, f("thresh", -35.0), f("min", 0.4), dur),
        "scenes" | "scene" | "shots" => scenes(ff, path, f("thresh", 0.3)),
        "loudness" | "lufs" => loudness(ff, path),
        "beats" | "tempo" | "bpm" => beats(ff, path),
        "transcript" | "transcribe" | "speech" | "stt" => transcript(ff, path, opts),
        "black" => black(ff, path),
        "stats" | "color" | "exposure" => stats(ff, path, dur),
        other => bail!("unknown analysis '{other}' ({})", KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join(" ")),
    }
}

/// Compact text rendering of an analysis result for AI clients.
pub fn summarize(kind: &str, v: &Value, max_items: usize) -> String {
    let f = |x: &Value| crate::model::value::fmt_num(x.as_f64().unwrap_or(0.0));
    match kind {
        "silence" | "silences" => {
            let arr = v["silences"].as_array().cloned().unwrap_or_default();
            let parts: Vec<String> = arr.iter().take(max_items).map(|r| format!("{}-{}", f(&r[0]), f(&r[1]))).collect();
            format!("{} silence(s), {}s total: {}{}", arr.len(), f(&v["silent_total"]), parts.join(" "), if arr.len() > max_items { " …" } else { "" })
        }
        "scenes" | "scene" | "shots" => {
            let arr = v["cuts"].as_array().cloned().unwrap_or_default();
            let parts: Vec<String> = arr.iter().take(max_items).map(f).collect();
            format!("{} cut(s): {}{}", arr.len(), parts.join(" "), if arr.len() > max_items { " …" } else { "" })
        }
        "beats" | "tempo" | "bpm" => {
            let arr = v["beats"].as_array().cloned().unwrap_or_default();
            let parts: Vec<String> = arr.iter().take(max_items).map(f).collect();
            format!("{} BPM, {} beats: {}{}", f(&v["bpm"]), arr.len(), parts.join(" "), if arr.len() > max_items { " …" } else { "" })
        }
        "transcript" | "transcribe" | "speech" | "stt" => {
            let segs = v["segments"].as_array().cloned().unwrap_or_default();
            let mut s = format!("{} segment(s) via {}\n", segs.len(), v["engine"].as_str().unwrap_or("?"));
            for seg in segs.iter().take(max_items) {
                s.push_str(&format!("[{}-{}] {}\n", f(&seg["s"]), f(&seg["e"]), seg["text"].as_str().unwrap_or("")));
            }
            if segs.len() > max_items {
                s.push_str(&format!("… {} more (use view what=transcript)", segs.len() - max_items));
            }
            s.trim_end().to_string()
        }
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_tracker_finds_tempo() {
        // 120 BPM clicks for 12 seconds
        let sr = 11025.0;
        let mut x = vec![0f32; (sr * 12.0) as usize];
        let mut t = 0.25;
        while t < 12.0 {
            let i = (t * sr) as usize;
            for k in 0..200 {
                if i + k < x.len() {
                    x[i + k] = ((k as f32) * 0.7).sin() * (1.0 - k as f32 / 200.0);
                }
            }
            t += 0.5;
        }
        let (bpm, beats, _) = beats_from_samples(&x, sr);
        assert!((bpm - 120.0).abs() < 4.0, "bpm {bpm}");
        assert!(beats.len() >= 18, "{}", beats.len());
    }

    #[test]
    fn subs() {
        let srt = "1\n00:00:01,000 --> 00:00:02,500\nHello <i>world</i>\n\n2\n00:00:03,000 --> 00:00:04,000\nBye\n";
        let s = parse_subs(srt);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0]["text"], "Hello world");
        assert_eq!(s[1]["s"], 3.0);
    }
}
