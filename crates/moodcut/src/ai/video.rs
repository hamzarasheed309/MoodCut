//! AI Video: text to video and image to video through Runway (Gen-4.5, Veo 3.1, Kling,
//! Seedance, Gemini Omni) and fal.ai (Veo 3.1, Kling, Seedance, MiniMax Hailuo, Wan, or any fal
//! video endpoint). A first frame (and for most models a last frame) comes from the Program
//! monitor, a project still or video frame, or a file, so a generated shot can continue the edit.
//!
//! A job is sent, followed (the provider's progress when it reports one, else an estimate from
//! the model's usual time), downloaded to `<project> assets/ai-video/`, and imported as one
//! undoable edit (tag `ai-video`, the prompt as the note, optionally placed at a time). Jobs
//! keep running at the provider if MoodCut closes; the next start picks them up again
//! (`ai-video-jobs.json` in the data folder). Cancel also stops the job at the provider.

use super::http::{self, Body};
use super::tasks::{self, Progress};
use super::{at_of, clip_for, first_words, keys, media_for, out_dir, slug, str_of, unique_path};
use crate::engine::Engine;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const RUNWAY: &str = "https://api.dev.runwayml.com";
const RUNWAY_VERSION: &str = "2024-11-06";
const FAL: &str = "https://queue.fal.run";
const HIGGSFIELD: &str = "https://api.higgsfield.ai";

/// How a model's request is written.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Runway,
    FalVeo,
    FalKling3,
    FalKling4,
    FalSeedance,
    FalMinimax,
    FalWan,
    FalCustom,
    HfCinema,
    HfKling3,
    HfSeedance,
    HfMinimax,
}

/// Whether a model makes sound: on request, always, or never.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sound {
    Param,
    Always,
    None,
}

pub struct Model {
    pub id: &'static str,
    pub provider: &'static str,
    pub label: &'static str,
    pub maker: &'static str,
    pub blurb: &'static str,
    shape: Shape,
    /// The provider's model (Runway) or endpoint (fal) for text to video and image to video.
    t2v: Option<&'static str>,
    i2v: Option<&'static str>,
    /// Seconds: (shortest, longest), or only `steps` when listed.
    durs: (u32, u32),
    steps: &'static [u32],
    dur_default: u32,
    /// (aspect, resolution, value sent): a Runway ratio, or the resolution fal takes ("" = none).
    sizes: &'static [(&'static str, &'static str, &'static str)],
    sound: Sound,
    negative: bool,
    seed: bool,
    last_frame: bool,
    /// Image to video follows the image's framing (no aspect choice).
    i2v_auto: bool,
    max_prompt: usize,
    /// Usual generation time (s) for a default clip, for the estimate when there is no progress.
    secs: f64,
}

const SZ_RUNWAY_169_916_720: &[(&str, &str, &str)] = &[("16:9", "720p", "1280:720"), ("9:16", "720p", "720:1280")];
const SZ_RUNWAY_VEO: &[(&str, &str, &str)] = &[("16:9", "1080p", "1920:1080"), ("16:9", "720p", "1280:720"), ("9:16", "1080p", "1080:1920"), ("9:16", "720p", "720:1280")];
const SZ_RUNWAY_KLING3: &[(&str, &str, &str)] = &[("16:9", "1080p", "1920:1080"), ("9:16", "1080p", "1080:1920"), ("1:1", "1080p", "1440:1440")];
const SZ_RUNWAY_SEEDANCE: &[(&str, &str, &str)] = &[
    ("16:9", "1080p", "1920:1080"),
    ("16:9", "720p", "1280:720"),
    ("16:9", "4k", "3840:2160"),
    ("9:16", "1080p", "1080:1920"),
    ("9:16", "720p", "720:1280"),
    ("9:16", "4k", "2160:3840"),
    ("1:1", "1080p", "1440:1440"),
    ("1:1", "720p", "960:960"),
    ("21:9", "1080p", "2206:946"),
];
const SZ_RUNWAY_OMNI: &[(&str, &str, &str)] = &[("16:9", "1080p", "1920:1080"), ("16:9", "720p", "1280:720"), ("16:9", "4k", "3840:2160"), ("9:16", "1080p", "1080:1920"), ("9:16", "720p", "720:1280"), ("9:16", "4k", "2160:3840")];
const SZ_RUNWAY_KLING4: &[(&str, &str, &str)] = &[("16:9", "1080p", "1920:1080"), ("16:9", "720p", "1280:720"), ("9:16", "1080p", "1080:1920"), ("9:16", "720p", "720:1280"), ("1:1", "1080p", "1440:1440")];
const SZ_FAL_VEO: &[(&str, &str, &str)] = &[("16:9", "1080p", "1080p"), ("16:9", "720p", "720p"), ("16:9", "4k", "4k"), ("9:16", "1080p", "1080p"), ("9:16", "720p", "720p"), ("9:16", "4k", "4k")];
const SZ_FAL_KLING3: &[(&str, &str, &str)] = &[("16:9", "1080p", ""), ("9:16", "1080p", ""), ("1:1", "1080p", "")];
const SZ_FAL_KLING4: &[(&str, &str, &str)] = &[("16:9", "1080p", "1080p"), ("16:9", "720p", "720p"), ("16:9", "4k", "4k"), ("9:16", "1080p", "1080p"), ("9:16", "720p", "720p"), ("9:16", "4k", "4k"), ("1:1", "1080p", "1080p")];
const SZ_FAL_SEEDANCE: &[(&str, &str, &str)] = &[
    ("16:9", "1080p", "1080p"),
    ("16:9", "720p", "720p"),
    ("16:9", "4k", "4k"),
    ("9:16", "1080p", "1080p"),
    ("9:16", "720p", "720p"),
    ("9:16", "4k", "4k"),
    ("1:1", "1080p", "1080p"),
    ("21:9", "1080p", "1080p"),
];
const SZ_FAL_MINIMAX: &[(&str, &str, &str)] = &[("16:9", "2k", "2K"), ("16:9", "768p", "768P"), ("16:9", "4k", "4K"), ("9:16", "2k", "2K"), ("9:16", "768p", "768P"), ("1:1", "2k", "2K"), ("21:9", "2k", "2K")];
const SZ_HF_CINEMA: &[(&str, &str, &str)] = &[("16:9", "720p", "720p"), ("16:9", "480p", "480p"), ("9:16", "720p", "720p"), ("9:16", "480p", "480p"), ("1:1", "720p", "720p"), ("21:9", "720p", "720p"), ("4:3", "720p", "720p")];
const SZ_HF_SEEDANCE: &[(&str, &str, &str)] = &[("16:9", "1080p", "1080p"), ("16:9", "720p", "720p"), ("16:9", "480p", "480p"), ("9:16", "1080p", "1080p"), ("9:16", "720p", "720p"), ("1:1", "1080p", "1080p"), ("21:9", "1080p", "1080p")];
const SZ_HF_MINIMAX: &[(&str, &str, &str)] = &[("16:9", "2k", "2K"), ("9:16", "2k", "2K"), ("1:1", "2k", "2K"), ("21:9", "2k", "2K"), ("4:3", "2k", "2K")];
const SZ_FAL_WAN: &[(&str, &str, &str)] = &[("16:9", "1080p", "1080p"), ("16:9", "720p", "720p"), ("9:16", "1080p", "1080p"), ("9:16", "720p", "720p"), ("1:1", "1080p", "1080p"), ("4:3", "1080p", "1080p")];

