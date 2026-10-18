//! Edit QA (`check` tool): finds the mistakes that make an edit look
//! unprofessional or not what was asked, and proposes MoodCut Script fixes.
//!
//! Structure: black gaps, 1-2 frame gaps, flash frames, layers hidden under
//! opaque full-frame clips, audio running past the picture. Media: offline
//! files, upscaling, frame-rate mismatch, pillarboxing. Graphics and text
//! (measured from real renders): title-safe area, off-screen layers, overlaps,
//! text size, reading time, caption speed and contrast against the picture.
//! Transitions without source handles. Audio: integrated loudness and true
//! peak of the actual mix, music under dialogue without ducking, abrupt
//! endings. Expectations: duration, size, fps, loudness, captions, pacing.

use crate::engine::Engine;
use crate::model::project::{Comp, Item, ItemKind, MediaKind, Project, TrackKind};
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Val};
use crate::render::comp::{render_frame, RenderCtx};
use crate::render::encode::{self, Tile};
use anyhow::{anyhow, bail, Result};
use rayon::prelude::*;
use resvg::tiny_skia::{Paint, PathBuilder, Pixmap, Stroke, Transform};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

#[derive(Default, Clone)]
pub struct Expect {
    pub dur: Option<f64>,
    pub dur_tol: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub aspect: Option<f64>,
    pub fps: Option<f64>,
    pub lufs: Option<f64>,
    pub captions: Option<bool>,
    pub music: Option<bool>,
    pub max_shot: Option<f64>,
    pub min_shot: Option<f64>,
}

#[derive(Default, Clone)]
pub struct CheckOpts {
    pub comp: Option<String>,
    /// Delivery target: web (default) | youtube | social | podcast | broadcast.
    pub target: Option<String>,
    pub expect: Expect,
    /// Measure the real mix loudness (renders the audio once).
    pub audio: bool,
    /// Return frames of the problems with safe-area guides.
    pub frames: bool,
    /// Apply the proposed fixes as one undoable edit.
    pub fix: bool,
    /// Title-safe margin as a fraction of width/height (default 0.05).
    pub safe: Option<f64>,
}

