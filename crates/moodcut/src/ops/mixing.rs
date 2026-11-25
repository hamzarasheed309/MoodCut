//! Mixing verbs: buses and sends, track/bus automation, effect racks, and the
//! measured audio tools (`repair`, `roomtone`, `match ... eq|loudness`).
//!
//! ```text
//! bus add B1 name=Dialogue | bus set B1 vol=-3 pan=0 out=B3 mute=1 | bus del B1
//! track set A1 out=B1          route a track to a bus
//! send A1 B2 -12dB [pre]       a send (shared reverb, parallel compression); send A1 B2 off
//! kf A1 vol 0:-6 4:0           track / bus automation (comp seconds), also pan
//! afx A1 rack=podcast-voice    expands to ordinary effects
//! repair c1,c2                 measures, then adds only what is needed
//! roomtone A1                  fills gaps with tone from the quietest stretch
//! match c2 to=c1 eq|loudness   tonal and level matching of dialogue
//! ```

use super::exec::{Exec, A};
use crate::model::anim::{Anim, Ease, Key};
use crate::model::project::*;
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Val};
use crate::render::audio_scope::{Stats, BANDS};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

/// Effect racks: (name, use, script lines with `{t}` for the target).
pub const RACKS: &[(&str, &str, &[&str])] = &[
    ("podcast-voice", "spoken voice, close mic", &["afx {t} highpass freq=80", "afx {t} deess", "afx {t} compress threshold=-20 ratio=3", "afx {t} peq bands='200,-2,1,bell 3500,2,1,bell 12000,1.5,0.7,highshelf'", "afx {t} limiter"]),
    ("voiceover", "studio narration", &["afx {t} highpass freq=70", "afx {t} compress threshold=-18 ratio=2.5", "afx {t} peq bands='150,1,0.8,lowshelf 4000,1.5,1,bell'", "afx {t} deess"]),
    ("phone", "phone call / radio", &["afx {t} telephone", "afx {t} compress threshold=-22 ratio=4"]),
    ("music-bed", "music under dialogue", &["afx {t} peq bands='300,-2,0.8,bell 2500,-3,1,bell'", "afx {t} widen amount=1.2"]),
    ("glue", "bus glue compression", &["afx {t} compress threshold=-14 ratio=2 attack=20 release=200"]),
    ("room-verb", "a shared reverb bus (send to it)", &["afx {t} reverb type=room amount=1 mix=1"]),
    ("hall-verb", "a big shared reverb", &["afx {t} reverb type=hall amount=1 mix=1"]),
    ("master", "a gentle master chain", &["afx {t} multiband", "afx {t} limiter truepeak=1"]),
];

fn db(s: &str) -> Result<f64> {
    s.trim().trim_end_matches("dB").trim_end_matches("db").parse::<f64>().map_err(|_| anyhow!("'{s}' isn't a level in dB"))
}

impl<'a> Exec<'a> {
    fn bus_exists(&self, id: &str) -> bool {
        self.comp().buses.iter().any(|b| b.id.eq_ignore_ascii_case(id))
    }

    /// Where a node feeds (main out and sends), for cycle checks.
    fn feeds_of(&self, id: &str) -> Vec<String> {
        let c = self.comp();
        let (out, sends) = if let Some(b) = c.buses.iter().find(|b| b.id.eq_ignore_ascii_case(id)) {
            (b.out.clone(), b.sends.clone())
        } else if let Some(t) = c.track(id) {
            (t.out.clone(), t.sends.clone())
        } else {
            (None, vec![])
        };
        out.into_iter().chain(sends.into_iter().map(|s| s.to)).collect()
    }