/// The models offered, best known first per provider (the first of a provider is its default).
pub const MODELS: &[Model] = &[
    Model { id: "runway/gen4.5", provider: "runway", label: "Gen-4.5", maker: "Runway", blurb: "Runway's flagship: cinematic camera moves, believable physics, strong prompt following", shape: Shape::Runway, t2v: Some("gen4.5"), i2v: Some("gen4.5"), durs: (2, 10), steps: &[], dur_default: 5, sizes: SZ_RUNWAY_169_916_720, sound: Sound::None, negative: false, seed: true, last_frame: false, i2v_auto: false, max_prompt: 1000, secs: 75.0 },
    Model { id: "runway/veo3.1", provider: "runway", label: "Veo 3.1", maker: "Google", blurb: "Photoreal shots with native dialogue, sound effects and music", shape: Shape::Runway, t2v: Some("veo3.1"), i2v: Some("veo3.1"), durs: (4, 8), steps: &[4, 6, 8], dur_default: 8, sizes: SZ_RUNWAY_VEO, sound: Sound::Param, negative: true, seed: true, last_frame: true, i2v_auto: false, max_prompt: 1000, secs: 100.0 },
    Model { id: "runway/veo3.1_fast", provider: "runway", label: "Veo 3.1 Fast", maker: "Google", blurb: "Veo 3.1 at about half the time and cost: drafts and b-roll with sound", shape: Shape::Runway, t2v: Some("veo3.1_fast"), i2v: Some("veo3.1_fast"), durs: (4, 8), steps: &[4, 6, 8], dur_default: 8, sizes: SZ_RUNWAY_VEO, sound: Sound::Param, negative: true, seed: true, last_frame: true, i2v_auto: false, max_prompt: 1000, secs: 50.0 },
    Model { id: "runway/kling3.0_pro", provider: "runway", label: "Kling 3.0 Pro", maker: "Kuaishou", blurb: "Dynamic action and people in motion, 1080p, up to 15 s with sound", shape: Shape::Runway, t2v: Some("kling3.0_pro"), i2v: Some("kling3.0_pro"), durs: (3, 15), steps: &[], dur_default: 5, sizes: SZ_RUNWAY_KLING3, sound: Sound::Param, negative: false, seed: false, last_frame: true, i2v_auto: false, max_prompt: 2500, secs: 150.0 },
    Model { id: "runway/kling_4", provider: "runway", label: "Kling 4", maker: "Kuaishou", blurb: "Long shots up to 30 s, consistent characters, first/last frame control", shape: Shape::Runway, t2v: Some("kling_4"), i2v: Some("kling_4"), durs: (3, 30), steps: &[], dur_default: 5, sizes: SZ_RUNWAY_KLING4, sound: Sound::None, negative: false, seed: false, last_frame: true, i2v_auto: true, max_prompt: 26000, secs: 180.0 },
    Model { id: "runway/seedance2_5", provider: "runway", label: "Seedance 2.5", maker: "ByteDance", blurb: "Multi-shot storytelling, up to 4K and 30 s, with sound", shape: Shape::Runway, t2v: Some("seedance2_5"), i2v: Some("seedance2_5"), durs: (4, 30), steps: &[], dur_default: 5, sizes: SZ_RUNWAY_SEEDANCE, sound: Sound::Param, negative: false, seed: true, last_frame: true, i2v_auto: false, max_prompt: 15000, secs: 120.0 },
    Model { id: "runway/gemini_omni_flash_1.2", provider: "runway", label: "Gemini Omni Flash", maker: "Google", blurb: "Google's newest video model (Veo's successor): sound, up to 4K and 15 s", shape: Shape::Runway, t2v: Some("gemini_omni_flash_1.2"), i2v: Some("gemini_omni_flash_1.2"), durs: (3, 15), steps: &[], dur_default: 8, sizes: SZ_RUNWAY_OMNI, sound: Sound::Always, negative: false, seed: false, last_frame: true, i2v_auto: false, max_prompt: 4000, secs: 90.0 },
    Model { id: "fal/kling-v3-pro", provider: "fal", label: "Kling 3.0 Pro", maker: "Kuaishou", blurb: "Dynamic action and people in motion, 1080p, up to 15 s with sound", shape: Shape::FalKling3, t2v: Some("fal-ai/kling-video/v3/pro/text-to-video"), i2v: Some("fal-ai/kling-video/v3/pro/image-to-video"), durs: (3, 15), steps: &[], dur_default: 5, sizes: SZ_FAL_KLING3, sound: Sound::Param, negative: true, seed: false, last_frame: true, i2v_auto: true, max_prompt: 2500, secs: 150.0 },
    Model { id: "fal/veo3.1", provider: "fal", label: "Veo 3.1", maker: "Google", blurb: "Photoreal shots with native dialogue, sound effects and music, up to 4K", shape: Shape::FalVeo, t2v: Some("fal-ai/veo3.1"), i2v: Some("fal-ai/veo3.1/image-to-video"), durs: (4, 8), steps: &[4, 6, 8], dur_default: 8, sizes: SZ_FAL_VEO, sound: Sound::Param, negative: true, seed: true, last_frame: false, i2v_auto: true, max_prompt: 5000, secs: 100.0 },
    Model { id: "fal/veo3.1-fast", provider: "fal", label: "Veo 3.1 Fast", maker: "Google", blurb: "Veo 3.1 at about half the time and cost", shape: Shape::FalVeo, t2v: Some("fal-ai/veo3.1/fast"), i2v: Some("fal-ai/veo3.1/fast/image-to-video"), durs: (4, 8), steps: &[4, 6, 8], dur_default: 8, sizes: SZ_FAL_VEO, sound: Sound::Param, negative: true, seed: true, last_frame: false, i2v_auto: true, max_prompt: 5000, secs: 50.0 },
    Model { id: "fal/kling-v4", provider: "fal", label: "Kling 4", maker: "Kuaishou", blurb: "Long shots up to 30 s, up to 4K, first/last frame control", shape: Shape::FalKling4, t2v: Some("kling/v4/text-to-video"), i2v: Some("kling/v4/image-to-video"), durs: (3, 30), steps: &[], dur_default: 5, sizes: SZ_FAL_KLING4, sound: Sound::None, negative: false, seed: false, last_frame: true, i2v_auto: true, max_prompt: 26000, secs: 180.0 },
    Model { id: "fal/seedance-2.5", provider: "fal", label: "Seedance 2.5", maker: "ByteDance", blurb: "Multi-shot storytelling, up to 4K and 30 s, with sound", shape: Shape::FalSeedance, t2v: Some("bytedance/seedance-2.5/us/text-to-video"), i2v: Some("bytedance/seedance-2.5/us/image-to-video"), durs: (4, 30), steps: &[], dur_default: 5, sizes: SZ_FAL_SEEDANCE, sound: Sound::Param, negative: false, seed: false, last_frame: true, i2v_auto: true, max_prompt: 15000, secs: 120.0 },
    Model { id: "fal/minimax-h3", provider: "fal", label: "Hailuo 3", maker: "MiniMax", blurb: "Expressive characters and camera direction, up to 4K and 15 s", shape: Shape::FalMinimax, t2v: Some("minimax/h3/text-to-video"), i2v: Some("minimax/h3/image-to-video"), durs: (5, 15), steps: &[], dur_default: 5, sizes: SZ_FAL_MINIMAX, sound: Sound::None, negative: false, seed: true, last_frame: true, i2v_auto: true, max_prompt: 6000, secs: 150.0 },
    Model { id: "fal/wan-3.0", provider: "fal", label: "Wan 3.0", maker: "Alibaba", blurb: "Text to video with sound at a low price, up to 1080p", shape: Shape::FalWan, t2v: Some("alibaba/wan-3.0/text-to-video"), i2v: None, durs: (2, 15), steps: &[], dur_default: 5, sizes: SZ_FAL_WAN, sound: Sound::Param, negative: false, seed: true, last_frame: false, i2v_auto: false, max_prompt: 20000, secs: 120.0 },
    Model { id: "higgsfield/cinema-studio-4", provider: "higgsfield", label: "Cinema Studio 4", maker: "Higgsfield", blurb: "Higgsfield's film-look director: genre, era, lens and camera moves from the prompt, up to 30 s with sound; an image guides it as a reference", shape: Shape::HfCinema, t2v: Some("higgsfield/cinema-studio/4.0"), i2v: Some("higgsfield/cinema-studio/4.0"), durs: (4, 30), steps: &[], dur_default: 5, sizes: SZ_HF_CINEMA, sound: Sound::Param, negative: false, seed: false, last_frame: false, i2v_auto: false, max_prompt: 5000, secs: 150.0 },
    Model { id: "higgsfield/kling-3-pro", provider: "higgsfield", label: "Kling 3.0 Pro", maker: "Kuaishou", blurb: "Dynamic action and people in motion, 1080p, up to 15 s with sound", shape: Shape::HfKling3, t2v: Some("kling-video/v3.0/pro/text-to-video"), i2v: Some("kling-video/v3.0/pro/image-to-video"), durs: (3, 15), steps: &[], dur_default: 5, sizes: SZ_FAL_KLING3, sound: Sound::Param, negative: false, seed: false, last_frame: true, i2v_auto: true, max_prompt: 2500, secs: 150.0 },
    Model { id: "higgsfield/seedance-2.5", provider: "higgsfield", label: "Seedance 2.5", maker: "ByteDance", blurb: "Multi-shot storytelling up to 30 s, with sound", shape: Shape::HfSeedance, t2v: Some("bytedance/seedance-2.5/text-to-video"), i2v: Some("bytedance/seedance-2.5/image-to-video"), durs: (4, 30), steps: &[], dur_default: 5, sizes: SZ_HF_SEEDANCE, sound: Sound::Param, negative: false, seed: false, last_frame: true, i2v_auto: true, max_prompt: 15000, secs: 120.0 },
    Model { id: "higgsfield/minimax-h3", provider: "higgsfield", label: "Hailuo 3", maker: "MiniMax", blurb: "Expressive characters and camera direction, 2K, up to 15 s", shape: Shape::HfMinimax, t2v: Some("minimax/h3/text-to-video"), i2v: Some("minimax/h3/image-to-video"), durs: (5, 15), steps: &[], dur_default: 5, sizes: SZ_HF_MINIMAX, sound: Sound::None, negative: false, seed: false, last_frame: true, i2v_auto: true, max_prompt: 6000, secs: 150.0 },
    Model { id: "fal/custom", provider: "fal", label: "Other fal model", maker: "fal.ai", blurb: "Any fal.ai video endpoint by its id (endpoint=owner/app/path); prompt, image_url and your params are sent", shape: Shape::FalCustom, t2v: Some(""), i2v: Some(""), durs: (1, 60), steps: &[], dur_default: 5, sizes: &[], sound: Sound::None, negative: false, seed: false, last_frame: false, i2v_auto: true, max_prompt: 100_000, secs: 120.0 },
];

