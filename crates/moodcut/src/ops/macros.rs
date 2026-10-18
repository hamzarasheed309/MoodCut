//! High-level editing macros: one command that replaces dozens of manual edits.
//! precomp, autocut (silence / scenes), captions, montage (beat-synced),
//! beats (markers), reframe (aspect conversion), keep (rough cut from ranges).

use super::exec::{parse_size, parse_track_spec, Exec, Mode, A, EPS};
use crate::model::anim::Anim;
use crate::model::project::*;
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Val};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

impl<'a> Exec<'a> {
    /// Runs nested script lines in a given comp, optionally hiding their log lines.
    pub fn run_sub(&mut self, comp: &str, script: &str, quiet: bool) -> Result<Vec<String>> {
        let saved_comp = std::mem::replace(&mut self.comp, comp.to_string());
        let log_len = self.log.len();
        let created_len = self.created.len();
        let res = (|| -> Result<()> {
            for c in super::lex::parse_script(script)? {
                self.cmd(&c).map_err(|e| anyhow!("{} (in `{}`)", e, c.text))?;
            }
            Ok(())
        })();
        self.comp = saved_comp;
        let new_ids = self.created[created_len..].to_vec();
        if quiet {
            self.log.truncate(log_len);
        }
        res?;
        Ok(new_ids)
    }

