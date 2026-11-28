//! AI Voice through ElevenLabs: the person's voices (cloned in MoodCut or on ElevenLabs) and
//! ElevenLabs' library, instant voice cloning from a project clip or a file (with the speaker's
//! permission), and speech: one clip per paragraph, 1-3 takes, imported into the project as one
//! undoable edit (tag `voice`, the script as the note, optionally placed at a time).
//!
//! Speech arrives as MP3 and is converted to 48 kHz WAV (evened out to about -16 LUFS unless
//! asked not to), written next to the project's graphics in `<project> assets/voice/`.

use super::http::{self, Body};
use super::tasks::{self, Progress};
use super::{at_of, first_words, keys, media_for, clip_for, notify, out_dir, slug, str_of, unique_path};
use crate::engine::Engine;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const BASE: &str = "https://api.elevenlabs.io";
const ID: &str = "elevenlabs";

/// Text-to-speech models: (id, label, characters per request, takes a language code,
/// continues from the neighbouring paragraphs).
pub const MODELS: &[(&str, &str, usize, bool, bool)] = &[
    ("eleven_v4", "Eleven v4: most natural and expressive, 90+ languages", 100_000, true, false),
    ("eleven_v4_turbo", "Eleven v4 Turbo: quick, 90+ languages", 40_000, true, false),
    ("eleven_v3", "Eleven v3: dramatic delivery, audio tags like [whispers] [laughs], 70+ languages", 5_000, true, false),
    ("eleven_multilingual_v2", "Multilingual v2: steady narration, 29 languages", 10_000, false, true),
    ("eleven_flash_v2_5", "Flash v2.5: fastest and cheapest, 32 languages", 40_000, true, true),
];
pub const DEFAULT_MODEL: &str = "eleven_v4";

/// Languages offered by name, with their ISO 639-1 codes.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("English", "en"),
    ("Spanish", "es"),
    ("French", "fr"),
    ("German", "de"),
    ("Italian", "it"),
    ("Portuguese", "pt"),
    ("Dutch", "nl"),
    ("Polish", "pl"),
    ("Russian", "ru"),
    ("Ukrainian", "uk"),
    ("Turkish", "tr"),
    ("Arabic", "ar"),
    ("Hindi", "hi"),
    ("Urdu", "ur"),
    ("Bengali", "bn"),
    ("Tamil", "ta"),
    ("Chinese", "zh"),
    ("Japanese", "ja"),
    ("Korean", "ko"),
    ("Vietnamese", "vi"),
    ("Indonesian", "id"),
    ("Malay", "ms"),
    ("Filipino", "fil"),
    ("Thai", "th"),
    ("Swedish", "sv"),
    ("Norwegian", "no"),
    ("Danish", "da"),
    ("Finnish", "fi"),
    ("Greek", "el"),
    ("Czech", "cs"),
    ("Romanian", "ro"),
    ("Hungarian", "hu"),
    ("Hebrew", "he"),
    ("Persian", "fa"),
];

/// Reference audio used for a clone when no end is given (ElevenLabs works best with 1-2 minutes).
const DEFAULT_REF_SECS: f64 = 120.0;
const MAX_REF_SECS: f64 = 300.0;

fn hdr(key: &str) -> Vec<String> {
    vec![format!("xi-api-key: {key}")]
}

fn model(id: &str) -> Option<&'static (&'static str, &'static str, usize, bool, bool)> {
    MODELS.iter().find(|m| m.0 == id)
}

fn language_code(l: &str) -> Option<String> {
    let l = l.trim();
    if l.is_empty() || l.eq_ignore_ascii_case("auto") {
        return None;
    }
    LANGUAGES.iter().find(|(n, c)| n.eq_ignore_ascii_case(l) || c.eq_ignore_ascii_case(l)).map(|(_, c)| c.to_string()).or_else(|| (l.len() <= 3 && l.chars().all(|c| c.is_ascii_alphabetic())).then(|| l.to_ascii_lowercase()))
}

// ---------------------------------------------------------------- account and voices

struct Cache {
    voices: Option<(Instant, Vec<Value>)>,
    account: Option<(Instant, Value)>,
    error: Option<String>,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache { voices: None, account: None, error: None }))
}

fn forget() {
    let mut c = cache().lock();
    c.voices = None;
    c.account = None;
}

