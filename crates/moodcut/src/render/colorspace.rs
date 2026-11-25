//! Input color transforms (`input=` on clips) and the `cst` effect: camera log curves and HDR
//! transfer functions to Rec.709 display. Each profile is a transfer curve to
//! scene linear, a gamut matrix to Rec.709 primaries, a gentle highlight
//! roll-off and the Rec.709 curve, baked into a 33-point `.cube` that FFmpeg's
//! `lut3d` applies while decoding, on the source's own 10-bit (or better)
//! data. The log-to-display step, where 8 bits hurt most, so runs at source
//! precision, and preview and export (which share the decoder) match.
//!
//! The curves are the published ones; the gamut matrices are the commonly
//! used approximations. Canon Cinema Gamut and DJI D-Gamut are treated as
//! Rec.709 primaries (curve only).

use anyhow::Result;
use std::path::PathBuf;

type M3 = [[f64; 3]; 3];

const ID: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const SGAMUT3_CINE: M3 = [[1.6269474, -0.5401385, -0.0868089], [-0.1785155, 1.4179409, -0.2394254], [-0.0444361, -0.1959199, 1.2403560]];
const AWG3: M3 = [[1.617523, -0.537287, -0.080237], [-0.070573, 1.334613, -0.264040], [-0.021102, -0.226954, 1.248056]];
const AWG4: M3 = [[1.765, -0.643, -0.122], [-0.129, 1.346, -0.217], [-0.023, -0.211, 1.234]];
const VGAMUT: M3 = [[1.806576, -0.695697, -0.110879], [-0.170090, 1.305955, -0.135865], [-0.025206, -0.154468, 1.179674]];
const BT2020: M3 = [[1.6605, -0.5876, -0.0728], [-0.1246, 1.1329, -0.0083], [-0.0182, -0.1006, 1.1187]];

/// Resolves `auto` from the file's tags: HLG and PQ are tagged; log is not.
pub fn resolve(input: &str, media_color: &str) -> &'static str {
    match input {
        "auto" | "" => {
            let trc = media_color.split('/').next().unwrap_or("");
            match trc {
                "arib-std-b67" => "hlg",
                "smpte2084" => "pq",
                _ => "rec709",
            }
        }
        other => crate::model::schema::INPUT_PROFILES.iter().find(|p| **p == other).copied().unwrap_or("rec709"),
    }
}

/// Encoded value (0..1) to scene linear (0.18 = mid gray) for a profile.
pub fn to_linear(profile: &str, x: f64) -> f64 {
    match profile {
        "slog3" => {
            let c = x * 1023.0;
            if c >= 171.2102946929 { 10f64.powf((c - 420.0) / 261.5) * 0.19 - 0.01 } else { (c - 95.0) * 0.01125 / (171.2102946929 - 95.0) }
        }
        "logc3" => {
            let (cut, a, b, c, d, e, f) = (0.010591, 5.555556, 0.052272, 0.247190, 0.385537, 5.367655, 0.092809);
            if x > e * cut + f { (10f64.powf((x - d) / c) - b) / a } else { (x - f) / e }
        }
        "logc4" => {
            let a = (2f64.powi(18) - 16.0) / 117.45;
            let b = (1023.0 - 95.0) / 1023.0;
            let c = 95.0 / 1023.0;
            let s = (7.0 * std::f64::consts::LN_2 * 2f64.powf(7.0 - 14.0 * c / b)) / (a * b);
            let t = (2f64.powf(14.0 * (-c / b) + 6.0) - 64.0) / a;
            if x >= 0.0 { (2f64.powf(14.0 * (x - c) / b + 6.0) - 64.0) / a } else { x * s + t }
        }
        "vlog" => {
            if x < 0.181 { (x - 0.125) / 5.6 } else { 10f64.powf((x - 0.598206) / 0.241514) - 0.00873 }
        }
        "clog3" => {
            if x < 0.097465473 {
                -(10f64.powf((0.12783901 - x) / 0.36726845) - 1.0) / 14.98325
            } else if x <= 0.15277891 {
                (x - 0.12512219) / 1.9754798
            } else {
                (10f64.powf((x - 0.12240537) / 0.36726845) - 1.0) / 14.98325
            }
        }
        "dlog" => {
            if x <= 0.14 { (x - 0.0929) / 6.025 } else { (10f64.powf(3.89616 * x - 2.27752) - 0.0108) / 0.9892 }
        }
        "hlg" => {
            let (a, b, c) = (0.17883277, 0.28466892, 0.55991073);
            let l = if x <= 0.5 { x * x / 3.0 } else { (((x - c) / a).exp() + b) / 12.0 };
            // HLG reference white (75% signal) to SDR white
            l / 0.2647 * 0.9
        }
        "pq" => {
            let (m1, m2, c1, c2, c3) = (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
            let p = x.max(0.0).powf(1.0 / m2);
            let nits = 10000.0 * ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1);
            // 203 nits (BT.2408 reference white) to SDR white
            nits / 203.0 * 0.9
        }
        _ => x,
    }
}

