//! Procedural sound design: whooshes, risers, impacts, pops... synthesized here instead of
//! shipped as samples. Deterministic per (kind, duration, pitch, seed), so a project can
//! always re-create its files and repeated effects can still vary.

use anyhow::{Context, Result};
use std::f32::consts::{PI, TAU};
use std::path::Path;

pub const RATE: u32 = 48000;

pub struct Kind {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Default length (s).
    pub dur: f64,
    /// Where the moment lands, as a fraction of the length: 0 = at the start (impacts),
    /// 1 = at the end (risers), between = a peak (whooshes peak on the cut).
    pub sync: f64,
    /// Default clip gain (dB).
    pub vol: f64,
    pub desc: &'static str,
}

pub const KINDS: &[Kind] = &[
    Kind { name: "whoosh", aliases: &["transition", "swoosh", "woosh"], dur: 0.7, sync: 0.6, vol: -10.0, desc: "air whoosh peaking on the cut (transitions, whip pans, fast moves)" },
    Kind { name: "swish", aliases: &["swipe", "flick"], dur: 0.35, sync: 0.5, vol: -12.0, desc: "short bright swish (text and graphics sliding in, quick pans)" },
    Kind { name: "riser", aliases: &["uplifter", "build", "tension"], dur: 2.0, sync: 1.0, vol: -10.0, desc: "rising tension ending on the moment (before a drop, reveal or title)" },
    Kind { name: "reverse", aliases: &["reversecymbal", "swell", "suck"], dur: 1.2, sync: 1.0, vol: -10.0, desc: "reverse-cymbal swell into the moment" },
    Kind { name: "downlifter", aliases: &["falldown", "release"], dur: 1.5, sync: 0.0, vol: -12.0, desc: "falling sweep after an impact" },
    Kind { name: "hit", aliases: &["impact", "slam", "punch", "thud"], dur: 1.0, sync: 0.0, vol: -6.0, desc: "punchy impact (title slams, beat drops, hard cuts)" },
    Kind { name: "boom", aliases: &["subdrop", "sub", "bass", "drop"], dur: 1.8, sync: 0.0, vol: -8.0, desc: "deep sub drop (big reveals, logos, the drop)" },
    Kind { name: "pop", aliases: &["bubble", "blip"], dur: 0.15, sync: 0.0, vol: -12.0, desc: "soft pop (icons, stickers, caption words appearing)" },
    Kind { name: "click", aliases: &["tick", "tap", "ui"], dur: 0.05, sync: 0.0, vol: -14.0, desc: "UI click (cursor clicks, toggles, counters)" },
    Kind { name: "ding", aliases: &["bell", "chime", "notification", "success"], dur: 1.4, sync: 0.0, vol: -14.0, desc: "bright bell (checkmarks, notifications, success)" },
    Kind { name: "glitch", aliases: &["digital", "error", "data"], dur: 0.4, sync: 0.0, vol: -14.0, desc: "digital glitch burst (glitch transitions, tech titles)" },
    Kind { name: "typing", aliases: &["keyboard", "typewriter", "keys"], dur: 1.5, sync: 0.0, vol: -16.0, desc: "keystrokes for typewriter text (length = dur)" },
    Kind { name: "shutter", aliases: &["camera", "snap", "photo"], dur: 0.3, sync: 0.0, vol: -12.0, desc: "camera shutter (freeze frames, photo reveals)" },
];

pub fn kind(name: &str) -> Option<&'static Kind> {
    let n = name.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
    KINDS.iter().find(|k| k.name == n || k.aliases.contains(&n.as_str()))
}

/// File name for a rendered effect (the parameters make it unique and reproducible).
pub fn file_name(k: &Kind, dur: f64, pitch: f64, seed: u64) -> String {
    format!("{}-{}ms-p{}-v{}.wav", k.name, (dur * 1000.0).round() as i64, (pitch * 100.0).round() as i64, seed)
}