/// Plan and usage (`null` when the key may not read them).
fn account(force: bool) -> Value {
    if let Some((at, v)) = &cache().lock().account {
        if !force && at.elapsed() < Duration::from_secs(60) {
            return v.clone();
        }
    }
    let Some((key, _)) = keys::key(ID) else { return Value::Null };
    let v = match http::json(ID, "GET", &format!("{BASE}/v1/user/subscription"), hdr(&key), Body::None, &AtomicBool::new(false)) {
        Ok(s) => json!({
            "tier": s["tier"], "status": s["status"],
            "used": s["character_count"], "limit": s["character_limit"], "reset": s["next_character_count_reset_unix"],
            "can_clone": s["can_use_instant_voice_cloning"], "voices_used": s["voice_slots_used"], "voice_limit": s["voice_limit"],
        }),
        Err(_) => Value::Null,
    };
    cache().lock().account = Some((Instant::now(), v.clone()));
    v
}

fn voice_entry(v: &Value) -> Value {
    let cat = v["category"].as_str().unwrap_or("premade");
    let mine = matches!(cat, "cloned" | "generated" | "professional") || v["is_owner"] == true;
    let l = &v["labels"];
    json!({
        "id": v["voice_id"], "name": v["name"], "category": cat, "mine": mine,
        "description": v["description"].as_str().or(l["description"].as_str()).unwrap_or(""),
        "accent": l["accent"], "gender": l["gender"], "age": l["age"], "use": l["use_case"].as_str().or(l["use case"].as_str()), "language": l["language"],
        "preview": v["preview_url"],
        "created": v["created_at_unix"],
    })
}

/// The voices this account can use: its own first (newest first), then the library by name.
pub fn voices(force: bool) -> Result<Vec<Value>> {
    if let Some((at, v)) = &cache().lock().voices {
        if !force && at.elapsed() < Duration::from_secs(300) {
            return Ok(v.clone());
        }
    }
    let key = keys::need(ID)?;
    let mut out = vec![];
    let mut token: Option<String> = None;
    for _ in 0..6 {
        let mut url = format!("{BASE}/v2/voices?page_size=100&include_total_count=false");
        if let Some(t) = &token {
            url.push_str(&format!("&next_page_token={}", urlencode(t)));
        }
        let r = http::json(ID, "GET", &url, hdr(&key), Body::None, &AtomicBool::new(false));
        let page = match r {
            Ok(p) => p,
            Err(e) => {
                cache().lock().error = Some(tasks::explain(&e).message);
                return Err(e);
            }
        };
        out.extend(page["voices"].as_array().cloned().unwrap_or_default().iter().map(voice_entry));
        token = page["next_page_token"].as_str().filter(|t| !t.is_empty() && page["has_more"] == true).map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| {
        let (ma, mb) = (a["mine"] == true, b["mine"] == true);
        mb.cmp(&ma).then_with(|| if ma { b["created"].as_u64().cmp(&a["created"].as_u64()) } else { a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()) })
    });
    let mut c = cache().lock();
    c.voices = Some((Instant::now(), out.clone()));
    c.error = None;
    Ok(out)
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A voice by id or name (any case).
fn resolve(key: &str) -> Result<Value> {
    let key = key.trim();
    if key.is_empty() {
        bail!("pick a voice");
    }
    for force in [false, true] {
        let all = voices(force)?;
        if let Some(v) = all.iter().find(|v| v["id"].as_str() == Some(key)).or_else(|| all.iter().find(|v| v["name"].as_str().is_some_and(|n| n.trim().eq_ignore_ascii_case(key)))) {
            return Ok(v.clone());
        }
        // a name that starts the voice's name ("Rachel" for "Rachel - calm narrator")
        let lower = key.to_lowercase();
        let starts: Vec<&Value> = all.iter().filter(|v| v["name"].as_str().is_some_and(|n| n.to_lowercase().starts_with(&lower))).collect();
        if starts.len() == 1 {
            return Ok(starts[0].clone());
        }
    }
    bail!("voice '{key}' not found: voice action=voices lists them (ids and names)")
}