pub struct CheckOut {
    pub text: String,
    pub sheet: Option<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sev {
    Error,
    Warn,
    Info,
}

#[derive(Clone)]
struct Issue {
    sev: Sev,
    t: Option<f64>,
    msg: String,
    fix: Option<String>,
    visual: bool,
}

#[derive(Default)]
struct Issues(Vec<Issue>);

impl Issues {
    fn add(&mut self, sev: Sev, t: Option<f64>, msg: impl Into<String>, fix: Option<String>, visual: bool) {
        self.0.push(Issue { sev, t, msg: msg.into(), fix, visual });
    }
}

/// c5,c6,c7,c9 -> c5..c7,c9 (the edit tools accept both)
fn compact_ids(ids: &[String]) -> String {
    let split = |id: &str| {
        let k = id.find(|ch: char| ch.is_ascii_digit())?;
        Some((id[..k].to_string(), id[k..].parse::<u64>().ok()?))
    };
    let mut out: Vec<String> = vec![];
    let mut run: Option<(String, u64, u64)> = None;
    let flush = |r: &Option<(String, u64, u64)>, out: &mut Vec<String>| {
        if let Some((p, a, b)) = r {
            out.push(if b > a { format!("{p}{a}..{p}{b}") } else { format!("{p}{a}") });
        }
    };
    for id in ids {
        match (split(id), &mut run) {
            (Some((p, n)), Some((rp, _, b))) if *rp == p && n == *b + 1 => *b = n,
            (Some((p, n)), _) => {
                flush(&run, &mut out);
                run = Some((p, n, n));
            }
            (None, _) => {
                flush(&run, &mut out);
                run = None;
                out.push(id.clone());
            }
        }
    }
    flush(&run, &mut out);
    out.join(",")
}

fn t2(t: f64) -> String {
    fmt_t((t * 100.0).round() / 100.0)
}

fn static_num(it: &Item, k: &str, d: f64) -> f64 {
    it.props.get(k).map(|a| a.v.num()).unwrap_or(d)
}

fn static_vec(it: &Item, k: &str, d: (f64, f64)) -> (f64, f64) {
    it.props.get(k).map(|a| a.v.vec()).unwrap_or(d)
}

fn static_str(it: &Item, k: &str) -> Option<String> {
    it.props.get(k).map(|a| a.v.str())
}

fn animated(it: &Item, k: &str) -> bool {
    it.props.get(k).is_some_and(|a| a.is_animated())
}

/// Largest value a (possibly keyframed) scale reaches.
fn max_scale(it: &Item) -> f64 {
    match it.props.get("scale") {
        None => 1.0,
        Some(a) => {
            let mut m = a.v.vec().0.max(a.v.vec().1);
            for k in &a.k {
                let (x, y) = k.v.vec();
                m = m.max(x).max(y);
            }
            m
        }
    }
}

fn label(it: &Item, p: &Project) -> String {
    match it.kind {
        ItemKind::Text => {
            let t: String = it.text.clone().unwrap_or_default().chars().take(24).collect();
            format!("{} '{}'", it.id, t.replace('\n', " "))
        }
        ItemKind::Clip => match it.src.as_deref().and_then(|s| p.media(s)) {
            Some(m) => format!("{} ({} {})", it.id, m.id, m.name.chars().take(20).collect::<String>()),
            None => it.id.clone(),
        },
        ItemKind::Comp => format!("{} ({})", it.id, it.src.as_deref().and_then(|s| p.comp(s)).map(|c| c.name.clone()).unwrap_or_default()),
        _ => match &it.name {
            Some(n) => format!("{} ({n})", it.id),
            None => it.id.clone(),
        },
    }
}

/// Relative luminance (WCAG) of an sRGB color 0..1.
fn luminance(r: f64, g: f64, b: f64) -> f64 {
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn contrast(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Items that produce picture (enabled, on visible tracks).
fn visual_items(c: &Comp, p: &Project) -> Vec<(usize, Item)> {
    let mut v = vec![];
    for t in c.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.hidden) {
        for it in &t.items {
            if it.off || it.matte_source_only(c) {
                continue;
            }
            if it.kind == ItemKind::Clip && it.src.as_deref().and_then(|s| p.media(s)).is_some_and(|m| m.kind == MediaKind::Audio) {
                continue;
            }
            if static_num(it, "opacity", 1.0) <= 0.001 && !animated(it, "opacity") {
                continue;
            }
            v.push((t.index(), it.clone()));
        }
    }
    v
}

fn merge(mut iv: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    iv.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(f64, f64)> = vec![];
    for (a, b) in iv {
        match out.last_mut() {
            Some(l) if a <= l.1 + 1e-6 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn overlap_len(iv: &[(f64, f64)], a: f64, b: f64) -> f64 {
    iv.iter().map(|(x, y)| (y.min(b) - x.max(a)).max(0.0)).sum()
}

trait MatteSource {
    fn matte_source_only(&self, c: &Comp) -> bool;
}

impl MatteSource for Item {
    /// Layers used as a track matte are hidden by the compositor.
    fn matte_source_only(&self, c: &Comp) -> bool {
        c.items().any(|o| o.matte.as_ref().is_some_and(|m| m.src == self.id))
    }
}

/// Measured on-screen box (comp px) and colors of an overlay layer.
struct Shot {
    t: f64,
    bbox: Option<[f64; 4]>,
    /// (text luminance, background luminance under the glyphs)
    lum: Option<(f64, f64)>,
}

const PROBE_W: u32 = 480;

/// Renders `keep` alone (plus its parents, invisible) on a transparent comp.
fn isolate(p: &Project, cid: &str, keep: &str) -> Project {
    let mut q = p.clone();
    let parents: HashSet<String> = {
        let c = p.comp(cid).unwrap();
        let mut s = HashSet::new();
        let mut cur = c.items().find(|i| i.id == keep).and_then(|i| i.parent.clone());
        while let Some(pid) = cur {
            if !s.insert(pid.clone()) {
                break;
            }
            cur = c.items().find(|i| i.id == pid).and_then(|i| i.parent.clone());
        }
        s
    };
    if let Some(c) = q.comps.iter_mut().find(|c| c.id == cid) {
        c.bg = crate::model::value::Color::TRANSPARENT;
        for t in c.tracks.iter_mut() {
            t.hidden = false;
            t.items.retain(|i| i.id == keep || parents.contains(&i.id));
            for i in t.items.iter_mut().filter(|i| i.id != keep) {
                i.props.insert("opacity".into(), crate::model::anim::Anim::new(Val::N(0.0)));
            }
        }
    }
    q
}

fn without(p: &Project, cid: &str, drop: &str) -> Project {
    let mut q = p.clone();
    if let Some(c) = q.comps.iter_mut().find(|c| c.id == cid) {
        for t in c.tracks.iter_mut() {
            t.items.retain(|i| i.id != drop);
        }
    }
    q
}

impl Engine {
    fn measure_overlay(&self, p: &Project, c: &Comp, it: &Item, want_contrast: bool) -> Shot {
        let t = (it.start + it.dur * 0.5).min(c.duration() - c.frame_dur()).max(0.0);
        let scale = PROBE_W as f32 / c.w as f32;
        let k = c.w as f64 / PROBE_W as f64;
        let alone = isolate(p, &c.id, &it.id);
        let rc = RenderCtx { p: &alone, frames: &self.frames, draft: true };
        let Ok(pm) = render_frame(&rc, &c.id, t, scale) else { return Shot { t, bbox: None, lum: None } };
        let (w, h) = (pm.width() as usize, pm.height() as usize);
        let d = pm.data();
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        let (mut tr, mut tg, mut tb, mut tn) = (0f64, 0f64, 0f64, 0f64);
        let mut mask = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                let a = d[o + 3];
                if a > 24 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
                if a > 200 {
                    let af = a as f64;
                    tr += d[o] as f64 / af;
                    tg += d[o + 1] as f64 / af;
                    tb += d[o + 2] as f64 / af;
                    tn += 1.0;
                    mask[y * w + x] = true;
                }
            }
        }
        if x0 == usize::MAX {
            return Shot { t, bbox: None, lum: None };
        }
        let bbox = Some([x0 as f64 * k, y0 as f64 * k, x1 as f64 * k, y1 as f64 * k]);
        let mut lum = None;
        if want_contrast && tn > 20.0 {
            let rest = without(p, &c.id, &it.id);
            let rc2 = RenderCtx { p: &rest, frames: &self.frames, draft: true };
            if let Ok(bg) = render_frame(&rc2, &c.id, t, scale) {
                let bd = bg.data();
                let (mut br, mut bgc, mut bb, mut bn) = (0f64, 0f64, 0f64, 0f64);
                for (i, on) in mask.iter().enumerate() {
                    if *on && i * 4 + 2 < bd.len() {
                        br += bd[i * 4] as f64 / 255.0;
                        bgc += bd[i * 4 + 1] as f64 / 255.0;
                        bb += bd[i * 4 + 2] as f64 / 255.0;
                        bn += 1.0;
                    }
                }
                if bn > 0.0 {
                    lum = Some((luminance(tr / tn, tg / tn, tb / tn), luminance(br / bn, bgc / bn, bb / bn)));
                }
            }
        }
        Shot { t, bbox, lum }
    }

    /// Integrated loudness, true peak and LRA of the comp's audio mix.
    fn measure_mix(&self, p: &Project, cid: &str) -> Result<Option<(f64, f64, f64)>> {
        let c = p.comp(cid).ok_or_else(|| anyhow!("no comp"))?;
        let ap = crate::render::audio::plan(p, &self.ff, cid, 0.0, c.duration(), 0);
        if ap.empty {
            return Ok(None);
        }
        let mut args = crate::render::audio::input_args(&ap);
        args.extend(["-filter_complex".into(), format!("{};[{}]loudnorm=print_format=json[mc_out]", ap.graph, ap.label), "-map".into(), "[mc_out]".into(), "-f".into(), "null".into(), "-".into()]);
        let (_, err) = self.ff.run_capture(&args)?;
        let j = err.rfind('{').and_then(|i| err[i..].find('}').map(|k| &err[i..=i + k])).ok_or_else(|| anyhow!("no loudness output"))?;
        let v: serde_json::Value = serde_json::from_str(j)?;
        let g = |k: &str| v[k].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(-70.0);
        Ok(Some((g("input_i"), g("input_tp"), g("input_lra"))))
    }

    pub fn check(&self, o: &CheckOpts, source: &str) -> Result<CheckOut> {
        let (p, _, active, _) = self.snapshot();
        let cid = o.comp.clone().unwrap_or(active);
        let c = p.comp(&cid).ok_or_else(|| anyhow!("no comp {cid}"))?.clone();
        let fd = c.frame_dur();
        let dur = c.duration();
        if dur <= 0.0 {
            bail!("{} is empty: nothing to check", c.id);
        }
        let mut is = Issues::default();
        let margin = o.safe.unwrap_or(0.05).clamp(0.0, 0.2);
        let target = o.target.clone().unwrap_or_else(|| "web".into()).to_ascii_lowercase();
        let (cw, ch) = (c.w as f64, c.h as f64);

        // ---------------- structure
        let vis = visual_items(&c, &p);
        let vis_iv = merge(vis.iter().map(|(_, i)| (i.start, i.end())).collect());
        let vis_end = vis_iv.last().map(|x| x.1).unwrap_or(0.0);
        if vis.is_empty() {
            is.add(Sev::Error, None, "nothing visible on the timeline", None, false);
        }
        let mut cursor = 0.0;
        for (a, b) in &vis_iv {
            if *a - cursor >= fd * 0.5 {
                let gap = a - cursor;
                if cursor == 0.0 {
                    is.add(Sev::Warn, Some(0.0), format!("starts with {}s of empty picture", t2(gap)), None, true);
                } else {
                    is.add(
                        if gap >= 2.0 * fd { Sev::Error } else { Sev::Warn },
                        Some(cursor + gap / 2.0),
                        format!("{} of empty picture at {}-{}s (nothing on any video track): fill it, extend a neighbour, or close the gap", if gap < 3.0 * fd { format!("{}-frame gap", (gap / fd).round().max(1.0)) } else { format!("{}s", t2(gap)) }, t2(cursor), t2(*a)),
                        None,
                        true,
                    );
                }
            }
            cursor = *b;
        }
        // tiny gaps / flash frames per video track
        for t in c.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.hidden) {
            let items: Vec<&Item> = t.items.iter().filter(|i| !i.off).collect();
            for w in items.windows(2) {
                let gap = w[1].start - w[0].end();
                if gap > fd * 0.25 && gap < fd * 3.0 {
                    is.add(Sev::Warn, Some(w[0].end()), format!("{}-frame gap between {} and {} on {} at {}s", (gap / fd).round().max(1.0), w[0].id, w[1].id, t.id, t2(w[0].end())), Some(format!("trim {} end=+{}", w[0].id, fmt_num((gap * 1000.0).round() / 1000.0))), true);
                }
            }
            for i in &items {
                if i.kind == ItemKind::Clip && i.dur < fd * 2.5 {
                    is.add(Sev::Warn, Some(i.start), format!("{} lasts {} frame(s): a flash frame", label(i, &p), (i.dur / fd).round().max(1.0)), Some(format!("del {}", i.id)), true);
                }
            }
        }
        // layers hidden under opaque full-frame clips above them
        let covers: Vec<(usize, f64, f64)> = vis
            .iter()
            .filter(|(_, i)| {
                let full = match i.kind {
                    ItemKind::Clip => i.src.as_deref().and_then(|s| p.media(s)).is_some_and(|m| {
                        let fit = static_str(i, "fit").unwrap_or_else(|| "fit".into());
                        let same_aspect = m.w > 0 && ((m.w as f64 / m.h.max(1) as f64) - cw / ch).abs() < 0.01;
                        !m.alpha && m.is_visual() && (fit == "fill" || fit == "stretch" || same_aspect)
                    }),
                    ItemKind::Solid => static_vec(i, "w", (cw, cw)).0 >= cw && static_vec(i, "h", (ch, ch)).0 >= ch && static_str(i, "color").is_none_or(|c| c.len() <= 7),
                    _ => false,
                };
                full && i.masks.is_empty()
                    && !i.fx.iter().any(|e| e.on && matches!(e.kind.as_str(), "crop" | "chromakey" | "lumakey" | "cornerpin"))
                    && static_num(i, "opacity", 1.0) >= 0.999
                    && !animated(i, "opacity")
                    && !["pos", "scale", "rot", "rx", "ry", "z"].iter().any(|k| animated(i, k))
                    && max_scale(i) >= 0.999
                    && static_num(i, "rot", 0.0).abs() < 0.01
                    && static_str(i, "blend").is_none_or(|b| b == "normal")
                    && i.parent.is_none()
                    && static_vec(i, "pos", (cw / 2.0, ch / 2.0)) == (cw / 2.0, ch / 2.0)
            })
            .map(|(ti, i)| (*ti, i.start + i.tin.as_ref().map(|t| t.dur).unwrap_or(0.0), i.end() - i.tout.as_ref().map(|t| t.dur).unwrap_or(0.0)))
            .collect();
        for (ti, i) in &vis {
            if i.dur <= 0.0 {
                continue;
            }
            let above = merge(covers.iter().filter(|(t, ..)| t > ti).map(|(_, a, b)| (*a, *b)).collect());
            if overlap_len(&above, i.start, i.end()) >= i.dur * 0.98 {
                is.add(Sev::Warn, Some(i.start + i.dur / 2.0), format!("{} on V{} is never visible: covered by an opaque full-frame clip above", label(i, &p), ti), None, true);
            }
        }

        // ---------------- media
        let mut fps_warned = HashSet::new();
        let mut soft: BTreeMap<String, (f64, Vec<String>, f64)> = BTreeMap::new(); // media -> (factor, clips, first time)
        let mut bars: BTreeMap<(bool, bool), Vec<(String, String, f64)>> = BTreeMap::new(); // (media portrait, comp portrait) -> (clip, label, time)
        for t in c.tracks.iter() {
            for i in t.items.iter().filter(|i| i.kind == ItemKind::Clip && !i.off) {
                let Some(m) = i.src.as_deref().and_then(|s| p.media(s)) else { continue };
                if !std::path::Path::new(&m.path).exists() {
                    is.add(Sev::Error, Some(i.start), format!("{} is offline: {} is missing (relink {} <new path>)", label(i, &p), m.path, m.id), None, false);
                    continue;
                }
                if t.kind == TrackKind::Video && m.is_visual() && m.gfx.is_none() && m.kind != MediaKind::Svg && m.w > 0 {
                    let fit = static_str(i, "fit").unwrap_or_else(|| "fit".into());
                    let (sx, sy) = (cw / m.w as f64, ch / m.h.max(1) as f64);
                    let base = match fit.as_str() {
                        "fill" | "stretch" => sx.max(sy),
                        "none" => 1.0,
                        _ => sx.min(sy),
                    };
                    let eff = base * max_scale(i);
                    if eff > 1.6 {
                        let e = soft.entry(format!("{}x{}", m.w, m.h)).or_insert((0.0, vec![], i.start + i.dur / 2.0));
                        e.0 = e.0.max(eff);
                        e.1.push(i.id.clone());
                    }
                    let (mp, cp) = (m.h > m.w, c.h > c.w);
                    if mp != cp && fit == "fit" && (m.w as f64 / m.h as f64 - cw / ch).abs() > 0.2 {
                        let below = vis.iter().any(|(ti, o)| *ti < t.index() && o.start < i.end() && o.end() > i.start);
                        if !below {
                            bars.entry((mp, cp)).or_default().push((i.id.clone(), label(i, &p), i.start + i.dur / 2.0));
                        }
                    }
                    if m.kind == MediaKind::Video && m.fps > 0.0 && (i.speed() - 1.0).abs() < 1e-6 && static_str(i, "interp").is_none_or(|x| x == "none") && fps_warned.insert(m.id.clone()) {
                        let r = c.fps / m.fps;
                        let clean = (r - r.round()).abs() < 0.02 || ((1.0 / r) - (1.0 / r).round()).abs() < 0.02;
                        if !clean {
                            is.add(Sev::Info, None, format!("{} is {} fps in a {} fps comp: uneven motion (judder)", m.id, fmt_num((m.fps * 100.0).round() / 100.0), fmt_num(c.fps)), Some(format!("set {} interp=blend", i.id)), false);
                        }
                    }
                }
            }
        }

        for ((mp, cp), v) in &bars {
            let (o, co) = (if *mp { "portrait" } else { "landscape" }, if *cp { "portrait" } else { "landscape" });
            let ids: Vec<String> = v.iter().map(|x| x.0.clone()).collect();
            let who = if v.len() == 1 { v[0].1.clone() } else if v.len() > 4 { format!("{} clips ({}…) are", v.len(), ids[..4].join(",")) } else { format!("{} are", ids.join(",")) };
            let verb = if v.len() == 1 { " is" } else { "" };
            is.add(Sev::Info, Some(v[0].2), format!("{who}{verb} {o} in a {co} comp: black bars on the sides"), Some(format!("set {} fit=blur", compact_ids(&ids))), true);
        }
        for (res, (eff, ids, t)) in &soft {
            let who = if ids.len() > 4 { format!("{} clips ({}…)", ids.len(), ids[..4].join(",")) } else { ids.join(",") };
            is.add(Sev::Warn, Some(*t), format!("{who} from {res} sources shown at up to {}x their resolution: soft/pixelated (use higher-res media, smaller scale, or a smaller comp)", fmt_num((eff * 10.0).round() / 10.0)), None, true);
        }

        // ---------------- transitions without source handles
        for t in c.tracks.iter().filter(|t| !t.hidden) {
            for w in t.items.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                let Some(tr) = &a.tout else { continue };
                if (b.start - a.end()).abs() > fd || tr.dur <= 0.0 {
                    continue;
                }
                let half = tr.dur / 2.0;
                let ha = a.src.as_deref().and_then(|s| p.media(s)).filter(|m| !m.is_still()).map(|m| m.dur - (a.src_in + a.dur * a.speed()));
                let hb = b.src.as_deref().and_then(|s| p.media(s)).filter(|m| !m.is_still()).map(|_| b.src_in);
                let short = [ha.map(|h| (a.id.as_str(), h, "after its out-point")), hb.map(|h| (b.id.as_str(), h, "before its in-point"))].into_iter().flatten().find(|(_, h, _)| *h + fd < half);
                if let Some((id, h, wher)) = short {
                    let ok_dur = (2.0 * h.max(0.0)).max(0.0);
                    let fix = (ok_dur >= 2.0 * fd).then(|| format!("tr {} {} {} {}", a.id, b.id, tr.kind, fmt_num((ok_dur * 100.0).floor() / 100.0)));
                    is.add(Sev::Warn, Some(a.end()), format!("{} {}→{}: {} has only {}s of media {wher} (needs {}s); frames freeze", tr.kind, a.id, b.id, id, t2(h.max(0.0)), t2(half)), fix, true);
                }
            }
        }

        // ---------------- overlays: title-safe, visibility, size, reading time, contrast, overlaps
        let overlays: Vec<Item> = c
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video && !t.hidden)
            .flat_map(|t| t.items.iter())
            .filter(|i| !i.off)
            .filter(|i| match i.kind {
                ItemKind::Text | ItemKind::Shape => true,
                ItemKind::Comp => true,
                ItemKind::Clip => i.src.as_deref().and_then(|s| p.media(s)).is_some_and(|m| m.gfx.is_some() || (m.alpha && m.is_visual())),
                _ => false,
            })
            .take(60)
            .cloned()
            .collect();
        let captions: Vec<&Item> = overlays.iter().filter(|i| i.kind == ItemKind::Text && (i.anims.iter().any(|a| a.kind == "hl") || c.tracks.iter().any(|t| t.name.as_deref() == Some("Captions") && t.items.iter().any(|x| x.id == i.id)))).collect();
        let cap_ids: HashSet<&str> = captions.iter().map(|i| i.id.as_str()).collect();
        let shots: Vec<(Item, Shot)> = overlays
            .par_iter()
            .map(|i| {
                let plain_text = i.kind == ItemKind::Text && static_str(i, "stroke").is_none_or(|s| s == "none") && static_str(i, "shadow").is_none_or(|s| s == "none") && static_str(i, "bg").is_none_or(|s| s == "none");
                (i.clone(), self.measure_overlay(&p, &c, i, plain_text))
            })
            .collect();
        let (sx0, sy0, sx1, sy1) = (cw * margin, ch * margin, cw * (1.0 - margin), ch * (1.0 - margin));
        let mut boxes: Vec<(&Item, [f64; 4])> = vec![];
        for (i, s) in &shots {
            let Some(b) = s.bbox else {
                if !cap_ids.contains(i.id.as_str()) {
                    is.add(Sev::Warn, Some(s.t), format!("{} is not visible at {}s (off-screen, transparent or empty)", label(i, &p), t2(s.t)), None, true);
                }
                continue;
            };
            let area = (b[2] - b[0]) * (b[3] - b[1]);
            if area > cw * ch * 0.85 {
                continue; // full-frame layer
            }
            boxes.push((i, b));
            if b[2] <= 0.0 || b[0] >= cw || b[3] <= 0.0 || b[1] >= ch {
                is.add(Sev::Warn, Some(s.t), format!("{} is outside the frame at {}s", label(i, &p), t2(s.t)), None, true);
                continue;
            }
            let out = b[0] < sx0 - 1.0 || b[1] < sy0 - 1.0 || b[2] > sx1 + 1.0 || b[3] > sy1 + 1.0;
            if out {
                let fits = b[2] - b[0] <= sx1 - sx0 && b[3] - b[1] <= sy1 - sy0;
                let dx = if b[0] < sx0 { sx0 - b[0] } else if b[2] > sx1 { sx1 - b[2] } else { 0.0 };
                let dy = if b[1] < sy0 { sy0 - b[1] } else if b[3] > sy1 { sy1 - b[3] } else { 0.0 };
                let movable = fits && !animated(i, "pos") && i.parent.is_none();
                // a layer cut off at the frame edge is wider than it measures: move, re-measure, repeat
                let fix = movable
                    .then(|| {
                        let p0 = static_vec(i, "pos", (cw / 2.0, ch / 2.0));
                        let (mut px, mut py) = p0;
                        let (mut ddx, mut ddy) = (dx, dy);
                        for _ in 0..6 {
                            px = (px + ddx).round();
                            py = (py + ddy).round();
                            let mut q = p.clone();
                            if let Some(it) = q.comps.iter_mut().find(|x| x.id == c.id).and_then(|cc| cc.tracks.iter_mut().flat_map(|t| t.items.iter_mut()).find(|x| x.id == i.id)) {
                                it.props.insert("pos".into(), crate::model::anim::Anim::new(Val::V(px, py)));
                            }
                            let cc = q.comp(&c.id).unwrap().clone();
                            let it = cc.items().find(|x| x.id == i.id).unwrap().clone();
                            let Some(nb) = self.measure_overlay(&q, &cc, &it, false).bbox else { return None };
                            let clipped = nb[0] <= 1.0 || nb[1] <= 1.0 || nb[2] >= cw - 1.0 || nb[3] >= ch - 1.0;
                            if !clipped {
                                // true size known: smallest move from the original position (layers translate rigidly)
                                if nb[2] - nb[0] > sx1 - sx0 - 4.0 || nb[3] - nb[1] > sy1 - sy0 - 4.0 {
                                    return None;
                                }
                                let (ox, oy) = (px - p0.0, py - p0.1);
                                let b0 = [nb[0] - ox, nb[1] - oy, nb[2] - ox, nb[3] - oy];
                                let mx = if b0[0] < sx0 + 2.0 { sx0 + 2.0 - b0[0] } else if b0[2] > sx1 - 2.0 { sx1 - 2.0 - b0[2] } else { 0.0 };
                                let my = if b0[1] < sy0 + 2.0 { sy0 + 2.0 - b0[1] } else if b0[3] > sy1 - 2.0 { sy1 - 2.0 - b0[3] } else { 0.0 };
                                return Some(format!("set {} pos={},{}", i.id, fmt_num((p0.0 + mx).round()), fmt_num((p0.1 + my).round())));
                            }
                            // an edge on the frame border is a cut-off, not the real extent: overshoot by half the box
                            let (hw, hh) = ((nb[2] - nb[0]) * 0.5, (nb[3] - nb[1]) * 0.5);
                            ddx = if nb[0] < sx0 + 2.0 { sx0 + 2.0 - nb[0] + if nb[0] <= 1.0 { hw } else { 0.0 } } else if nb[2] > sx1 - 2.0 { sx1 - 2.0 - nb[2] - if nb[2] >= cw - 1.0 { hw } else { 0.0 } } else { 0.0 };
                            ddy = if nb[1] < sy0 + 2.0 { sy0 + 2.0 - nb[1] + if nb[1] <= 1.0 { hh } else { 0.0 } } else if nb[3] > sy1 - 2.0 { sy1 - 2.0 - nb[3] - if nb[3] >= ch - 1.0 { hh } else { 0.0 } } else { 0.0 };
                            if ddx.abs() < 1.0 && ddy.abs() < 1.0 {
                                return Some(format!("set {} pos={},{}", i.id, fmt_num(px), fmt_num(py)));
                            }
                            if nb[2] - nb[0] > sx1 - sx0 || nb[3] - nb[1] > sy1 - sy0 {
                                return None; // wider than the safe area once fully measured
                            }
                        }
                        None
                    })
                    .flatten();
                let edges: Vec<&str> = [(b[0] < sx0 - 1.0, "left"), (b[2] > sx1 + 1.0, "right"), (b[1] < sy0 - 1.0, "top"), (b[3] > sy1 + 1.0, "bottom")].iter().filter(|x| x.0).map(|x| x.1).collect();
                is.add(
                    Sev::Warn,
                    Some(s.t),
                    format!("{} crosses the title-safe area ({} edge; box {:.0},{:.0}-{:.0},{:.0}){}", label(i, &p), edges.join("+"), b[0], b[1], b[2], b[3], if fits { "" } else { " and is too big to fit: scale it down" }),
                    fix,
                    true,
                );
            }
            if i.kind == ItemKind::Text {
                let size = static_num(i, "size", 96.0) * max_scale(i);
                if size < ch * 0.022 && !cap_ids.contains(i.id.as_str()) {
                    is.add(Sev::Warn, Some(s.t), format!("{} is small ({}px at {}p): hard to read on phones", label(i, &p), fmt_num(size.round()), c.h.min(c.w)), Some(format!("set {} size={}", i.id, fmt_num((ch * 0.03).round()))), true);
                }
                let text = i.text.clone().unwrap_or_default();
                let words = text.split_whitespace().count();
                if cap_ids.contains(i.id.as_str()) {
                    let cps = text.chars().filter(|c| !c.is_whitespace()).count() as f64 / i.dur.max(0.01);
                    if cps > 22.0 {
                        is.add(Sev::Info, Some(s.t), format!("caption {} needs {} chars/s (comfortable: ≤ 20)", i.id, fmt_num(cps.round())), None, false);
                    }
                } else if words > 2 && i.dur < (words as f64 / 3.0).max(1.0) * 0.8 {
                    is.add(Sev::Warn, Some(s.t), format!("{} shows {} words for {}s (~{}s needed to read)", label(i, &p), words, t2(i.dur), fmt_num((words as f64 / 3.0).max(1.0).ceil())), Some(format!("trim {} dur={}", i.id, fmt_num((words as f64 / 3.0).max(1.0).ceil()))), false);
                }
                if let Some((lt, lb)) = s.lum {
                    let r = contrast(lt, lb);
                    if r < 2.5 {
                        is.add(
                            Sev::Warn,
                            Some(s.t),
                            format!("{} has low contrast against the picture ({}:1)", label(i, &p), fmt_num((r * 10.0).round() / 10.0)),
                            Some(format!("set {} shadow=#000000b3 shadowBlur=18 shadowY=4", i.id)),
                            true,
                        );
                    }
                }
            }
        }
        for (k, (a, ba)) in boxes.iter().enumerate() {
            for (b, bb) in boxes.iter().skip(k + 1) {
                let (t0, t1) = (a.start.max(b.start), a.end().min(b.end()));
                if t1 - t0 < 0.2 || cap_ids.contains(a.id.as_str()) && cap_ids.contains(b.id.as_str()) {
                    continue;
                }
                let iw = (ba[2].min(bb[2]) - ba[0].max(bb[0])).max(0.0);
                let ih = (ba[3].min(bb[3]) - ba[1].max(bb[1])).max(0.0);
                let small = ((ba[2] - ba[0]) * (ba[3] - ba[1])).min((bb[2] - bb[0]) * (bb[3] - bb[1])).max(1.0);
                if iw * ih > small * 0.25 {
                    is.add(Sev::Warn, Some((t0 + t1) / 2.0), format!("{} and {} overlap on screen at {}-{}s", label(a, &p), label(b, &p), t2(t0), t2(t1)), None, true);
                }
            }
        }

        // ---------------- audio
        let audible = |i: &Item, tk: &crate::model::project::Track| -> bool {
            !i.off && !tk.muted && i.kind == ItemKind::Clip && !static_str(i, "mute").is_some_and(|m| m == "true" || m == "1") && i.src.as_deref().and_then(|s| p.media(s)).is_some_and(|m| m.has_audio || m.kind == MediaKind::Audio)
        };
        let mut aud_end: f64 = 0.0;
        let mut last_audio: Option<(Item, f64)> = None;
        for tk in &c.tracks {
            for i in tk.items.iter().filter(|i| audible(i, tk)) {
                if i.end() > aud_end {
                    aud_end = i.end();
                    last_audio = Some((i.clone(), static_num(i, "fadeOut", 0.0)));
                }
            }
        }
        if !vis.is_empty() && aud_end > vis_end + 2.0 * fd {
            let fixes: Vec<String> = c.tracks.iter().flat_map(|tk| tk.items.iter().filter(|i| audible(i, tk) && i.end() > vis_end + fd).map(|i| format!("trim {} end={}\nfade {} out=1 audio", i.id, fmt_num((vis_end * 1000.0).round() / 1000.0), i.id)).collect::<Vec<_>>()).collect();
            is.add(Sev::Warn, Some(vis_end + (aud_end - vis_end) / 2.0), format!("audio continues {}s after the last picture (black screen at the end)", t2(aud_end - vis_end)), (!fixes.is_empty()).then(|| fixes.join("\n")), true);
        } else if let Some((i, fo)) = &last_audio {
            if fo < &0.05 && (i.end() - dur).abs() < fd * 2.0 && i.tout.is_none() && i.dur > 3.0 {
                is.add(Sev::Info, None, format!("{} ends the mix without a fade (abrupt cut-off)", label(i, &p)), Some(format!("fade {} out=1 audio", i.id)), false);
            }
        }
        // music under speech without ducking
        let pr = &p;
        let speech: Vec<(String, f64, f64)> = c
            .tracks
            .iter()
            .flat_map(|tk| tk.items.iter().filter(|i| audible(i, tk)).filter_map(move |i| i.src.as_deref().and_then(|s| pr.media(s)).map(|m| (tk.id.clone(), i.clone(), m.clone()))))
            .filter(|(_, _, m)| (m.kind == MediaKind::Video && m.has_audio) || self.cached_analysis(m, "transcript").is_some())
            .map(|(t, i, _)| (t, i.start, i.end()))
            .collect();
        let mut duck_hint: BTreeMap<String, String> = BTreeMap::new();
        for tk in c.tracks.iter().filter(|t| t.kind == TrackKind::Audio && !t.muted && t.duck.is_none()) {
            for i in tk.items.iter().filter(|i| audible(i, tk)) {
                let Some(m) = i.src.as_deref().and_then(|s| p.media(s)) else { continue };
                let is_music = m.kind == MediaKind::Audio && self.cached_analysis(m, "transcript").is_none() && (m.dur > 15.0 || m.tags.iter().any(|t| t.eq_ignore_ascii_case("music")));
                if !is_music || static_num(i, "vol", 0.0) <= -14.0 {
                    continue;
                }
                // total speech under this music clip, per source track
                let mut per: BTreeMap<&str, f64> = BTreeMap::new();
                for (t, a, b) in speech.iter().filter(|(t, ..)| t != &tk.id) {
                    *per.entry(t.as_str()).or_default() += (b.min(i.end()) - a.max(i.start)).max(0.0);
                }
                if let Some((vt, _)) = per.iter().filter(|(_, d)| **d > 3.0).max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)) {
                    duck_hint.entry(tk.id.clone()).or_insert_with(|| vt.to_string());
                }
            }
        }
        for (mt, vt) in &duck_hint {
            is.add(Sev::Warn, None, format!("music on {mt} plays under dialogue on {vt} at full level"), Some(format!("duck {mt} by={vt} amount=-12")), false);
        }
        let (want_lufs, tol) = match target.as_str() {
            "broadcast" | "tv" | "ebu" => (-23.0, 1.0),
            "podcast" | "voice" => (-16.0, 1.5),
            "cinema" | "film" => (-27.0, 3.0),
            _ => (o.expect.lufs.unwrap_or(c.master.lufs.filter(|l| *l < 0.0).unwrap_or(-14.0)), 2.0),
        };
        let mut loud = None;
        if o.audio {
            match self.measure_mix(&p, &c.id) {
                Ok(Some((i, tp, lra))) => {
                    loud = Some((i, tp, lra));
                    if i > -69.0 && (i - want_lufs).abs() > tol {
                        let set = c.master.lufs.is_some_and(|l| (l - want_lufs).abs() < 0.05);
                        let msg = format!("mix loudness {} LUFS; target {} ({target}) ±{}", fmt_num((i * 10.0).round() / 10.0), fmt_num(want_lufs), fmt_num(tol));
                        if set {
                            // already normalized: the limiter is eating the gain (very peaky mix)
                            is.add(Sev::Warn, None, format!("{msg}: peaks limit the gain; lower the loudest track or compress it (afx A1 compress)"), None, false);
                        } else {
                            is.add(Sev::Warn, None, msg, Some(format!("master lufs={} limiter=1", fmt_num(want_lufs))), false);
                        }
                    }
                    if tp > -1.0 {
                        is.add(Sev::Warn, None, format!("true peak {} dBTP (> -1): may clip after encoding", fmt_num((tp * 10.0).round() / 10.0)), Some("master limiter=1".into()), false);
                    }
                }
                Ok(None) => {
                    if o.expect.music != Some(false) {
                        is.add(Sev::Info, None, "the edit has no audio", None, false);
                    }
                }
                Err(e) => is.add(Sev::Info, None, format!("couldn't measure loudness: {}", e.to_string().lines().next().unwrap_or("")), None, false),
            }
        }

        // ---------------- expectations
        let e = &o.expect;
        let mut exp = vec![];
        if let Some(d) = e.dur {
            let tol = e.dur_tol.unwrap_or((d * 0.03).max(1.0));
            let ok = (dur - d).abs() <= tol;
            exp.push(format!("dur {}s {}", fmt_num(d), if ok { "✓".to_string() } else { format!("✗ ({}s)", t2(dur)) }));
            if !ok {
                is.add(Sev::Error, None, format!("duration is {}s but {}s (±{}s) was asked for", t2(dur), fmt_num(d), t2(tol)), None, false);
            }
        }
        if let Some((w, h)) = e.size {
            let ok = (c.w, c.h) == (w, h);
            exp.push(format!("size {w}x{h} {}", if ok { "✓".to_string() } else { format!("✗ ({}x{})", c.w, c.h) }));
            if !ok {
                is.add(Sev::Error, None, format!("comp is {}x{} but {w}x{h} was asked for (reframe {w}x{h} or comp set)", c.w, c.h), None, false);
            }
        }
        if let Some(a) = e.aspect {
            let ok = (cw / ch - a).abs() < 0.01;
            exp.push(format!("aspect {} {}", fmt_num((a * 1000.0).round() / 1000.0), if ok { "✓" } else { "✗" }));
            if !ok {
                is.add(Sev::Error, None, format!("aspect is {}x{} ({}), not {}", c.w, c.h, fmt_num((cw / ch * 1000.0).round() / 1000.0), fmt_num(a)), None, false);
            }
        }
        if let Some(f) = e.fps {
            let ok = (c.fps - f).abs() < 0.01;
            exp.push(format!("fps {} {}", fmt_num(f), if ok { "✓".to_string() } else { format!("✗ ({})", fmt_num(c.fps)) }));
            if !ok {
                is.add(Sev::Error, None, format!("comp is {} fps, not {}", fmt_num(c.fps), fmt_num(f)), Some(format!("comp set {} fps={}", c.id, fmt_num(f))), false);
            }
        }
        if let Some(want) = e.captions {
            let has = !captions.is_empty();
            exp.push(format!("captions {}", if has == want { "✓" } else { "✗" }));
            if has != want {
                is.add(Sev::Error, None, if want { "captions were asked for but there are none (captions <clip>)" } else { "there are captions but none were wanted" }, None, false);
            }
        }
        if let Some(want) = e.music {
            let has = c.tracks.iter().any(|tk| tk.items.iter().any(|i| audible(i, tk) && i.src.as_deref().and_then(|s| p.media(s)).is_some_and(|m| m.kind == MediaKind::Audio)));
            exp.push(format!("music {}", if has == want { "✓" } else { "✗" }));
            if has != want {
                is.add(Sev::Error, None, if want { "music was asked for but no audio-only track plays" } else { "music plays but none was wanted" }, None, false);
            }
        }
        if let (Some(l), Some((i, ..))) = (e.lufs, loud) {
            exp.push(format!("lufs {} {}", fmt_num(l), if (i - l).abs() <= 1.5 { "✓".to_string() } else { format!("✗ ({})", fmt_num((i * 10.0).round() / 10.0)) }));
        }
        // shots on the lowest video track with clips
        let base_track = c.vtracks().find(|t| !t.hidden && t.items.iter().any(|i| i.kind == ItemKind::Clip || i.kind == ItemKind::Comp));
        let shots_len: Vec<f64> = base_track.map(|t| t.items.iter().filter(|i| !i.off).map(|i| i.dur).collect()).unwrap_or_default();
        if let Some(mx) = e.max_shot {
            let long: Vec<String> = base_track.map(|t| t.items.iter().filter(|i| i.dur > mx + fd).map(|i| format!("{} {}s", i.id, t2(i.dur))).collect()).unwrap_or_default();
            exp.push(format!("max shot {}s {}", fmt_num(mx), if long.is_empty() { "✓".to_string() } else { format!("✗ ({})", long.len()) }));
            if !long.is_empty() {
                is.add(Sev::Warn, None, format!("shots longer than {}s: {}", fmt_num(mx), long.iter().take(8).cloned().collect::<Vec<_>>().join(", ")), None, false);
            }
        }
        if let Some(mn) = e.min_shot {
            let short: Vec<String> = base_track.map(|t| t.items.iter().filter(|i| i.dur + fd < mn).map(|i| format!("{} {}s", i.id, t2(i.dur))).collect()).unwrap_or_default();
            exp.push(format!("min shot {}s {}", fmt_num(mn), if short.is_empty() { "✓".to_string() } else { format!("✗ ({})", short.len()) }));
            if !short.is_empty() {
                is.add(Sev::Warn, None, format!("shots shorter than {}s: {}", fmt_num(mn), short.iter().take(8).cloned().collect::<Vec<_>>().join(", ")), None, false);
            }
        }

        // ---------------- report
        is.0.sort_by(|a, b| a.sev.cmp(&b.sev).then(a.t.unwrap_or(f64::MAX).partial_cmp(&b.t.unwrap_or(f64::MAX)).unwrap_or(std::cmp::Ordering::Equal)));
        let count = |s: Sev| is.0.iter().filter(|i| i.sev == s).count();
        let (ne, nw, ni) = (count(Sev::Error), count(Sev::Warn), count(Sev::Info));
        let mut out = format!(
            "check {} '{}' {}x{}@{} {}s: {}",
            c.id,
            c.name,
            c.w,
            c.h,
            fmt_num(c.fps),
            t2(dur),
            if ne + nw + ni == 0 { "no problems found".to_string() } else { format!("{ne} error(s), {nw} warning(s), {ni} note(s)") }
        );
        let n_text = overlays.iter().filter(|i| i.kind == ItemKind::Text).count() - captions.len();
        let n_gfx = overlays.len() - overlays.iter().filter(|i| i.kind == ItemKind::Text).count();
        let mut summary = vec![];
        if !shots_len.is_empty() {
            let avg = shots_len.iter().sum::<f64>() / shots_len.len() as f64;
            summary.push(format!("{} shots (avg {}s, {} cuts/min)", shots_len.len(), t2(avg), fmt_num(((shots_len.len().saturating_sub(1)) as f64 / (dur / 60.0)).round())));
        }
        if n_text > 0 {
            summary.push(format!("{n_text} text"));
        }
        if !captions.is_empty() {
            summary.push(format!("{} captions", captions.len()));
        }
        if n_gfx > 0 {
            summary.push(format!("{n_gfx} graphics/overlays"));
        }
        if let Some((i, tp, _)) = loud {
            summary.push(format!("mix {} LUFS / {} dBTP", fmt_num((i * 10.0).round() / 10.0), fmt_num((tp * 10.0).round() / 10.0)));
        }
        if !summary.is_empty() {
            let _ = write!(out, "\n{}", summary.join(" · "));
        }
        let (mut ce, mut cwn, mut ci) = (0, 0, 0);
        let mut codes = vec![];
        for x in &is.0 {
            let code = match x.sev {
                Sev::Error => {
                    ce += 1;
                    format!("E{ce}")
                }
                Sev::Warn => {
                    cwn += 1;
                    format!("W{cwn}")
                }
                Sev::Info => {
                    ci += 1;
                    format!("N{ci}")
                }
            };
            let _ = write!(out, "\n{code} {}{}", x.t.map(|t| format!("@{}s ", t2(t))).unwrap_or_default(), x.msg);
            if let Some(f) = &x.fix {
                let _ = write!(out, "\n   fix: {}", f.replace('\n', " ; "));
            }
            codes.push(code);
        }
        if !exp.is_empty() {
            let _ = write!(out, "\nexpected: {}", exp.join(" · "));
        }
        let fixes: Vec<String> = is.0.iter().filter_map(|x| x.fix.clone()).collect();
        if o.fix && !fixes.is_empty() {
            match self.edit(&fixes.join("\n"), source) {
                Ok(r) => {
                    let _ = write!(out, "\napplied {} fix line(s) as rev {} (undo reverts them); run check again to confirm", fixes.len(), r.rev);
                }
                Err(e) => {
                    let _ = write!(out, "\n! fixes not applied: {}", e.to_string().lines().next().unwrap_or(""));
                }
            }
        } else if !fixes.is_empty() {
            let _ = write!(out, "\n{} fix line(s) available: check fix=true applies them in one undoable edit, or edit them yourself", fixes.len());
        }

        // frames of the visual problems, with title-safe guides
        let mut sheet = None;
        if o.frames {
            let mut picks: Vec<(String, f64)> = vec![];
            for (x, code) in is.0.iter().zip(codes.iter()) {
                if let (true, Some(t)) = (x.visual, x.t) {
                    let t = t.clamp(0.0, (dur - fd).max(0.0));
                    if !picks.iter().any(|(_, pt)| (pt - t).abs() < fd) {
                        picks.push((code.clone(), t));
                    }
                }
                if picks.len() >= 8 {
                    break;
                }
            }
            if !picks.is_empty() {
                let scale = 320.0 / c.w as f32;
                let rc = RenderCtx { p: &p, frames: &self.frames, draft: true };
                let tiles: Vec<Tile> = picks
                    .iter()
                    .filter_map(|(code, t)| {
                        let mut pm = render_frame(&rc, &c.id, *t, scale).ok()?;
                        draw_safe(&mut pm, margin as f32);
                        Some(Tile { label: format!("{code} {}s", t2(*t)), img: pm })
                    })
                    .collect();
                if !tiles.is_empty() {
                    let cw_t = 320;
                    let chh = (320.0 * ch / cw).round() as u32;
                    let cols = if c.h > c.w { tiles.len().min(6) } else { (tiles.len() as f64).sqrt().ceil() as usize };
                    let pm = encode::tile_sheet(&tiles, cw_t, chh, cols, false)?;
                    sheet = Some(encode::encode_jpeg(&pm, 78)?);
                }
            }
        }
        Ok(CheckOut { text: out, sheet })
    }
}

/// Thin yellow title-safe rectangle.
fn draw_safe(pm: &mut Pixmap, margin: f32) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let mut pb = PathBuilder::new();
    if let Some(r) = resvg::tiny_skia::Rect::from_ltrb(w * margin, h * margin, w * (1.0 - margin), h * (1.0 - margin)) {
        pb.push_rect(r);
    }
    if let Some(path) = pb.finish() {
        let mut paint = Paint { anti_alias: true, ..Default::default() };
        paint.set_color_rgba8(255, 214, 10, 200);
        pm.stroke_path(&path, &paint, &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compacts_id_lists() {
        let ids: Vec<String> = ["c5", "c6", "c7", "c9", "t2", "t3", "x"].iter().map(|s| s.to_string()).collect();
        assert_eq!(super::compact_ids(&ids), "c5..c7,c9,t2..t3,x");
    }
}