fn gamut(profile: &str) -> &'static M3 {
    match profile {
        "slog3" => &SGAMUT3_CINE,
        "logc3" => &AWG3,
        "logc4" => &AWG4,
        "vlog" => &VGAMUT,
        "hlg" | "pq" => &BT2020,
        _ => &ID,
    }
}

/// Highlight roll-off above 0.8 toward 1.0 (no hard clip).
fn rolloff(x: f64) -> f64 {
    if x <= 0.8 { x.max(0.0) } else { 0.8 + 0.2 * (1.0 - (-(x - 0.8) / 0.2).exp()) }
}

fn rec709(l: f64) -> f64 {
    if l < 0.018 { 4.5 * l } else { 1.099 * l.powf(0.45) - 0.099 }
}

/// The transform for one RGB value (0..1 in, 0..1 out).
pub fn transform(profile: &str, rgb: [f64; 3]) -> [f64; 3] {
    if profile == "rec709" {
        return rgb;
    }
    let lin = rgb.map(|v| to_linear(profile, v));
    let m = gamut(profile);
    let mut out = [0.0; 3];
    for (i, o) in out.iter_mut().enumerate() {
        let v = m[i][0] * lin[0] + m[i][1] * lin[1] + m[i][2] * lin[2];
        *o = rec709(rolloff(v)).clamp(0.0, 1.0);
    }
    out
}

/// The `.cube` text for a profile.
pub fn cube(profile: &str) -> String {
    let n = 33;
    let mut s = format!("TITLE \"MoodCut input {profile} to Rec.709\"\nLUT_3D_SIZE {n}\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let k = (n - 1) as f64;
                let o = transform(profile, [r as f64 / k, g as f64 / k, b as f64 / k]);
                s.push_str(&format!("{:.6} {:.6} {:.6}\n", o[0], o[1], o[2]));
            }
        }
    }
    s
}

/// The cube file for a profile in the cache folder (written once).
pub fn cube_file(profile: &str) -> Result<PathBuf> {
    let path = crate::config::cache_dir().join("luts").join(format!("input-{profile}-v1.cube"));
    if !path.is_file() {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, cube(profile))?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

/// Quotes a path for use inside an FFmpeg filtergraph option.
pub fn filter_path(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "\\'");
    format!("'{s}'")
}

/// The decoder filter for a clip's input transform, if any.
pub fn decode_filter(input: &str, media_color: &str) -> Option<String> {
    let profile = resolve(input, media_color);
    if profile == "rec709" {
        return None;
    }
    let f = cube_file(profile).ok()?;
    Some(format!("lut3d=file={}:interp=tetrahedral", filter_path(&f)))
}

// ---------------------------------------------------------------- color space transform

const AP1: M3 = [[1.7050510, -0.6217921, -0.0832590], [-0.1302564, 1.1408047, -0.0105483], [-0.0240034, -0.1289690, 1.1529724]];