/// A key works: its plan and characters left, or at least that voices can be read.
pub fn test_key() -> Result<String> {
    let key = keys::need(ID)?;
    let c = AtomicBool::new(false);
    match http::json(ID, "GET", &format!("{BASE}/v1/user/subscription"), hdr(&key), Body::None, &c) {
        Ok(s) => {
            forget();
            let (used, limit) = (s["character_count"].as_u64().unwrap_or(0), s["character_limit"].as_u64().unwrap_or(0));
            Ok(format!(
                "Connected: {} plan, {} of {} characters left this period{}.",
                s["tier"].as_str().unwrap_or("?"),
                fmt_int(limit.saturating_sub(used)),
                fmt_int(limit),
                if s["can_use_instant_voice_cloning"] == false { "; this plan can't clone voices (library voices work)" } else { "" }
            ))
        }
        Err(e) => {
            // a key without the user_read permission can still speak
            let forbidden = e.downcast_ref::<http::ApiError>().is_some_and(|a| a.status == 403);
            if !forbidden {
                return Err(e);
            }
            http::json(ID, "GET", &format!("{BASE}/v2/voices?page_size=1&include_total_count=false"), hdr(&key), Body::None, &c)?;
            forget();
            Ok("Connected (this key can't read the plan and usage; speech and voices work).".into())
        }
    }
}

fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- the editor's API

/// `GET/POST /api/voice`: `{action: status|voices|clone|speak|retry|cancel|clear|delete_voice|rename_voice}`.
pub fn api(eng: &Arc<Engine>, a: &Value) -> Result<Value> {
    let action = a["action"].as_str().unwrap_or("status");
    let mut extra = Value::Null;
    match action {
        "status" => {}
        "voices" | "refresh" => {
            voices(true)?;
            account(true);
        }
        "clone" => extra = json!({"task": submit(eng, "clone", a)?}),
        "speak" | "generate" => extra = json!({"task": submit(eng, "speak", a)?}),
        "retry" => extra = json!({"task": tasks::retry(eng, "voice", a["id"].as_u64().ok_or_else(|| anyhow!("which task?"))?)?}),
        "cancel" => {
            tasks::cancel("voice", a["id"].as_u64());
        }
        "clear" => tasks::clear("voice"),
        "delete_voice" => {
            let v = resolve(a["id"].as_str().or(a["voice"].as_str()).unwrap_or(""))?;
            if v["mine"] != true {
                bail!("\"{}\" is an ElevenLabs library voice: only voices you cloned or designed can be deleted", v["name"].as_str().unwrap_or(""));
            }
            let key = keys::need(ID)?;
            http::json(ID, "DELETE", &format!("{BASE}/v1/voices/{}", v["id"].as_str().unwrap_or("")), hdr(&key), Body::None, &AtomicBool::new(false))?;
            forget();
            notify(eng, "voice", "voice deleted");
        }
        "rename_voice" => {
            let v = resolve(a["id"].as_str().or(a["voice"].as_str()).unwrap_or(""))?;
            let name = str_of(a, "name").ok_or_else(|| anyhow!("name required"))?;
            if v["mine"] != true {
                bail!("library voices can't be renamed");
            }
            let key = keys::need(ID)?;
            http::json(ID, "POST", &format!("{BASE}/v1/voices/{}/edit", v["id"].as_str().unwrap_or("")), hdr(&key), Body::Form(vec![("name".into(), name.into())], vec![]), &AtomicBool::new(false))?;
            forget();
        }
        other => bail!("unknown action '{other}'"),
    }
    let mut s = status(true);
    if !extra.is_null() {
        s["submitted"] = extra;
    }
    Ok(s)
}

fn status(fetch: bool) -> Value {
    let connected = keys::key(ID);
    let (voice_list, err) = if connected.is_some() {
        let cached = cache().lock().voices.as_ref().map(|(_, v)| v.clone());
        match cached {
            Some(v) => (v, None),
            None if fetch => match voices(false) {
                Ok(v) => (v, None),
                Err(e) => (vec![], Some(tasks::explain(&e))),
            },
            None => (vec![], None),
        }
    } else {
        (vec![], None)
    };
    json!({
        "ok": true,
        "provider": keys::status().as_array().and_then(|a| a.iter().find(|p| p["id"] == ID).cloned()),
        "account": if connected.is_some() { account(false) } else { Value::Null },
        "voices": voice_list,
        "voices_error": err.as_ref().map(|x| json!({"message": x.message, "hint": x.hint, "fix": x.fix, "link": x.link})),
        "models": MODELS.iter().map(|m| json!({"id": m.0, "label": m.1, "max_chars": m.2, "language": m.3})).collect::<Vec<_>>(),
        "default_model": DEFAULT_MODEL,
        "languages": LANGUAGES.iter().map(|(n, c)| json!({"name": n, "code": c})).collect::<Vec<_>>(),
        "tasks": tasks::list("voice"),
    })
}