/// Deterministic noise.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03 | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// White noise in [-1, 1).
    fn n(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

/// RBJ band-pass (0 dB peak), retuned as it runs.
#[derive(Default)]
struct BandPass {
    b0: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BandPass {
    fn tune(&mut self, fc: f32, q: f32) {
        let w = TAU * (fc / RATE as f32).clamp(0.0005, 0.45);
        let alpha = w.sin() / (2.0 * q.max(0.05));
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * w.cos() / a0;
        self.a2 = (1.0 - alpha) / a0;
    }
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// One-pole low-pass.
struct LowPass {
    y: f32,
    a: f32,
}

impl LowPass {
    fn new(fc: f32) -> LowPass {
        LowPass { y: 0.0, a: 1.0 - (-TAU * fc / RATE as f32).exp() }
    }
    fn run(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

/// Equal-power pan, p in 0 (left) .. 1 (right).
fn pan(x: f32, p: f32) -> [f32; 2] {
    let a = p.clamp(0.0, 1.0) * PI / 2.0;
    [x * a.cos() * std::f32::consts::SQRT_2, x * a.sin() * std::f32::consts::SQRT_2]
}

/// Smooth bump peaking at `peak` (0..1 of the length): rises like a power curve, decays exponentially.
fn swell(u: f32, peak: f32, rise: f32, fall: f32) -> f32 {
    if u < peak {
        (u / peak.max(1e-3)).powf(rise)
    } else {
        (-(u - peak) / fall.max(1e-3)).exp()
    }
}

/// Renders an effect as stereo frames at [`RATE`].
pub fn synth(k: &Kind, dur: f64, pitch: f64, seed: u64) -> Vec<[f32; 2]> {
    let n = ((dur.max(0.02)) * RATE as f64).round() as usize;
    let len = n as f32 / RATE as f32;
    let mut r = Rng::new(seed.wrapping_add(k.name.len() as u64 * 7919));
    // each variation is tuned slightly differently, so repeated pops and hits don't sound copied
    let p = pitch.clamp(0.25, 4.0) as f32 * (1.0 + 0.04 * r.n());
    let mut out = vec![[0f32; 2]; n];
    let sr = RATE as f32;
    match k.name {
        "whoosh" | "swish" => {
            let bright = k.name == "swish";
            let peak = k.sync as f32 + r.range(-0.05, 0.05);
            let (lo, hi) = if bright { (1200.0, 6500.0) } else { (220.0, 2600.0) };
            let (mut bl, mut br) = (BandPass::default(), BandPass::default());
            let mut body = LowPass::new(if bright { 2500.0 } else { 600.0 });
            let dir = if r.f() < 0.5 { 1.0 } else { -1.0 };
            for (i, o) in out.iter_mut().enumerate() {
                let u = i as f32 / n as f32;
                let s = swell(u, peak, 2.2, if bright { 0.12 } else { 0.18 });
                if i % 32 == 0 {
                    let fc = p * (lo + (hi - lo) * s);
                    bl.tune(fc, 0.9);
                    br.tune(fc * 1.03, 0.9);
                }
                let (nl, nr) = (r.n(), r.n());
                let b = body.run((nl + nr) * 0.5) * 0.6;
                let l = (bl.run(nl) * 1.6 + b) * s;
                let rr = (br.run(nr) * 1.6 + b) * s;
                // moves across the stereo field, centered on the peak
                let [gl, gr] = pan(1.0, 0.5 + dir * (u - peak) * 0.8);
                *o = [l * gl, rr * gr];
            }
        }
        "riser" | "reverse" => {
            let cymbal = k.name == "reverse";
            let (mut bl, mut br) = (BandPass::default(), BandPass::default());
            let mut ph = [0f32; 2];
            for (i, o) in out.iter_mut().enumerate() {
                let u = i as f32 / n as f32;
                let t = i as f32 / sr;
                let (nl, nr) = (r.n(), r.n());
                if cymbal {
                    if i % 32 == 0 {
                        bl.tune(p * 7000.0, 0.7);
                        br.tune(p * 7400.0, 0.7);
                    }
                    let e = (6.0 * (u - 1.0)).exp();
                    *o = [bl.run(nl) * 2.2 * e, br.run(nr) * 2.2 * e];
                } else {
                    if i % 32 == 0 {
                        let fc = p * 300.0 * 20f32.powf(u);
                        bl.tune(fc, 2.0);
                        br.tune(fc * 1.05, 2.0);
                    }
                    let f = p * 110.0 * 8f32.powf(u);
                    let mut tone = [0f32; 2];
                    for (c, det) in [0.994f32, 1.006].iter().enumerate() {
                        ph[c] = (ph[c] + TAU * f * det / sr) % TAU;
                        tone[c] = ph[c].sin() + 0.3 * (3.0 * ph[c]).sin() + 0.12 * (5.0 * ph[c]).sin();
                    }
                    let trem = 1.0 - 0.25 * (0.5 + 0.5 * (TAU * (4.0 + 12.0 * u) * t).sin());
                    let e = u.powf(2.5) * trem;
                    *o = [(bl.run(nl) * 2.0 + tone[0] * 0.35) * e, (br.run(nr) * 2.0 + tone[1] * 0.35) * e];
                }
            }
        }
        "downlifter" => {
            let (mut bl, mut br) = (BandPass::default(), BandPass::default());
            let mut ph = 0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let u = i as f32 / n as f32;
                if i % 32 == 0 {
                    let fc = p * 6000.0 * 0.05f32.powf(u);
                    bl.tune(fc, 1.5);
                    br.tune(fc * 1.04, 1.5);
                }
                let f = p * 600.0 * (80.0f32 / 600.0).powf(u);
                ph = (ph + TAU * f / sr) % TAU;
                let e = (1.0 - u).powf(2.0) * (i as f32 / (0.01 * sr)).min(1.0);
                let tone = ph.sin() * 0.25;
                *o = [(bl.run(r.n()) * 2.0 + tone) * e, (br.run(r.n()) * 2.0 + tone) * e];
            }
        }
        "hit" | "boom" => {
            let boom = k.name == "boom";
            let mut ph = 0f32;
            let mut click = LowPass::new(3000.0);
            let mut crack = BandPass::default();
            crack.tune(p * 1800.0, 1.2);
            // the room tail is dark (two low-pass stages) so impacts don't hiss
            let (mut tl, mut tr) = (LowPass::new(500.0), LowPass::new(500.0));
            let (mut tl2, mut tr2) = (LowPass::new(700.0), LowPass::new(700.0));
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / sr;
                let f = if boom { p * (28.0 + 70.0 * (-t / 0.35).exp()) } else { p * (45.0 + 130.0 * (-t / 0.04).exp()) };
                ph = (ph + TAU * f / sr) % TAU;
                let attack = (t / 0.003).min(1.0);
                let body = ph.sin() * (-t / if boom { 0.7 } else { 0.28 }).exp() * attack;
                let transient = click.run(r.n()) * (-t / 0.008).exp() * if boom { 0.4 } else { 0.9 };
                let snap = if boom { 0.0 } else { crack.run(r.n()) * (-t / 0.06).exp() * 0.5 };
                let tail = (-t / if boom { 0.35 } else { 0.25 }).exp() * if boom { 0.15 } else { 0.25 };
                let x = ((body * 1.2 + transient + snap) * 1.5).tanh();
                *o = [x + tl2.run(tl.run(r.n())) * tail, x + tr2.run(tr.run(r.n())) * tail];
            }
        }
        "pop" => {
            let mut ph = 0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / sr;
                let f = p * (320.0 + 700.0 * (-t / 0.02).exp());
                ph = (ph + TAU * f / sr) % TAU;
                let x = ph.sin() * (-t / 0.035).exp() * (t / 0.001).min(1.0);
                *o = [x, x];
            }
        }
        "click" => {
            let mut last = 0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / sr;
                let nz = r.n();
                let hp = nz - last;
                last = nz;
                let x = hp * (-t / 0.004).exp() * 0.8 + (TAU * 2500.0 * p * t).sin() * (-t / 0.003).exp() * 0.5;
                *o = [x, x];
            }
        }
        "ding" => {
            let f0 = 1046.5 * p;
            let parts = [(1.0f32, 1.0f32, 1.0f32), (2.0, 0.4, 0.6), (2.76, 0.35, 0.4), (5.4, 0.15, 0.2)];
            let scale = len / 1.4;
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / sr;
                let mut x = 0.0;
                for (ratio, amp, decay) in parts {
                    x += (TAU * f0 * ratio * t).sin() * amp * (-t / (decay * scale)).exp();
                }
                x *= (t / 0.002).min(1.0) * 0.5;
                *o = [x, x * 0.96];
            }
        }
        "glitch" => {
            let mut i = 0;
            while i < n {
                let seg = ((r.range(0.015, 0.045)) * sr) as usize;
                let kind = r.f();
                let amp = r.range(0.5, 1.0);
                let pp = r.f();
                let f = p * r.range(200.0, 2000.0);
                let hold = (r.range(2.0, 24.0)) as usize;
                let mut held = 0f32;
                for j in 0..seg.min(n - i) {
                    let t = j as f32 / sr;
                    let x = if kind < 0.2 {
                        0.0
                    } else if kind < 0.6 {
                        if (t * f).fract() < 0.5 { 0.6 } else { -0.6 }
                    } else {
                        if j % hold == 0 {
                            held = r.n();
                        }
                        held
                    };
                    out[i + j] = pan(x * amp, pp);
                }
                i += seg;
            }
        }
        "typing" | "shutter" => {
            let mut strokes = vec![];
            if k.name == "shutter" {
                strokes.push((0.0f32, 1.0f32, 0.0f32));
                strokes.push((0.07 + r.range(0.0, 0.02), 0.8, 0.0));
            } else {
                let mut t = 0.02;
                while t < len - 0.05 {
                    strokes.push((t, r.range(0.6, 1.0), r.range(-0.3, 0.3)));
                    t += if r.f() < 0.12 { r.range(0.2, 0.3) } else { r.range(0.07, 0.16) };
                }
            }
            for (t0, amp, pp) in strokes {
                let thock = r.range(180.0, 260.0) * p;
                let mut bp = BandPass::default();
                bp.tune(r.range(2500.0, 4000.0) * p, 1.0);
                for rel in [0.0f32, 0.05] {
                    let start = ((t0 + rel) * sr) as usize;
                    let a = if rel == 0.0 { amp } else { amp * 0.35 };
                    for j in 0..(0.06 * sr) as usize {
                        let Some(o) = out.get_mut(start + j) else { break };
                        let t = j as f32 / sr;
                        let x = (bp.run(r.n()) * 3.0 * (-t / 0.005).exp() + (TAU * thock * t).sin() * 0.5 * (-t / 0.012).exp()) * a;
                        let [l, rr] = pan(x, 0.5 + pp);
                        o[0] += l;
                        o[1] += rr;
                    }
                }
            }
        }
        _ => {}
    }
    // normalize to -1 dBFS and de-click the ends (risers stop on the moment, so a short tail fade)
    let peak = out.iter().flat_map(|f| f.iter()).fold(0f32, |m, &x| m.max(x.abs()));
    let g = if peak > 1e-6 { 0.89 / peak } else { 0.0 };
    let fade_in = (0.002 * sr) as usize;
    let fade_out = (if k.sync >= 1.0 { 0.012 } else { 0.005 } * sr) as usize;
    for (i, f) in out.iter_mut().enumerate() {
        let mut e = g;
        if i < fade_in && k.sync > 0.0 {
            e *= i as f32 / fade_in as f32;
        }
        if i + fade_out > n {
            e *= (n - i) as f32 / fade_out as f32;
        }
        f[0] *= e;
        f[1] *= e;
    }
    out
}

