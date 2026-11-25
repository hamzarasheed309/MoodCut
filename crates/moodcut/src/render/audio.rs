//! Compiles a comp's audio into a single FFmpeg filtergraph: per-clip trims,
//! speed/reverse, gain automation (keyframes, fades, crossfades), pan, audio
//! effects, track buses with sidechain ducking, master bus with loudnorm.

use super::eval::Ev;
use super::ffmpeg::Ff;
use crate::model::anim::{Anim, Ease};
use crate::model::expr::ExprCtx;
use crate::model::project::*;
use crate::model::schema::{self, VCtx};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct AudioInput {
    pub path: String,
    pub ss: f64,
    pub t: f64,
}

#[derive(Clone, Debug, Default)]
pub struct AudioPlan {
    pub inputs: Vec<AudioInput>,
    pub graph: String,
    pub label: String,
    pub empty: bool,
}

struct Seg {
    path: String,
    src: f64,
    span: f64,
    speed: f64,
    reverse: bool,
    delay: f64,
    out_dur: f64,
    gain: String,
    pan: f64,
    afx: Vec<Stage>,
    /// Pan automation over the segment's own time (an FFmpeg expression).
    pan_expr: Option<String>,
    track: String,
}

/// Maps comp time c to output time o = a + c / k.
#[derive(Clone, Copy)]
struct TMap {
    a: f64,
    k: f64,
}

fn n(v: f64) -> String {
    format!("{:.6}", v).trim_end_matches('0').trim_end_matches('.').to_string()
}

fn db_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// A fade gain curve over x = 0..1: linear, log (fast start), exp (slow
/// start), scurve, or power (equal-power, for crossfades).
fn fade_shape(x: &str, curve: &str) -> String {
    match curve {
        "log" => format!("(1-pow(1-{x},2))"),
        "exp" => format!("pow({x},2)"),
        "scurve" | "s" => format!("({x}*{x}*(3-2*{x}))"),
        "power" | "equalpower" => format!("sin(PI/2*{x})"),
        _ => x.to_string(),
    }
}

/// Piecewise expression of a dB anim over item-local time `tl` (an ffmpeg expr string).
fn db_expr(a: Option<&Anim>, tl: &str) -> String {
    let Some(a) = a else { return "0".into() };
    if a.k.is_empty() {
        return n(a.v.num());
    }
    // sample eased segments into linear pieces
    let mut pts: Vec<(f64, f64)> = vec![];
    for w in a.k.windows(2) {
        let (k0, k1) = (&w[0], &w[1]);
        let steps = if k0.e == Ease::Linear || k0.e == Ease::Hold { 1 } else { 6 };
        for s in 0..steps {
            let f = s as f64 / steps as f64;
            let t = k0.t + (k1.t - k0.t) * f;
            pts.push((t, a.keyed(t).num()));
        }
    }
    let last = a.k.last().unwrap();
    pts.push((last.t, last.v.num()));
    let mut e = n(pts.last().unwrap().1);
    for i in (0..pts.len() - 1).rev() {
        let (t0, v0) = pts[i];
        let (t1, v1) = pts[i + 1];
        let seg = if (t1 - t0).abs() < 1e-9 { n(v1) } else { format!("({}+({})*({}-{})/{})", n(v0), n(v1 - v0), tl, n(t0), n(t1 - t0)) };
        e = format!("if(lt({tl},{}),{seg},{e})", n(t1));
    }
    format!("if(lt({tl},{}),{},{e})", n(pts[0].0), n(pts[0].1))
}

fn atempo_chain(mut f: f64) -> Vec<String> {
    let mut v = vec![];
    if (f - 1.0).abs() < 1e-6 {
        return v;
    }
    while f > 2.0 {
        v.push("atempo=2".into());
        f /= 2.0;
    }
    while f < 0.5 {
        v.push("atempo=0.5".into());
        f /= 0.5;
    }
    v.push(format!("atempo={}", n(f)));
    v
}

