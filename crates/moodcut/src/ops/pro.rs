//! Finishing techniques editors and motion designers reach for to make a video feel
//! produced rather than assembled: sound design on cuts and titles, speed ramps,
//! punch-ins on jump cuts, J / L split edits, and a brand kit so every title,
//! caption and graphic shares one design.

use super::exec::{same_path, Exec, Mode, A, EPS};
use crate::model::anim::{Anim, Ease, Key};
use crate::model::project::*;
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Color, Val};
use crate::sfx;
use anyhow::{anyhow, bail, Result};

/// Brand presets: cohesive palettes so titles don't fall back to generic defaults.
pub const BRAND_PRESETS: &[(&str, &str, [&str; 6])] = &[
    // name, use, [font, weight, primary, accent, text, dark]
    ("clean", "corporate, tutorials, SaaS", ["Inter", "semibold", "#111827", "#2563eb", "#ffffff", "#0b0f19"]),
    ("bold", "creator / Reels energy, Hormozi-style captions", ["Inter", "black", "#000000", "#ffd60a", "#ffffff", "#000000"]),
    ("luxury", "fashion, real estate, premium products", ["Inter", "light", "#111111", "#c9a96e", "#f5f1e8", "#0d0d0d"]),
    ("tech", "launches, AI, gaming", ["Inter", "bold", "#7c3aed", "#00e5ff", "#ffffff", "#05060a"]),
    ("playful", "lifestyle, kids, food", ["Inter", "extrabold", "#ffc93c", "#ff5c8a", "#ffffff", "#2b1d3a"]),
    ("warm", "travel, vlogs, documentary", ["Inter", "bold", "#2b2118", "#ff7a45", "#fff7ed", "#1c140f"]),
    ("mono", "editorial, minimal, film", ["Inter", "medium", "#ffffff", "#ffffff", "#ffffff", "#0a0a0a"]),
];

impl<'a> Exec<'a> {
    // ------------------------------------------------------------ sound design

    /// `sfx <kind> at=T[,T2..] | on=cuts|transitions|titles|markers[:prefix]|<ids> [track=V1] [dur=] [vol=] [pitch=] [same]`
    pub fn c_sfx(&mut self, a: &mut A) -> Result<()> {
        let names = sfx::KINDS.iter().map(|k| k.name).collect::<Vec<_>>().join(" ");
        let usage = format!("sfx <{names}> at=T[,T..] | on=cuts|transitions|titles|markers|beats|<ids> [dur=] [vol=-10dB] [pitch=1] [track=V1] [same]");
        let name = a.shift().ok_or_else(|| anyhow!(usage.clone()))?;
        let k = sfx::kind(&name).ok_or_else(|| {
            let hint = crate::model::schema::suggest(sfx::KINDS.iter().map(|k| k.name), &name).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
            anyhow!("unknown sound '{name}'.{hint} ({names})")
        })?;
        let dur = a.take(&["dur", "len", "length"]).map(|s| self.time(&s)).transpose()?.unwrap_or(k.dur).clamp(0.03, 12.0);
        let pitch = match a.take(&["pitch"]) {
            Some(s) => super::tools::parse_factor(&s)?,
            None => 1.0,
        };
        let vol = a.take(&["vol", "gain", "volume"]).unwrap_or_else(|| format!("{}dB", fmt_num(k.vol)));
        let same = a.flag("same");
        let base_track = a.take(&["track"]);
        let times = if let Some(at) = a.take(&["at", "t"]) {
            at.split(',').map(|t| self.time(t.trim())).collect::<Result<Vec<_>>>()?
        } else if let Some(on) = a.take(&["on", "every"]) {
            self.moments(&on, base_track.as_deref())?
        } else {
            bail!(usage)
        };
        let fd = 1.0 / self.fps();
        let mut times: Vec<f64> = times.into_iter().filter(|t| t.is_finite() && *t >= 0.0).collect();
        times.sort_by(|x, y| x.partial_cmp(y).unwrap());
        times.dedup_by(|x, y| (*x - *y).abs() < fd);
        if times.is_empty() {
            bail!("no moments found for sfx {}", k.name);
        }
        // variations continue after the ones already in the project, so repeats never sound copied
        let made = self.p.media.iter().filter(|m| m.tags.iter().any(|t| t == "sfx") && m.name.starts_with(k.name)).count() as u64;
        let sync = k.sync * dur;
        for (i, t) in times.iter().enumerate() {
            let seed = if same { 0 } else { made + i as u64 };
            let mid = self.sfx_media(k, dur, pitch, seed)?;
            // a riser that should end at 0.5 s starts before 0: cut its head instead of shifting it
            let start = (t - sync).max(0.0);
            let src_in = (sync - t).max(0.0);
            let len = dur - src_in;
            if len < 0.02 {
                continue;
            }
            let mut it = self.new_item(ItemKind::Clip, start, len);
            it.src = Some(mid);
            it.src_in = src_in;
            it.name = Some(k.name.to_string());
            let id = it.id.clone();
            let min = self.sfx_track_floor();
            let tid = self.pick_track(TrackKind::Audio, start, start + len, min);
            self.place(it, &tid, Mode::Strict)?;
            self.apply_props(&id, &[("vol".into(), vol.clone())])?;
            self.note(format!("+{id} sfx {} {tid} {}-{} (on {})", k.name, fmt_t(start), fmt_t(start + len), fmt_t(*t)));
            self.created.push(id);
        }
        Ok(())
    }