    /// True when routing `from` into bus `to` would make a loop.
    pub(crate) fn makes_loop(&self, from: &str, to: &str) -> bool {
        let mut stack = vec![to.to_string()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(n) = stack.pop() {
            if n.eq_ignore_ascii_case(from) {
                return true;
            }
            if seen.insert(n.to_ascii_uppercase()) {
                stack.extend(self.feeds_of(&n));
            }
        }
        false
    }

    /// Validates an output/send target for `from`.
    pub(crate) fn check_route(&self, from: &str, to: &str) -> Result<String> {
        let b = self.comp().buses.iter().find(|b| b.id.eq_ignore_ascii_case(to)).ok_or_else(|| anyhow!("no bus {to} (bus add {to})"))?;
        if self.makes_loop(from, &b.id) {
            bail!("routing {from} to {} would make a feedback loop", b.id);
        }
        Ok(b.id.clone())
    }

    /// `bus add|set|del|list`
    pub fn c_bus(&mut self, a: &mut A) -> Result<()> {
        let usage = "bus add [B1] [name=Dialogue] | bus set B1 vol=-3 pan=0 out=B2|master mute=0 | bus del B1 | bus list";
        let sub = a.shift().ok_or_else(|| anyhow!(usage))?.to_ascii_lowercase();
        let ci = self.ci();
        match sub.as_str() {
            "add" | "new" => {
                let id = match a.shift() {
                    Some(id) => {
                        let up = id.to_ascii_uppercase();
                        if !up.starts_with('B') || up[1..].parse::<u32>().is_err() {
                            bail!("bus ids are B1, B2, ... (not {id})");
                        }
                        up
                    }
                    None => (1..).map(|i| format!("B{i}")).find(|i| !self.bus_exists(i)).unwrap(),
                };
                if self.bus_exists(&id) {
                    bail!("{id} already exists");
                }
                let name = a.take(&["name"]);
                self.p.comps[ci].buses.push(Bus { id: id.clone(), name: name.clone(), ..Default::default() });
                self.note(format!("+{id} bus{}", name.map(|n| format!(" {n}")).unwrap_or_default()));
                let rest = a.rest();
                if !rest.is_empty() {
                    self.bus_set(&id, &rest)?;
                }
            }
            "set" => {
                let id = a.shift().ok_or_else(|| anyhow!(usage))?;
                let kvs = a.rest();
                self.bus_set(&id, &kvs)?;
            }
            "del" | "delete" | "rm" => {
                let id = a.shift().ok_or_else(|| anyhow!(usage))?;
                let c = &mut self.p.comps[ci];
                let before = c.buses.len();
                c.buses.retain(|b| !b.id.eq_ignore_ascii_case(&id));
                if c.buses.len() == before {
                    bail!("no bus {id}");
                }
                // whatever fed it goes to the master again
                for t in c.tracks.iter_mut() {
                    if t.out.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(&id)) {
                        t.out = None;
                    }
                    t.sends.retain(|s| !s.to.eq_ignore_ascii_case(&id));
                }
                for b in c.buses.iter_mut() {
                    if b.out.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(&id)) {
                        b.out = None;
                    }
                    b.sends.retain(|s| !s.to.eq_ignore_ascii_case(&id));
                }
                self.note(format!("-{id}"));
            }
            "list" | "ls" => {
                let lines = crate::mcp::docs::mix_view(self.p, &self.comp.clone(), None);
                self.note(lines);
            }
            o => bail!("bus {o}? {usage}"),
        }
        Ok(())
    }

    fn bus_set(&mut self, id: &str, kvs: &[(String, String)]) -> Result<()> {
        let ci = self.ci();
        let bid = self.comp().buses.iter().find(|b| b.id.eq_ignore_ascii_case(id)).map(|b| b.id.clone()).ok_or_else(|| anyhow!("no bus {id}"))?;
        for (k, v) in kvs {
            let lk = k.to_ascii_lowercase();
            if matches!(lk.as_str(), "out" | "output" | "to") {
                let out = if matches!(v.to_ascii_lowercase().as_str(), "master" | "none" | "main") { None } else { Some(self.check_route(&bid, v)?) };
                self.p.comps[ci].buses.iter_mut().find(|b| b.id == bid).unwrap().out = out;
                continue;
            }
            let b = self.p.comps[ci].buses.iter_mut().find(|b| b.id == bid).unwrap();
            match lk.as_str() {
                "vol" | "volume" | "gain" => b.vol = db(v)?.clamp(-96.0, 24.0),
                "pan" => b.pan = v.parse::<f64>()?.clamp(-1.0, 1.0),
                "name" => b.name = Some(v.clone()),
                "mute" | "muted" => b.muted = !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"),
                other => bail!("unknown bus setting '{other}' (vol pan out name mute)"),
            }
        }
        self.note(format!("~{bid} {}", kvs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")));
        Ok(())
    }

    /// `send <track|bus> <bus> <dB> [pre]` | `send <from> <bus> off`
    pub fn c_send(&mut self, a: &mut A) -> Result<()> {
        let usage = "send <A1|B1> <bus> <-12dB> [pre] | send <A1> <bus> off";
        let from = a.shift().ok_or_else(|| anyhow!(usage))?;
        let to = a.shift().ok_or_else(|| anyhow!(usage))?;
        let pre = a.flag("pre");
        let lvl = a.take(&["db", "level", "amount"]).or_else(|| a.shift()).unwrap_or_else(|| "-12".into());
        let ci = self.ci();
        let is_bus = self.bus_exists(&from);
        if !is_bus && self.comp().track(&from).is_none() {
            bail!("no track or bus {from}");
        }
        let off = matches!(lvl.to_ascii_lowercase().as_str(), "off" | "none" | "del" | "rm");
        let tid = if off { to.to_ascii_uppercase() } else { self.check_route(&from, &to)? };
        let s = AuxSend { to: tid.clone(), db: if off { 0.0 } else { db(&lvl)?.clamp(-96.0, 12.0) }, pre };
        let c = &mut self.p.comps[ci];
        let list = if is_bus { &mut c.buses.iter_mut().find(|b| b.id.eq_ignore_ascii_case(&from)).unwrap().sends } else { &mut c.track_mut(&from).unwrap().sends };
        list.retain(|x| !x.to.eq_ignore_ascii_case(&tid));
        if off {
            self.note(format!("~{from} send {tid} off"));
        } else {
            list.push(s.clone());
            self.note(format!("~{from} send {tid} {}dB{}", fmt_num(s.db), if pre { " pre" } else { "" }));
        }
        Ok(())
    }

    /// Automation keys on a track or bus (`kf A1 vol 0:-6 4:0`), in comp seconds.
    pub fn kf_auto(&mut self, node: &str, prop: &str, a: &mut A) -> Result<()> {
        let prop = match prop.to_ascii_lowercase().as_str() {
            "vol" | "volume" | "gain" => "vol",
            "pan" => "pan",
            o => bail!("track and bus automation covers vol and pan, not {o}"),
        };
        let tokens: Vec<String> = a.pos.drain(..).collect();
        let mut keys = vec![];
        for tok in &tokens {
            let (ts, vs) = tok.split_once(':').ok_or_else(|| anyhow!("keyframe '{tok}' must look like time:value (comp seconds), e.g. 0:-6 4:0"))?;
            let t = self.time(ts)?;
            let v = if prop == "vol" { db(vs)?.clamp(-96.0, 24.0) } else { vs.parse::<f64>()?.clamp(-1.0, 1.0) };
            keys.push(Key { t, v: Val::N(v), e: Ease::Linear });
        }
        let ci = self.ci();
        let c = &mut self.p.comps[ci];
        let auto = if let Some(b) = c.buses.iter_mut().find(|b| b.id.eq_ignore_ascii_case(node)) {
            &mut b.auto
        } else {
            &mut c.track_mut(node).ok_or_else(|| anyhow!("no track or bus {node}"))?.auto
        };
        if keys.is_empty() || tokens.iter().any(|t| t.eq_ignore_ascii_case("clear")) {
            auto.remove(prop);
            self.note(format!("~{node} {prop} automation cleared"));
            return Ok(());
        }
        let n = keys.len();
        let mut an = Anim::new(Val::N(0.0));
        an.set_keys(keys);
        auto.insert(prop.to_string(), an);
        self.note(format!("~{node} {prop} automation ({n} keys)"));
        Ok(())
    }

    /// Expands a rack into ordinary effects on `target`.
    pub fn afx_rack(&mut self, target: &str, rack: &str) -> Result<()> {
        let r = RACKS.iter().find(|r| r.0.eq_ignore_ascii_case(rack)).ok_or_else(|| anyhow!("unknown rack '{rack}' ({})", RACKS.iter().map(|r| r.0).collect::<Vec<_>>().join(" ")))?;
        for line in r.2 {
            for c in super::lex::parse_script(&line.replace("{t}", target))? {
                self.cmd(&c)?;
            }
        }
        self.note(format!("~{target} rack {}", r.0));
        Ok(())
    }

    /// Measures a clip's audio (as recorded, before its effects).
    fn clip_audio_stats(&mut self, id: &str) -> Result<(Stats, Media, f64, f64)> {
        let l = self.relocate(id)?;
        let it = self.item_at(l).clone();
        let m = it.src.as_deref().and_then(|s| self.p.media(s)).cloned().ok_or_else(|| anyhow!("{id} has no media"))?;
        if !m.has_audio {
            bail!("{id} has no audio");
        }
        let (t0, t1) = (it.src_in, it.src_in + it.dur * it.speed());
        let mut opts = BTreeMap::new();
        opts.insert("t0".to_string(), format!("{t0:.3}"));
        opts.insert("t1".to_string(), format!("{t1:.3}"));
        let v = self.env.analyze(self.p, &m.id, "audiostats", &opts)?;
        let st: Stats = Stats {
            secs: v["secs"].as_f64().unwrap_or(0.0),
            integrated: v["integrated"].as_f64().unwrap_or(-70.0),
            short_max: v["short_max"].as_f64().unwrap_or(-70.0),
            momentary_max: v["momentary_max"].as_f64().unwrap_or(-70.0),
            lra: v["lra"].as_f64().unwrap_or(0.0),
            true_peak: v["true_peak"].as_f64().unwrap_or(-96.0),
            floor: v["floor"].as_f64().unwrap_or(-96.0),
            clipped: v["clipped"].as_u64().unwrap_or(0) as usize,
            dc: v["dc"].as_f64().unwrap_or(0.0),
            bands: {
                let mut b = [0.0; 6];
                for (i, x) in b.iter_mut().enumerate() {
                    *x = v["bands"][i].as_f64().unwrap_or(-60.0);
                }
                b
            },
            corr: v["corr"].as_f64().unwrap_or(1.0),
            hum: v["hum"].as_array().and_then(|h| Some((h.first()?.as_f64()?, h.get(1)?.as_f64()?))),
            quiet_at: v["quiet_at"].as_f64().unwrap_or(0.0),
            momentary: vec![],
            spectrum: vec![],
        };
        Ok((st, m, t0, t1))
    }

    /// `repair <clips>`: measure, then add only what's needed.
    pub fn c_repair(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("repair <clips|track>"))?;
        let ids: Vec<String> = if self.comp().track(&spec).is_some() && self.p.locate(&spec).is_none() {
            self.comp().track(&spec).unwrap().items.iter().filter(|i| i.kind == ItemKind::Clip).map(|i| i.id.clone()).collect()
        } else {
            self.targets(&spec)?
        };
        for id in ids {
            let Ok((st, _, _, _)) = self.clip_audio_stats(&id) else { continue };
            let mut found = vec![];
            let mut add = vec![];
            if let Some((f, d)) = st.hum {
                found.push(format!("hum {f:.0} Hz (+{d:.0} dB)"));
                add.push(format!("afx {id} dehum freq={f:.0}"));
            }
            let clip_share = st.clipped as f64 / (st.secs * 48000.0).max(1.0);
            if clip_share > 1e-4 {
                found.push(format!("{} clipped samples", st.clipped));
                add.push(format!("afx {id} declip"));
            }
            if st.bands[0] > -12.0 {
                found.push(format!("rumble (sub band {:.0} dB)", st.bands[0]));
                add.push(format!("afx {id} highpass freq=80"));
            }
            if st.floor > -58.0 {
                let amount = ((st.floor + 70.0) * 0.8).clamp(6.0, 30.0);
                found.push(format!("noise floor {:.0} dB", st.floor));
                add.push(format!("afx {id} denoise amount={amount:.0}"));
            }
            if st.dc.abs() > 0.01 {
                found.push(format!("DC offset {:.3}", st.dc));
                add.push(format!("afx {id} highpass freq=20"));
            }
            if add.is_empty() {
                self.note(format!("~{id} repair: clean (floor {:.0} dB, no hum, no clipping)", st.floor));
                continue;
            }
            for line in &add {
                for c in super::lex::parse_script(line)? {
                    self.cmd(&c)?;
                }
            }
            let what: Vec<String> = add.iter().map(|l| format!("+{}", l.split_whitespace().nth(2).unwrap_or(""))).collect();
            self.note(format!("~{id} repair: {} -> {}", found.join(", "), what.join(" ")));
        }
        Ok(())
    }

    /// `roomtone <track> [db=-6]`: fills gaps between clips with tone looped
    /// from the quietest stretch of the track's own recordings.
    pub fn c_roomtone(&mut self, a: &mut A) -> Result<()> {
        let tid = a.shift().ok_or_else(|| anyhow!("roomtone <A1> [gain=dB]"))?;
        let gain = a.take(&["gain", "db", "vol"]).map(|s| db(&s)).transpose()?.unwrap_or(0.0);
        let t = self.comp().track(&tid).ok_or_else(|| anyhow!("no track {tid}"))?.clone();
        let mut clips: Vec<Item> = t.items.iter().filter(|i| i.kind == ItemKind::Clip).cloned().collect();
        clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        if clips.len() < 2 {
            bail!("{tid} has no gaps between clips to fill");
        }
        // the quietest second across the track's clips
        let mut best: Option<(f64, String, f64)> = None;
        for c in &clips {
            let Ok((st, m, t0, t1)) = self.clip_audio_stats(&c.id) else { continue };
            if t1 - t0 < 1.0 {
                continue;
            }
            let level = st.floor;
            if best.as_ref().is_none_or(|b| level < b.0) {
                best = Some((level, m.id.clone(), t0 + st.quiet_at));
            }
        }
        let (_, mid, at) = best.ok_or_else(|| anyhow!("no clip on {tid} has a second of audio to take room tone from"))?;
        let mut made = 0;
        let mut lines = vec![];
        for w in clips.windows(2) {
            let (gs, ge) = (w[0].end(), w[1].start);
            if ge - gs < 0.04 {
                continue;
            }
            let mut s = gs;
            while s < ge - 0.02 {
                let d = (ge - s).min(1.0);
                lines.push(format!("add {mid} {tid} at={} dur={} in={} vol={}", fmt_num(s), fmt_num(d), fmt_num(at), fmt_num(gain)));
                s += d;
                made += 1;
            }
        }
        for l in &lines {
            for c in super::lex::parse_script(l)? {
                self.cmd(&c)?;
            }
        }
        self.note(format!("~{tid} roomtone: {made} fills from {mid} at {}", fmt_t(at)));
        Ok(())
    }

    /// `match <ids> to=<clip> eq|loudness`: dialogue matching by measurement.
    pub fn match_audio(&mut self, spec: &str, to: &str, eq: bool, loud: bool) -> Result<()> {
        let rid = self.item_id(to)?;
        let (rs, _, _, _) = self.clip_audio_stats(&rid)?;
        for id in self.targets(spec)? {
            if id == rid {
                continue;
            }
            let (s, _, _, _) = self.clip_audio_stats(&id)?;
            let mut done = vec![];
            if eq {
                // band differences become peq bands (bell at each band's center)
                let mut bands = vec![];
                for (i, (_, lo, hi)) in BANDS.iter().enumerate() {
                    let d = (rs.bands[i] - s.bands[i]).clamp(-9.0, 9.0);
                    if d.abs() >= 1.0 {
                        bands.push(format!("{:.0},{:.1},0.8,bell", (lo * hi).sqrt(), d));
                    }
                }
                if !bands.is_empty() {
                    let line = format!("afx {id} peq bands='{}'", bands.join(" "));
                    for c in super::lex::parse_script(&line)? {
                        self.cmd(&c)?;
                    }
                    done.push(format!("eq {} bands", bands.len()));
                }
            }
            if loud {
                let d = (rs.integrated - s.integrated).clamp(-24.0, 24.0);
                let l = self.relocate(&id)?;
                let it = self.item_mut_at(l);
                let cur = it.props.get("vol").map(|a| a.v.num()).unwrap_or(0.0);
                if it.props.get("vol").is_some_and(|a| a.is_animated()) {
                    if let Some(a) = it.props.get_mut("vol") {
                        for k in a.k.iter_mut() {
                            k.v = Val::N(k.v.num() + d);
                        }
                    }
                } else {
                    it.props.insert("vol".into(), Anim::new(Val::N((cur + d).clamp(-96.0, 24.0))));
                }
                done.push(format!("gain {:+.1} dB ({:.1} -> {:.1} LUFS)", d, s.integrated, rs.integrated));
            }
            self.note(format!("~{id} match {rid}: {}", if done.is_empty() { "already close".into() } else { done.join(", ") }));
        }
        Ok(())
    }
}