fn runway_hdr(key: &str) -> Vec<String> {
    vec![format!("Authorization: Bearer {key}"), format!("X-Runway-Version: {RUNWAY_VERSION}")]
}

fn fal_hdr(key: &str) -> Vec<String> {
    vec![format!("Authorization: Key {key}")]
}

/// Higgsfield's key is "KEY_ID:KEY_SECRET", sent the same way as fal's.
fn hf_hdr(key: &str) -> Vec<String> {
    vec![format!("Authorization: Key {key}")]
}

/// Higgsfield takes input images only as URLs: a frame is uploaded through a presigned URL
/// (the storage gets no API key) and its public URL is used.
fn hf_upload(key: &str, data_uri: &str, cancel: &AtomicBool) -> Result<String> {
    let (head, b64) = data_uri.split_once(";base64,").ok_or_else(|| anyhow!("the frame isn't a base64 image"))?;
    let mime = head.trim_start_matches("data:").to_string();
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).context("decoding the frame")?;
    let ext = if mime.contains("png") { "png" } else if mime.contains("webp") { "webp" } else { "jpg" };
    let file = http::tmp_file(ext);
    std::fs::write(&file, bytes).with_context(|| format!("writing {}", file.display()))?;
    let r = (|| -> Result<String> {
        let u = http::json("higgsfield", "POST", &format!("{HIGGSFIELD}/files/generate-upload-url"), hf_hdr(key), Body::Json(&json!({"content_type": mime})), cancel)?;
        let upload = u["upload_url"].as_str().ok_or_else(|| anyhow!("Higgsfield returned no upload_url"))?;
        let public = u["public_url"].as_str().ok_or_else(|| anyhow!("Higgsfield returned no public_url"))?.to_string();
        let mut headers: Vec<String> = u["upload_headers"].as_object().map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|v| format!("{k}: {v}"))).collect()).unwrap_or_default();
        if !headers.iter().any(|h| h.to_ascii_lowercase().starts_with("content-type:")) {
            headers.push(format!("Content-Type: {mime}"));
        }
        let call = http::Call { provider: "higgsfield", method: "PUT", url: upload.to_string(), headers, body: Body::Raw(file.clone()), timeout: 300 };
        let r = http::send(&call, None, cancel, &|_| {})?;
        if !r.ok() {
            return Err(http::api_error("higgsfield", &r).into());
        }
        Ok(public)
    })();
    let _ = std::fs::remove_file(&file);
    r.context("uploading the frame to Higgsfield")
}

/// A model by id ("runway/veo3.1"), short id ("veo3.1"), label or provider model name;
/// `provider` (or one with a key) decides between two with the same name.
fn find(name: &str, provider: Option<&str>) -> Option<&'static Model> {
    let n = name.trim().to_ascii_lowercase();
    let short = |m: &Model| m.id.split('/').nth(1).unwrap_or("").to_string();
    let hits: Vec<&'static Model> = MODELS
        .iter()
        .filter(|m| m.id == n || short(m) == n || m.label.to_ascii_lowercase() == n || m.t2v.is_some_and(|t| !t.is_empty() && t.to_ascii_lowercase() == n) || short(m).replace(['-', '_', '.'], "") == n.replace(['-', '_', '.', ' '], ""))
        .collect();
    let prov = provider.and_then(keys::provider).map(|p| p.id);
    hits.iter().find(|m| Some(m.provider) == prov).or_else(|| hits.iter().find(|m| keys::key(m.provider).is_some())).or(hits.first()).copied()
}

/// The model a request names, or the default of a connected provider.
fn model_of(a: &Value) -> Result<&'static Model> {
    let provider = str_of(a, "provider");
    match str_of(a, "model") {
        Some(m) => find(m, provider).ok_or_else(|| anyhow!("unknown video model '{m}': video action=models lists them")),
        None => {
            let want = provider.and_then(keys::provider).map(|p| p.id);
            let pick = |id: &str| MODELS.iter().find(|m| m.provider == id);
            let found = match want {
                Some(p) => pick(p),
                None => ["runway", "fal", "higgsfield"].iter().find(|p| keys::key(p).is_some()).and_then(|p| pick(p)).or(MODELS.first()),
            };
            found.ok_or_else(|| anyhow!("no video models"))
        }
    }
}

/// The allowed seconds closest to the request (and whether it had to change).
fn pick_dur(m: &Model, want: Option<f64>) -> (u32, bool) {
    let Some(w) = want.filter(|w| w.is_finite() && *w > 0.0) else { return (m.dur_default, false) };
    let d = if m.steps.is_empty() {
        (w.round() as u32).clamp(m.durs.0, m.durs.1)
    } else {
        *m.steps.iter().min_by(|a, b| ((**a as f64) - w).abs().total_cmp(&((**b as f64) - w).abs())).unwrap_or(&m.dur_default)
    };
    (d, (d as f64 - w).abs() > 0.01)
}

/// The size for an aspect and resolution: the exact one, else the first of the aspect.
fn pick_size(m: &Model, aspect: Option<&str>, res: Option<&str>) -> Result<(&'static str, &'static str, &'static str)> {
    if m.sizes.is_empty() {
        // any fal endpoint: its size settings travel in params
        return Ok(("", "", ""));
    }
    let norm = |s: &str| s.trim().to_ascii_lowercase().replace(['x', '/'], ":");
    let aspect = aspect.map(norm).map(|a| match a.as_str() {
        "landscape" | "wide" | "horizontal" => "16:9".to_string(),
        "portrait" | "vertical" | "tall" | "reels" | "tiktok" | "shorts" => "9:16".to_string(),
        "square" => "1:1".to_string(),
        _ => a,
    });
    let res = res.map(|r| r.trim().to_ascii_lowercase());
    let of_aspect: Vec<&(&str, &str, &str)> = m.sizes.iter().filter(|s| aspect.as_deref().map_or(true, |a| s.0 == a)).collect();
    if of_aspect.is_empty() {
        let mut all: Vec<&str> = m.sizes.iter().map(|s| s.0).collect();
        all.dedup();
        bail!("{} makes {} videos, not {}", m.label, all.join(", "), aspect.unwrap_or_default());
    }
    let hit = match &res {
        Some(r) => of_aspect.iter().find(|s| s.1 == r.as_str()).copied().ok_or_else(|| {
            let rs: Vec<&str> = of_aspect.iter().map(|s| s.1).collect();
            anyhow!("{} makes {} at {}, not {r}", m.label, of_aspect[0].0, rs.join(", "))
        })?,
        None => of_aspect[0],
    };
    Ok(*hit)
}

fn expected_secs(m: &Model, dur: u32, res: &str) -> f64 {
    let r = match res {
        "4k" => 2.0,
        "1080p" | "2k" => 1.25,
        _ => 1.0,
    };
    m.secs * (dur as f64 / m.dur_default.max(1) as f64).sqrt().clamp(0.6, 2.5) * r
}

/// Progress from the elapsed time and the usual time: linear to 85% at the usual time, then
/// creeping towards 95%; the seconds left while it is still within the usual time.
fn estimate(el: f64, expected: f64) -> (f64, Option<f64>) {
    let r = el / expected.max(5.0);
    if r <= 1.0 {
        (0.85 * r, Some(expected - el))
    } else {
        (0.85 + 0.10 * (1.0 - (-(r - 1.0) * 1.5).exp()), None)
    }
}

// ---------------------------------------------------------------- first and last frames

fn mime_of(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// A frame of a video, or an image re-encoded, as JPEG no larger than 1920 px.
fn ffmpeg_jpeg(eng: &Engine, src: &str, t: f64, big: bool) -> Result<Vec<u8>> {
    let out = http::tmp_file("jpg");
    let ts = format!("{:.3}", t.max(0.0));
    let mut args: Vec<String> = ["-y", "-ss", ts.as_str(), "-i", src, "-frames:v", "1"].iter().map(|s| s.to_string()).collect();
    if big {
        args.extend(["-vf", "scale=w=1920:h=1920:force_original_aspect_ratio=decrease:force_divisible_by=2"].iter().map(|s| s.to_string()));
    }
    args.extend(["-q:v", "3"].iter().map(|s| s.to_string()));
    args.push(out.to_string_lossy().to_string());
    let r = eng.ff.run_capture(&args).context("reading the frame");
    let bytes = std::fs::read(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    r?;
    if bytes.is_empty() {
        bail!("no frame at {t:.2} s of {src}");
    }
    Ok(bytes)
}

/// A still for the model as a data URI, and what it was: `frame` (the Program monitor at `t` or
/// the playhead), a project media id (a video's frame at `t`, default its first), or a file.
fn still(eng: &Engine, src: &str, t: Option<f64>) -> Result<(String, String)> {
    let s = src.trim();
    let jpeg = |b: Vec<u8>| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b));
    if matches!(s.to_ascii_lowercase().as_str(), "frame" | "program" | "playhead" | "timeline" | "current") {
        let t = t.or(eng.ui().playhead).unwrap_or(0.0).max(0.0);
        let (cw, ch) = {
            let st = eng.st.read();
            st.p.comp(&st.active).map(|c| (c.w, c.h)).unwrap_or((1920, 1080))
        };
        let long = cw.max(ch).max(1);
        let width = if long > 1920 { cw * 1920 / long } else { cw };
        let pm = eng.frame(None, t, width, false)?;
        let bytes = crate::render::encode::encode_jpeg(&pm, 92)?;
        return Ok((jpeg(bytes), format!("the Program monitor at {t:.2} s")));
    }
    let (path, label, is_video, big) = match eng.media(s) {
        Some(m) => {
            use crate::model::project::MediaKind;
            match m.kind {
                MediaKind::Video => (m.path.clone(), format!("{} {}", m.id, m.name), true, m.w.max(m.h) > 1920),
                MediaKind::Image => (m.path.clone(), format!("{} {}", m.id, m.name), false, m.w.max(m.h) > 1920),
                MediaKind::Svg => bail!("{} is an SVG: use a PNG or JPEG image, or place it and use frame (the Program monitor)", m.id),
                MediaKind::Audio => bail!("{} is audio: pick an image or a video for the frame", m.id),
            }
        }
        None => {
            eng.check_path(s)?;
            if !Path::new(s).is_file() {
                bail!("'{s}' is neither frame, a media id (m3) nor a file that exists");
            }
            let m = eng.ff.probe(s).with_context(|| format!("reading {s}"))?;
            let label = Path::new(s).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| s.to_string());
            (s.to_string(), label, mime_of(s).is_none() && m.has_video, m.w.max(m.h) > 1920)
        }
    };
    if is_video {
        let t = t.unwrap_or(0.0);
        return Ok((jpeg(ffmpeg_jpeg(eng, &path, t, big)?), format!("the frame at {t:.2} s of {label}")));
    }
    // an image small enough is sent as it is; others are re-encoded (and scaled down)
    let size = super::file_len(Path::new(&path));
    match mime_of(&path) {
        Some(mime) if !big && size > 0 && size < 3_000_000 => {
            let bytes = std::fs::read(&path).with_context(|| format!("reading {path}"))?;
            Ok((format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)), label))
        }
        _ => Ok((jpeg(ffmpeg_jpeg(eng, &path, 0.0, big)?), label)),
    }
}