    /// Imports (once) the rendered sound file for these parameters.
    fn sfx_media(&mut self, k: &sfx::Kind, dur: f64, pitch: f64, seed: u64) -> Result<String> {
        let mut m = self.env.sfx(k.name, dur, pitch, seed)?;
        if let Some(e) = self.p.media.iter().find(|x| same_path(&x.path, &m.path)) {
            return Ok(e.id.clone());
        }
        m.id = self.p.next_media_id();
        m.name = format!("{} {}", k.name, seed + 1);
        m.tags = vec!["sfx".into()];
        m.note = k.desc.into();
        let id = m.id.clone();
        self.p.media.push(m);
        Ok(id)
    }

    /// Sound effects go on audio tracks above dialogue and music.
    fn sfx_track_floor(&self) -> usize {
        let p = &*self.p;
        let is_sfx = |i: &Item| i.src.as_deref().and_then(|s| p.media(s)).is_some_and(|m| m.tags.iter().any(|t| t == "sfx"));
        self.comp().tracks.iter().filter(|t| t.kind == TrackKind::Audio && t.items.iter().any(|i| !is_sfx(i))).map(|t| t.index()).max().unwrap_or(0) + 1
    }

    /// Times for `sfx ... on=`: cuts, transitions, titles, markers[:prefix], beats, or item ids (their starts).
    pub(crate) fn moments(&self, on: &str, track: Option<&str>) -> Result<Vec<f64>> {
        let c = self.comp();
        let fd = 1.0 / self.fps();
        let (what, arg) = on.split_once(':').map(|(w, x)| (w, Some(x))).unwrap_or((on, None));
        let base = track.unwrap_or("V1");
        Ok(match what.to_ascii_lowercase().as_str() {
            "cuts" | "cut" | "edits" => {
                let tr = c.track(base).ok_or_else(|| anyhow!("no track {base}"))?;
                tr.items.windows(2).filter(|w| (w[1].start - w[0].end()).abs() < fd).map(|w| w[1].start).collect()
            }
            "transitions" | "transition" | "tr" => c
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Video)
                .flat_map(|t| t.items.iter().filter(|i| i.tout.is_some()).map(|i| i.end()))
                .collect(),
            "titles" | "text" | "graphics" | "titles+graphics" => {
                let bi = c.track(base).map(|t| t.index()).unwrap_or(1);
                let p = &*self.p;
                c.tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Video && t.index() > bi)
                    .flat_map(|t| t.items.iter())
                    .filter(|i| match i.kind {
                        ItemKind::Text | ItemKind::Shape | ItemKind::Comp => true,
                        ItemKind::Clip => i.src.as_deref().and_then(|s| p.media(s)).is_some_and(|m| m.gfx.is_some() || matches!(m.kind, MediaKind::Image | MediaKind::Svg)),
                        _ => false,
                    })
                    .map(|i| i.start)
                    .collect()
            }
            "markers" | "marker" => c.markers.iter().filter(|m| arg.is_none_or(|x| m.name.starts_with(x))).map(|m| m.t).collect(),
            "beats" | "beat" => c.markers.iter().filter(|m| m.name.len() > 1 && m.name.starts_with('b') && m.name[1..].chars().all(|ch| ch.is_ascii_digit())).map(|m| m.t).collect(),
            _ => {
                let mut v = vec![];
                for id in self.targets(on)? {
                    let l = self.relocate(&id)?;
                    v.push(self.item_at(l).start);
                }
                v
            }
        })
    }

    // ------------------------------------------------------------ speed ramp

    /// `speedramp <clip> at=T [to=4x] [dur=0.6] [ramp=0.2]`: from `at` for `dur` the clip plays at `to`
    /// (ramping in and out), the rest at its own speed. Uses time remapping, so the clip keeps its place
    /// and length and fast sections use more of the source.
    pub fn c_speedramp(&mut self, a: &mut A) -> Result<()> {
        let usage = "speedramp <clip|V1> at=T [to=4x|30%] [dur=0.6] [ramp=0.2]";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let at = self.time(&a.take(&["at", "t", "from"]).ok_or_else(|| anyhow!(usage))?)?;
        // a track or selector means: the clip playing there at `at`
        let id = self.one_at(&spec, at)?;
        let mut to = super::tools::parse_factor(&a.take(&["to", "speed"]).unwrap_or_else(|| "4x".into()))?.clamp(0.05, 20.0);
        let sec = a.take(&["dur", "for"]).map(|s| self.time(&s)).transpose()?.unwrap_or(0.6);
        let l = self.relocate(&id)?;
        let it = self.item_at(l).clone();
        let m = it.src.as_deref().and_then(|s| self.p.media(s)).cloned();
        let Some(m) = m.filter(|m| it.kind == ItemKind::Clip && m.kind == MediaKind::Video) else {
            bail!("{id}: speed ramps need a video clip");
        };
        if it.reversed() {
            bail!("{id} plays reversed; speed ramps need forward clips");
        }
        let ramp = a.take(&["ramp", "ease"]).map(|s| self.time(&s)).transpose()?.unwrap_or((sec * 0.35).min(0.25)).clamp(0.0, sec / 2.0);
        let (s0, s1) = (at - it.start, at - it.start + sec);
        if s0 < -EPS || s1 > it.dur + EPS || sec <= 0.0 {
            bail!("{id}: the ramp {}-{} must lie inside the clip ({}-{})", fmt_t(at), fmt_t(at + sec), fmt_t(it.start), fmt_t(it.end()));
        }
        let fps = self.fps();
        let step = 1.0 / (fps * 4.0);
        // the clip's current time mapping (an earlier ramp, or plain speed), extended past its end
        let old = it.props.get("remap").filter(|r| r.is_animated()).cloned();
        let sp = it.speed();
        let base = |x: f64| -> f64 {
            match &old {
                Some(r) => {
                    if x <= it.dur {
                        r.keyed(x.max(0.0)).num()
                    } else {
                        let e = r.keyed(it.dur).num();
                        e + (e - r.keyed(it.dur - 1.0 / fps).num()) * fps * (x - it.dur)
                    }
                }
                None => it.src_in + x * sp,
            }
        };
        let smooth = |u: f64| 0.5 - 0.5 * (std::f64::consts::PI * u.clamp(0.0, 1.0)).cos();
        let mult = |t: f64, to: f64| -> f64 {
            if t < s0 || t > s1 {
                1.0
            } else if ramp > 0.0 && t < s0 + ramp {
                1.0 + (to - 1.0) * smooth((t - s0) / ramp)
            } else if ramp > 0.0 && t > s1 - ramp {
                1.0 + (to - 1.0) * smooth((s1 - t) / ramp)
            } else {
                to
            }
        };
        // item time -> warped item time, integrated
        let warp = |to: f64| -> Vec<(f64, f64)> {
            let mut out = vec![(0.0, 0.0)];
            let (mut t, mut g) = (0.0, 0.0);
            while t < it.dur - 1e-9 {
                let h = step.min(it.dur - t);
                g += (mult(t, to) + 4.0 * mult(t + h / 2.0, to) + mult(t + h, to)) / 6.0 * h;
                t += h;
                out.push((t, g));
            }
            out
        };
        let limit = m.dur - 1.0 / m.fps.max(1.0);
        let mut w = warp(to);
        if to > 1.0 && base(w.last().unwrap().1) > limit {
            // not enough source after the out point: lower the peak speed until it fits
            let (mut lo, mut hi) = (1.0, to);
            for _ in 0..30 {
                let mid = (lo + hi) / 2.0;
                if base(warp(mid).last().unwrap().1) > limit {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            self.warn(format!("{id}: only {}s of source after the out point, ramp peaks at {}x instead of {}x (trim the clip or use a lower speed)", fmt_num(((m.dur - base(it.dur)) * 100.0).round() / 100.0), fmt_num((lo * 100.0).floor() / 100.0), fmt_num(to)));
            to = lo;
            w = warp(to);
        }
        // keys: every 2 frames where the speed changes (or always, over an earlier ramp), the ends otherwise
        let every = 2.0 / fps;
        let mut keys: Vec<Key> = vec![];
        let mut last_t = -1.0;
        for (i, (t, g)) in w.iter().enumerate() {
            let edge = i == 0 || i + 1 == w.len() || [s0, s0 + ramp, s1 - ramp, s1].iter().any(|e| (t - e).abs() < step / 2.0);
            let changing = old.is_some() || (*t > s0 && *t < s0 + ramp) || (*t > s1 - ramp && *t < s1);
            if edge || (changing && t - last_t >= every - 1e-9) {
                keys.push(Key { t: *t, v: Val::N(base(*g)), e: Ease::Linear });
                last_t = *t;
            }
        }
        let mut anim = Anim::new(Val::N(-1.0));
        for k in keys {
            anim.add_key(k);
        }
        let n = anim.k.len();
        let itm = self.item_mut_at(l);
        itm.props.insert("remap".into(), anim);
        if to < 1.0 && itm.props.get("interp").is_none_or(|v| v.v.str() == "none") {
            itm.props.insert("interp".into(), Anim::new(Val::S("blend".into())));
        }
        let muted = m.has_audio && !itm.props.get("mute").is_some_and(|v| v.v.bool());
        if muted {
            itm.props.insert("mute".into(), Anim::new(Val::B(true)));
        }
        self.note(format!(
            "~{id} speed ramp {}x {}-{} (ramps {}s, {n} remap keys){}",
            fmt_num((to * 100.0).round() / 100.0),
            fmt_t(at),
            fmt_t(at + sec),
            fmt_num(ramp),
            if muted { ", clip audio muted" } else { "" }
        ));
        Ok(())
    }

    /// The single item `spec` names, or among several (a track, a selector) the one under time `t`.
    fn one_at(&self, spec: &str, t: f64) -> Result<String> {
        let ids = self.targets(spec)?;
        if ids.len() == 1 {
            return Ok(ids[0].clone());
        }
        ids.into_iter()
            .find(|id| self.relocate(id).is_ok_and(|l| { let i = self.item_at(l); i.start <= t + EPS && t < i.end() - EPS }))
            .ok_or_else(|| anyhow!("nothing in {spec} at {}", fmt_t(t)))
    }

    // ------------------------------------------------------------ punch-ins

    /// `punchin <clips|V1> [scale=1.12] [every=2] [focus=x,y]`: alternating punch-ins hide jump cuts
    /// and give talking heads a second camera angle.
    pub fn c_punchin(&mut self, a: &mut A) -> Result<()> {
        let usage = "punchin <clips|V1> [scale=1.12] [every=2] [focus=x,y]";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        let s = super::tools::parse_factor(&a.take(&["scale", "zoom", "amount"]).unwrap_or_else(|| "1.12".into()))?;
        let every: usize = a.take(&["every", "n"]).map(|x| x.parse()).transpose()?.unwrap_or(2).max(1);
        let cx = self.vctx();
        let focus = match a.take(&["focus", "on", "at"]) {
            Some(f) => match crate::model::schema::parse_val(crate::model::schema::find(crate::model::schema::LAYER, "pos").unwrap(), &f, cx)? {
                Val::V(x, y) => (x, y),
                _ => bail!("focus=x,y"),
            },
            None => (cx.cw / 2.0, cx.ch * 0.42),
        };
        let mut ids: Vec<(f64, String)> = vec![];
        for id in self.targets(&spec)? {
            let l = self.relocate(&id)?;
            let it = self.item_at(l);
            if matches!(it.kind, ItemKind::Clip | ItemKind::Comp | ItemKind::Solid) {
                ids.push((it.start, id));
            }
        }
        ids.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        if ids.is_empty() {
            bail!("punchin: no clips in {spec}");
        }
        let (mut punched, mut skipped) = (vec![], vec![]);
        for (i, (_, id)) in ids.iter().enumerate() {
            let l = self.relocate(id)?;
            let it = self.item_at(l).clone();
            if ["pos", "scale"].iter().any(|p| it.props.get(*p).is_some_and(|a| a.is_animated())) {
                skipped.push(id.clone());
                continue;
            }
            let (px, py) = it.props.get("pos").map(|a| a.v.vec()).unwrap_or((cx.cw / 2.0, cx.ch / 2.0));
            let s0 = it.props.get("scale").map(|a| a.v.vec().0).unwrap_or(1.0);
            // the one in `every` clips gets the punch; the rest go back to the wide framing
            let target = if i % every == every - 1 { s } else { 1.0 };
            // keep the focus point where it is on screen while scaling around the anchor
            let (dx, dy) = ((focus.0 - px) / s0, (focus.1 - py) / s0);
            let np = if (target - 1.0).abs() < 1e-9 { (cx.cw / 2.0, cx.ch / 2.0) } else { (focus.0 - dx * target, focus.1 - dy * target) };
            let itm = self.item_mut_at(l);
            itm.props.insert("scale".into(), Anim::new(Val::V(target, target)));
            itm.props.insert("pos".into(), Anim::new(Val::V((np.0 * 10.0).round() / 10.0, (np.1 * 10.0).round() / 10.0)));
            if target != 1.0 {
                punched.push(id.clone());
            }
        }
        self.note(format!("~punch-in {}% on {} of {} clips: {}", fmt_num((s * 100.0).round()), punched.len(), ids.len(), punched.join(",")));
        if !skipped.is_empty() {
            self.warn(format!("{} keep their animated framing", skipped.join(",")));
        }
        Ok(())
    }

    // ------------------------------------------------------------ J / L cuts

    /// `jcut <clip> [by=0.5]` (its audio starts before its picture) and `lcut <clip> [by=0.5]`
    /// (its audio runs past the cut). The clip's audio is detached first if needed.
    pub fn c_splitedit(&mut self, a: &mut A, j: bool) -> Result<()> {
        let verb = if j { "jcut" } else { "lcut" };
        let spec = a.shift().ok_or_else(|| anyhow!("{verb} <clip> [by=0.5]"))?;
        let ids = self.targets(&spec)?;
        let [id] = ids.as_slice() else { bail!("{verb}: {spec} is {} clips; name one", ids.len()) };
        let id = id.clone();
        let by = a.take(&["by", "dur", "amount"]).or_else(|| a.shift()).map(|s| self.time(&s)).transpose()?.unwrap_or(0.5);
        if by <= 0.0 {
            bail!("{verb}: by= must be positive");
        }
        let aid = match self.linked_audio(&id)? {
            Some(x) => x,
            None => {
                let mut sub = A::from_pos(vec![id.clone()]);
                self.c_detach(&mut sub)?;
                self.created.last().cloned().ok_or_else(|| anyhow!("{id}: no audio to split"))?
            }
        };
        let l = self.relocate(&aid)?;
        let au = self.item_at(l).clone();
        let m = au.src.as_deref().and_then(|s| self.p.media(s)).cloned().ok_or_else(|| anyhow!("{aid} has no media"))?;
        let sp = au.speed();
        let (start, dur, src_in) = if j {
            if au.src_in < by * sp - EPS {
                bail!("{id}: only {}s of audio before its in point; a J-cut of {}s needs more handle", fmt_num(au.src_in / sp), fmt_num(by));
            }
            if au.start < by - EPS {
                bail!("{id} starts at {}: nothing before it to overlap", fmt_t(au.start));
            }
            (au.start - by, au.dur + by, au.src_in - by * sp)
        } else {
            if au.src_in + (au.dur + by) * sp > m.dur + EPS {
                bail!("{id}: only {}s of audio after its out point; an L-cut of {}s needs more handle", fmt_num((m.dur - au.src_in - au.dur * sp) / sp), fmt_num(by));
            }
            (au.start, au.dur + by, au.src_in)
        };
        let mut moved = self.take_item(&aid)?;
        moved.start = start;
        moved.dur = dur;
        moved.src_in = src_in;
        // a short fade where the two sounds overlap
        let fade = (by * 0.6).clamp(0.1, 0.5);
        moved.props.insert(if j { "fadeIn" } else { "fadeOut" }.into(), Anim::new(Val::N(fmt_num(fade).parse().unwrap())));
        let tid = self.pick_track(TrackKind::Audio, start, start + dur, 1);
        self.place(moved, &tid, Mode::Strict)?;
        self.note(format!("~{verb} {id}: audio {aid} {tid} {}-{} ({}s {})", fmt_t(start), fmt_t(start + dur), fmt_num(by), if j { "early" } else { "late" }));
        Ok(())
    }

    /// The detached audio clip that belongs to a video clip, if any.
    fn linked_audio(&self, id: &str) -> Result<Option<String>> {
        let l = self.relocate(id)?;
        let v = self.item_at(l);
        if v.kind == ItemKind::Clip && self.p.media(v.src.as_deref().unwrap_or("")).is_some_and(|m| m.kind == MediaKind::Audio) {
            return Ok(Some(id.to_string()));
        }
        // in sync = the same source time at the same timeline time (also after a J/L cut moved its ends)
        let fd = 1.0 / self.fps();
        let offset = |i: &Item| i.src_in - i.start * i.speed();
        Ok(self
            .comp()
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .flat_map(|t| t.items.iter())
            .find(|i| i.src == v.src && (i.speed() - v.speed()).abs() < 1e-9 && (offset(i) - offset(v)).abs() < fd && i.start < v.end() && i.end() > v.start)
            .map(|i| i.id.clone()))
    }

    // ------------------------------------------------------------ brand kit

    /// `brand [preset] [font=] [weight=] [primary=] [accent=] [text=] [dark=] | brand clear | brand`
    pub fn c_brand(&mut self, a: &mut A) -> Result<()> {
        let mut b = self.p.brand.clone();
        if let Some(first) = a.shift() {
            let f = first.to_ascii_lowercase();
            if matches!(f.as_str(), "clear" | "none" | "reset") {
                self.p.brand = Brand::default();
                self.note("~brand cleared");
                return Ok(());
            }
            let (_, _, v) = BRAND_PRESETS.iter().find(|p| p.0 == f).ok_or_else(|| anyhow!("unknown brand preset '{first}' ({})", BRAND_PRESETS.iter().map(|p| p.0).collect::<Vec<_>>().join(" ")))?;
            b = Brand { font: Some(v[0].into()), weight: Some(v[1].into()), primary: Some(v[2].into()), accent: Some(v[3].into()), text: Some(v[4].into()), dark: Some(v[5].into()) };
        }
        for (k, v) in a.rest() {
            let val = if matches!(v.as_str(), "-" | "none" | "") { None } else { Some(v.clone()) };
            match k.to_ascii_lowercase().as_str() {
                "font" | "family" => b.font = val,
                "weight" => b.weight = val,
                key @ ("primary" | "accent" | "text" | "dark" | "color" | "highlight" | "bg") => {
                    if let Some(c) = &val {
                        Color::parse(c)?;
                    }
                    match key {
                        "primary" | "color" => b.primary = val,
                        "accent" | "highlight" => b.accent = val,
                        "text" => b.text = val,
                        _ => b.dark = val,
                    }
                }
                other => bail!("unknown brand field '{other}' (font weight primary accent text dark)"),
            }
        }
        self.p.brand = b;
        let s = brand_line(&self.p.brand);
        self.note(format!("~brand {s}"));
        Ok(())
    }

    /// Brand defaults for a text layer: font, weight and color unless given.
    pub(crate) fn brand_text_props(&self, props: &mut Vec<(String, String)>, title: bool) {
        let b = &self.p.brand;
        let has = |props: &Vec<(String, String)>, k: &str| props.iter().any(|(x, _)| x.eq_ignore_ascii_case(k));
        if let Some(f) = &b.font {
            if !has(props, "font") {
                props.push(("font".into(), f.clone()));
            }
        }
        if let (Some(w), true) = (&b.weight, title) {
            if !has(props, "weight") {
                props.push(("weight".into(), w.clone()));
            }
        }
        if let Some(c) = &b.text {
            if !has(props, "color") {
                props.push(("color".into(), c.clone()));
            }
        }
    }
}

pub fn brand_line(b: &Brand) -> String {
    let f = |k: &str, v: &Option<String>| v.as_ref().map(|v| format!("{k}={}", if v.contains(' ') { format!("'{v}'") } else { v.clone() }));
    [f("font", &b.font), f("weight", &b.weight), f("primary", &b.primary), f("accent", &b.accent), f("text", &b.text), f("dark", &b.dark)].into_iter().flatten().collect::<Vec<_>>().join(" ")
}