/// Scene linear (0.18 = mid gray) to the encoded value: the inverse of
/// [`to_linear`] for the log and HDR curves.
pub fn from_linear(profile: &str, l: f64) -> f64 {
    match profile {
        "slog3" => {
            let c = if l >= 0.01125 { 420.0 + ((l + 0.01) / 0.19).max(1e-12).log10() * 261.5 } else { l * (171.2102946929 - 95.0) / 0.01125 + 95.0 };
            c / 1023.0
        }
        "logc3" => {
            let (cut, a, b, c, d, e, f) = (0.010591, 5.555556, 0.052272, 0.247190, 0.385537, 5.367655, 0.092809);
            if l > cut { c * (a * l + b).max(1e-12).log10() + d } else { e * l + f }
        }
        "logc4" => {
            let a = (2f64.powi(18) - 16.0) / 117.45;
            let b = (1023.0 - 95.0) / 1023.0;
            let c = 95.0 / 1023.0;
            let s = (7.0 * std::f64::consts::LN_2 * 2f64.powf(7.0 - 14.0 * c / b)) / (a * b);
            let t = (2f64.powf(14.0 * (-c / b) + 6.0) - 64.0) / a;
            if l >= t { ((a * l + 64.0).max(1e-12).log2() - 6.0) / 14.0 * b + c } else { (l - t) / s }
        }
        "vlog" => {
            if l < 0.01 { 5.6 * l + 0.125 } else { 0.241514 * (l + 0.00873).max(1e-12).log10() + 0.598206 }
        }
        "clog3" => {
            if l < -0.014 {
                0.12783901 - 0.36726845 * (1.0 - 14.98325 * l).max(1e-12).log10()
            } else if l <= 0.014 {
                l * 1.9754798 + 0.12512219
            } else {
                0.36726845 * (14.98325 * l + 1.0).max(1e-12).log10() + 0.12240537
            }
        }
        "dlog" => {
            if l <= 0.0078 { 6.025 * l + 0.0929 } else { ((l * 0.9892 + 0.0108).max(1e-12).log10() + 2.27752) / 3.89616 }
        }
        "hlg" => {
            let (a, b, c) = (0.17883277, 0.28466892, 0.55991073);
            let e = (l * 0.2647 / 0.9).max(0.0);
            if e <= 1.0 / 12.0 { (3.0 * e).sqrt() } else { a * (12.0 * e - b).max(1e-12).ln() + c }
        }
        "pq" => {
            let (m1, m2, c1, c2, c3) = (0.1593017578125, 78.84375, 0.8359375, 18.8515625, 18.6875);
            let y = (l / 0.9 * 203.0 / 10000.0).max(0.0).powf(m1);
            ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
        }
        _ => l,
    }
}

/// Encoded value to scene linear for any `cst` space.
pub fn decode(space: &str, x: f64) -> f64 {
    match space {
        "linear" => x,
        "rec709" => {
            if x < 0.081 { x / 4.5 } else { ((x + 0.099) / 1.099).powf(1.0 / 0.45) }
        }
        "srgb" => {
            if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
        }
        "acescct" => {
            if x <= 0.155251141552511 { (x - 0.0729055341958355) / 10.5402377416545 } else { 2f64.powf(x * 17.52 - 9.72) }
        }
        p => to_linear(p, x),
    }
}

/// Scene linear to the encoded value for any `cst` space.
pub fn encode(space: &str, l: f64) -> f64 {
    match space {
        "linear" => l,
        "rec709" => rec709(l.max(0.0)),
        "srgb" => {
            let l = l.max(0.0);
            if l <= 0.0031308 { 12.92 * l } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
        }
        "acescct" => {
            if l <= 0.0078125 { 10.5402377416545 * l + 0.0729055341958355 } else { (l.max(1e-12).log2() + 9.72) / 17.52 }
        }
        p => from_linear(p, l),
    }
}

/// The space's gamut to Rec.709 primaries.
fn gamut_of(space: &str) -> M3 {
    match space {
        "acescct" => AP1,
        p => *gamut(p),
    }
}

fn mul3(m: &M3, v: [f64; 3]) -> [f64; 3] {
    [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2], m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2], m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
}

/// Inverse of a 3x3 matrix (identity if singular).
pub fn inv3(m: &M3) -> M3 {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return ID;
    }
    let k = 1.0 / det;
    [
        [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) * k, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * k, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * k],
        [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) * k, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * k, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * k],
        [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) * k, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * k, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * k],
    ]
}