/// Writes 16-bit stereo PCM.
pub fn write_wav(path: &Path, frames: &[[f32; 2]]) -> Result<()> {
    let data = (frames.len() * 4) as u32;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for f in frames {
        for x in f {
            b.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    std::fs::write(path, b).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[[f32; 2]]) -> f32 {
        (x.iter().map(|f| f[0] * f[0] + f[1] * f[1]).sum::<f32>() / (2 * x.len().max(1)) as f32).sqrt()
    }

    #[test]
    fn every_kind_is_audible_bounded_and_reproducible() {
        for k in KINDS {
            let a = synth(k, k.dur, 1.0, 1);
            assert_eq!(a.len(), (k.dur * RATE as f64).round() as usize, "{}", k.name);
            let peak = a.iter().flat_map(|f| f.iter()).fold(0f32, |m, &x| m.max(x.abs()));
            assert!(peak > 0.5 && peak <= 0.9, "{}: peak {peak}", k.name);
            assert!(a.iter().flatten().all(|x| x.is_finite()), "{}", k.name);
            assert!(a == synth(k, k.dur, 1.0, 1), "{} is deterministic", k.name);
            assert!(a != synth(k, k.dur, 1.0, 2), "{} varies with the seed", k.name);
        }
    }

    #[test]
    fn energy_lands_on_the_sync_point() {
        // risers build toward the end, impacts start loud, whooshes peak near their sync point
        let seg = |x: &[[f32; 2]], a: f64, b: f64| rms(&x[(a * x.len() as f64) as usize..(b * x.len() as f64) as usize]);
        let riser = synth(kind("riser").unwrap(), 2.0, 1.0, 0);
        assert!(seg(&riser, 0.8, 1.0) > seg(&riser, 0.0, 0.3) * 5.0);
        let hit = synth(kind("impact").unwrap(), 1.0, 1.0, 0);
        assert!(seg(&hit, 0.0, 0.15) > seg(&hit, 0.6, 1.0) * 5.0);
        let w = synth(kind("whoosh").unwrap(), 0.7, 1.0, 0);
        assert!(seg(&w, 0.5, 0.7) > seg(&w, 0.0, 0.2) * 2.0 && seg(&w, 0.5, 0.7) > seg(&w, 0.9, 1.0) * 2.0);
    }

    #[test]
    fn wav_header() {
        let dir = std::env::temp_dir().join(format!("moodcut-sfx-{}", std::process::id()));
        let p = dir.join("t.wav");
        write_wav(&p, &synth(kind("pop").unwrap(), 0.1, 1.0, 0)).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(b.len(), 44 + (0.1 * RATE as f64).round() as usize * 4);
        let _ = std::fs::remove_dir_all(dir);
    }
}
