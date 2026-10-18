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
    afx: Vec<String>,
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
        "limiter" => format!("alimiter=limit={}:level=disabled", n(db_lin(g("limit")).clamp(0.0625, 1.0))),
        "gate" => format!("agate=threshold={}", n(db_lin(g("threshold")).clamp(0.0, 1.0))),
        "denoise" => format!("afftdn=nr={}:nf=-40", n(g("amount").clamp(0.01, 97.0))),
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
        "deess" => format!("deesser=i={}", n(g("amount").clamp(0.0, 1.0))),
        "voice" => {
            let a = g("amount").clamp(0.0, 1.0);
            format!("highpass=f=80,afftdn=nr={},acompressor=threshold=0.125:ratio={}:attack=10:release=200:makeup=2,equalizer=f=3000:t=q:w=1:g={}", n(6.0 + 8.0 * a), n(2.0 + 2.0 * a), n(2.0 * a + 1.0))
        }
        "telephone" => "highpass=f=300,lowpass=f=3400,acompressor=ratio=4".into(),
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
                        if fi > 0.0 {
                            parts.push(format!("clip({}/{},0,1)", tl, n(fi)));
                        }
                        if fo > 0.0 {
                            parts.push(format!("clip(({}-{})/{},0,1)", n(it.dur), tl, n(fo)));
                        }
                        if lead > 0.0 {
                            parts.push(format!("clip(({}+{})/{},0,1)", tl, n(lead), n(2.0 * lead)));
                        }
                        if tail > 0.0 {
                            parts.push(format!("clip(({}+{}-{})/{},0,1)", n(it.dur), n(tail), tl, n(2.0 * tail)));
                        }
                        let afx = it.afx.iter().filter_map(|e| afx_filter(e, self.ff)).collect();
                        self.segs.push(Seg { path: m.path.clone(), src, span, speed: eff, reverse: rev, delay, out_dur: we - ws, gain: parts.join("*"), pan: ev.num(&it.props, "pan"), afx, track: key.clone() });
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

fn mix_key(pl: &AudioPlan) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    pl.graph.hash(&mut h);
    for i in &pl.inputs {
        i.path.hash(&mut h);
        i.ss.to_bits().hash(&mut h);
        i.t.to_bits().hash(&mut h);
        std::fs::metadata(&i.path).and_then(|m| m.modified()).ok().hash(&mut h);
    }
    h.finish()
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
    if !measure {
        return None;
    }
    let mut args = input_args(&pre);
    args.extend(["-filter_complex".into(), format!("{};[{}]loudnorm=print_format=json[mc_m]", pre.graph, pre.label), "-map".into(), "[mc_m]".into(), "-f".into(), "null".into(), "-".into()]);
    let (_, err) = ff.run_capture(&args).ok()?;
    let j = err.rfind('{').and_then(|i| err[i..].find('}').map(|k| &err[i..=i + k]))?;
    let v: serde_json::Value = serde_json::from_str(j).ok()?;
    let i = v["input_i"].as_str()?.parse::<f64>().ok().filter(|i| i.is_finite() && *i > -69.0)?;
    LOUDNESS.lock().ok()?.insert(key, i);
    Some(i)
}