/// Display-referred spaces: tone mapping brings scene values into 0..1.
fn display(space: &str) -> bool {
    matches!(space, "rec709" | "srgb")
}

/// Color space transform of one value (0..1 in, 0..1 out): decode, gamut
/// to Rec.709 (where exposure, gamut mapping and tone mapping happen),
/// gamut to the output, encode.
pub fn cst(from: &str, to: &str, rgb: [f64; 3], tonemap: &str, gamut_map: &str, exposure: f64) -> [f64; 3] {
    let k = 2f64.powf(exposure);
    let lin = mul3(&gamut_of(from), rgb.map(|v| decode(from, v) * k));
    // gamut: pull negative components toward neutral along the line to luma
    let mut v = lin;
    if gamut_map != "clip" {
        let y = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
        let mn = v[0].min(v[1]).min(v[2]);
        if mn < 0.0 && y > 0.0 {
            let t = y / (y - mn);
            v = v.map(|c| y + (c - y) * t);
        }
    }
    // tone: wide-range input into a display range
    if display(to) && !display(from) {
        match tonemap {
            "none" => {}
            "luma" => {
                let y = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
                if y > 0.8 {
                    let s = rolloff(y) / y;
                    v = v.map(|c| c * s);
                }
                v = v.map(|c| c.min(1.0));
            }
            _ => v = v.map(rolloff),
        }
    }
    let out = mul3(&inv3(&gamut_of(to)), v);
    out.map(|c| encode(to, c).clamp(0.0, 1.0))
}

#[cfg(test)]
mod cst_tests {
    use super::*;

    #[test]
    fn curves_round_trip() {
        for p in ["slog3", "logc3", "logc4", "vlog", "clog3", "dlog", "hlg", "pq", "acescct", "rec709", "srgb"] {
            for l in [0.0, 0.01, 0.18, 0.5, 0.9] {
                let back = decode(p, encode(p, l));
                assert!((back - l).abs() < 1e-4, "{p} at {l}: {back}");
            }
        }
    }

    #[test]
    fn cst_matches_the_input_transform_and_inverts() {
        let g = [420.0 / 1023.0; 3];
        let a = cst("slog3", "rec709", g, "simple", "clip", 0.0);
        let b = transform("slog3", g);
        assert!((a[0] - b[0]).abs() < 0.02, "{a:?} vs {b:?}");
        // rec709 -> logc3 -> rec709 is (nearly) the identity for mid tones
        let m = cst("rec709", "logc3", [0.4, 0.5, 0.3], "none", "clip", 0.0);
        let back = cst("logc3", "rec709", m, "none", "clip", 0.0);
        assert!((back[0] - 0.4).abs() < 0.01 && (back[1] - 0.5).abs() < 0.01 && (back[2] - 0.3).abs() < 0.01, "{back:?}");
        let i = inv3(&AP1);
        let v = mul3(&AP1, mul3(&i, [0.3, 0.6, 0.1]));
        assert!((v[1] - 0.6).abs() < 1e-9);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_mid_gray_lands_near_display_mid_gray() {
        // published 18% gray code values
        for (p, code) in [("slog3", 420.0 / 1023.0), ("logc3", 0.391), ("vlog", 0.423), ("logc4", 0.278)] {
            let l = to_linear(p, code);
            assert!((l - 0.18).abs() < 0.02, "{p}: {l}");
        }
        let o = transform("slog3", [420.0 / 1023.0; 3]);
        assert!((o[0] - 0.41).abs() < 0.05, "{o:?}");
        assert_eq!(resolve("auto", "arib-std-b67/bt2020"), "hlg");
        assert_eq!(resolve("auto", "bt709/bt709"), "rec709");
        assert!(cube("vlog").lines().filter(|l| l.starts_with(char::is_numeric)).count() == 33 * 33 * 33);
        assert_eq!(filter_path(std::path::Path::new("C:\\a b\\x.cube")), "'C\\:/a b/x.cube'");
    }
}