/// Checks a request and queues it; problems a person can fix at once are errors right away.
fn submit(eng: &Arc<Engine>, kind: &'static str, req: &Value) -> Result<u64> {
    keys::need(ID)?;
    let label = match kind {
        "clone" => {
            let name = str_of(req, "name").ok_or_else(|| anyhow!("give the voice a name"))?;
            if req["consent"] != true {
                bail!("cloning needs the speaker's permission: confirm it only when it is your voice or the speaker agreed");
            }
            if str_of(req, "media").is_none() && str_of(req, "path").is_none() {
                bail!("pick the reference audio: a clip from the project or a file");
            }
            format!("Clone voice \"{name}\"")
        }
        _ => {
            let v = resolve(req["voice"].as_str().unwrap_or(""))?;
            let text = req["text"].as_str().unwrap_or("").trim();
            if text.is_empty() {
                bail!("write the script to speak");
            }
            let m = str_of(req, "model").unwrap_or(DEFAULT_MODEL);
            if let Some(info) = model(m) {
                for (i, p) in parts(req).iter().enumerate() {
                    let n = p.chars().count();
                    if n > info.2 {
                        bail!("paragraph {} has {n} characters; {} takes up to {} per clip: split it with a blank line", i + 1, m, info.2);
                    }
                }
            }
            format!("Speak \"{}\" ({})", first_words(text, 8), v["name"].as_str().unwrap_or("voice"))
        }
    };
    let work: tasks::Work = if kind == "clone" { clone_work } else { speak_work };
    Ok(tasks::submit(eng, "voice", kind, ID, label, req.clone(), work))
}

/// The script's paragraphs (a blank line starts the next clip), or all of it as one.
fn parts(req: &Value) -> Vec<String> {
    let text = req["text"].as_str().unwrap_or("").replace("\r\n", "\n");
    if req["split"] != false {
        text.split("\n\n").map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|p| !p.is_empty()).collect()
    } else {
        let all = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if all.is_empty() {
            vec![]
        } else {
            vec![all]
        }
    }
}

// ---------------------------------------------------------------- clone

/// Cuts the reference (a project clip or a file; its In/Out or up to two minutes), uploads it as
/// an instant voice clone and lists the new voice.
fn clone_work(eng: &Arc<Engine>, req: &Value, p: &Progress) -> Result<Value> {
    let key = keys::need(ID)?;
    let name = str_of(req, "name").ok_or_else(|| anyhow!("give the voice a name"))?.to_string();
    if req["consent"] != true {
        bail!("cloning needs the speaker's permission");
    }
    let (src, src_label, src_dur) = match (str_of(req, "media"), str_of(req, "path")) {
        (Some(id), _) => {
            let m = eng.media(id).ok_or_else(|| anyhow!("unknown media {id}"))?;
            if !m.has_audio {
                bail!("{id} ({}) has no audio", m.name);
            }
            (m.path.clone(), format!("{} {}", m.id, m.name), m.dur)
        }
        (None, Some(path)) => {
            eng.check_path(path)?;
            if !Path::new(path).is_file() {
                bail!("{path} wasn't found");
            }
            let dur = eng.ff.probe(path).map(|m| m.dur).unwrap_or(0.0);
            let label = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
            (path.to_string(), label, dur)
        }
        _ => bail!("pick the reference audio: a clip from the project or a file"),
    };
    let start = req["start"].as_f64().unwrap_or(0.0).max(0.0);
    let end = req["end"].as_f64().filter(|e| *e > start);
    let mut len = end.map(|e| e - start).unwrap_or(DEFAULT_REF_SECS).min(MAX_REF_SECS);
    if src_dur > 0.0 {
        len = len.min(src_dur - start);
    }
    if len < 4.0 {
        bail!("the reference is too short ({len:.1} s): use at least 10 seconds of clear speech, ideally 1-2 minutes");
    }
    p.set(Some(0.05), "preparing the reference audio");
    let mp3 = eng.ff.has_encoder("libmp3lame");
    let cut = http::tmp_file(if mp3 { "mp3" } else { "wav" });
    let (ss, t) = (format!("{start:.3}"), format!("{len:.3}"));
    let mut args: Vec<String> = ["-y", "-ss", ss.as_str(), "-t", t.as_str(), "-i", src.as_str(), "-vn", "-sn", "-ac", "1", "-ar", "44100"].iter().map(|s| s.to_string()).collect();
    if mp3 {
        args.extend(["-c:a", "libmp3lame", "-b:a", "192k"].iter().map(|s| s.to_string()));
    } else {
        args.extend(["-c:a", "pcm_s16le"].iter().map(|s| s.to_string()));
    }
    args.push(cut.to_string_lossy().to_string());
    eng.ff.run_capture(&args).context("cutting the reference audio")?;
    if super::file_len(&cut) < 2000 {
        let _ = std::fs::remove_file(&cut);
        bail!("the reference has no usable audio at {start:.1} s");
    }
    p.set(None, format!("uploading {len:.0} s of audio to ElevenLabs"));
    let mut fields = vec![("name".to_string(), name.clone()), ("remove_background_noise".to_string(), (req["denoise"] == true).to_string())];
    fields.push(("description".into(), str_of(req, "description").map(str::to_string).unwrap_or_else(|| format!("Cloned in MoodCut from {src_label}"))));
    let files = vec![("files".to_string(), cut.clone(), if mp3 { "audio/mpeg".to_string() } else { "audio/wav".to_string() })];
    let r = http::json(ID, "POST", &format!("{BASE}/v1/voices/add"), hdr(&key), Body::Form(fields, files), &p.cancel);
    let _ = std::fs::remove_file(&cut);
    let r = r?;
    let id = r["voice_id"].as_str().ok_or_else(|| anyhow!("ElevenLabs returned no voice id"))?.to_string();
    forget();
    p.set(Some(0.95), "listing the new voice");
    let _ = voices(true);
    let mut text = format!("voice \"{name}\" cloned from {len:.1} s of {src_label}");
    if len < 30.0 {
        text.push_str(" (short reference: 1-2 minutes of clean speech gives a closer likeness)");
    }
    if r["requires_verification"] == true {
        text.push_str("; ElevenLabs asks to verify this voice on its website before it can be used");
    }
    Ok(json!({"voice": id, "name": name, "secs": (len * 10.0).round() / 10.0, "text": text}))
}