// ---------------------------------------------------------------- requests

struct Ask<'a> {
    m: &'static Model,
    prompt: &'a str,
    negative: Option<&'a str>,
    dur: u32,
    size: (&'static str, &'static str, &'static str),
    audio: bool,
    seed: Option<u64>,
    first: Option<&'a str>,
    last: Option<&'a str>,
}

fn body_of(a: &Ask, params: &Value) -> Value {
    let m = a.m;
    let i2v = a.first.is_some();
    let (aspect, res, value) = a.size;
    let mut b = match m.shape {
        Shape::Runway => {
            let mut b = json!({"model": if i2v { m.i2v } else { m.t2v }, "promptText": a.prompt, "duration": a.dur});
            b["ratio"] = json!(if i2v && m.i2v_auto { format!("auto_{res}") } else { value.to_string() });
            if m.sound == Sound::Param {
                b["audio"] = json!(a.audio);
            }
            if let (true, Some(n)) = (m.negative, a.negative) {
                b["negativePrompt"] = json!(n);
            }
            if let Some(f) = a.first {
                let mut imgs = vec![json!({"uri": f, "position": "first"})];
                if let Some(l) = a.last {
                    imgs.push(json!({"uri": l, "position": "last"}));
                }
                b["promptImage"] = Value::Array(imgs);
            }
            b
        }
        Shape::FalVeo => {
            let mut b = json!({"prompt": a.prompt, "aspect_ratio": if i2v { "auto" } else { aspect }, "duration": format!("{}s", a.dur), "resolution": value, "generate_audio": a.audio});
            if let Some(n) = a.negative {
                b["negative_prompt"] = json!(n);
            }
            if let Some(f) = a.first {
                b["image_url"] = json!(f);
            }
            b
        }
        Shape::FalKling3 => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur.to_string(), "generate_audio": a.audio});
            if let Some(n) = a.negative {
                b["negative_prompt"] = json!(n);
            }
            match a.first {
                Some(f) => {
                    b["start_image_url"] = json!(f);
                    if let Some(l) = a.last {
                        b["end_image_url"] = json!(l);
                    }
                }
                None => b["aspect_ratio"] = json!(aspect),
            }
            b
        }
        Shape::FalKling4 => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur, "resolution": value});
            match a.first {
                Some(f) => {
                    b["start_image_url"] = json!(f);
                    if let Some(l) = a.last {
                        b["end_image_url"] = json!(l);
                    }
                }
                None => b["aspect_ratio"] = json!(aspect),
            }
            b
        }
        Shape::FalSeedance => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur.to_string(), "resolution": value, "aspect_ratio": if i2v { "auto" } else { aspect }, "generate_audio": a.audio});
            if let Some(f) = a.first {
                b["image_url"] = json!(f);
                if let Some(l) = a.last {
                    b["end_image_url"] = json!(l);
                }
            }
            b
        }
        Shape::FalMinimax => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur, "resolution": value});
            match a.first {
                Some(f) => {
                    b["image_url"] = json!(f);
                    if let Some(l) = a.last {
                        b["end_image_url"] = json!(l);
                    }
                }
                None => b["aspect_ratio"] = json!(aspect),
            }
            b
        }
        Shape::FalWan => json!({"prompt": a.prompt, "duration": a.dur, "resolution": value, "aspect_ratio": aspect, "audio": a.audio}),
        Shape::FalCustom => {
            let mut b = json!({"prompt": a.prompt});
            if let Some(f) = a.first {
                b["image_url"] = json!(f);
            }
            b
        }
        Shape::HfCinema => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur, "resolution": value, "aspect_ratio": aspect, "generate_audio": a.audio});
            // Cinema Studio takes images as references, not as the first frame
            if let Some(f) = a.first {
                b["image_urls"] = json!([f]);
            }
            b
        }
        Shape::HfKling3 => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur, "sound": if a.audio { "on" } else { "off" }});
            match a.first {
                Some(f) => {
                    b["image_url"] = json!(f);
                    if let Some(l) = a.last {
                        b["last_image_url"] = json!(l);
                    }
                }
                None => b["aspect_ratio"] = json!(aspect),
            }
            b
        }
        Shape::HfSeedance => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur, "resolution": value, "generate_audio": a.audio});
            match a.first {
                Some(f) => {
                    b["image_url"] = json!(f);
                    if let Some(l) = a.last {
                        b["end_image_url"] = json!(l);
                    }
                }
                None => b["aspect_ratio"] = json!(aspect),
            }
            b
        }
        Shape::HfMinimax => {
            let mut b = json!({"prompt": a.prompt, "duration": a.dur, "resolution": value, "aspect_ratio": if i2v { "auto" } else { aspect }});
            if let Some(f) = a.first {
                b["image_url"] = json!(f);
                if let Some(l) = a.last {
                    b["end_image_url"] = json!(l);
                }
            }
            b
        }
    };
    if let (true, Some(s)) = (m.seed, a.seed) {
        b["seed"] = json!(s);
    }
    // advanced: the person's own fields go last and win
    if let (Some(o), Some(extra)) = (b.as_object_mut(), params.as_object()) {
        for (k, v) in extra {
            if v.is_null() {
                o.remove(k);
            } else {
                o.insert(k.clone(), v.clone());
            }
        }
    }
    b
}

/// fal endpoint ids: owner/app[/path...], letters, digits, - _ .
fn valid_endpoint(e: &str) -> bool {
    let parts: Vec<&str> = e.split('/').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && *p != "..")
}

/// Sends the job; the handle that finds it again (also after a restart).
fn start(m: &Model, key: &str, body: &Value, endpoint: &str, i2v: bool, cancel: &AtomicBool) -> Result<Value> {
    if m.provider == "runway" {
        let path = if i2v { "image_to_video" } else { "text_to_video" };
        let r = http::json("runway", "POST", &format!("{RUNWAY}/v1/{path}"), runway_hdr(key), Body::Json(body), cancel)?;
        let id = r["id"].as_str().ok_or_else(|| anyhow!("Runway returned no task id"))?;
        return Ok(json!({"provider": "runway", "id": id, "credits": r["estimatedCost"]["credits"]}));
    }
    if m.provider == "higgsfield" {
        let r = http::json("higgsfield", "POST", &format!("{HIGGSFIELD}/{endpoint}"), hf_hdr(key), Body::Json(body), cancel)?;
        let status = r["status_url"].as_str().ok_or_else(|| anyhow!("Higgsfield returned no status_url"))?;
        return Ok(json!({"provider": "higgsfield", "id": r["request_id"], "status_url": status, "cancel_url": r["cancel_url"]}));
    }
    let r = http::json("fal", "POST", &format!("{FAL}/{endpoint}"), fal_hdr(key), Body::Json(body), cancel)?;
    let status = r["status_url"].as_str().ok_or_else(|| anyhow!("fal.ai returned no status_url"))?;
    Ok(json!({"provider": "fal", "id": r["request_id"], "status_url": status, "response_url": r["response_url"], "cancel_url": r["cancel_url"], "queue": r["queue_position"]}))
}

enum Poll {
    /// queued (true) or generating, what the provider says, its fraction, the credits
    Waiting(bool, String, Option<f64>, Option<String>),
    Done(String, Value),
    Failed(String),
}

