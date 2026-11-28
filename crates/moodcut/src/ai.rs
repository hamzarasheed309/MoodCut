//! AI services through cloud APIs, with the person's own API keys:
//!
//! - **AI Voice** (`ai/voice.rs`): ElevenLabs: clone a voice from a recording, a library of
//!   ready voices, and speech in 70+ languages, imported into the project as audio clips.
//! - **AI Video** (`ai/video.rs`): Runway and fal.ai, which between them serve Gen-4.5, Veo 3.1,
//!   Kling, Seedance, Gemini Omni, MiniMax Hailuo and Wan: text to video and image to video
//!   (from a project still, a video frame or the Program monitor), imported as video clips.
//!
//! Nothing runs on this computer but the requests: no models, no Python, no downloads beyond
//! the results. Shared parts:
//!
//! - `keys`: the API keys, saved encrypted for this Windows user (DPAPI) or read from the
//!   environment (`ELEVENLABS_API_KEY`, `RUNWAYML_API_SECRET`, `FAL_KEY`). Keys never reach
//!   the editor page or AI apps: they see only whether one is set and its last characters.
//! - `http`: requests through curl (built into Windows 10 and later), keys passed on curl's
//!   stdin, never on its command line.
//! - `tasks`: the queue both use: progress (the provider's own when it reports one, otherwise
//!   an estimate from the model's usual time, marked as such), cancel (also at the provider),
//!   retry, and errors explained in plain words with what to do.

use crate::engine::Engine;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

mod http;
pub mod keys;
mod tasks;
pub mod video;
pub mod voice;

/// `GET/POST /api/ai`: the services and their keys. `{action: status|set_key|remove_key|test_key, provider, key}`.
pub fn api(a: &Value) -> Result<Value> {
    let action = a["action"].as_str().unwrap_or("status");
    let provider = || -> Result<&'static keys::Provider> {
        let id = a["provider"].as_str().unwrap_or("");
        keys::provider(id).ok_or_else(|| anyhow!("unknown service '{id}': {}", keys::PROVIDERS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")))
    };
    let mut note = Value::Null;
    match action {
        "status" => {}
        "set_key" => {
            let p = provider()?;
            keys::set(p.id, a["key"].as_str().unwrap_or(""))?;
            note = match test_key(p) {
                Ok(t) => json!({"ok": true, "text": t}),
                Err(e) => {
                    let x = tasks::explain(&e);
                    json!({"ok": false, "text": x.message, "hint": x.hint})
                }
            };
        }
        "remove_key" => keys::remove(provider()?.id),
        "test_key" => {
            let p = provider()?;
            note = match test_key(p) {
                Ok(t) => json!({"ok": true, "text": t}),
                Err(e) => {
                    let x = tasks::explain(&e);
                    json!({"ok": false, "text": x.message, "hint": x.hint})
                }
            };
        }
        other => bail!("unknown action '{other}'"),
    }
    Ok(json!({"ok": true, "providers": keys::status(), "test": note}))
}

/// A cheap authenticated call that proves a key works, and what the account has left.
pub fn test_key(p: &keys::Provider) -> Result<String> {
    match p.id {
        "elevenlabs" => voice::test_key(),
        "runway" | "fal" | "higgsfield" => video::test_key(p.id),
        _ => bail!("no test for {}", p.name),
    }
}

/// Status of the services for AI apps (the `voice` and `video` tools start with it).
pub fn keys_text(ids: &[&str]) -> String {
    let mut out = vec![];
    for id in ids {
        let Some(p) = keys::provider(id) else { continue };
        match keys::key(id) {
            Some((_, from)) => out.push(format!("{}: connected ({from}).", p.name)),
            None => out.push(format!(
                "{}: no API key. Ask the person to add one in the editor (Edit > AI Services, or the panel's Connect button; keys from {}) or to set {} before starting MoodCut. Never ask them to paste a key into the chat.",
                p.name,
                p.keys_url,
                p.env[0]
            )),
        }
    }
    out.join("\n")
}

// ---------------------------------------------------------------- shared helpers

fn notify(eng: &Engine, kind: &str, text: impl Into<String>) {
    let rev = eng.st.read().rev;
    eng.emit(kind, rev, "ai", text.into());
}

/// Where results of a project go: `<project> assets/<sub>` (the data folder until it is saved).
fn out_dir(eng: &Engine, sub: &str) -> Result<PathBuf> {
    let gdir = crate::gfx::graphics_dir(eng.st.read().path.as_deref());
    let dir = gdir.parent().map(|d| d.join(sub)).unwrap_or_else(|| gdir.join(sub));
    std::fs::create_dir_all(&dir).map_err(|e| anyhow!("creating {}: {e}", dir.display()))?;
    Ok(dir)
}

fn slug(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.chars().count() >= max {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "clip".into()
    } else {
        out
    }
}

fn first_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut s = words.iter().take(n).cloned().collect::<Vec<_>>().join(" ");
    if words.len() > n {
        s.push('…');
    }
    s
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    p
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v[k].as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// `at=` as seconds: a number, "playhead", or a number in a string.
fn at_of(eng: &Engine, v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if s.trim().eq_ignore_ascii_case("playhead") => Some(eng.ui().playhead.unwrap_or(0.0)),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|t| t.is_finite() && *t >= 0.0)
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// The media id a file got when it was imported (the newest one with that path).
fn media_for(eng: &Engine, file: &Path) -> Option<String> {
    let p = eng.st.read().p.clone();
    let f = file.to_string_lossy();
    p.media.iter().rev().find(|m| crate::ops::exec::same_path(&m.path, &f)).map(|m| m.id.clone())
}

/// The clip last placed from a media item (the highest clip id using it).
fn clip_for(eng: &Engine, media: &str) -> Option<String> {
    let p = eng.st.read().p.clone();
    p.comps.iter().flat_map(|c| c.items()).filter(|it| it.src.as_deref() == Some(media)).map(|it| it.id.clone()).max_by_key(|id| id.get(1..).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_words() {
        assert_eq!(slug("Hello, World!", 40), "hello-world");
        assert_eq!(slug("  ", 40), "clip");
        assert_eq!(first_words("one two three four", 2), "one two…");
        assert_eq!(first_words("one two", 5), "one two");
    }
}