// ---------------------------------------------------------------- speak

/// Speaks a script: one clip per paragraph in 1-3 takes, converted to WAV next to the project's
/// graphics, then imported (tag `voice`, the script and seed as the note) and optionally placed,
/// all as one undoable edit.
fn speak_work(eng: &Arc<Engine>, req: &Value, p: &Progress) -> Result<Value> {
    let key = keys::need(ID)?;
    let voice = resolve(req["voice"].as_str().unwrap_or(""))?;
    let vid = voice["id"].as_str().unwrap_or("").to_string();
    let vname = voice["name"].as_str().unwrap_or(&vid).to_string();
    let parts = parts(req);
    if parts.is_empty() {
        bail!("write the script to speak");
    }
    let model_id = str_of(req, "model").unwrap_or(DEFAULT_MODEL).to_string();
    let info = model(&model_id);
    let takes = req["takes"].as_u64().unwrap_or(1).clamp(1, 3) as usize;
    let base_seed = req["seed"].as_u64().unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() as u64 % 1_000_000).unwrap_or(1));
    let language = str_of(req, "language").and_then(language_code).filter(|_| info.map_or(true, |m| m.3));
    let num = |k: &str, lo: f64, hi: f64| req[k].as_f64().filter(|v| v.is_finite()).map(|v| v.clamp(lo, hi));
    let mut stability = num("stability", 0.0, 1.0).unwrap_or(0.5);
    // v3 takes three stability presets: creative, natural, robust
    if model_id == "eleven_v3" {
        stability = [0.0, 0.5, 1.0].into_iter().min_by(|a, b| (a - stability).abs().total_cmp(&(b - stability).abs())).unwrap_or(0.5);
    }
    let settings = json!({
        "stability": stability,
        "similarity_boost": num("similarity", 0.0, 1.0).unwrap_or(0.75),
        "style": num("style", 0.0, 1.0).unwrap_or(0.0),
        "use_speaker_boost": req["boost"] != false,
        "speed": num("speed", 0.7, 1.2).unwrap_or(1.0),
    });
    let loudness = match &req["loudness"] {
        Value::Bool(false) => None,
        Value::Number(n) => n.as_f64().map(|l| l.clamp(-30.0, -10.0)),
        _ => Some(-16.0),
    };
    let normalize = if req["numbers"] == false { "off" } else { "auto" };
    let at = at_of(eng, &req["at"]);
    let dir = out_dir(eng, "voice")?;
    struct Out {
        path: std::path::PathBuf,
        dur: f64,
        text: String,
        take: usize,
        seed: u64,
    }
    let total = (parts.len() * takes) as f64;
    let mut outs: Vec<Out> = vec![];
    let mut chars = 0usize;
    for (i, text) in parts.iter().enumerate() {
        for k in 0..takes {
            let n = (i * takes + k) as f64;
            let what = match (parts.len() > 1, takes > 1) {
                (true, true) => format!("part {} of {}, take {}: ", i + 1, parts.len(), k + 1),
                (true, false) => format!("part {} of {}: ", i + 1, parts.len()),
                (false, true) => format!("take {} of {takes}: ", k + 1),
                _ => String::new(),
            };
            p.set(Some((n + 0.1) / total * 0.95), format!("{what}ElevenLabs is speaking"));
            let seed = base_seed + k as u64;
            let mut body = json!({"text": text, "model_id": model_id, "voice_settings": settings, "seed": seed, "apply_text_normalization": normalize});
            if let Some(l) = &language {
                body["language_code"] = json!(l);
            }
            if info.is_some_and(|m| m.4) {
                if i > 0 {
                    body["previous_text"] = json!(parts[i - 1]);
                }
                if let Some(next) = parts.get(i + 1) {
                    body["next_text"] = json!(next);
                }
            }
            let mp3 = http::tmp_file("mp3");
            let call = http::Call { provider: ID, method: "POST", url: format!("{BASE}/v1/text-to-speech/{vid}?output_format=mp3_44100_128"), headers: hdr(&key), body: Body::Json(&body), timeout: 600 };
            let r = http::send(&call, Some(&mp3), &p.cancel, &|_| {})?;
            if !r.ok() {
                let _ = std::fs::remove_file(&mp3);
                return Err(http::api_error(ID, &r).into());
            }
            chars += text.chars().count();
            p.cost(format!("{} characters", fmt_int(chars as u64)));
            p.set(Some((n + 0.8) / total * 0.95), format!("{what}converting"));
            let stem = format!("{}-{}{}", slug(&vname, 20), slug(&first_words(text, 5), 32), if takes > 1 { format!("-take{}", k + 1) } else { String::new() });
            let out = unique_path(&dir, &stem, "wav");
            let mut args: Vec<String> = vec!["-y".into(), "-i".into(), mp3.to_string_lossy().to_string()];
            if let Some(l) = loudness {
                args.extend(["-af".to_string(), format!("loudnorm=I={l}:TP=-1.5:LRA=11")]);
            }
            args.extend(["-ar", "48000", "-c:a", "pcm_s16le"].iter().map(|s| s.to_string()));
            args.push(out.to_string_lossy().to_string());
            let conv = eng.ff.run_capture(&args).context("converting the speech to WAV");
            let _ = std::fs::remove_file(&mp3);
            conv?;
            let dur = eng.ff.probe(&out.to_string_lossy()).map(|m| m.dur).unwrap_or(0.0);
            outs.push(Out { path: out, dur, text: text.clone(), take: k + 1, seed });
        }
    }
    p.set(Some(0.97), "adding to the project");
    let q = crate::ops::lex::quote;
    let mut t = at;
    let mut lines = vec![];
    for o in &outs {
        let file = o.path.to_string_lossy().to_string();
        let take = if takes > 1 { format!(" (take {})", o.take) } else { String::new() };
        lines.push(format!("import {} tag=voice as={}", q(&file), q(&format!("{vname}: {}{take}", first_words(&o.text, 6)))));
        let note: String = format!("AI voice ({vname}, ElevenLabs {model_id}, seed {}): {}", o.seed, o.text).chars().take(600).collect();
        lines.push(format!("note $ {}", q(&note)));
        // the first take of each part goes on the timeline; other takes wait in the Project panel
        if let (Some(at), 1) = (t, o.take) {
            lines.push(format!("add {} at={:.3}", q(&file), at));
            t = Some(at + o.dur + 0.25);
        }
    }
    eng.edit(&lines.join("\n"), "gui")?;
    let mut items = vec![];
    for o in &outs {
        let media = media_for(eng, &o.path);
        let clip = match (&media, at.is_some() && o.take == 1) {
            (Some(mid), true) => clip_for(eng, mid),
            _ => None,
        };
        items.push(json!({"media": media, "clip": clip, "file": o.path, "dur": (o.dur * 100.0).round() / 100.0, "take": o.take, "seed": o.seed, "text": o.text}));
    }
    forget();
    let ids: Vec<String> = items.iter().filter_map(|i| i["media"].as_str().map(str::to_string)).collect();
    let spoken: f64 = outs.iter().filter(|o| o.take == 1).map(|o| o.dur).sum();
    let what = if takes > 1 { format!("{} clips ({} parts × {takes} takes)", outs.len(), parts.len()) } else { format!("{} clip{}", outs.len(), if outs.len() == 1 { "" } else { "s" }) };
    Ok(json!({
        "media": ids,
        "items": items,
        "dur": (spoken * 100.0).round() / 100.0,
        "chars": chars,
        "text": format!("{what}, {spoken:.1} s, in the project{}{}", if ids.is_empty() { String::new() } else { format!(": {}", ids.join(", ")) }, if at.is_some() { " and on the timeline" } else { "" }),
    }))
}