fn poll_runway(key: &str, h: &Value, cancel: &AtomicBool) -> Result<Poll> {
    let id = h["id"].as_str().ok_or_else(|| anyhow!("no Runway task id"))?;
    let t = http::json("runway", "GET", &format!("{RUNWAY}/v1/tasks/{id}"), runway_hdr(key), Body::None, cancel)?;
    let credits = t["cost"]["credits"].as_f64().or(t["estimatedCost"]["credits"].as_f64()).or(h["credits"].as_f64()).map(|c| format!("{c:.0} credits"));
    Ok(match t["status"].as_str().unwrap_or("") {
        "SUCCEEDED" => match t["output"][0].as_str() {
            Some(u) => Poll::Done(u.to_string(), json!({"credits": t["cost"]["credits"]})),
            None => Poll::Failed("Runway finished but returned no video".into()),
        },
        "FAILED" => Poll::Failed(format!(
            "Runway couldn't make the video: {}{}",
            t["failure"].as_str().unwrap_or("it failed"),
            t["failureCode"].as_str().map(|c| format!(" [{c}]")).unwrap_or_default()
        )),
        "CANCELLED" => Poll::Failed("cancelled".into()),
        "THROTTLED" => Poll::Waiting(true, "Waiting at Runway: your plan's limit of jobs at once is reached".into(), None, credits),
        "PENDING" => Poll::Waiting(true, "Queued at Runway".into(), None, credits),
        _ => Poll::Waiting(false, "Runway is generating".into(), t["progress"].as_f64(), credits),
    })
}

/// The video's URL in a fal result, wherever the model puts it.
fn find_video_url(v: &Value) -> Option<String> {
    for p in ["/video/url", "/videos/0/url", "/video_url", "/output/url", "/output/video/url", "/url"] {
        if let Some(u) = v.pointer(p).and_then(Value::as_str).filter(|u| u.starts_with("http")) {
            return Some(u.to_string());
        }
    }
    fn walk(v: &Value) -> Option<String> {
        match v {
            Value::String(s) if s.starts_with("http") && [".mp4", ".mov", ".webm", ".m4v"].iter().any(|e| s.to_ascii_lowercase().split('?').next().unwrap_or("").ends_with(e)) => Some(s.clone()),
            Value::Array(a) => a.iter().find_map(walk),
            Value::Object(o) => o.values().find_map(walk),
            _ => None,
        }
    }
    walk(v)
}

fn poll_fal(key: &str, h: &Value, cancel: &AtomicBool) -> Result<Poll> {
    let su = h["status_url"].as_str().ok_or_else(|| anyhow!("no fal.ai status_url"))?;
    let url = if su.contains('?') { format!("{su}&logs=1") } else { format!("{su}?logs=1") };
    let s = http::json("fal", "GET", &url, fal_hdr(key), Body::None, cancel)?;
    Ok(match s["status"].as_str().unwrap_or("") {
        "IN_QUEUE" => Poll::Waiting(
            true,
            match s["queue_position"].as_u64() {
                Some(n) if n > 0 => format!("In fal.ai's queue, position {n}"),
                _ => "In fal.ai's queue, next up".into(),
            },
            None,
            None,
        ),
        "IN_PROGRESS" => {
            let log = s["logs"].as_array().and_then(|l| l.iter().rev().find_map(|e| e["message"].as_str().map(str::trim).filter(|m| !m.is_empty()))).map(|m| first_words(m, 10));
            Poll::Waiting(false, log.map(|l| format!("fal.ai is generating: {l}")).unwrap_or_else(|| "fal.ai is generating".into()), None, None)
        }
        "COMPLETED" => {
            if let Some(err) = s["error"].as_str().filter(|e| !e.trim().is_empty()) {
                return Ok(Poll::Failed(format!("fal.ai couldn't make the video: {err}{}", s["error_type"].as_str().map(|t| format!(" [{t}]")).unwrap_or_default())));
            }
            let ru = h["response_url"].as_str().ok_or_else(|| anyhow!("no fal.ai response_url"))?;
            let out = http::json("fal", "GET", ru, fal_hdr(key), Body::None, cancel)?;
            match find_video_url(&out) {
                Some(u) => Poll::Done(u, json!({"seed": out["seed"], "prompt": out["expanded_prompt"].as_str().or(out["actual_prompt"].as_str())})),
                None => Poll::Failed("fal.ai finished but returned no video".into()),
            }
        }
        other => Poll::Waiting(true, format!("fal.ai: {}", other.to_ascii_lowercase().replace('_', " ")), None, None),
    })
}

fn poll_higgsfield(key: &str, h: &Value, cancel: &AtomicBool) -> Result<Poll> {
    let su = h["status_url"].as_str().ok_or_else(|| anyhow!("no Higgsfield status_url"))?;
    let s = http::json("higgsfield", "GET", su, hf_hdr(key), Body::None, cancel)?;
    Ok(match s["status"].as_str().unwrap_or("") {
        "queued" => Poll::Waiting(true, "Queued at Higgsfield".into(), None, None),
        "in_progress" => Poll::Waiting(false, "Higgsfield is generating".into(), None, None),
        "completed" => match find_video_url(&s) {
            Some(u) => Poll::Done(u, json!({})),
            None => Poll::Failed("Higgsfield finished but returned no video".into()),
        },
        "nsfw" => Poll::Failed("Higgsfield's content moderation (nsfw) rejected the input or the result".into()),
        "canceled" | "cancelled" => Poll::Failed("cancelled".into()),
        "failed" => Poll::Failed(format!(
            "Higgsfield couldn't make the video: {}",
            s["error"].as_str().map(str::to_string).or_else(|| s["error"]["message"].as_str().map(str::to_string)).unwrap_or_else(|| "it failed".into())
        )),
        other => Poll::Waiting(true, format!("Higgsfield: {}", other.replace('_', " ")), None, None),
    })
}

/// Stops a job at the provider (best effort: it may be about to finish).
fn cancel_remote(key: &str, h: &Value) {
    let c = AtomicBool::new(false);
    match h["provider"].as_str() {
        Some("runway") => {
            if let Some(id) = h["id"].as_str() {
                let _ = http::json("runway", "DELETE", &format!("{RUNWAY}/v1/tasks/{id}"), runway_hdr(key), Body::None, &c);
            }
        }
        Some("fal") => {
            if let Some(u) = h["cancel_url"].as_str() {
                let _ = http::json("fal", "PUT", u, fal_hdr(key), Body::None, &c);
            }
        }
        // only possible while queued; once generating, it finishes (and is billed)
        Some("higgsfield") => {
            if let Some(u) = h["cancel_url"].as_str() {
                let _ = http::json("higgsfield", "POST", u, hf_hdr(key), Body::None, &c);
            }
        }
        _ => {}
    }
}

/// Follows a job until it has a video URL, updating the task.
fn follow(m: &Model, key: &str, h: &Value, dur: u32, res: &str, p: &Progress) -> Result<(String, Value)> {
    let expected = expected_secs(m, dur, res);
    let name = keys::name(m.provider);
    let t0 = Instant::now();
    let mut gen_start: Option<Instant> = None;
    let mut fails = 0;
    let every = Duration::from_secs(if m.provider == "runway" { 5 } else { 3 });
    loop {
        let r = match m.provider {
            "runway" => poll_runway(key, h, &p.cancel),
            "higgsfield" => poll_higgsfield(key, h, &p.cancel),
            _ => poll_fal(key, h, &p.cancel),
        };
        match r {
            Ok(Poll::Done(url, extra)) => return Ok((url, extra)),
            Ok(Poll::Failed(why)) => bail!("{why}"),
            Ok(Poll::Waiting(queued, note, frac, credits)) => {
                fails = 0;
                if let Some(c) = credits {
                    p.cost(c);
                }
                let total = tasks::clock(t0.elapsed().as_secs_f64());
                if queued {
                    p.estimate(0.04, None, format!("{note} · {total}"));
                } else {
                    let since = gen_start.get_or_insert_with(Instant::now).elapsed().as_secs_f64();
                    match frac.filter(|f| *f > 0.0) {
                        Some(f) => {
                            let eta = (f > 0.03).then(|| since * (1.0 - f) / f);
                            p.real(0.05 + 0.85 * f, eta, format!("{note} · {}% · {total}", (f * 100.0).round()));
                        }
                        None => {
                            let (f, eta) = estimate(since, expected);
                            p.estimate(0.05 + 0.85 * f, eta, format!("{note} · {total}"));
                        }
                    }
                }
            }
            Err(e) => {
                if p.cancelled() {
                    return Err(e);
                }
                // a rejected request is final; a dropped connection is tried again a few times
                let final_ = e.downcast_ref::<http::ApiError>().is_some_and(|a| (400..500).contains(&a.status) && a.status != 429 && a.status != 408);
                fails += 1;
                if final_ || fails > 6 {
                    return Err(e);
                }
                p.set(None, format!("{name} didn't answer; trying again ({fails} of 6)"));
            }
        }
        p.sleep(every)?;
    }
}

// ---------------------------------------------------------------- jobs that outlive MoodCut

fn jobs_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn jobs_file() -> PathBuf {
    crate::config::ensure_data_dir().join("ai-video-jobs.json")
}