    fn analysis_opts(a: &mut A, keys: &[&str]) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for k in keys {
            if let Some(v) = a.take(&[k]) {
                m.insert(k.to_string(), v);
            }
        }
        m
    }

    // ------------------------------------------------------------ precomp

    pub fn c_precomp(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("precomp <ids> [name=]"))?;
        let name = a.take(&["name"]);
        let alias = a.take(&["as"]);
        let ids = self.targets(&spec)?;
        let ci = self.ci();
        let mut t0 = f64::INFINITY;
        let mut t1: f64 = 0.0;
        let mut lowest: Option<(TrackKind, usize)> = None;
        for id in &ids {
            let (c, t, _) = self.relocate(id)?;
            if c != ci {
                bail!("{id} isn't in the active comp");
            }
            let tr = &self.p.comps[c].tracks[t];
            let it = self.p.item(id).unwrap();
            t0 = t0.min(it.start);
            t1 = t1.max(it.end());
            if tr.kind == TrackKind::Video {
                let idx = tr.index();
                if lowest.map(|(_, i)| idx < i).unwrap_or(true) {
                    lowest = Some((TrackKind::Video, idx));
                }
            }
        }
        let (w, h, fps) = (self.comp().w, self.comp().h, self.comp().fps);
        let cid = self.p.next_comp_id();
        let cname = name.unwrap_or_else(|| format!("Precomp {}", self.p.comps.len()));
        let mut nc = Comp::new(&cid, &cname, w, h, fps);
        nc.tracks.clear();
        nc.bg = crate::model::value::Color::TRANSPARENT;
        let id_set: std::collections::HashSet<String> = ids.iter().cloned().collect();
        for id in &ids {
            let (c, t, _) = self.relocate(id)?;
            let tid = self.p.comps[c].tracks[t].id.clone();
            let kind = self.p.comps[c].tracks[t].kind;
            let mut it = self.remove_item(id)?;
            it.start -= t0;
            if it.parent.as_ref().map(|p| !id_set.contains(p)).unwrap_or(false) {
                it.parent = None;
            }
            if it.matte.as_ref().map(|m| !id_set.contains(&m.src)).unwrap_or(false) {
                it.matte = None;
            }
            let idx = tid[1..].parse().unwrap_or(1);
            let tid2 = nc.ensure_track(kind, idx);
            let tr = nc.track_mut(&tid2).unwrap();
            tr.items.push(it);
            tr.sort();
        }
        if nc.track_count(TrackKind::Audio) == 0 {
            nc.ensure_track(TrackKind::Audio, 1);
        }
        nc.dur = Some(t1 - t0);
        self.p.comps.push(nc);
        let mut it = self.new_item(ItemKind::Comp, t0, t1 - t0);
        it.src = Some(cid.clone());
        it.name = alias;
        let nid = it.id.clone();
        let tid = format!("V{}", lowest.map(|(_, i)| i).unwrap_or(1));
        let tid = if self.comp().track(&tid).map(|t| t.is_free(t0, t1, None)).unwrap_or(true) { tid } else { self.pick_track(TrackKind::Video, t0, t1, 1) };
        self.place(it, &tid, Mode::Strict)?;
        self.note(format!("+{cid} '{cname}' with {} item(s); +{nid} nested on {tid} {}-{}", ids.len(), fmt_t(t0), fmt_t(t1)));
        self.created.push(nid);
        Ok(())
    }

    // ------------------------------------------------------------ autocut

    pub fn c_autocut(&mut self, a: &mut A) -> Result<()> {
        let mode = a.shift().ok_or_else(|| anyhow!("autocut silence <clip> [thresh=-35] [min=0.5] [pad=0.12] [sync] | autocut scenes <clip> [thresh=0.3]"))?.to_ascii_lowercase();
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("autocut needs a clip id"))?)?;
        let l = self.relocate(&id)?;
        let it = self.item_at(l).clone();
        if it.kind != ItemKind::Clip {
            bail!("{id} is not a media clip");
        }
        let mid = it.src.clone().unwrap();
        let sp = it.speed();
        let src_a = it.src_in;
        let src_b = it.src_in + it.dur * sp;
        let to_tl = |s: f64| it.start + (s - src_a) / sp;
        match mode.as_str() {
            "silence" | "silences" | "pauses" | "jumpcut" => {
                let sync = a.flag("sync") || a.flag("all");
                let pad: f64 = a.take(&["pad", "padding"]).map(|s| s.parse()).transpose()?.unwrap_or(0.12);
                let min_cut: f64 = a.take(&["mincut"]).map(|s| s.parse()).transpose()?.unwrap_or(0.2);
                let opts = Self::analysis_opts(a, &["thresh", "min"]);
                let res = self.env.analyze(self.p, &mid, "silence", &opts)?;
                let ranges: Vec<(f64, f64)> = res["silences"].as_array().map(|v| v.iter().filter_map(|r| Some((r[0].as_f64()?, r[1].as_f64()?))).collect()).unwrap_or_default();
                // silent ranges inside the clip window, padded
                let mut cuts: Vec<(f64, f64)> = ranges
                    .into_iter()
                    .filter_map(|(s, e)| {
                        let s = s.max(src_a) + if s <= src_a + EPS { 0.0 } else { pad };
                        let e = e.min(src_b) - if e >= src_b - EPS { 0.0 } else { pad };
                        if e - s >= min_cut {
                            Some((to_tl(s), to_tl(e)))
                        } else {
                            None
                        }
                    })
                    .collect();
                if cuts.is_empty() {
                    self.note(format!("~{id} no silences found (try thresh=-30 or min=0.3)"));
                    return Ok(());
                }
                cuts.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
                let (ci, ti, _) = l;
                let tid = self.p.comps[ci].tracks[ti].id.clone();
                let mut removed = 0.0;
                let n = cuts.len();
                for (s, e) in cuts {
                    // find the piece containing s..e (pieces change as we cut from the end)
                    let piece = self.p.comps[ci]
                        .track(&tid)
                        .unwrap()
                        .items
                        .iter()
                        .find(|i| i.start <= s + EPS && i.end() >= e - EPS && i.src.as_deref() == Some(mid.as_str()))
                        .map(|i| i.id.clone());
                    let Some(pid) = piece else { continue };
                    let (ps, pe) = self.item_span(&pid).unwrap();
                    let fps = self.p.comps[ci].fps;
                    let mut mid_piece = pid.clone();
                    if s > ps + 0.5 / fps {
                        mid_piece = self.split_item(&pid, s)?;
                    }
                    if e < pe - 0.5 / fps {
                        self.split_item(&mid_piece, e)?;
                    }
                    let gone = self.remove_item(&mid_piece)?;
                    self.ripple(ci, &tid, gone.end(), -gone.dur, sync);
                    removed += gone.dur;
                }
                self.note(format!("~{id} autocut removed {n} silence(s), {}s shorter", fmt_t(removed)));
            }
            "scenes" | "scene" | "shots" => {
                let opts = Self::analysis_opts(a, &["thresh"]);
                let res = self.env.analyze(self.p, &mid, "scenes", &opts)?;
                let cuts: Vec<f64> = res["cuts"].as_array().map(|v| v.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
                let mut n = 0;
                let mut times: Vec<f64> = cuts.into_iter().filter(|c| *c > src_a + 0.1 && *c < src_b - 0.1).map(to_tl).collect();
                times.sort_by(|x, y| y.partial_cmp(x).unwrap());
                for t in times {
                    let piece = {
                        let (ci, ti, _) = self.relocate(&id)?;
                        self.p.comps[ci].tracks[ti].items.iter().find(|i| i.contains(t) && i.src.as_deref() == Some(mid.as_str())).map(|i| i.id.clone())
                    };
                    if let Some(pid) = piece {
                        if let Ok(nid) = self.split_item(&pid, t) {
                            self.created.push(nid);
                            n += 1;
                        }
                    }
                }
                self.note(format!("~{id} split into {} shot(s)", n + 1));
            }
            other => bail!("autocut {other}? use silence|scenes"),
        }
        Ok(())
    }

    // ------------------------------------------------------------ captions

    pub fn c_captions(&mut self, a: &mut A) -> Result<()> {
        let id = self.item_id(&a.shift().ok_or_else(|| anyhow!("captions <clip> [style=bold|box|minimal|neon|karaoke] [words=3] [pos=bottom] [size=] [hl=#ffd60a] [track=]"))?)?;
        let style = a.take(&["style"]).unwrap_or_else(|| "bold".into());
        let words_per: usize = a.take(&["words", "n"]).map(|s| s.parse()).transpose()?.unwrap_or(3).max(1);
        let pos = a.take(&["pos", "position"]);
        let size = a.take(&["size"]);
        let hl = a.take(&["hl", "highlight"]).unwrap_or_else(|| "#ffd60a".into());
        let track = a.take(&["track"]);
        let enter = a.take(&["enter", "anim"]).unwrap_or_else(|| "pop".into());
        let caps = a.take(&["caps"]);
        let opts = Self::analysis_opts(a, &["lang", "model"]);
        let l = self.relocate(&id)?;
        let it = self.item_at(l).clone();
        let mid = it.src.clone().ok_or_else(|| anyhow!("{id} has no media"))?;
        let res = self.env.analyze(self.p, &mid, "transcript", &opts)?;
        let sp = it.speed();
        let (src_a, src_b) = (it.src_in, it.src_in + it.dur * sp);
        let mut words: Vec<(String, f64, f64)> = vec![];
        for seg in res["segments"].as_array().cloned().unwrap_or_default() {
            let ws = seg["words"].as_array().cloned().unwrap_or_default();
            if ws.is_empty() {
                // segment-level only: spread words evenly
                let text = seg["text"].as_str().unwrap_or("").trim().to_string();
                let (s, e) = (seg["s"].as_f64().unwrap_or(0.0), seg["e"].as_f64().unwrap_or(0.0));
                let toks: Vec<&str> = text.split_whitespace().collect();
                let n = toks.len().max(1) as f64;
                for (i, w) in toks.iter().enumerate() {
                    let ws = s + (e - s) * i as f64 / n;
                    words.push((w.to_string(), ws, ws + (e - s) / n));
                }
            } else {
                for w in ws {
                    let t = w["w"].as_str().unwrap_or("").trim().to_string();
                    if t.is_empty() {
                        continue;
                    }
                    words.push((t, w["s"].as_f64().unwrap_or(0.0), w["e"].as_f64().unwrap_or(0.0)));
                }
            }
        }
        let words: Vec<(String, f64, f64)> = words
            .into_iter()
            .filter(|(_, s, e)| *e > src_a && *s < src_b)
            .map(|(w, s, e)| (w, it.start + (s.max(src_a) - src_a) / sp, it.start + (e.min(src_b) - src_a) / sp))
            .collect();
        if words.is_empty() {
            bail!("no transcribed words inside {id}'s range");
        }
        // group into phrases: N words, break on punctuation or long pauses
        let mut phrases: Vec<Vec<(String, f64, f64)>> = vec![];
        let mut cur: Vec<(String, f64, f64)> = vec![];
        for (i, w) in words.iter().enumerate() {
            let gap = if i > 0 { w.1 - words[i - 1].2 } else { 0.0 };
            if !cur.is_empty() && (cur.len() >= words_per || gap > 0.6) {
                phrases.push(std::mem::take(&mut cur));
            }
            let ends = w.0.ends_with(['.', '!', '?', ',', ';', ':']);
            cur.push(w.clone());
            if ends && cur.len() >= 2 {
                phrases.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            phrases.push(cur);
        }
        let (cw, ch) = (self.comp().w as f64, self.comp().h as f64);
        let vertical = ch > cw;
        let default_pos = if vertical { format!("{},{}", fmt_num(cw / 2.0), fmt_num(ch * 0.68)) } else { format!("{},{}", fmt_num(cw / 2.0), fmt_num(ch * 0.84)) };
        let pos = pos.unwrap_or(default_pos);
        let base_size = size.unwrap_or_else(|| fmt_num(if vertical { cw * 0.075 } else { ch * 0.065 }));
        let tid = match track {
            Some(t) => t,
            None => {
                let s = phrases.first().unwrap()[0].1;
                let e = phrases.last().unwrap().last().unwrap().2;
                self.pick_track_above(TrackKind::Video, s, e)
            }
        };
        let style_props: Vec<(String, String)> = match style.to_ascii_lowercase().as_str() {
            "bold" | "tiktok" | "karaoke" => vec![("weight".into(), "black".into()), ("stroke".into(), "#000000".into()), ("strokeW".into(), fmt_num(base_size.parse::<f64>().unwrap_or(72.0) * 0.11)), ("caps".into(), caps.clone().unwrap_or_else(|| "1".into()))],
            "box" | "boxed" => vec![("weight".into(), "bold".into()), ("bg".into(), "#000000b3".into()), ("bgMode".into(), "line".into())],
            "minimal" | "clean" => vec![("weight".into(), "medium".into()), ("shadow".into(), "#000000aa".into())],
            "neon" => vec![("weight".into(), "bold".into()), ("shadow".into(), hl.clone()), ("shadowY".into(), "0".into()), ("shadowBlur".into(), "24".into())],
            other => bail!("unknown caption style '{other}' (bold box minimal neon karaoke)"),
        };
        let karaoke = !matches!(style.to_ascii_lowercase().as_str(), "minimal" | "clean");
        let n = phrases.len();
        let ci = self.ci();
        for (pi, ph) in phrases.iter().enumerate() {
            let s = ph[0].1;
            let next_start = phrases.get(pi + 1).map(|p| p[0].1);
            let mut e = ph.last().unwrap().2 + 0.15;
            if let Some(ns) = next_start {
                e = e.min(ns).max(s + 0.25);
                if ns - e < 0.25 {
                    e = ns; // avoid flicker between close phrases
                }
            }
            let dur = (e - s).max(0.25);
            let text: String = ph.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ");
            let mut item = self.new_item(ItemKind::Text, s, dur);
            item.text = Some(text);
            let iid = item.id.clone();
            self.place(item, &tid, Mode::Overwrite)?;
            let mut props = vec![("size".to_string(), base_size.clone()), ("pos".to_string(), pos.clone()), ("boxW".to_string(), fmt_num(cw * 0.86))];
            props.extend(style_props.iter().cloned());
            self.apply_props(&iid, &props)?;
            if karaoke {
                let times: Vec<String> = ph.iter().map(|w| fmt_num(((w.1 - s) * 1000.0).round() / 1000.0)).collect();
                let l = self.relocate(&iid)?;
                let itm = self.item_mut_at(l);
                let xid = itm.next_sub("x");
                let mut p = Props::new();
                p.insert("unit".into(), Anim::new(Val::S("word".into())));
                p.insert("times".into(), Anim::new(Val::S(times.join(","))));
                p.insert("dur".into(), Anim::new(Val::N(0.06)));
                p.insert("color".into(), Anim::new(Val::C(crate::model::value::Color::parse(&hl)?)));
                p.insert("scale".into(), Anim::new(Val::N(1.06)));
                itm.anims.push(Effect { id: xid, kind: "hl".into(), on: true, props: p });
            }
            if enter != "none" {
                let o = super::presets::AnimOpts { dur: Some(0.18), ..Default::default() };
                if super::presets::is_layer_anim(&enter) {
                    self.add_anim_preset(&iid, &enter, &o, 0)?;
                } else if enter == "pop" {
                    self.add_anim_preset(&iid, "popIn", &super::presets::AnimOpts { dur: Some(0.22), ..Default::default() }, 0)?;
                } else {
                    self.add_anim_preset(&iid, &enter, &o, ph.len() * 5)?;
                }
            }
            self.created.push(iid);
        }
        // name the track so tools (and people) can tell captions from titles
        if let Some(t) = self.p.comps[ci].track_mut(&tid) {
            if t.name.is_none() {
                t.name = Some("Captions".into());
            }
        }
        self.note(format!("+{n} caption item(s) on {tid} ({style}, {words_per} words)"));
        Ok(())
    }

    // ------------------------------------------------------------ beats & montage

    fn music_window(&mut self, key: &str) -> Result<(String, f64, f64, f64, f64)> {
        // returns (media id, timeline start, timeline end, src_in, speed)
        if let Ok(id) = self.item_id(key) {
            let l = self.relocate(&id)?;
            let it = self.item_at(l).clone();
            let m = it.src.clone().ok_or_else(|| anyhow!("{id} has no media"))?;
            return Ok((m, it.start, it.end(), it.src_in, it.speed()));
        }
        let mid = self.media_or_import(key)?;
        let d = self.p.media(&mid).unwrap().dur;
        Ok((mid, 0.0, d, 0.0, 1.0))
    }

    fn beat_times(&mut self, key: &str, every: usize) -> Result<(Vec<f64>, f64, String, f64, f64)> {
        let (mid, s, e, src_in, sp) = self.music_window(key)?;
        let res = self.env.analyze(self.p, &mid, "beats", &BTreeMap::new())?;
        let bpm = res["bpm"].as_f64().unwrap_or(0.0);
        let beats: Vec<f64> = res["beats"].as_array().map(|v| v.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
        let tl: Vec<f64> = beats.into_iter().map(|b| s + (b - src_in) / sp).filter(|t| *t >= s - EPS && *t <= e + EPS).collect();
        let every = every.max(1);
        let picked: Vec<f64> = tl.iter().enumerate().filter(|(i, _)| i % every == 0).map(|(_, t)| *t).collect();
        Ok((picked, bpm, mid, s, e))
    }

    pub fn c_beats(&mut self, a: &mut A) -> Result<()> {
        let key = a.shift().ok_or_else(|| anyhow!("beats <music clip|media> [every=1]"))?;
        let every: usize = a.take(&["every", "n"]).map(|s| s.parse()).transpose()?.unwrap_or(1);
        let (beats, bpm, _, _, _) = self.beat_times(&key, every)?;
        let ci = self.ci();
        for (i, b) in beats.iter().enumerate() {
            self.p.comps[ci].markers.push(Marker { t: *b, name: format!("b{}", i + 1), color: None });
        }
        self.p.comps[ci].markers.sort_by(|x, y| x.t.partial_cmp(&y.t).unwrap());
        self.note(format!("+{} beat marker(s) @{} BPM (named b1, b2 ...; use @b4 as a time)", beats.len(), fmt_num(bpm.round())));
        Ok(())
    }

    pub fn c_montage(&mut self, a: &mut A) -> Result<()> {
        let list = a.shift().ok_or_else(|| anyhow!("montage <m1,m2,...> music=<clip|media> [beats=2] [at=0] [track=V1] [dur=] [kenburns]"))?;
        let music = a.take(&["music", "audio", "song"]).ok_or_else(|| anyhow!("montage needs music=<clip or media>"))?;
        let every: usize = a.take(&["beats", "every"]).map(|s| s.parse()).transpose()?.unwrap_or(2);
        let track = a.take(&["track"]).unwrap_or_else(|| "V1".into());
        let kb = a.flag("kenburns");
        let tr = a.take(&["tr", "transition"]);
        let limit = a.take(&["dur", "duration"]).map(|s| self.time(&s)).transpose()?;
        // place music if it's a media id
        let at = a.take(&["at"]);
        let placed_music = self.item_id(&music).is_err();
        let music_item = if !placed_music {
            music.clone()
        } else {
            let ids = self.run_sub(&self.comp.clone(), &format!("add {music} A1 at={}", at.as_deref().unwrap_or("0")), false)?;
            ids[0].clone()
        };
        let (beats, bpm, _, mstart, me) = self.beat_times(&music_item, every)?;
        // with an existing music clip, at= starts the montage partway into it
        let ms = match at.as_deref().filter(|_| !placed_music) {
            Some(t) => self.time(t)?.max(mstart),
            None => mstart,
        };
        if ms >= me - 0.1 {
            bail!("montage starts at {}s but the music ends at {}s", fmt_num(ms), fmt_num(me));
        }
        let mut cuts: Vec<f64> = beats.into_iter().filter(|b| *b >= ms - EPS).collect();
        // the first shot starts with the music, not on its first detected beat
        match cuts.first().copied() {
            Some(c) if c <= ms + 0.25 => cuts[0] = ms,
            _ => cuts.insert(0, ms),
        }
        let end = limit.map(|d| ms + d).unwrap_or(me).min(me);
        if placed_music && end < me - 0.05 {
            // music we added ends with the montage, faded out
            let fade = (end - ms).min(1.0) * 0.999;
            self.run_sub(&self.comp.clone(), &format!("trim {music_item} end={}\nset {music_item} fadeOut={}", fmt_num(end), fmt_num(fade)), true)?;
        }
        cuts.retain(|c| *c < end - 0.05);
        cuts.push(end);
        let srcs: Vec<String> = list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if srcs.is_empty() {
            bail!("montage needs at least one media");
        }
        let mut uses: BTreeMap<String, f64> = BTreeMap::new();
        let mut placed = 0;
        let ci = self.ci();
        let (kind, idx) = parse_track_spec(&track).ok_or_else(|| anyhow!("bad track {track}"))?;
        let tid = self.p.comps[ci].ensure_track(kind, idx);
        for w in cuts.windows(2) {
            let (s, e) = (w[0], w[1]);
            if e - s < 0.1 {
                continue;
            }
            let key = &srcs[placed % srcs.len()];
            let mid = self.media_or_import(key)?;
            let m = self.p.media(&mid).unwrap().clone();
            let need = e - s;
            let src_in = if m.is_still() {
                0.0
            } else {
                let used = uses.entry(mid.clone()).or_insert(if m.dur > need * 3.0 { m.dur * 0.15 } else { 0.0 });
                let mut si = *used;
                if si + need > m.dur {
                    si = 0.0;
                }
                *used = si + need;
                si
            };
            let mut it = self.new_item(ItemKind::Clip, s, need.min(if m.is_still() { need } else { m.dur - src_in }));
            it.src = Some(mid.clone());
            it.src_in = src_in;
            let iid = it.id.clone();
            self.place(it, &tid, Mode::Overwrite)?;
            if kb && m.is_still() {
                self.add_anim_preset(&iid, "kenburns", &super::presets::AnimOpts::default(), 0)?;
            }
            self.created.push(iid);
            placed += 1;
        }
        if let Some(t) = tr {
            self.run_sub(&self.comp.clone(), &format!("tr {tid} {t} 0.3"), true)?;
        }
        let trimmed = if placed_music && end < me - 0.05 { format!(", {music_item} trimmed to end at {} with a fade", fmt_num(end)) } else { String::new() };
        self.note(format!("+montage {placed} clip(s) on {tid} cut every {every} beat(s) @{} BPM{trimmed}", fmt_num(bpm.round())));
        Ok(())
    }

    // ------------------------------------------------------------ reframe

    pub fn c_reframe(&mut self, a: &mut A) -> Result<()> {
        let size = a.shift().ok_or_else(|| anyhow!("reframe <9:16|1:1|4:5|WxH> [src=comp] [mode=blur|fill|fit] [name=] [x=offset] [main]"))?;
        let (w, h) = parse_size(&size).ok_or_else(|| anyhow!("bad size '{size}' (9:16, 1:1, 4:5, 1080x1920 ...)"))?;
        let src = a.take(&["src", "comp", "from"]).unwrap_or_else(|| self.comp.clone());
        let src_id = self.p.comp(&src).ok_or_else(|| anyhow!("unknown comp '{src}'"))?.id.clone();
        let mode = a.take(&["mode"]).unwrap_or_else(|| "blur".into()).to_ascii_lowercase();
        let make_main = a.flag("main");
        let xoff: f64 = a.take(&["x", "offset"]).map(|s| s.parse()).transpose()?.unwrap_or(0.0);
        let name = a.take(&["name"]).unwrap_or_else(|| format!("{} {}x{}", self.p.comp(&src_id).unwrap().name, w, h));
        let (fps, dur) = {
            let c = self.p.comp(&src_id).unwrap();
            (c.fps, c.duration())
        };
        if dur <= 0.0 {
            bail!("source comp is empty");
        }
        let script = format!("comp new '{name}' {w}x{h} {fps}");
        self.run_sub(&self.comp.clone(), &script, true)?;
        let new_id = self.p.comps.last().unwrap().id.clone();
        let cx = w as f64 / 2.0 + xoff;
        let cy = h as f64 / 2.0;
        let body = match mode.as_str() {
            "fill" | "crop" => format!("add {src_id} V1 at=0 fit=fill pos={},{}", fmt_num(cx), fmt_num(cy)),
            "fit" | "letterbox" => format!("add {src_id} V1 at=0 fit=fit"),
            "blur" => format!(
                "add {src_id} V1 at=0 fit=fill\nfx $ blur radius=60\nfx $ bc brightness=-25\nset $ mute=1\nadd {src_id} V2 at=0 fit=fit pos={},{}",
                fmt_num(cx),
                fmt_num(cy)
            ),
            o => bail!("reframe mode must be blur|fill|fit, not {o}"),
        };
        self.run_sub(&new_id, &body, true)?;
        self.comp = new_id.clone();
        if make_main {
            self.p.main = new_id.clone();
        }
        self.note(format!("+{new_id} '{name}' {w}x{h} reframed from {src_id} ({mode}){} (active)", if make_main { " main" } else { "" }));
        self.created.push(new_id);
        Ok(())
    }

    // ------------------------------------------------------------ rough cut from ranges

    pub fn c_keep(&mut self, a: &mut A) -> Result<()> {
        let key = a.shift().ok_or_else(|| anyhow!("keep <media> <in-out,in-out,...> [track=V1] [at=end]"))?;
        let ranges = a.take(&["ranges", "r"]).or_else(|| a.shift()).ok_or_else(|| anyhow!("keep needs ranges like 0-12.5,15-30"))?;
        let track = a.take(&["track"]);
        let at = a.take(&["at"]);
        let mid = self.media_or_import(&key)?;
        let mut parts = vec![];
        for r in ranges.split(',') {
            let (x, y) = r.split_once('-').ok_or_else(|| anyhow!("bad range '{r}'"))?;
            parts.push(format!("{mid}[{}-{}]", x.trim(), y.trim()));
        }
        let mut script = format!("append {}", parts.join(" "));
        if let Some(t) = track {
            script.push_str(&format!(" track={t}"));
        }
        if let Some(t) = at {
            script.push_str(&format!(" at={t}"));
        }
        let ids = self.run_sub(&self.comp.clone(), &script, true)?;
        let total: f64 = ids.iter().filter_map(|i| self.item_span(i)).map(|(s, e)| e - s).sum();
        self.note(format!("+{} clip(s) from {mid}: {} ({}s total)", ids.len(), ids.join(","), fmt_t(total)));
        Ok(())
    }
}