// ---------------------------------------------------------------- the MCP tool

fn status_text(search: Option<&str>, which: Option<&str>) -> String {
    let mut out = vec![super::keys_text(&[ID])];
    if keys::key(ID).is_none() {
        return out.join("\n");
    }
    let a = account(false);
    if a.is_object() {
        let used = a["used"].as_u64().unwrap_or(0);
        let limit = a["limit"].as_u64().unwrap_or(0);
        out.push(format!(
            "Plan: {}, {} of {} characters left{}{}.",
            a["tier"].as_str().unwrap_or("?"),
            fmt_int(limit.saturating_sub(used)),
            fmt_int(limit),
            if a["can_clone"] == false { "; this plan can't clone voices" } else { "" },
            match (a["voices_used"].as_u64(), a["voice_limit"].as_u64()) {
                (Some(u), Some(l)) => format!("; {u} of {l} voice slots used"),
                _ => String::new(),
            }
        ));
    }
    match voices(false) {
        Ok(list) => {
            let s = search.map(str::to_lowercase);
            let shown: Vec<&Value> = list
                .iter()
                .filter(|v| match which {
                    Some("mine") => v["mine"] == true,
                    Some("library") => v["mine"] != true,
                    _ => true,
                })
                .filter(|v| s.as_ref().map_or(true, |s| v.to_string().to_lowercase().contains(s.as_str())))
                .collect();
            let mine = list.iter().filter(|v| v["mine"] == true).count();
            out.push(format!("Voices ({} yours, {} library; showing {}):", mine, list.len() - mine, shown.len().min(60)));
            for v in shown.iter().take(60) {
                let traits: Vec<String> = ["gender", "age", "accent", "use"].iter().filter_map(|k| v[*k].as_str().filter(|x| !x.is_empty()).map(str::to_string)).collect();
                out.push(format!(
                    "  {} '{}'{}{}{}",
                    v["id"].as_str().unwrap_or(""),
                    v["name"].as_str().unwrap_or(""),
                    if v["mine"] == true { " [yours]" } else { "" },
                    if traits.is_empty() { String::new() } else { format!(" ({})", traits.join(", ")) },
                    v["description"].as_str().filter(|d| !d.is_empty()).map(|d| format!(": {}", first_words(d, 14))).unwrap_or_default()
                ));
            }
            if shown.len() > 60 {
                out.push("  … narrow it with search=<words> or which=mine|library".into());
            }
        }
        Err(e) => out.push(format!("Voices couldn't be listed: {}", tasks::explain(&e).message)),
    }
    let recent = tasks::list("voice");
    if !recent.is_empty() {
        out.push("Recent tasks:".into());
        for t in recent.iter().rev().take(5) {
            out.push(format!("  {}", tasks::line(t)));
        }
    }
    out.join("\n")
}