fn jobs_read() -> Map<String, Value> {
    std::fs::read_to_string(jobs_file()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn jobs_edit(f: impl FnOnce(&mut Map<String, Value>)) {
    let _g = jobs_lock().lock();
    let mut m = jobs_read();
    f(&mut m);
    let file = jobs_file();
    if m.is_empty() {
        let _ = std::fs::remove_file(file);
    } else if let Ok(t) = serde_json::to_string_pretty(&Value::Object(m)) {
        let _ = std::fs::write(file, t);
    }
}

/// Picks up jobs that were still running at the provider when MoodCut closed (once per run).
fn resume_pending(eng: &Arc<Engine>) {
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_err() {
        return;
    }
    for (job, e) in jobs_read() {
        let req = e["req"].clone();
        let created = e["created"].as_u64().unwrap_or(0);
        // providers keep results for a day or two
        if !req["resume"].is_object() || now_secs().saturating_sub(created) > 36 * 3600 {
            jobs_edit(|m| {
                m.remove(&job);
            });
            continue;
        }
        let Ok(m) = model_of(&req) else { continue };
        let label = format!("{} (resumed): \"{}\"", m.label, first_words(req["prompt"].as_str().unwrap_or(""), 8));
        tasks::submit(eng, "video", "video", m.provider, label, req, video_work);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A name for a job in the jobs file: time, process and a counter.
fn job_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{:x}-{:x}-{:x}-{}", now_secs(), nanos, std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

// ---------------------------------------------------------------- the job

fn video_work(eng: &Arc<Engine>, req: &Value, p: &Progress) -> Result<Value> {
    let m = model_of(req)?;
    let key = keys::need(m.provider)?;
    let name = keys::name(m.provider);
    let prompt = str_of(req, "prompt").ok_or_else(|| anyhow!("describe the shot in the prompt"))?.to_string();
    let (dur, _) = pick_dur(m, req["duration"].as_f64());
    let size = pick_size(m, str_of(req, "aspect"), str_of(req, "resolution"))?;
    let job = str_of(req, "job").unwrap_or("job").to_string();
    let handle = if req["resume"].is_object() {
        p.set(None, format!("checking on the job at {name}"));
        req["resume"].clone()
    } else {
        p.set(Some(0.01), "preparing");
        let t_of = |k: &str| req[k].as_f64().filter(|t| t.is_finite());
        let first = match str_of(req, "image").filter(|s| !s.eq_ignore_ascii_case("none")) {
            Some(src) => {
                p.set(Some(0.01), "reading the first frame");
                Some(still(eng, src, t_of("image_t"))?)
            }
            None => None,
        };
        let last = match str_of(req, "end_image").filter(|s| !s.eq_ignore_ascii_case("none")) {
            Some(src) => {
                if !m.last_frame {
                    bail!("{} can't take a last frame: pick Veo 3.1, Kling, Seedance, Hailuo or Gemini Omni for that", m.label);
                }
                if first.is_none() {
                    bail!("a last frame needs a first frame too (image=)");
                }
                p.set(Some(0.02), "reading the last frame");
                Some(still(eng, src, t_of("end_image_t"))?)
            }
            None => None,
        };
        // Higgsfield wants URLs: upload the frames first
        let (first, last) = if m.provider == "higgsfield" {
            let up = |x: Option<(String, String)>, what: &str| -> Result<Option<(String, String)>> {
                match x {
                    Some((data, label)) => {
                        p.set(Some(0.025), format!("uploading the {what} frame to Higgsfield"));
                        Ok(Some((hf_upload(&key, &data, &p.cancel)?, label)))
                    }
                    None => Ok(None),
                }
            };
            (up(first, "first")?, up(last, "last")?)
        } else {
            (first, last)
        };
        let endpoint = if m.shape == Shape::FalCustom {
            let e = str_of(req, "endpoint").ok_or_else(|| anyhow!("give the fal.ai endpoint id, like fal-ai/kling-video/v3/pro/text-to-video"))?;
            if !valid_endpoint(e) {
                bail!("'{e}' isn't a fal.ai endpoint id (owner/app/path)");
            }
            e.to_string()
        } else if first.is_some() {
            m.i2v.ok_or_else(|| anyhow!("{} makes videos from text only: pick another model to start from an image", m.label))?.to_string()
        } else {
            m.t2v.ok_or_else(|| anyhow!("{} needs a first frame (image=)", m.label))?.to_string()
        };
        let ask = Ask {
            m,
            prompt: &prompt,
            negative: str_of(req, "negative"),
            dur,
            size,
            audio: req["audio"] != false,
            seed: req["seed"].as_u64(),
            first: first.as_ref().map(|f| f.0.as_str()),
            last: last.as_ref().map(|f| f.0.as_str()),
        };
        let body = body_of(&ask, &req["params"]);
        p.set(Some(0.03), format!("sending to {name}"));
        let mut h = start(m, &key, &body, &endpoint, first.is_some(), &p.cancel)?;
        h["i2v"] = json!(first.is_some());
        if let Some(c) = h["credits"].as_f64() {
            p.cost(format!("about {c:.0} credits"));
        }
        // remember it: the job keeps running at the provider if MoodCut closes now
        let mut saved = req.clone();
        saved["resume"] = h.clone();
        let entry = json!({"req": saved, "created": now_secs()});
        jobs_edit(|j| {
            j.insert(job.clone(), entry);
        });
        h
    };
    let (url, extra) = match follow(m, &key, &handle, dur, size.1, p) {
        Ok(x) => x,
        Err(e) => {
            if p.cancelled() {
                p.set(None, format!("stopping the job at {name}"));
                cancel_remote(&key, &handle);
            }
            jobs_edit(|j| {
                j.remove(&job);
            });
            return Err(e);
        }
    };
    // download
    let dir = out_dir(eng, "ai-video")?;
    let path = url.split('?').next().unwrap_or("").to_ascii_lowercase();
    let ext = if path.ends_with(".mov") {
        "mov"
    } else if path.ends_with(".webm") {
        "webm"
    } else if path.ends_with(".m4v") {
        "m4v"
    } else {
        "mp4"
    };
    let stem = format!("{}-{}", slug(&first_words(&prompt, 6), 40), slug(m.label, 16));
    let file = unique_path(&dir, &stem, ext);
    p.set(Some(0.91), "downloading the video");
    let got = http::download(m.provider, &url, &file, &p.cancel, &|n| p.set(Some(0.92), format!("downloading the video · {}", crate::library::fmt_bytes(n))));
    jobs_edit(|j| {
        j.remove(&job);
    });
    got?;
    // import (and place) as one undoable edit
    p.set(Some(0.97), "adding to the project");
    let q = crate::ops::lex::quote;
    let f = file.to_string_lossy().to_string();
    let label = format!("{}: {}", m.label, first_words(&prompt, 6));
    let mut lines = vec![format!("import {} tag=ai-video as={}", q(&f), q(&label))];
    let seed = req["seed"].as_u64().map(|s| json!(s)).unwrap_or(extra["seed"].clone());
    let note: String = format!(
        "AI video ({} by {}, via {name}, {dur} s, {} {}{}): {prompt}",
        m.label,
        m.maker,
        size.0,
        size.1,
        seed.as_u64().map(|s| format!(", seed {s}")).unwrap_or_default()
    )
    .chars()
    .take(900)
    .collect();
    lines.push(format!("note $ {}", q(&note)));
    let at = at_of(eng, &req["at"]);
    if let Some(t) = at {
        let track = str_of(req, "track").filter(|t| (2..=3).contains(&t.len()) && t.starts_with(['V', 'v']) && t[1..].chars().all(|c| c.is_ascii_digit())).map(|t| format!(" {}", t.to_ascii_uppercase())).unwrap_or_default();
        lines.push(format!("add {}{track} at={t:.3}", q(&f)));
    }
    eng.edit(&lines.join("\n"), "gui")?;
    let media = media_for(eng, &file);
    let clip = match (&media, at) {
        (Some(mid), Some(_)) => clip_for(eng, mid),
        _ => None,
    };
    let secs = media.as_deref().and_then(|id| eng.media(id)).map(|m| m.dur).unwrap_or(dur as f64);
    let credits = extra["credits"].as_f64().or(handle["credits"].as_f64());
    if let Some(c) = credits {
        p.cost(format!("{c:.0} credits"));
    }
    Ok(json!({
        "media": media, "clip": clip, "file": file, "model": m.id, "provider": m.provider, "dur": (secs * 100.0).round() / 100.0,
        "credits": credits, "seed": seed, "expanded_prompt": extra["prompt"],
        "text": format!("{:.1} s video from {} in the project{}{}", secs, m.label, media.as_ref().map(|i| format!(" as {i}")).unwrap_or_default(), clip.as_ref().map(|c| format!(", placed as {c}")).unwrap_or_default()),
    }))
}

// ---------------------------------------------------------------- the editor's API

fn runway_credits(force: bool) -> Option<f64> {
    static C: OnceLock<Mutex<Option<(Instant, Option<f64>)>>> = OnceLock::new();
    let cell = C.get_or_init(|| Mutex::new(None));
    if let Some((at, v)) = *cell.lock() {
        if !force && at.elapsed() < Duration::from_secs(60) {
            return v;
        }
    }
    let v = keys::key("runway").and_then(|(k, _)| http::json("runway", "GET", &format!("{RUNWAY}/v1/organization"), runway_hdr(&k), Body::None, &AtomicBool::new(false)).ok()).and_then(|o| o["creditBalance"].as_f64());
    *cell.lock() = Some((Instant::now(), v));
    v
}

/// A key works (and for Runway, the credits left).
pub fn test_key(provider: &str) -> Result<String> {
    let key = keys::need(provider)?;
    let c = AtomicBool::new(false);
    if provider == "runway" {
        let o = http::json("runway", "GET", &format!("{RUNWAY}/v1/organization"), runway_hdr(&key), Body::None, &c)?;
        let _ = runway_credits(true);
        return Ok(format!("Connected: {} credits left.", o["creditBalance"].as_f64().map(|c| format!("{c:.0}")).unwrap_or_else(|| "?".into())));
    }
    if provider == "higgsfield" {
        let r = http::send(
            &http::Call { provider: "higgsfield", method: "GET", url: format!("{HIGGSFIELD}/requests/00000000-0000-0000-0000-000000000000/status"), headers: hf_hdr(&key), body: Body::None, timeout: 60 },
            None,
            &c,
            &|_| {},
        )?;
        if r.status == 401 || r.status == 403 {
            return Err(http::api_error("higgsfield", &r).into());
        }
        return Ok("Key accepted by Higgsfield. Its credits show at console.higgsfield.ai.".into());
    }
    // fal has no balance endpoint: the status of a request that doesn't exist proves the key
    let r = http::send(
        &http::Call { provider: "fal", method: "GET", url: format!("{FAL}/fal-ai/veo3.1/requests/00000000-0000-0000-0000-000000000000/status"), headers: fal_hdr(&key), body: Body::None, timeout: 60 },
        None,
        &c,
        &|_| {},
    )?;
    if r.status == 401 || r.status == 403 {
        return Err(http::api_error("fal", &r).into());
    }
    Ok("Key saved. fal.ai shows no balance through its API: the first video confirms the key.".into())
}

/// The providers with a key (read once: the editor asks every second while a job runs).
fn connected() -> Vec<&'static str> {
    keys::PROVIDERS.iter().filter(|p| keys::key(p.id).is_some()).map(|p| p.id).collect()
}

fn catalog() -> Value {
    let conn = connected();
    Value::Array(
        MODELS
            .iter()
            .map(|m| {
                let durs: Vec<u32> = if m.steps.is_empty() { (m.durs.0..=m.durs.1).collect() } else { m.steps.to_vec() };
                json!({
                    "id": m.id, "provider": m.provider, "label": m.label, "maker": m.maker, "blurb": m.blurb,
                    "t2v": m.t2v.is_some(), "i2v": m.i2v.is_some(), "custom": m.shape == Shape::FalCustom,
                    "durs": durs, "dur_default": m.dur_default,
                    "sizes": m.sizes.iter().map(|s| json!({"aspect": s.0, "res": s.1})).collect::<Vec<_>>(),
                    "sound": match m.sound { Sound::Param => "param", Sound::Always => "always", Sound::None => "none" },
                    "negative": m.negative, "seed": m.seed, "last_frame": m.last_frame, "i2v_auto": m.i2v_auto,
                    "max_prompt": m.max_prompt, "secs": m.secs,
                    "connected": conn.contains(&m.provider),
                })
            })
            .collect(),
    )
}

/// Checks a request and queues it; problems a person can fix at once are errors right away.
fn submit(eng: &Arc<Engine>, a: &Value) -> Result<u64> {
    let m = model_of(a)?;
    keys::need(m.provider)?;
    let prompt = str_of(a, "prompt").ok_or_else(|| anyhow!("describe the shot in the prompt"))?;
    let n = prompt.chars().count();
    if n > m.max_prompt {
        bail!("the prompt has {n} characters; {} takes up to {}", m.label, m.max_prompt);
    }
    pick_size(m, str_of(a, "aspect"), str_of(a, "resolution"))?;
    if let Some(src) = str_of(a, "image").filter(|s| !s.eq_ignore_ascii_case("none")) {
        if m.i2v.is_none() {
            bail!("{} makes videos from text only: pick another model to start from an image", m.label);
        }
        let s = src.to_ascii_lowercase();
        if !matches!(s.as_str(), "frame" | "program" | "playhead" | "timeline" | "current") && eng.media(src).is_none() && !Path::new(src).is_file() {
            bail!("image='{src}' is neither frame, a media id nor a file");
        }
    } else if m.t2v.is_none() {
        bail!("{} needs a first frame: image=frame (the Program monitor), a media id or a file", m.label);
    }
    if m.shape == Shape::FalCustom && !str_of(a, "endpoint").is_some_and(valid_endpoint) {
        bail!("give the fal.ai endpoint id, like fal-ai/kling-video/v3/pro/text-to-video");
    }
    let mut req = a.clone();
    req["model"] = json!(m.id);
    req["job"] = json!(job_id());
    if let Some(o) = req.as_object_mut() {
        o.remove("resume");
        o.remove("wait");
    }
    let label = format!("{}: \"{}\"", m.label, first_words(prompt, 8));
    Ok(tasks::submit(eng, "video", "video", m.provider, label, req, video_work))
}

/// `GET/POST /api/video`: `{action: status|generate|retry|cancel|clear}`.
pub fn api(eng: &Arc<Engine>, a: &Value) -> Result<Value> {
    resume_pending(eng);
    let mut submitted = Value::Null;
    match a["action"].as_str().unwrap_or("status") {
        "status" => {}
        "generate" => submitted = json!(submit(eng, a)?),
        "retry" => submitted = json!(tasks::retry(eng, "video", a["id"].as_u64().ok_or_else(|| anyhow!("which task?"))?)?),
        "cancel" => {
            tasks::cancel("video", a["id"].as_u64());
        }
        "clear" => tasks::clear("video"),
        other => bail!("unknown action '{other}'"),
    }
    let providers: Vec<Value> = keys::status().as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["id"] == "runway" || p["id"] == "fal" || p["id"] == "higgsfield").collect();
    Ok(json!({
        "ok": true,
        "providers": providers,
        "credits": {"runway": if keys::key("runway").is_some() { runway_credits(false) } else { None }},
        "models": catalog(),
        "tasks": tasks::list("video"),
        "submitted": submitted,
    }))
}

// ---------------------------------------------------------------- the MCP tool

fn models_text() -> String {
    let mut out = vec!["Video models (model=<id>; durations in seconds; sizes as aspect resolution):".to_string()];
    let conn = connected();
    for m in MODELS {
        let durs = if m.steps.is_empty() { format!("{}-{}", m.durs.0, m.durs.1) } else { m.steps.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("/") };
        let mut sizes: Vec<String> = m.sizes.iter().map(|s| format!("{} {}", s.0, s.1)).collect();
        sizes.dedup();
        let mut can = vec![];
        if m.t2v.is_some() {
            can.push("text");
        }
        if m.i2v.is_some() {
            can.push("image=first frame");
        }
        if m.last_frame {
            can.push("end_image=last frame");
        }
        match m.sound {
            Sound::Param => can.push("audio=true|false"),
            Sound::Always => can.push("always with sound"),
            Sound::None => can.push("silent"),
        }
        if m.negative {
            can.push("negative=");
        }
        if m.seed {
            can.push("seed=");
        }
        out.push(format!(
            "  {}{} — {} by {}: {}. {} s (default {}); {}; {}{}",
            m.id,
            if conn.contains(&m.provider) { "" } else { " [no key]" },
            m.label,
            m.maker,
            m.blurb,
            durs,
            m.dur_default,
            if sizes.is_empty() { "any size via params".to_string() } else { sizes.join(", ") },
            can.join(", "),
            if m.max_prompt < 5000 { format!("; prompt up to {} characters", m.max_prompt) } else { String::new() }
        ));
    }
    out.join("\n")
}

fn status_text() -> String {
    let mut out = vec![super::keys_text(&["runway", "fal", "higgsfield"])];
    if keys::key("runway").is_some() {
        if let Some(c) = runway_credits(false) {
            out.push(format!("Runway credits left: {c:.0}."));
        }
    }
    let recent = tasks::list("video");
    if !recent.is_empty() {
        out.push("Video jobs:".into());
        for t in recent.iter().rev().take(8) {
            out.push(format!("  {}", tasks::line(t)));
        }
    }
    out.push("video action=models lists the models; video action=generate prompt='...' starts one.".into());
    out.join("\n")
}

fn done_text(t: &tasks::Task) -> Result<String> {
    if t.state == "error" {
        bail!("{}{}", t.error.clone().unwrap_or_else(|| "failed".into()), t.hint.as_ref().map(|h| format!(" {h}")).unwrap_or_default());
    }
    if t.state != "done" {
        return Ok(format!("{}\nStill generating (video takes 1-5 minutes): video action=wait id={} waits again; video action=status shows every job.", tasks::line(t), t.id));
    }
    let r = t.result.clone().unwrap_or(Value::Null);
    let mut s = format!("{} ({})", r["text"].as_str().unwrap_or("done"), tasks::clock(t.secs));
    if let Some(c) = &t.cost {
        s.push_str(&format!(", {c}"));
    }
    if let Some(sd) = r["seed"].as_u64() {
        s.push_str(&format!("\nseed {sd}: the same seed and prompt give a similar take"));
    }
    s.push_str("\nLook before using it: preview media=<id> (or a contact sheet of the clip on the timeline). Generated media is tagged ai-video with the prompt as its note.");
    Ok(s)
}

/// The `video` MCP tool: the same models, jobs and progress as the AI Video panel.
pub fn mcp_tool(eng: &Arc<Engine>, a: &Value) -> Result<String> {
    resume_pending(eng);
    let wait = a["wait"].as_bool().unwrap_or(true);
    // MCP clients give a tool call a few minutes: wait about four, then hand back the job id
    let finish = |id: u64| -> Result<String> {
        if !wait {
            return Ok(format!("video job #{id} started: video action=wait id={id} waits for it; video action=status shows progress"));
        }
        done_text(&tasks::wait(id, Duration::from_secs(240))?)
    };
    match a["action"].as_str().unwrap_or("status") {
        "status" | "jobs" => Ok(status_text()),
        "models" => Ok(models_text()),
        "generate" | "create" | "make" => finish(submit(eng, a)?),
        "wait" => done_text(&tasks::wait(a["id"].as_u64().ok_or_else(|| anyhow!("wait needs id="))?, Duration::from_secs(240))?),
        "retry" => finish(tasks::retry(eng, "video", a["id"].as_u64().ok_or_else(|| anyhow!("retry needs id="))?)?),
        "cancel" => {
            let n = tasks::cancel("video", a["id"].as_u64());
            Ok(if n == 0 { "nothing was running".into() } else { format!("cancelling {n} job{} (also at the provider)", if n == 1 { "" } else { "s" }) })
        }
        other => bail!("unknown action '{other}': status, models, generate, wait, retry, cancel"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask<'a>(m: &'static Model, first: Option<&'a str>, last: Option<&'a str>) -> Ask<'a> {
        Ask { m, prompt: "a fox at dawn", negative: Some("blur"), dur: m.dur_default, size: m.sizes.first().copied().unwrap_or(("16:9", "", "")), audio: true, seed: Some(7), first, last }
    }

    #[test]
    fn every_model_is_consistent() {
        let mut ids = std::collections::HashSet::new();
        for m in MODELS {
            assert!(ids.insert(m.id), "duplicate {}", m.id);
            assert!(m.id.starts_with(&format!("{}/", m.provider)), "{}", m.id);
            assert!(keys::provider(m.provider).is_some(), "{}", m.id);
            assert!(m.durs.0 <= m.dur_default && m.dur_default <= m.durs.1, "{}", m.id);
            assert!(m.steps.is_empty() || m.steps.contains(&m.dur_default), "{}", m.id);
            assert!(m.t2v.is_some() || m.i2v.is_some(), "{}", m.id);
            if m.shape != Shape::FalCustom {
                assert!(!m.sizes.is_empty(), "{}", m.id);
            }
            if m.provider == "runway" {
                for s in m.sizes {
                    let (w, h) = s.2.split_once(':').expect("ratio");
                    let (w, h): (f64, f64) = (w.parse().unwrap(), h.parse().unwrap());
                    let (aw, ah) = s.0.split_once(':').expect("aspect");
                    let (aw, ah): (f64, f64) = (aw.parse().unwrap(), ah.parse().unwrap());
                    assert!((w / h - aw / ah).abs() < 0.03, "{} {:?}", m.id, s);
                }
            }
        }
    }

    #[test]
    fn models_are_found_by_any_name() {
        assert_eq!(find("runway/veo3.1", None).map(|m| m.id), Some("runway/veo3.1"));
        assert_eq!(find("veo3.1", Some("fal")).map(|m| m.id), Some("fal/veo3.1"));
        assert_eq!(find("Gen-4.5", None).map(|m| m.id), Some("runway/gen4.5"));
        assert_eq!(find("kling-v3-pro", None).map(|m| m.id), Some("fal/kling-v3-pro"));
        assert!(find("sora-2", None).is_none());
    }

    #[test]
    fn durations_and_sizes_snap_to_what_a_model_takes() {
        let veo = find("runway/veo3.1", None).unwrap();
        assert_eq!(pick_dur(veo, Some(5.0)), (4, true));
        assert_eq!(pick_dur(veo, Some(7.0)), (6, true));
        assert_eq!(pick_dur(veo, None), (8, false));
        let gen = find("runway/gen4.5", None).unwrap();
        assert_eq!(pick_dur(gen, Some(30.0)), (10, true));
        assert_eq!(pick_size(veo, Some("vertical"), None).unwrap(), ("9:16", "1080p", "1080:1920"));
        assert_eq!(pick_size(veo, Some("16:9"), Some("720p")).unwrap().2, "1280:720");
        assert!(pick_size(veo, Some("1:1"), None).unwrap_err().to_string().contains("16:9, 9:16"));
        assert!(pick_size(gen, Some("16:9"), Some("4k")).is_err());
    }

    #[test]
    fn requests_follow_each_providers_schema() {
        let veo = find("runway/veo3.1", None).unwrap();
        let b = body_of(&ask(veo, Some("data:image/jpeg;base64,AA"), Some("data:image/jpeg;base64,BB")), &Value::Null);
        assert_eq!(b["model"], "veo3.1");
        assert_eq!(b["ratio"], "1920:1080");
        assert_eq!(b["duration"], 8);
        assert_eq!(b["audio"], true);
        assert_eq!(b["negativePrompt"], "blur");
        assert_eq!(b["promptImage"][1]["position"], "last");
        let gen = find("runway/gen4.5", None).unwrap();
        let b = body_of(&ask(gen, None, None), &json!({"seed": null, "contentModeration": {"publicFigureThreshold": "low"}}));
        assert!(b.get("audio").is_none() && b.get("negativePrompt").is_none() && b.get("seed").is_none());
        assert_eq!(b["contentModeration"]["publicFigureThreshold"], "low");
        let k4 = find("runway/kling_4", None).unwrap();
        assert_eq!(body_of(&ask(k4, Some("data:x"), None), &Value::Null)["ratio"], "auto_1080p");
        let fveo = find("fal/veo3.1", None).unwrap();
        let b = body_of(&ask(fveo, None, None), &Value::Null);
        assert_eq!((b["duration"].as_str(), b["aspect_ratio"].as_str(), b["resolution"].as_str()), (Some("8s"), Some("16:9"), Some("1080p")));
        let k3 = find("fal/kling-v3-pro", None).unwrap();
        let b = body_of(&ask(k3, Some("data:x"), Some("data:y")), &Value::Null);
        assert_eq!((b["start_image_url"].as_str(), b["end_image_url"].as_str(), b.get("aspect_ratio")), (Some("data:x"), Some("data:y"), None));
        assert_eq!(b["duration"], "5");
        let mm = find("fal/minimax-h3", None).unwrap();
        assert_eq!(body_of(&ask(mm, None, None), &Value::Null)["resolution"], "2K");
        let sd = find("fal/seedance-2.5", None).unwrap();
        assert_eq!(body_of(&ask(sd, Some("data:x"), None), &Value::Null)["aspect_ratio"], "auto");
        // Higgsfield
        let hk = find("higgsfield/kling-3-pro", None).unwrap();
        let b = body_of(&ask(hk, Some("https://cdn/x.jpg"), Some("https://cdn/y.jpg")), &Value::Null);
        assert_eq!((b["image_url"].as_str(), b["last_image_url"].as_str(), b["sound"].as_str(), b["duration"].as_u64()), (Some("https://cdn/x.jpg"), Some("https://cdn/y.jpg"), Some("on"), Some(5)));
        let hc = find("higgsfield/cinema-studio-4", None).unwrap();
        let b = body_of(&ask(hc, Some("https://cdn/x.jpg"), None), &Value::Null);
        assert_eq!((b["image_urls"][0].as_str(), b["resolution"].as_str(), b["generate_audio"].as_bool()), (Some("https://cdn/x.jpg"), Some("720p"), Some(true)));
        let hm = find("higgsfield/minimax-h3", None).unwrap();
        assert_eq!(body_of(&ask(hm, None, None), &Value::Null)["resolution"], "2K");
    }

    #[test]
    fn results_and_endpoints() {
        assert_eq!(find_video_url(&json!({"video": {"url": "https://v3.fal.media/a.mp4"}})).as_deref(), Some("https://v3.fal.media/a.mp4"));
        assert_eq!(find_video_url(&json!({"data": [{"file": "https://x.io/b.MOV?sig=1"}]})).as_deref(), Some("https://x.io/b.MOV?sig=1"));
        assert_eq!(find_video_url(&json!({"image": {"url": "https://x.io/c.png"}})), None);
        assert!(valid_endpoint("fal-ai/kling-video/v3/pro/text-to-video"));
        assert!(!valid_endpoint("fal-ai") && !valid_endpoint("a/../b") && !valid_endpoint("a/b c"));
    }

    #[test]
    fn estimates_rise_and_never_finish() {
        let (a, eta) = estimate(30.0, 60.0);
        assert!((a - 0.425).abs() < 1e-9 && eta == Some(30.0));
        let (b, eta) = estimate(600.0, 60.0);
        assert!(b < 0.95 && b > 0.9 && eta.is_none());
    }
}