fn plan_with(p: &Project, ff: &Ff, comp_id: &str, t0: f64, t1: f64, first_input: usize, loud: Loud) -> AudioPlan {
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
    let mut by_track: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (i, s) in cx.segs.iter().enumerate() {
        let idx = first_input + inputs.len();
        inputs.push(AudioInput { path: s.path.clone(), ss: s.src, t: s.span + 0.05 });
        let mut chain = vec!["aresample=48000".to_string(), "aformat=sample_fmts=fltp:channel_layouts=stereo".into()];
        if s.reverse {
            chain.push("areverse".into());
        }
        chain.extend(atempo_chain(s.speed));
        chain.push("asetpts=PTS-STARTPTS".into());
        chain.push(format!("atrim=0:{}", n(s.out_dur)));
        chain.push(format!("volume='{}':eval=frame", s.gain));
        if s.pan.abs() > 1e-4 {
            let l = (1.0 - s.pan).min(1.0);
            let r = (1.0 + s.pan).min(1.0);
            chain.push(format!("pan=stereo|c0={}*c0|c1={}*c1", n(l), n(r)));
        }
        chain.extend(s.afx.iter().cloned());
        let ms = (s.delay * 1000.0).max(0.0).round() as i64;
        if ms > 0 {
            chain.push(format!("adelay={ms}:all=1"));
        }
        let _ = write!(g, "[{idx}:a]{}[s{i}];", chain.join(","));
        by_track.entry(s.track.clone()).or_default().push(format!("s{i}"));
    }
    // track buses
    let mut track_labels: BTreeMap<String, String> = BTreeMap::new();
    for (tid, labels) in &by_track {
        let tl = format!("t_{tid}");
        let mut chain = vec![];
        let tr = comp.track(tid);
        if let Some(t) = tr {
            if t.vol != 0.0 {
                chain.push(format!("volume={}dB", n(t.vol)));
            }
            if t.pan.abs() > 1e-4 {
                chain.push(format!("pan=stereo|c0={}*c0|c1={}*c1", n((1.0 - t.pan).min(1.0)), n((1.0 + t.pan).min(1.0))));
            }
            chain.extend(t.afx.iter().filter_map(|e| afx_filter(e, ff)));
        }
        if labels.len() == 1 {
            let _ = write!(g, "[{}]{}[{tl}];", labels[0], if chain.is_empty() { "anull".into() } else { chain.join(",") });
        } else {
            let ins: String = labels.iter().map(|l| format!("[{l}]")).collect();
            let mix = format!("amix=inputs={}:normalize=0:dropout_transition=0:duration=longest", labels.len());
            let mut all = vec![mix];
            all.extend(chain);
            let _ = write!(g, "{ins}{}[{tl}];", all.join(","));
        }
        track_labels.insert(tid.clone(), tl);
    }
    // ducking: split sidechain sources
    let mut ducks: Vec<(String, String)> = vec![];
    for t in &comp.tracks {
        if let Some(d) = &t.duck {
            if track_labels.contains_key(&t.id) && track_labels.contains_key(&d.by) {
                ducks.push((t.id.clone(), d.by.clone()));
            }
        }
    }
    let mut sc_counter: BTreeMap<String, usize> = BTreeMap::new();
    for (_, by) in &ducks {
        *sc_counter.entry(by.clone()).or_default() += 1;
    }
    for (by, cnt) in &sc_counter {
        let src = track_labels[by].clone();
        let outs: String = (0..=*cnt).map(|i| format!("[{src}_{i}]")).collect();
        let _ = write!(g, "[{src}]asplit={}{outs};", cnt + 1);
        track_labels.insert(by.clone(), format!("{src}_0"));
    }
    let mut used_sc: BTreeMap<String, usize> = BTreeMap::new();
    for (tid, by) in &ducks {
        let t = comp.track(tid).unwrap();
        let d = t.duck.as_ref().unwrap();
        let k = used_sc.entry(by.clone()).or_insert(0);
        *k += 1;
        let base = format!("t_{by}");
        let sc = format!("{base}_{k}");
        let src = track_labels[tid].clone();
        let out = format!("{src}_d");
        let ratio = (1.0 + d.amount.abs() / 2.5).clamp(1.5, 20.0);
        let _ = write!(g, "[{src}][{sc}]sidechaincompress=threshold=0.025:ratio={}:attack={}:release={}:makeup=1[{out}];", n(ratio), n(d.attack.clamp(0.01, 2000.0)), n(d.release.clamp(0.01, 9000.0)));
        track_labels.insert(tid.clone(), out);
    }
    // master
    let tls: Vec<String> = track_labels.values().cloned().collect();
    let mut chain = vec![];
    if tls.len() > 1 {
        chain.push(format!("amix=inputs={}:normalize=0:dropout_transition=0:duration=longest", tls.len()));
    } else {
        chain.push("anull".into());
    }
    let m = &comp.master;
    if m.vol != 0.0 {
        chain.push(format!("volume={}dB", n(m.vol)));
    }
    chain.extend(m.afx.iter().filter_map(|e| afx_filter(e, ff)));
    if let Some(l) = m.lufs.filter(|_| loud != Loud::Off) {
        // Single-pass loudnorm can't settle on short programs and lands several
        // LU low, so the whole mix is measured and given the exact gain, then a
        // limiter holds peaks under -1.5 dBFS.
        let target = l.clamp(-70.0, -5.0);
        match premaster_lufs(p, ff, comp, loud == Loud::Measure) {
            Some(i) => {
                chain.push(format!("volume={}dB", n(((target - i) * 100.0).round() / 100.0)));
                chain.push("alimiter=limit=0.841:attack=2:release=60:level=disabled".into());
            }
            None => {
                chain.push(format!("loudnorm=I={}:TP=-1.5:LRA=11", n(target)));
                chain.push("aresample=48000".into());
            }
        }
    }
    if m.limiter && loud != Loud::Off {
        chain.push("alimiter=limit=0.944:level=disabled".into());
    }
    chain.push(format!("apad=whole_dur={}", n(dur)));
    chain.push(format!("atrim=0:{}", n(dur)));
    let ins: String = tls.iter().map(|l| format!("[{l}]")).collect();
    let _ = write!(g, "{ins}{}[aout]", chain.join(","));
    AudioPlan { inputs, graph: g, label: "aout".into(), empty: false }
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
    fn db_expression() {
        let mut a = Anim::new(Val::N(0.0));
        a.set_keys(vec![Key { t: 0.0, v: Val::N(-60.0), e: Ease::Linear }, Key { t: 2.0, v: Val::N(0.0), e: Ease::Linear }]);
        let e = db_expr(Some(&a), "T");
        assert!(e.contains("if(lt(T,2)"), "{e}");
        assert_eq!(atempo_chain(4.0), vec!["atempo=2", "atempo=2"]);
        assert_eq!(atempo_chain(0.25).len(), 2);
    }
}
