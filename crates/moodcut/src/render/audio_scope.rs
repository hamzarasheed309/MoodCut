//! Audio measurement and scopes, so an AI that can't listen can still mix by
//! numbers: integrated / short-term / momentary loudness (BS.1770), loudness
//! range, true peak, noise floor, clipped samples, DC offset, energy in six
//! bands, stereo correlation, mains hum and rumble. Scopes draw the same data:
//! spectrum, spectrogram, loudness over time, and a phase (goniometer) plot.

use super::ffmpeg::Ff;
use anyhow::{bail, Result};
use resvg::tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

pub const KINDS: &[&str] = &["spectrum", "spectrogram", "loudness", "phase", "mix"];
pub const RATE: f64 = 48000.0;
pub const BANDS: [(&str, f64, f64); 6] = [("sub", 20.0, 60.0), ("bass", 60.0, 250.0), ("lowmid", 250.0, 500.0), ("mid", 500.0, 2000.0), ("presence", 2000.0, 6000.0), ("air", 6000.0, 20000.0)];

/// Stereo f32 samples at 48 kHz.
pub type Pcm = Vec<[f32; 2]>;

/// Decodes FFmpeg output (stereo f32le at 48 kHz) from `args`, which must
/// end before the output options.
pub fn decode(ff: &Ff, mut args: Vec<String>) -> Result<Pcm> {
    args.extend(["-ac", "2", "-ar", "48000", "-f", "f32le", "pipe:1"].iter().map(|s| s.to_string()));
    let (out, _) = ff.run_capture(&args)?;
    Ok(out.chunks_exact(8).map(|c| [f32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_le_bytes([c[4], c[5], c[6], c[7]])]).collect())
}

/// A media file's audio from t0 for `dur` seconds.
pub fn decode_file(ff: &Ff, path: &str, t0: f64, dur: f64) -> Result<Pcm> {
    let args = vec!["-v".into(), "error".into(), "-ss".into(), format!("{t0:.4}"), "-t".into(), format!("{:.4}", dur.clamp(0.05, 600.0)), "-i".into(), path.to_string(), "-vn".into()];
    decode(ff, args)
}

struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    z: [f64; 2],
}

impl Biquad {
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[1] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[2] * y;
        y
    }
}

/// K-weighted mean square per 100 ms step (BS.1770 at 48 kHz).
fn k_power(pcm: &[[f32; 2]]) -> Vec<f64> {
    let mk = || {
        [
            Biquad { b: [1.53512485958697, -2.69169618940638, 1.19839281085285], a: [1.0, -1.69065929318241, 0.73248077421585], z: [0.0; 2] },
            Biquad { b: [1.0, -2.0, 1.0], a: [1.0, -1.99004745483398, 0.99007225036621], z: [0.0; 2] },
        ]
    };
    let (mut l, mut r) = (mk(), mk());
    let step = (RATE / 10.0) as usize;
    let mut out = vec![];
    let mut acc = 0.0;
    for (i, s) in pcm.iter().enumerate() {
        let a0 = l[0].run(s[0] as f64);
        let a = l[1].run(a0);
        let b0 = r[0].run(s[1] as f64);
        let b = r[1].run(b0);
        acc += a * a + b * b;
        if (i + 1) % step == 0 {
            out.push(acc / step as f64);
            acc = 0.0;
        }
    }
    out
}

fn lufs(ms: f64) -> f64 {
    -0.691 + 10.0 * ms.max(1e-12).log10()
}

/// Mean-square blocks of `n` 100 ms steps, hopping by one step.
fn blocks(p: &[f64], n: usize) -> Vec<f64> {
    if p.len() < n {
        return if p.is_empty() { vec![] } else { vec![p.iter().sum::<f64>() / p.len() as f64] };
    }
    p.windows(n).map(|w| w.iter().sum::<f64>() / n as f64).collect()
}