pub fn afx_filter(e: &Effect, ff: &Ff) -> Option<String> {
    if !e.on {
        return None;
    }
    let def = schema::audio_effect(&e.kind)?;
    let g = |k: &str| -> f64 {
        e.props.get(k).map(|a| a.v.num()).unwrap_or_else(|| schema::find(def.params, k).map(|p| schema::default_val(p, VCtx { cw: 0.0, ch: 0.0 }).num()).unwrap_or(0.0))
    };
    Some(match def.name {
        "eq" => {
            let mut v = vec![];
            if g("bass") != 0.0 {
                v.push(format!("bass=g={}:f=100", n(g("bass"))));
            }
            if g("mid") != 0.0 {
                v.push(format!("equalizer=f=1000:t=q:w=1:g={}", n(g("mid"))));
            }
            if g("treble") != 0.0 {
                v.push(format!("treble=g={}:f=8000", n(g("treble"))));
            }
            if v.is_empty() {
                return None;
            }
            v.join(",")
        }
        "highpass" => format!("highpass=f={}", n(g("freq").max(10.0))),
        "lowpass" => format!("lowpass=f={}", n(g("freq").max(20.0))),
        "compress" => format!(
            "acompressor=threshold={}:ratio={}:attack={}:release={}:makeup={}",
            n(db_lin(g("threshold")).clamp(0.000976563, 1.0)),
            n(g("ratio").clamp(1.0, 20.0)),
            n(g("attack").clamp(0.01, 2000.0)),
            n(g("release").clamp(0.01, 9000.0)),
            n(db_lin(g("makeup")).clamp(1.0, 64.0))
        ),
        "limiter" => limiter(ff, db_lin(g("limit")).clamp(0.0625, 1.0), g("truepeak") >= 0.5),
        "multiband" => {
            // three bands: points input/output dB with the threshold and ratio
            let (t, r) = (g("threshold").clamp(-60.0, 0.0), g("ratio").clamp(1.0, 20.0));
            let pts = format!("-90/-90,{}/{},0/{}", n(t), n(t), n(t - t / r));
            let (lo, hi) = (g("low").clamp(40.0, 2000.0), g("high").clamp(500.0, 16000.0));
            let hi = hi.max(lo * 1.5);
            format!("mcompand='0.005,0.1 6 {pts} {} | 0.003,0.05 6 {pts} {} | 0.000625,0.0125 6 {pts} 22000'", n(lo), n(hi))
        }
        "declip" => "adeclip".into(),
        "stereo" => format!(
            "stereotools=mlev=1:slev={}:balance_out={}:phasel={}:phaser={}",
            n(g("width").clamp(0.015625, 3.0)),
            n(g("balance").clamp(-1.0, 1.0)),
            (g("invertl") >= 0.5) as i32,
            (g("invertr") >= 0.5) as i32
        ),
        "gate" => format!("agate=threshold={}", n(db_lin(g("threshold")).clamp(0.0, 1.0))),
        "denoise" => {
            let nr = n(g("amount").clamp(0.01, 97.0));
            let learn = e.props.get("learn").map(|a| a.v.str()).unwrap_or_default();
            match learn.split_once('-').and_then(|(a, b)| Some((a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?))).filter(|(a, b)| b > a) {
                // noise print: sample the noise over a quiet range, then reduce with it
                Some((a, b)) if ff.has_filter("asendcmd") => format!("asendcmd=c='{} afftdn sn start; {} afftdn sn stop',afftdn=nr={nr}:nf=-40", n(a), n(b)),
                _ => format!("afftdn=nr={nr}:nf=-40"),
            }
        }
        "normalize" => format!("loudnorm=I={}:TP={}:LRA=11", n(g("lufs").clamp(-70.0, -5.0)), n(g("tp").clamp(-9.0, 0.0))),
        "reverb" => {
            let a = g("amount").clamp(0.0, 1.0);
            let s = g("size").clamp(0.0, 1.0);
            format!("aecho=0.8:0.85:{}|{}|{}:{}|{}|{}", n(30.0 + s * 60.0), n(55.0 + s * 110.0), n(90.0 + s * 160.0), n(a * 0.5), n(a * 0.35), n(a * 0.22))
        }
        "echo" => format!("aecho=0.8:0.9:{}:{}", n(g("delay").clamp(1.0, 5000.0)), n(g("decay").clamp(0.0, 1.0))),
        "pitch" => {
            let f = 2f64.powf(g("semitones") / 12.0);
            if (f - 1.0).abs() < 1e-6 {
                return None;
            }
            if ff.has_filter("rubberband") {
                format!("rubberband=pitch={}", n(f))
            } else {
                format!("asetrate=48000*{},aresample=48000,{}", n(f), atempo_chain(1.0 / f).join(","))
            }
        }
        "deess" => {
            let a = g("amount").clamp(0.0, 1.0);
            // adynamicequalizer would be more precise, but its option names changed
            // between FFmpeg 6 and 7; deesser behaves the same everywhere
            format!("deesser=i={}", n(a))
        }
        "voice" => {
            let a = g("amount").clamp(0.0, 1.0);
            format!("highpass=f=80,afftdn=nr={},acompressor=threshold=0.125:ratio={}:attack=10:release=200:makeup=2,equalizer=f=3000:t=q:w=1:g={}", n(6.0 + 8.0 * a), n(2.0 + 2.0 * a), n(2.0 * a + 1.0))
        }
        "telephone" => "highpass=f=300,lowpass=f=3400,acompressor=ratio=4".into(),
        "peq" if e.props.get("bands").is_some_and(|b| !b.v.str().trim().is_empty()) => {
            let spec = e.props.get("bands").map(|b| b.v.str()).unwrap_or_default();
            let mut out = vec![];
            for b in spec.split_whitespace().take(8) {
                let v: Vec<&str> = b.split(',').collect();
                let num = |i: usize, d: f64| v.get(i).and_then(|x| x.trim().parse::<f64>().ok()).unwrap_or(d);
                let (f, gain, q) = (num(0, 1000.0).clamp(20.0, 20000.0), num(1, 0.0).clamp(-30.0, 30.0), num(2, 1.0).clamp(0.1, 20.0));
                out.push(match v.get(3).map(|s| s.trim()).unwrap_or("bell") {
                    "lowshelf" | "ls" => format!("lowshelf=f={}:g={}:t=q:w={}", n(f), n(gain), n(q.min(2.0))),
                    "highshelf" | "hs" => format!("highshelf=f={}:g={}:t=q:w={}", n(f), n(gain), n(q.min(2.0))),
                    "highpass" | "hp" | "lowcut" => format!("highpass=f={}", n(f)),
                    "lowpass" | "lp" | "highcut" => format!("lowpass=f={}", n(f)),
                    _ if gain.abs() < 1e-6 => continue,
                    _ => format!("equalizer=f={}:t=q:w={}:g={}", n(f), n(q), n(gain)),
                });
            }
            if out.is_empty() {
                return None;
            }
            out.join(",")
        }
        "peq" => {
            let bands: Vec<String> = (1..=3)
                .filter(|i| g(&format!("g{i}")).abs() > 1e-6)
                .map(|i| format!("equalizer=f={}:t=q:w={}:g={}", n(g(&format!("f{i}")).clamp(20.0, 20000.0)), n(g(&format!("q{i}")).clamp(0.1, 20.0)), n(g(&format!("g{i}")).clamp(-30.0, 30.0))))
                .collect();
            if bands.is_empty() {
                return None;
            }
            bands.join(",")
        }
        "bassboost" => format!("bass=g={}:f={}", n(g("gain").clamp(-30.0, 30.0)), n(g("freq").clamp(20.0, 1000.0))),
        "gain" => {
            if g("db").abs() < 1e-6 {
                return None;
            }
            format!("volume={}dB", n(g("db").clamp(-60.0, 30.0)))
        }
        "chorus" => {
            let (d, m) = (g("depth").clamp(0.0, 1.0), g("mix").clamp(0.0, 1.0));
            format!("chorus=0.6:0.9:45|62:{}|{}:0.25|0.4:{}|{}", n(0.5 * m), n(0.4 * m), n(1.0 + 2.0 * d), n(1.3 + 2.0 * d))
        }
        "flanger" => format!("flanger=delay=0:depth={}:regen=0:width=71:speed={}", n(g("depth").clamp(0.0, 10.0)), n(g("speed").clamp(0.1, 10.0))),
        "phaser" => format!("aphaser=in_gain=0.4:out_gain=0.74:delay=3:decay={}:speed={}", n(g("decay").clamp(0.0, 0.99)), n(g("speed").clamp(0.1, 2.0))),
        "tremolo" => format!("tremolo=f={}:d={}", n(g("freq").clamp(0.1, 20000.0)), n(g("depth").clamp(0.0, 1.0))),
        "vibrato" => format!("vibrato=f={}:d={}", n(g("freq").clamp(0.1, 20000.0)), n(g("depth").clamp(0.0, 1.0))),
        "distortion" => {
            let drive = 1.0 + g("amount").clamp(0.0, 1.0) * 24.0;
            if ff.has_filter("asoftclip") {
                format!("volume={}dB,asoftclip=type=tanh,volume=-{}dB", n(drive), n(drive * 0.6))
            } else {
                format!("volume={}dB,alimiter=limit=0.5,volume=-{}dB", n(drive), n(drive * 0.6))
            }
        }
        "bitcrush" => format!("acrusher=bits={}:mode=lin:mix=1:aa=1:samples={}", n(g("bits").clamp(1.0, 16.0)), n(g("downsample").clamp(1.0, 250.0))),
        "widen" => format!("extrastereo=m={}", n(g("amount").clamp(0.0, 4.0))),
        "mono" => "pan=stereo|c0=0.5*c0+0.5*c1|c1=0.5*c0+0.5*c1".into(),
        "swap" => "pan=stereo|c0=c1|c1=c0".into(),
        "dehum" => {
            let f = if g("freq") >= 55.0 { 60.0 } else { 50.0 };
            (1..=(g("harmonics").round().clamp(1.0, 8.0) as i32)).map(|k| format!("bandreject=f={}:t=q:w=30", n(f * k as f64))).collect::<Vec<_>>().join(",")
        }
        "declick" => "adeclick".into(),
        "exciter" => {
            if ff.has_filter("aexciter") {
                format!("aexciter=amount={}:freq={}", n(g("amount").clamp(0.0, 64.0)), n(g("freq").clamp(2000.0, 12000.0)))
            } else {
                format!("treble=g={}:f={}", n(2.0 * g("amount").clamp(0.0, 10.0)), n(g("freq").clamp(2000.0, 12000.0)))
            }
        }
        "autogain" => format!("dynaudnorm=f=250:g=15:p=0.9:m={}", n(1.0 + g("strength").clamp(0.0, 1.0) * 19.0)),
        "ff" => {
            let f = e.props.get("filter").map(|a| a.v.str()).unwrap_or_default();
            if f.trim().is_empty() {
                return None;
            }
            f
        }
        _ => return None,
    })
}