fn done_text(t: &tasks::Task) -> Result<String> {
    if t.state == "error" {
        bail!("{}{}", t.error.clone().unwrap_or_else(|| "failed".into()), t.hint.as_ref().map(|h| format!(" {h}")).unwrap_or_default());
    }
    if t.state != "done" {
        return Ok(format!("{}\nStill working: check with voice action=status (or wait id={}).", tasks::line(t), t.id));
    }
    let r = t.result.clone().unwrap_or(Value::Null);
    let mut s = format!("{} ({})", r["text"].as_str().unwrap_or("done"), tasks::clock(t.secs));
    if let Some(v) = r["voice"].as_str() {
        s.push_str(&format!("\nvoice id: {v}. Speak with it: voice action=speak voice={v} text='...'"));
    }
    if let Some(items) = r["items"].as_array() {
        for it in items {
            s.push_str(&format!(
                "\n+{} {:.2}s take {} seed {}{}",
                it["media"].as_str().unwrap_or("?"),
                it["dur"].as_f64().unwrap_or(0.0),
                it["take"],
                it["seed"],
                it["clip"].as_str().map(|c| format!(" placed as {c}")).unwrap_or_default()
            ));
        }
        s.push_str("\nListen before using it (preview media=<id> shows the waveform); seed=<n> repeats a take, another seed gives a new reading.");
    }
    if let Some(c) = &t.cost {
        s.push_str(&format!("\nUsed {c}."));
    }
    Ok(s)
}