/// Everything measured about a stretch of audio.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub secs: f64,
    pub integrated: f64,
    pub short_max: f64,
    pub momentary_max: f64,
    pub lra: f64,
    pub true_peak: f64,
    /// Quietest 10% of 50 ms windows (dBFS RMS).
    pub floor: f64,
    pub clipped: usize,
    pub dc: f64,
    /// dB of each band relative to the total.
    pub bands: [f64; 6],
    pub corr: f64,
    /// Mains hum: (frequency, dB above its neighborhood).
    pub hum: Option<(f64, f64)>,
    /// Start (s) of the quietest second (room tone).
    pub quiet_at: f64,
    /// Momentary loudness every 100 ms (for the loudness scope).
    #[serde(skip)]
    pub momentary: Vec<f64>,
    /// Average power spectrum (4096-point FFT bins).
    #[serde(skip)]
    pub spectrum: Vec<f64>,
}

pub fn measure(pcm: &[[f32; 2]]) -> Stats {
    let mut st = Stats { secs: pcm.len() as f64 / RATE, ..Default::default() };
    if pcm.is_empty() {
        st.integrated = -70.0;
        st.floor = -96.0;
        return st;
    }
    let p = k_power(pcm);
    let mom = blocks(&p, 4);
    let short = blocks(&p, 30);
    st.momentary = mom.iter().map(|m| lufs(*m)).collect();
    st.momentary_max = st.momentary.iter().cloned().fold(-70.0, f64::max);
    st.short_max = short.iter().map(|m| lufs(*m)).fold(-70.0, f64::max);
    // integrated: absolute gate -70, relative gate -10
    let abs: Vec<f64> = mom.iter().cloned().filter(|m| lufs(*m) > -70.0).collect();
    st.integrated = if abs.is_empty() {
        -70.0
    } else {
        let rel = lufs(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
        let g: Vec<f64> = abs.iter().cloned().filter(|m| lufs(*m) > rel).collect();
        if g.is_empty() { -70.0 } else { lufs(g.iter().sum::<f64>() / g.len() as f64) }
    };
    // LRA: short-term loudness, gated -70 abs / -20 rel, 10th to 95th percentile
    let mut sl: Vec<f64> = short.iter().map(|m| lufs(*m)).filter(|l| *l > -70.0).collect();
    if !sl.is_empty() {
        let rel = lufs(short.iter().cloned().filter(|m| lufs(*m) > -70.0).sum::<f64>() / sl.len() as f64) - 20.0;
        sl.retain(|l| *l > rel);
        sl.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if sl.len() > 1 {
            let q = |f: f64| sl[((sl.len() - 1) as f64 * f).round() as usize];
            st.lra = q(0.95) - q(0.10);
        }
    }
    // true peak: 4x oversampled (Catmull-Rom between samples)
    let mut tp = 0f64;
    for ch in 0..2 {
        for i in 1..pcm.len().saturating_sub(2) {
            let (p0, p1, p2, p3) = (pcm[i - 1][ch] as f64, pcm[i][ch] as f64, pcm[i + 1][ch] as f64, pcm[i + 2][ch] as f64);
            tp = tp.max(p1.abs());
            for k in 1..4 {
                let t = k as f64 / 4.0;
                let v = 0.5 * ((2.0 * p1) + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
                tp = tp.max(v.abs());
            }
        }
    }
    st.true_peak = 20.0 * tp.max(1e-9).log10();
    st.clipped = pcm.iter().filter(|s| s[0].abs() >= 0.999 || s[1].abs() >= 0.999).count();
    st.dc = pcm.iter().map(|s| (s[0] + s[1]) as f64 * 0.5).sum::<f64>() / pcm.len() as f64;
    // noise floor and the quietest second
    let w = (RATE * 0.05) as usize;
    let rms: Vec<f64> = pcm.chunks(w).map(|c| (c.iter().map(|s| ((s[0] * s[0] + s[1] * s[1]) * 0.5) as f64).sum::<f64>() / c.len() as f64).sqrt()).collect();
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    st.floor = 20.0 * sorted[(sorted.len() as f64 * 0.1) as usize].max(1e-6).log10();
    if rms.len() >= 20 {
        let best = rms.windows(20).enumerate().map(|(i, x)| (i, x.iter().sum::<f64>())).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|x| x.0).unwrap_or(0);
        st.quiet_at = best as f64 * 0.05;
    }
    // correlation
    let (mut lr, mut ll, mut rr) = (0f64, 0f64, 0f64);
    for s in pcm {
        lr += (s[0] * s[1]) as f64;
        ll += (s[0] * s[0]) as f64;
        rr += (s[1] * s[1]) as f64;
    }
    st.corr = if ll > 1e-9 && rr > 1e-9 { lr / (ll * rr).sqrt() } else { 1.0 };
    // spectrum, bands, hum
    st.spectrum = avg_spectrum(pcm, 4096);
    let total: f64 = st.spectrum.iter().sum::<f64>().max(1e-20);
    for (i, (_, lo, hi)) in BANDS.iter().enumerate() {
        let e: f64 = st.spectrum.iter().enumerate().filter(|(k, _)| { let f = bin_hz(*k, 4096); f >= *lo && f < *hi }).map(|(_, v)| v).sum();
        st.bands[i] = 10.0 * (e / total).max(1e-12).log10();
    }
    // hum: a finer spectrum (2.9 Hz bins) so neighbors sit outside the peak
    let fine = avg_spectrum(pcm, 16384);
    for f0 in [50.0, 60.0] {
        let at = |f: f64| fine[((f / RATE * 16384.0).round() as usize).min(fine.len() - 1)];
        let peak = (1..=3).map(|h| at(f0 * h as f64)).sum::<f64>();
        let around = (1..=3).map(|h| at(f0 * h as f64 - 15.0) + at(f0 * h as f64 + 15.0)).sum::<f64>() / 2.0;
        let db = 10.0 * (peak / around.max(1e-20)).log10();
        if db > 10.0 && st.hum.is_none_or(|(_, d)| db > d) {
            st.hum = Some((f0, db));
        }
    }
    st
}

fn bin_hz(k: usize, n: usize) -> f64 {
    k as f64 * RATE / n as f64
}

/// In-place radix-2 FFT (n a power of two).
pub fn fft(re: &mut [f64], im: &mut [f64]) {
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
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        for i in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (c, s) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                let (a, b) = (i + k, i + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Power spectrum of one Hann-windowed frame (mono mix) starting at `at`.
fn frame_power(pcm: &[[f32; 2]], at: usize, n: usize) -> Vec<f64> {
    let mut re: Vec<f64> = (0..n).map(|i| {
        let s = pcm.get(at + i).map(|s| (s[0] + s[1]) as f64 * 0.5).unwrap_or(0.0);
        s * (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos())
    }).collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    (0..n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
}

/// Average power spectrum over up to 200 frames spread through the audio.
pub fn avg_spectrum(pcm: &[[f32; 2]], n: usize) -> Vec<f64> {
    let frames = (pcm.len() / n).clamp(1, 200);
    let hop = (pcm.len().saturating_sub(n) / frames.max(1)).max(1);
    let mut acc = vec![0.0; n / 2];
    for f in 0..frames {
        for (a, v) in acc.iter_mut().zip(frame_power(pcm, f * hop, n)) {
            *a += v;
        }
    }
    acc.iter().map(|v| v / frames as f64).collect()
}

/// One line for AI clients.
pub fn line(name: &str, s: &Stats) -> String {
    let bands: Vec<String> = BANDS.iter().zip(s.bands).map(|((n, _, _), d)| format!("{n} {d:.0}")).collect();
    let mut l = format!(
        "{name}: I {:.1} LUFS, S max {:.1}, M max {:.1}, LRA {:.1} LU, TP {:.1} dBTP, floor {:.0} dB, corr {:.2}, bands dB [{}]",
        s.integrated, s.short_max, s.momentary_max, s.lra, s.true_peak, s.floor, s.corr, bands.join(", ")
    );
    if s.clipped > 0 {
        l.push_str(&format!(", !{} clipped samples", s.clipped));
    }
    if s.dc.abs() > 0.005 {
        l.push_str(&format!(", DC offset {:.3}", s.dc));
    }
    if let Some((f, d)) = s.hum {
        l.push_str(&format!(", !hum {f:.0} Hz +{d:.0} dB"));
    }
    l
}

// ---------------------------------------------------------------- drawing

fn canvas(w: u32, h: u32) -> Result<Pixmap> {
    let mut pm = Pixmap::new(w.max(64), h.max(48)).ok_or_else(|| anyhow::anyhow!("bad scope size"))?;
    pm.fill(Color::from_rgba8(14, 14, 18, 255));
    Ok(pm)
}

fn poly(pm: &mut Pixmap, pts: &[(f32, f32)], rgba: [u8; 4], width: f32) {
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        if i == 0 { pb.move_to(*x, *y) } else { pb.line_to(*x, *y) }
    }
    if let Some(path) = pb.finish() {
        let mut p = Paint { anti_alias: true, ..Default::default() };
        p.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
        pm.stroke_path(&path, &p, &Stroke { width, ..Default::default() }, Transform::identity(), None);
    }
}

fn rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) {
    if let Some(r) = Rect::from_xywh(x, y, w.max(0.5), h.max(0.5)) {
        let mut p = Paint::default();
        p.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
        pm.fill_rect(r, &p, Transform::identity(), None);
    }
}

/// x position of a frequency on a log axis from 20 Hz to 20 kHz.
fn fx(f: f64, w: f32) -> f32 {
    ((f.max(20.0) / 20.0).log10() / 3.0) as f32 * w
}

pub fn draw(kind: &str, pcm: &[[f32; 2]], s: &Stats, w: u32, h: u32) -> Result<Pixmap> {
    let mut pm = canvas(w, h)?;
    let (wf, hf) = (pm.width() as f32, pm.height() as f32);
    match kind {
        "spectrum" => {
            for f in [100.0, 1000.0, 10000.0] {
                rect(&mut pm, fx(f, wf), 0.0, 1.0, hf, [60, 60, 70, 255]);
            }
            let sp = &s.spectrum;
            let mx = sp.iter().cloned().fold(1e-20, f64::max);
            let pts: Vec<(f32, f32)> = sp.iter().enumerate().skip(2).map(|(k, v)| {
                let db = 10.0 * (v / mx).max(1e-12).log10();
                (fx(bin_hz(k, 4096), wf), (-db / 90.0).clamp(0.0, 1.0) as f32 * hf)
            }).collect();
            poly(&mut pm, &pts, [120, 220, 255, 255], 1.5);
        }
        "spectrogram" => {
            let cols = w.max(64) as usize;
            let n = 2048;
            let hop = (pcm.len().saturating_sub(n) / cols).max(1);
            let mut peak = 1e-20f64;
            let colp: Vec<Vec<f64>> = (0..cols).map(|c| frame_power(pcm, c * hop, n)).collect();
            for c in &colp {
                peak = peak.max(c.iter().cloned().fold(0.0, f64::max));
            }
            let rows = pm.height() as usize;
            let width = pm.width() as usize;
            let data = pm.data_mut();
            for (cx, c) in colp.iter().enumerate().take(width) {
                for ry in 0..rows {
                    let f = 20.0 * 1000f64.powf(1.0 - ry as f64 / rows as f64);
                    let k = ((f / RATE * n as f64) as usize).min(c.len() - 1);
                    let db = 10.0 * (c[k] / peak).max(1e-12).log10();
                    let v = ((db + 90.0) / 90.0).clamp(0.0, 1.0);
                    let i = (ry * width + cx) * 4;
                    data[i..i + 4].copy_from_slice(&[(255.0 * v.powf(0.7)) as u8, (200.0 * v * v) as u8, (120.0 * (1.0 - v) * v * 2.0) as u8, 255]);
                }
            }
        }
        "loudness" => {
            for (db, c) in [(-14.0, [255, 190, 60, 160]), (-23.0, [120, 120, 140, 160])] {
                let y = ((-db) / 60.0) as f32 * hf;
                rect(&mut pm, 0.0, y, wf, 1.0, c);
            }
            let m = &s.momentary;
            let pts: Vec<(f32, f32)> = m.iter().enumerate().map(|(i, l)| (i as f32 / m.len().max(1) as f32 * wf, ((-l) / 60.0).clamp(0.0, 1.0) as f32 * hf)).collect();
            poly(&mut pm, &pts, [140, 255, 140, 255], 1.5);
        }
        "phase" => {
            rect(&mut pm, wf / 2.0, 0.0, 1.0, hf, [60, 60, 70, 255]);
            let r = wf.min(hf) * 0.45;
            let step = (pcm.len() / 20000).max(1);
            let width = pm.width() as usize;
            let rows = pm.height() as usize;
            let data = pm.data_mut();
            for s in pcm.iter().step_by(step) {
                // goniometer: mid up, side across
                let (m, sd) = ((s[0] + s[1]) as f32 * 0.707, (s[1] - s[0]) as f32 * 0.707);
                let (x, y) = ((wf / 2.0 + sd * r) as isize, (hf / 2.0 - m * r) as isize);
                if x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < rows {
                    let i = (y as usize * width + x as usize) * 4;
                    data[i] = data[i].saturating_add(40);
                    data[i + 1] = data[i + 1].saturating_add(90);
                    data[i + 2] = data[i + 2].saturating_add(40);
                }
            }
        }
        other => bail!("unknown audio scope {other} ({})", KINDS.join(" ")),
    }
    Ok(pm)
}

/// Bars of integrated loudness per node (the `mix` scope).
pub fn draw_mix(nodes: &[(String, Stats)], w: u32, h: u32) -> Result<Pixmap> {
    let mut pm = canvas(w, h)?;
    let (wf, hf) = (pm.width() as f32, pm.height() as f32);
    let bw = wf / nodes.len().max(1) as f32;
    for (i, (_, s)) in nodes.iter().enumerate() {
        let v = ((s.integrated + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
        let tp = ((s.true_peak + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
        rect(&mut pm, i as f32 * bw + bw * 0.15, hf * (1.0 - v), bw * 0.7, hf * v, [90, 170, 255, 255]);
        rect(&mut pm, i as f32 * bw + bw * 0.1, hf * (1.0 - tp), bw * 0.8, 2.0, if s.true_peak > -1.0 { [255, 60, 60, 255] } else { [255, 214, 10, 255] });
    }
    Ok(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_measures_as_expected() {
        // a -20 dBFS 1 kHz sine in both channels: about -23 LUFS, peak -20 dBTP
        let a = 0.1f32;
        let pcm: Pcm = (0..48000 * 3).map(|i| { let v = a * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin(); [v, v] }).collect();
        let s = measure(&pcm);
        assert!((s.integrated - (-20.0 - 3.0 + 3.0 - 0.691 + 0.0)).abs() < 2.0, "I {}", s.integrated);
        assert!((s.true_peak + 20.0).abs() < 0.3, "TP {}", s.true_peak);
        assert!((s.corr - 1.0).abs() < 1e-6);
        assert_eq!(s.clipped, 0);
        // the mid band holds the energy
        let best = s.bands.iter().cloned().enumerate().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap().0;
        assert_eq!(BANDS[best].0, "mid");
        assert!(s.hum.is_none());
    }

    #[test]
    fn hum_is_found() {
        let pcm: Pcm = (0..48000 * 2).map(|i| { let t = i as f32 / 48000.0; let v = 0.05 * (2.0 * std::f32::consts::PI * 50.0 * t).sin() + 0.002 * ((i * 7919 % 1000) as f32 / 500.0 - 1.0); [v, v] }).collect();
        assert_eq!(measure(&pcm).hum.map(|h| h.0), Some(50.0));
    }
}