struct Ctx<'a> {
    p: &'a Project,
    ff: &'a Ff,
    segs: Vec<Seg>,
    depth: u32,
}

impl<'a> Ctx<'a> {
    fn collect(&mut self, comp: &Comp, map: TMap, win: (f64, f64), gain_db: f64, track_key: Option<&str>) {
        if self.depth > 16 {
            return;
        }
        let solo = comp.tracks.iter().any(|t| t.solo);
        let fd = 0.5 / comp.fps.max(1.0);
        for tr in &comp.tracks {
            if tr.muted || (solo && !tr.solo) {
                continue;
            }
            let key = match track_key {
                Some(k) => k.to_string(),
                None => tr.id.clone(),
            };
            let items = &tr.items;
            for (i, it) in items.iter().enumerate() {
                if it.off {
                    continue;
                }
                let lead = if i > 0 && (it.start - items[i - 1].end()).abs() <= fd { items[i - 1].tout.as_ref().map(|t| t.dur / 2.0).unwrap_or(0.0) } else { 0.0 };
                let tail = if i + 1 < items.len() && (items[i + 1].start - it.end()).abs() <= fd { it.tout.as_ref().map(|t| t.dur / 2.0).unwrap_or(0.0) } else { 0.0 };
                let o_s = map.a + (it.start - lead) / map.k;
                let o_e = map.a + (it.end() + tail) / map.k;
                let ws = o_s.max(win.0);
                let we = o_e.min(win.1);
                if we - ws < 0.005 {
                    continue;
                }
                let vc = VCtx { cw: comp.w as f64, ch: comp.h as f64 };
                let x = ExprCtx { dur: it.dur, fps: comp.fps, cw: comp.w as f64, ch: comp.h as f64, ..Default::default() };
                let lists = [schema::item_props(it.kind), schema::LAYER];
                let ev = Ev::new(0.0, &x, vc, &lists);
                if ev.bool(&it.props, "mute") {
                    continue;
                }
                // root tracks apply their gain on the bus; nested tracks fold it into the clip
                let vol_db = gain_db + if track_key.is_some() { tr.vol } else { 0.0 };
                match it.kind {
                    ItemKind::Clip => {
                        let Some(m) = it.src.as_deref().and_then(|s| self.p.media(s)) else { continue };
                        if !m.has_audio {
                            continue;
                        }
                        let sp = it.speed();
                        let eff = sp * map.k; // source s per output s
                        // comp-time of segment start
                        let c_ws = (ws - map.a) * map.k;
                        let c_we = (we - map.a) * map.k;
                        let tl0 = c_ws - it.start;
                        let rev = it.reversed();
                        let mut src = if rev { it.src_in + (it.end() - c_we) * sp } else { it.src_in + tl0 * sp };
                        let span = (we - ws) * eff;
                        let mut delay = ws - win.0;
                        if src < 0.0 {
                            delay += -src / eff;
                            src = 0.0;
                        }
                        if src >= m.dur {
                            continue;
                        }
                        // gain expression over output time t (since segment start): tl = tl0 + t*k
                        let tl = format!("({}+t*{})", n(tl0), n(map.k));
                        let mut parts = vec![format!("pow(10,({}+{})/20)", db_expr(it.props.get("vol"), &tl), n(vol_db))];
                        let fi = ev.num(&it.props, "fadeIn");
                        let fo = ev.num(&it.props, "fadeOut");
                        let curve = ev.str(&it.props, "fadeCurve");
                        if fi > 0.0 {
                            parts.push(fade_shape(&format!("clip({}/{},0,1)", tl, n(fi)), &curve));
                        }
                        if fo > 0.0 {
                            parts.push(fade_shape(&format!("clip(({}-{})/{},0,1)", n(it.dur), tl, n(fo)), &curve));
                        }
                        // audio transitions are equal-power crossfades (no dip in the middle)
                        if lead > 0.0 {
                            parts.push(fade_shape(&format!("clip(({}+{})/{},0,1)", tl, n(lead), n(2.0 * lead)), "power"));
                        }
                        if tail > 0.0 {
                            parts.push(fade_shape(&format!("clip(({}+{}-{})/{},0,1)", n(it.dur), n(tail), tl, n(2.0 * tail)), "power"));
                        }
                        let afx = it.afx.iter().filter_map(|e| afx_stage(e, self.ff)).collect();
                        let pan_expr = it.props.get("pan").filter(|a| a.is_animated()).map(|a| db_expr(Some(a), &tl));
                        self.segs.push(Seg { path: m.path.clone(), src, span, speed: eff, reverse: rev, delay, out_dur: we - ws, gain: parts.join("*"), pan: ev.num(&it.props, "pan"), pan_expr, afx, track: key.clone() });
                    }
                    ItemKind::Comp => {
                        let Some(c) = it.src.as_deref().and_then(|s| self.p.comp(s)) else { continue };
                        let sp = it.speed();
                        // child comp time cc maps to parent time: start + (cc - src_in)/sp
                        // output: o = map.a + (start + (cc - src_in)/sp)/map.k
                        let child = TMap { a: map.a + (it.start - it.src_in / sp) / map.k, k: map.k * sp };
                        let g = gain_db + ev.num(&it.props, "vol");
                        self.depth += 1;
                        self.collect(c, child, (ws, we), g, Some(&key));
                        self.depth -= 1;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// How the master loudness target is met.
#[derive(Clone, Copy, PartialEq)]
enum Loud {
    /// measure the whole mix first if it isn't cached (renders, checks)
    Measure,
    /// use a cached measurement, else adaptive loudnorm (live preview)
    Cached,
    /// pre-master mix for measuring: no loudness stage, no limiter
    Off,
}

/// Builds the audio plan for comp `comp_id` over [t0, t1). Inputs are numbered from `first_input`.
pub fn plan(p: &Project, ff: &Ff, comp_id: &str, t0: f64, t1: f64, first_input: usize) -> AudioPlan {
    plan_with(p, ff, comp_id, t0, t1, first_input, Loud::Measure)
}

/// Like [`plan`] but never blocks on a loudness measurement.
pub fn plan_preview(p: &Project, ff: &Ff, comp_id: &str, t0: f64, t1: f64, first_input: usize) -> AudioPlan {
    plan_with(p, ff, comp_id, t0, t1, first_input, Loud::Cached)
}

static LOUDNESS: std::sync::Mutex<BTreeMap<u64, f64>> = std::sync::Mutex::new(BTreeMap::new());

/// Stable across runs and builds (it names files in the cache folder).
fn mix_key(pl: &AudioPlan) -> u64 {
    let mut s = pl.graph.clone();
    for i in &pl.inputs {
        let modified = std::fs::metadata(&i.path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
        let _ = write!(s, "|{}|{}|{}|{modified}", i.path, i.ss.to_bits(), i.t.to_bits());
    }
    super::eval::hash_str(&s)
}

/// Integrated loudness (LUFS) of the whole comp mix before the master
/// loudness stage. Cached per mix, so range renders and re-renders reuse it.
fn premaster_lufs(p: &Project, ff: &Ff, comp: &Comp, measure: bool) -> Option<f64> {
    let pre = plan_with(p, ff, &comp.id, 0.0, comp.duration(), 0, Loud::Off);
    if pre.empty {
        return None;
    }
    let key = mix_key(&pre);
    if let Some(v) = LOUDNESS.lock().ok()?.get(&key) {
        return Some(*v);
    }
    // measured in an earlier session: <data>/cache/loudness/<key>.txt
    let disk = crate::config::cache_dir().join("loudness").join(format!("{key:016x}.txt"));
    if let Some(v) = std::fs::read_to_string(&disk).ok().and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| v.is_finite()) {
        crate::cache::touch(&disk);
        LOUDNESS.lock().ok()?.insert(key, v);
        return Some(v);
    }
    if !measure {
        return None;
    }
    let mut args = input_args(&pre);
    let (gargs, _graph_file) = ff.graph_args(&format!("{};[{}]loudnorm=print_format=json[mc_m]", pre.graph, pre.label)).ok()?;
    args.extend(gargs);
    args.extend(["-map".into(), "[mc_m]".into(), "-f".into(), "null".into(), "-".into()]);
    let (_, err) = ff.run_capture(&args).ok()?;
    let j = err.rfind('{').and_then(|i| err[i..].find('}').map(|k| &err[i..=i + k]))?;
    let v: serde_json::Value = serde_json::from_str(j).ok()?;
    let i = v["input_i"].as_str()?.parse::<f64>().ok().filter(|i| i.is_finite() && *i > -69.0)?;
    LOUDNESS.lock().ok()?.insert(key, i);
    if let Some(dir) = disk.parent() {
        if std::fs::create_dir_all(dir).is_ok() {
            let _ = std::fs::write(&disk, format!("{i}"));
        }
    }
    Some(i)
}

fn plan_with(p: &Project, ff: &Ff, comp_id: &str, t0: f64, t1: f64, first_input: usize, loud: Loud) -> AudioPlan {
    plan_only(p, ff, comp_id, t0, t1, first_input, loud, None)
}

/// One track or bus alone, before the master stage (stems, audio scopes).
pub fn plan_node(p: &Project, ff: &Ff, comp_id: &str, t0: f64, t1: f64, first_input: usize, node: &str) -> AudioPlan {
    plan_only(p, ff, comp_id, t0, t1, first_input, Loud::Off, Some(node))
}

#[allow(clippy::too_many_arguments)]
fn plan_only(p: &Project, ff: &Ff, comp_id: &str, t0: f64, t1: f64, first_input: usize, loud: Loud, only: Option<&str>) -> AudioPlan {
    let Some(comp) = p.comp(comp_id) else { return AudioPlan { empty: true, ..Default::default() } };
    let dur = (t1 - t0).max(0.001);
    let mut cx = Ctx { p, ff, segs: vec![], depth: 0 };
    cx.collect(comp, TMap { a: -t0, k: 1.0 }, (0.0, dur), 0.0, None);
    let mut g = String::new();
    let mut inputs = vec![];
    if cx.segs.is_empty() {
        let _ = write!(g, "anullsrc=r=48000:cl=stereo,atrim=0:{}[aout]", n(dur));
        return AudioPlan { inputs, graph: g, label: "aout".into(), empty: true };
    }
    let mut gw = Gw { g, n: 0, taps: BTreeMap::new() };
    let mut by_track: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (i, s) in cx.segs.iter().enumerate() {
        let idx = first_input + inputs.len();
        inputs.push(AudioInput { path: s.path.clone(), ss: s.src, t: s.span + 0.05 });
        let mut st = vec![Stage::F("aresample=48000".into()), Stage::F("aformat=sample_fmts=fltp:channel_layouts=stereo".into())];
        if s.reverse {
            st.push(Stage::F("areverse".into()));
        }
        st.extend(atempo_chain(s.speed).into_iter().map(Stage::F));
        st.push(Stage::F("asetpts=PTS-STARTPTS".into()));
        st.push(Stage::F(format!("atrim=0:{}", n(s.out_dur))));
        st.push(Stage::F(format!("volume='{}':eval=frame", s.gain)));
        match &s.pan_expr {
            Some(e) => st.push(Stage::PanAuto(e.clone())),
            None if s.pan.abs() > 1e-4 => st.push(Stage::F(format!("pan=stereo|c0={}*c0|c1={}*c1", n((1.0 - s.pan).min(1.0)), n((1.0 + s.pan).min(1.0))))),
            None => {}
        }
        st.extend(s.afx.iter().cloned());
        let ms = (s.delay * 1000.0).max(0.0).round() as i64;
        if ms > 0 {
            st.push(Stage::F(format!("adelay={ms}:all=1")));
        }
        gw.chain(&[format!("{idx}:a")], &st, &format!("s{i}"));
        by_track.entry(s.track.clone()).or_default().push(format!("s{i}"));
    }
    // comp time in graph expressions
    let tc = format!("(t+{})", n(t0));
    let strip = |vol: f64, pan: f64, auto: &BTreeMap<String, Anim>, afx: &[Effect]| -> Vec<Stage> {
        let mut st = vec![];
        match auto.get("vol").filter(|a| a.is_animated()) {
            Some(a) => st.push(Stage::F(format!("volume='pow(10,({}+{})/20)':eval=frame", db_expr(Some(a), &tc), n(vol)))),
            None if vol != 0.0 => st.push(Stage::F(format!("volume={}dB", n(vol)))),
            None => {}
        }
        match auto.get("pan").filter(|a| a.is_animated()) {
            Some(a) => st.push(Stage::PanAuto(format!("({}+{})", db_expr(Some(a), &tc), n(pan)))),
            None if pan.abs() > 1e-4 => st.push(Stage::F(format!("pan=stereo|c0={}*c0|c1={}*c1", n((1.0 - pan).min(1.0)), n((1.0 + pan).min(1.0))))),
            None => {}
        }
        st.extend(afx.iter().filter_map(|e| afx_stage(e, ff)));
        st
    };
    // which buses carry sound (fed by a sounding track or bus, not muted)
    let bus = |id: &str| comp.buses.iter().find(|b| b.id.eq_ignore_ascii_case(id));
    let mut active: BTreeMap<String, bool> = BTreeMap::new();
    let feeds = |out: &Option<String>, sends: &[AuxSend]| -> Vec<String> { out.iter().cloned().chain(sends.iter().map(|s| s.to.clone())).filter_map(|b| bus(&b).map(|x| x.id.clone())).collect() };
    for tid in by_track.keys() {
        if let Some(t) = comp.track(tid) {
            for b in feeds(&t.out, &t.sends) {
                active.insert(b, true);
            }
        }
    }
    loop {
        let before = active.len();
        for b in &comp.buses {
            if active.contains_key(&b.id) && !b.muted {
                for x in feeds(&b.out, &b.sends) {
                    active.insert(x, true);
                }
            }
        }
        if active.len() == before {
            break;
        }
    }
    active.retain(|id, _| bus(id).is_some_and(|b| !b.muted));
    let ducked = |d: &Option<Duck>| d.as_ref().is_some_and(|d| by_track.contains_key(&d.by) || active.contains_key(&d.by));
    let fin = |id: &str| -> String {
        if let Some(b) = bus(id).filter(|_| active.contains_key(id)) {
            if ducked(&b.duck) { format!("b_{}_d", b.id) } else { format!("b_{}", b.id) }
        } else if comp.track(id).is_some_and(|t| ducked(&t.duck)) {
            format!("t_{id}_d")
        } else {
            format!("t_{id}")
        }
    };
    let mut bus_in: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut master_in: Vec<String> = vec![];
    // a node's outputs: main out (a bus or the master) and sends
    let route = |gw: &mut Gw, id: &str, out: &Option<String>, sends: &[AuxSend], pre: Option<String>, bus_in: &mut BTreeMap<String, Vec<String>>, master_in: &mut Vec<String>| {
        let f = fin(id);
        match out.as_deref().and_then(bus) {
            Some(b) if active.contains_key(&b.id) => bus_in.entry(b.id.clone()).or_default().push(gw.tap(&f)),
            Some(_) => {
                let x = gw.tap(&f);
                let _ = write!(gw.g, "[{x}]anullsink;");
            }
            None => master_in.push(gw.tap(&f)),
        }
        for s in sends {
            let Some(b) = bus(&s.to).filter(|b| active.contains_key(&b.id)) else { continue };
            let src = if s.pre { pre.clone().unwrap_or_else(|| f.clone()) } else { f.clone() };
            let x = gw.tap(&src);
            let y = gw.label("snd");
            let _ = write!(gw.g, "[{x}]volume={}dB[{y}];", n(s.db));
            bus_in.entry(b.id.clone()).or_default().push(y);
        }
    };
    for (tid, labels) in &by_track {
        let t = comp.track(tid);
        let st = t.map(|t| strip(t.vol, t.pan, &t.auto, &t.afx)).unwrap_or_default();
        let pre = t.filter(|t| t.sends.iter().any(|s| s.pre)).map(|_| format!("tp_{tid}"));
        match &pre {
            Some(pl) => {
                gw.chain(labels, &[], pl);
                let x = gw.tap(pl);
                gw.chain(&[x], &st, &format!("t_{tid}"));
            }
            None => gw.chain(labels, &st, &format!("t_{tid}")),
        }
        let (out, sends) = t.map(|t| (t.out.clone(), t.sends.clone())).unwrap_or_default();
        route(&mut gw, tid, &out, &sends, pre, &mut bus_in, &mut master_in);
    }
    for b in comp.buses.iter().filter(|b| active.contains_key(&b.id)) {
        route(&mut gw, &b.id, &b.out, &b.sends, None, &mut bus_in, &mut master_in);
    }
    for b in comp.buses.iter().filter(|b| active.contains_key(&b.id)) {
        let ins = bus_in.remove(&b.id).unwrap_or_default();
        if ins.is_empty() {
            let _ = write!(gw.g, "anullsrc=r=48000:cl=stereo,atrim=0:{}[b_{}];", n(dur), b.id);
        } else {
            gw.chain(&ins, &strip(b.vol, b.pan, &b.auto, &b.afx), &format!("b_{}", b.id));
        }
    }
    // ducking: a track or bus compressed by another's signal
    let duckers: Vec<(String, String, Duck)> = comp.tracks.iter().filter(|t| by_track.contains_key(&t.id)).filter_map(|t| t.duck.clone().map(|d| (t.id.clone(), format!("t_{}", t.id), d))).chain(comp.buses.iter().filter(|b| active.contains_key(&b.id)).filter_map(|b| b.duck.clone().map(|d| (b.id.clone(), format!("b_{}", b.id), d)))).collect();
    for (id, base, d) in duckers {
        if !(by_track.contains_key(&d.by) || active.contains_key(&d.by)) || d.by == id {
            continue;
        }
        let (x, sc) = (gw.tap(&base), gw.tap(&fin(&d.by)));
        let ratio = (1.0 + d.amount.abs() / 2.5).clamp(1.5, 20.0);
        let _ = write!(gw.g, "[{x}][{sc}]sidechaincompress=threshold=0.025:ratio={}:attack={}:release={}:makeup=1[{base}_d];", n(ratio), n(d.attack.clamp(0.01, 2000.0)), n(d.release.clamp(0.01, 9000.0)));
    }
    // one node alone (stems, audio scopes): the rest is discarded
    if let Some(o) = only {
        let id = comp.track(o).map(|t| t.id.clone()).or_else(|| bus(o).map(|b| b.id.clone())).unwrap_or_default();
        if !id.eq_ignore_ascii_case("master") {
            for l in master_in.drain(..) {
                let _ = write!(gw.g, "[{l}]anullsink;");
            }
            if by_track.contains_key(&id) || active.contains_key(&id) {
                master_in.push(gw.tap(&fin(&id)));
            }
        }
    }
    if master_in.is_empty() {
        let l = gw.label("sil");
        let _ = write!(gw.g, "anullsrc=r=48000:cl=stereo,atrim=0:{}[{l}];", n(dur));
        master_in.push(l);
    }
    // master
    let mut chain = vec![];
    if master_in.len() > 1 {
        chain.push(format!("amix=inputs={}:normalize=0:dropout_transition=0:duration=longest", master_in.len()));
    } else {
        chain.push("anull".into());
    }
    let m = &comp.master;
    if only.is_none() {
        if m.vol != 0.0 {
            chain.push(format!("volume={}dB", n(m.vol)));
        }
        chain.extend(m.afx.iter().filter_map(|e| afx_filter(e, ff)));
    }
    if let Some(l) = m.lufs.filter(|_| loud != Loud::Off && only.is_none()) {
        // Single-pass loudnorm can't settle on short programs and lands several
        // LU low, so the whole mix is measured and given the exact gain, then a
        // limiter holds peaks under -1.5 dBFS.
        let target = l.clamp(-70.0, -5.0);
        match premaster_lufs(p, ff, comp, loud == Loud::Measure) {
            Some(i) => {
                chain.push(format!("volume={}dB", n(((target - i) * 100.0).round() / 100.0)));
                chain.push(limiter(ff, 0.841, m.truepeak));
            }
            None => {
                chain.push(format!("loudnorm=I={}:TP=-1.5:LRA=11", n(target)));
                chain.push("aresample=48000".into());
            }
        }
    }
    if m.limiter && loud != Loud::Off && only.is_none() {
        chain.push(limiter(ff, 0.944, m.truepeak));
    }
    chain.push(format!("apad=whole_dur={}", n(dur)));
    chain.push(format!("atrim=0:{}", n(dur)));
    let ins: String = master_in.iter().map(|l| format!("[{l}]")).collect();
    let _ = write!(gw.g, "{ins}{}[aout]", chain.join(","));
    // taps: one consumer is a rename, several an asplit
    let mut g = std::mem::take(&mut gw.g);
    for (src, outs) in &gw.taps {
        let o: String = outs.iter().map(|l| format!("[{l}]")).collect();
        if outs.len() == 1 {
            let _ = write!(g, ";[{src}]anull{o}");
        } else {
            let _ = write!(g, ";[{src}]asplit={}{o}", outs.len());
        }
    }
    AudioPlan { inputs, graph: g, label: "aout".into(), empty: false }
}

/// A step in an audio chain: a plain filter, a filter blended with the dry
/// signal (`mix=`), or pan automation (a per-channel gain expression).
#[derive(Clone, Debug)]
pub enum Stage {
    F(String),
    Wet(String, f64),
    PanAuto(String),
    /// Convolution with an impulse response file, blended in at the amount.
    Conv(String, f64),
}

/// Writes filtergraph text with unique labels and deferred splits.
struct Gw {
    g: String,
    n: usize,
    /// label -> consumers; resolved into anull / asplit at the end
    taps: BTreeMap<String, Vec<String>>,
}

impl Gw {
    fn label(&mut self, p: &str) -> String {
        self.n += 1;
        format!("{p}{}", self.n)
    }

    /// A fresh label carrying `src` to one more consumer.
    fn tap(&mut self, src: &str) -> String {
        let v = self.taps.entry(src.to_string()).or_default();
        let l = format!("{src}_k{}", v.len());
        v.push(l.clone());
        l
    }

    /// `ins` (mixed when several) through `stages` into `[out]`.
    fn chain(&mut self, ins: &[String], stages: &[Stage], out: &str) {
        let mut src: String = ins.iter().map(|l| format!("[{l}]")).collect();
        let mut plain: Vec<String> = vec![];
        if ins.len() > 1 {
            plain.push(format!("amix=inputs={}:normalize=0:dropout_transition=0:duration=longest", ins.len()));
        }
        let flush = |gw: &mut Gw, src: &mut String, plain: &mut Vec<String>| -> String {
            let x = gw.label("c");
            let body = if plain.is_empty() { "anull".to_string() } else { plain.join(",") };
            let _ = write!(gw.g, "{src}{body}[{x}];");
            plain.clear();
            x
        };
        for st in stages {
            match st {
                Stage::F(f) => plain.push(f.clone()),
                Stage::Wet(f, m) => {
                    let x = flush(self, &mut src, &mut plain);
                    let (d, w, fx, y) = (self.label("dry"), self.label("wet"), self.label("fx"), self.label("c"));
                    let m = m.clamp(0.0, 1.0);
                    let _ = write!(self.g, "[{x}]asplit=2[{d}][{w}];[{w}]{f}[{fx}];[{d}][{fx}]amix=inputs=2:weights='{} {}':normalize=0:duration=first[{y}];", n(1.0 - m), n(m));
                    src = format!("[{y}]");
                }
                Stage::Conv(path, amt) => {
                    let x = flush(self, &mut src, &mut plain);
                    let (d, w, ir, rv, y) = (self.label("dry"), self.label("wet"), self.label("ir"), self.label("rv"), self.label("c"));
                    let _ = write!(self.g, "[{x}]asplit=2[{d}][{w}];amovie={path},aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[{ir}];[{w}][{ir}]afir[{rv}];[{d}][{rv}]amix=inputs=2:weights='1 {}':normalize=0:duration=first[{y}];", n(amt.clamp(0.0, 1.0)));
                    src = format!("[{y}]");
                }
                Stage::PanAuto(e) => {
                    let x = flush(self, &mut src, &mut plain);
                    let (l, r, l2, r2, y) = (self.label("pl"), self.label("pr"), self.label("pl"), self.label("pr"), self.label("c"));
                    let _ = write!(self.g, "[{x}]channelsplit=channel_layout=stereo[{l}][{r}];[{l}]volume='min(1,1-({e}))':eval=frame[{l2}];[{r}]volume='min(1,1+({e}))':eval=frame[{r2}];[{l2}][{r2}]amerge=inputs=2[{y}];");
                    src = format!("[{y}]");
                }
            }
        }
        let body = if plain.is_empty() { "anull".to_string() } else { plain.join(",") };
        let _ = write!(self.g, "{src}{body}[{out}];");
    }
}

/// An audio effect as a chain stage: wet/dry blended when `mix` is below 1
/// (effects with their own wet control, like chorus, keep theirs).
pub fn afx_stage(e: &Effect, ff: &Ff) -> Option<Stage> {
    let def = schema::audio_effect(&e.kind)?;
    if def.name == "reverb" && e.on && ff.has_filter("afir") && ff.has_filter("amovie") {
        let kind = e.props.get("type").map(|a| a.v.str()).unwrap_or_else(|| "room".into());
        if kind != "echo" {
            let num = |k: &str, d: f64| e.props.get(k).map(|a| a.v.num()).unwrap_or(d);
            if let Some(p) = ir_file(&kind, num("size", 0.5)) {
                let mix = num("mix", 1.0).clamp(0.0, 1.0);
                return Some(Stage::Conv(crate::render::colorspace::filter_path(&p), num("amount", 0.3).clamp(0.0, 1.0) * mix));
            }
        }
    }
    let f = afx_filter(e, ff)?;
    if def.params.iter().any(|p| p.name == "mix") {
        return Some(Stage::F(f));
    }
    let m = e.props.get("mix").map(|a| a.v.num()).unwrap_or(1.0);
    Some(if m < 0.999 { Stage::Wet(f, m) } else { Stage::F(f) })
}

/// The peak limiter; `truepeak` oversamples 4x so inter-sample peaks stay
/// under the ceiling too (-1 dBTP delivery).
pub fn limiter(ff: &Ff, limit: f64, truepeak: bool) -> String {
    let base = format!("alimiter=limit={}:attack=2:release=60:level=disabled", n(limit));
    if truepeak && ff.has_filter("aresample") {
        format!("aresample=192000,{base},aresample=48000")
    } else {
        base
    }
}

/// FFmpeg input arguments for a plan.
pub fn input_args(plan: &AudioPlan) -> Vec<String> {
    let mut v = vec![];
    for i in &plan.inputs {
        if i.ss > 0.0 {
            v.push("-ss".into());
            v.push(format!("{:.6}", i.ss));
        }
        v.push("-t".into());
        v.push(format!("{:.6}", i.t));
        v.push("-i".into());
        v.push(i.path.clone());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::anim::Key;
    use crate::model::value::Val;

    #[test]
    fn every_audio_effect_builds_a_valid_filter() {
        let Ok(ff) = Ff::find() else { return };
        for def in schema::AUDIO_EFFECTS.iter().filter(|d| d.name != "ff") {
            let mut e = Effect { id: "a1".into(), kind: def.name.into(), on: true, props: Default::default() };
            for (k, v) in [("g2", 4.0), ("db", -3.0), ("semitones", 2.0)] {
                if schema::find(def.params, k).is_some() {
                    e.props.insert(k.into(), Anim::new(crate::model::value::Val::N(v)));
                }
            }
            let Some(f) = afx_filter(&e, &ff) else { continue };
            let args: Vec<String> = ["-v", "error", "-f", "lavfi", "-i", "sine=f=440:d=0.2,aformat=channel_layouts=stereo", "-af", &f, "-f", "null", "-"].iter().map(|s| s.to_string()).collect();
            let (_, err) = ff.run_capture(&args).unwrap_or_else(|e| panic!("{}: {f}: {e}", def.name));
            assert!(!err.to_lowercase().contains("error"), "{}: {f}: {err}", def.name);
        }
    }

    #[test]
    fn long_graphs_go_through_a_file() {
        assert_eq!(crate::render::ffmpeg::ffmpeg_major("7.1-full_build-www.gyan.dev"), Some(7));
        assert_eq!(crate::render::ffmpeg::ffmpeg_major("n6.1.2"), Some(6));
        assert_eq!(crate::render::ffmpeg::ffmpeg_major("2025-01-20-git-1a2b3c"), None);
        let Ok(ff) = Ff::find() else { return };
        // a 40 KB graph: longer than a Windows command line allows
        let chain = "volume=volume=1.0000,".repeat(1700);
        let graph = format!("[0:a]{chain}anull[mc_o]");
        assert!(graph.len() > 32_768);
        let (gargs, gf) = ff.graph_args(&graph).unwrap();
        assert!(gf.as_ref().is_some_and(|g| g.path.is_file()));
        let mut args: Vec<String> = ["-v", "error", "-f", "lavfi", "-i", "sine=f=440:d=0.2"].iter().map(|s| s.to_string()).collect();
        args.extend(gargs);
        args.extend(["-map", "[mc_o]", "-f", "null", "-"].iter().map(|s| s.to_string()));
        ff.run_capture(&args).unwrap_or_else(|e| panic!("long graph via file: {e}"));
        let p = gf.as_ref().unwrap().path.clone();
        drop(gf);
        assert!(!p.exists());
    }

    #[test]
    fn buses_sends_and_automation_render() {
        struct E;
        impl crate::ops::Env for E {
            fn probe(&self, _: &str) -> anyhow::Result<Media> {
                anyhow::bail!("no probing in this test")
            }
            fn analyze(&self, _: &Project, _: &str, _: &str, _: &BTreeMap<String, String>) -> anyhow::Result<serde_json::Value> {
                anyhow::bail!("no analysis in this test")
            }
        }
        let dir = std::env::temp_dir().join(format!("mc-mix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("tone.wav");
        let frames: Vec<[f32; 2]> = (0..48000 * 2).map(|i| { let v = 0.2 * (i as f32 * 0.05).sin(); [v, v] }).collect();
        crate::sfx::write_wav(&wav, &frames).unwrap();
        let mut p = Project::new("t", 640, 360, 30.0);
        p.media.push(serde_json::from_value(serde_json::json!({"id": "m1", "path": wav.to_string_lossy(), "name": "tone", "kind": "audio", "dur": 2.0, "has_audio": true, "channels": 2, "rate": 48000})).unwrap());
        let main = p.main.clone();
        let script = "add m1 A1 at=0 dur=2\nadd m1 A2 at=0 dur=2\nbus add B1 name=Dialogue\nbus add B2\ntrack set A1 out=B1\nsend A1 B2 -6dB\nsend A2 B2 -12 pre\nafx B2 reverb type=room amount=0.5\nafx B1 compress mix=0.5\nkf A1 vol 0:-12 2:0\nkf B1 pan 0:-1 2:1\nduck A2 by=B1\nset $1 pan=0";
        crate::ops::run(&mut p, &E, &main, script).unwrap_or_else(|e| panic!("{e}"));
        // routing a bus into itself is refused
        assert!(crate::ops::run(&mut p.clone(), &E, &main, "bus set B2 out=B2").is_err());
        assert!(crate::ops::run(&mut p.clone(), &E, &main, "bus set B2 out=B1\nbus set B1 out=B2").is_err());
        let Ok(ff) = Ff::find() else { return };
        for pl in [plan(&p, &ff, &main, 0.0, 2.0, 0), plan_node(&p, &ff, &main, 0.0, 2.0, 0, "B1"), plan_node(&p, &ff, &main, 0.0, 2.0, 0, "A2")] {
            let mut args: Vec<String> = vec!["-v".into(), "error".into()];
            args.extend(input_args(&pl));
            let (g, _gf) = ff.graph_args(&pl.graph).unwrap();
            args.extend(g);
            args.extend(["-map".to_string(), format!("[{}]", pl.label), "-f".into(), "null".into(), "-".into()]);
            ff.run_capture(&args).unwrap_or_else(|e| panic!("{e}\n{}", pl.graph));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn db_expression() {
        let mut a = Anim::new(Val::N(0.0));
        a.set_keys(vec![Key { t: 0.0, v: Val::N(-60.0), e: Ease::Linear }, Key { t: 2.0, v: Val::N(0.0), e: Ease::Linear }]);
        let e = db_expr(Some(&a), "T");
        assert!(e.contains("if(lt(T,2)"), "{e}");
        assert_eq!(atempo_chain(4.0), vec!["atempo=2", "atempo=2"]);
        assert_eq!(atempo_chain(0.25).len(), 2);
    }
}

/// A synthesized stereo impulse response (room, hall, plate, booth), cached
/// in the data folder: decorrelated noise with an exponential decay, a few
/// early reflections and darkening tails (air absorption), as small rooms and
/// halls sound; the plate stays bright and dense.
pub fn ir_file(kind: &str, size: f64) -> Option<std::path::PathBuf> {
    let size = size.clamp(0.0, 1.0);
    let (t60, early, damp) = match kind {
        "booth" => (0.25, 3, 0.55),
        "hall" => (2.4, 8, 0.25),
        "plate" => (1.5, 0, 0.05),
        _ => (0.7, 6, 0.4),
    };
    let t60 = t60 * (0.6 + 0.8 * size);
    let path = crate::config::cache_dir().join("irs").join(format!("{kind}-{:.0}-v1.wav", size * 100.0));
    if path.is_file() {
        return Some(path);
    }
    let rate = 48000.0;
    let len = ((t60 * 1.2).min(4.0) * rate) as usize;
    let mut seed = 0x9e37_79b9u32 ^ (kind.len() as u32 * 7919) ^ ((size * 1000.0) as u32);
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let mut ir = vec![[0f32; 2]; len];
    let (mut lp_l, mut lp_r) = (0f32, 0f32);
    for (i, s) in ir.iter_mut().enumerate() {
        let t = i as f64 / rate;
        let env = (-6.9 * t / t60).exp() as f32;
        // darker as it decays
        let k = (1.0 - damp * (t / t60).min(1.0)) as f32;
        lp_l += (rnd() - lp_l) * k;
        lp_r += (rnd() - lp_r) * k;
        *s = [lp_l * env * 0.5, lp_r * env * 0.5];
    }
    for e in 0..early {
        let at = ((0.004 + 0.007 * e as f64 * (0.5 + size)) * rate) as usize;
        if at < len {
            let g = 0.6 / (1.0 + e as f32);
            ir[at][e % 2] += g;
            ir[(at + 37).min(len - 1)][(e + 1) % 2] += g * 0.7;
        }
    }
    ir[0] = [0.0, 0.0];
    std::fs::create_dir_all(path.parent()?).ok()?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    crate::sfx::write_wav(&tmp, &ir).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}