/// The `voice` MCP tool: the same account, voices and tasks as the AI Voice panel.
pub fn mcp_tool(eng: &Arc<Engine>, a: &Value) -> Result<String> {
    let action = a["action"].as_str().unwrap_or("status");
    let wait = a["wait"].as_bool().unwrap_or(true);
    let finish = |id: u64| -> Result<String> {
        if !wait {
            return Ok(format!("task #{id} queued: follow it with voice action=status (or voice action=wait id={id})"));
        }
        done_text(&tasks::wait(id, Duration::from_secs(600))?)
    };
    match action {
        "status" | "voices" | "list" => {
            if a["refresh"] == true {
                let _ = voices(true);
                account(true);
            }
            Ok(status_text(str_of(a, "search"), str_of(a, "which")))
        }
        "models" => Ok(MODELS.iter().map(|m| format!("{}{}: {} (up to {} characters per clip)", m.0, if m.0 == DEFAULT_MODEL { " (default)" } else { "" }, m.1, fmt_int(m.2 as u64))).collect::<Vec<_>>().join("\n")),
        "speak" | "generate" | "say" => finish(submit(eng, "speak", a)?),
        "clone" => finish(submit(eng, "clone", a)?),
        "wait" => done_text(&tasks::wait(a["id"].as_u64().ok_or_else(|| anyhow!("wait needs id="))?, Duration::from_secs(600))?),
        "retry" => finish(tasks::retry(eng, "voice", a["id"].as_u64().ok_or_else(|| anyhow!("retry needs id="))?)?),
        "rename" => {
            let v = resolve(a["voice"].as_str().unwrap_or(""))?;
            let new = str_of(a, "name").ok_or_else(|| anyhow!("rename needs name="))?;
            api(eng, &json!({"action": "rename_voice", "id": v["id"], "name": new}))?;
            Ok(format!("voice {} renamed '{new}'", v["id"].as_str().unwrap_or("")))
        }
        "delete" => {
            let v = resolve(a["voice"].as_str().unwrap_or(""))?;
            api(eng, &json!({"action": "delete_voice", "id": v["id"]}))?;
            Ok(format!("voice '{}' deleted from ElevenLabs; clips already generated stay", v["name"].as_str().unwrap_or("")))
        }
        "cancel" => {
            let n = tasks::cancel("voice", a["id"].as_u64());
            Ok(if n == 0 { "nothing was running".into() } else { format!("cancelled {n} task{}", if n == 1 { "" } else { "s" }) })
        }
        other => bail!("unknown action '{other}': status, voices, models, speak, clone, wait, retry, rename, delete, cancel"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_by_name_or_code() {
        assert_eq!(language_code("Urdu").as_deref(), Some("ur"));
        assert_eq!(language_code("de").as_deref(), Some("de"));
        assert_eq!(language_code("auto"), None);
        assert_eq!(language_code("Klingon"), None);
        assert_eq!(language_code("fil").as_deref(), Some("fil"));
    }

    #[test]
    fn scripts_split_into_paragraphs() {
        let r = json!({"text": "One.\r\n\r\nTwo  words.\n\n\n"});
        assert_eq!(parts(&r), vec!["One.", "Two words."]);
        let r = json!({"text": "One.\n\nTwo.", "split": false});
        assert_eq!(parts(&r), vec!["One. Two."]);
    }

    #[test]
    fn numbers_and_urls() {
        assert_eq!(fmt_int(1234567), "1,234,567");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
        assert!(model(DEFAULT_MODEL).is_some());
    }

    #[test]
    fn library_voices_are_not_mine() {
        let v = voice_entry(&json!({"voice_id": "x", "name": "Rachel", "category": "premade", "labels": {"accent": "american"}}));
        assert_eq!((v["mine"].as_bool(), v["accent"].as_str()), (Some(false), Some("american")));
        assert_eq!(voice_entry(&json!({"voice_id": "y", "name": "Me", "category": "cloned"}))["mine"], true);
    }
}
