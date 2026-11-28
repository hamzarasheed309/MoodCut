//! API keys of the AI services. Saved in `settings.json` (`ai_keys`) encrypted with Windows
//! DPAPI for this user account, so the file is useless on another account or computer; on other
//! systems it is only encoded (the settings file lives in the user's own data folder). An
//! environment variable is used when no key is saved. The editor and AI apps only ever see
//! whether a key is set, where it came from and its last characters.

use anyhow::{bail, Result};
use base64::Engine as _;
use serde_json::{json, Value};

pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// Environment variables read when no key is saved (first is the documented one).
    pub env: &'static [&'static str],
    /// What MoodCut uses it for.
    pub what: &'static str,
    pub keys_url: &'static str,
    pub billing_url: &'static str,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "elevenlabs",
        name: "ElevenLabs",
        env: &["ELEVENLABS_API_KEY", "XI_API_KEY"],
        what: "AI Voice: clone a voice, a library of ready voices, speech in 70+ languages",
        keys_url: "https://elevenlabs.io/app/settings/api-keys",
        billing_url: "https://elevenlabs.io/app/subscription",
    },
    Provider {
        id: "runway",
        name: "Runway",
        env: &["RUNWAYML_API_SECRET", "RUNWAY_API_KEY"],
        what: "AI Video: Gen-4.5, Veo 3.1, Kling, Seedance, Gemini Omni",
        keys_url: "https://dev.runway.com",
        billing_url: "https://dev.runway.com",
    },
    Provider {
        id: "fal",
        name: "fal.ai",
        env: &["FAL_KEY", "FAL_API_KEY"],
        what: "AI Video: Veo 3.1, Kling, Seedance, MiniMax Hailuo, Wan and any fal video model",
        keys_url: "https://fal.ai/dashboard/keys",
        billing_url: "https://fal.ai/dashboard/billing",
    },
    Provider {
        id: "higgsfield",
        name: "Higgsfield",
        env: &["HF_CREDENTIALS", "HIGGSFIELD_API_KEY"],
        what: "AI Video: Higgsfield Cinema Studio 4, Kling 3.0, Seedance 2.5, MiniMax Hailuo",
        keys_url: "https://console.higgsfield.ai",
        billing_url: "https://console.higgsfield.ai",
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    let id = id.trim().to_ascii_lowercase();
    let id = match id.as_str() {
        "eleven" | "11labs" | "elevenlabs" | "xi" => "elevenlabs",
        "runwayml" | "runway" => "runway",
        "fal" | "falai" | "fal.ai" | "fal-ai" => "fal",
        "higgsfield" | "higgsfield.ai" | "higgs" | "hf" => "higgsfield",
        other => other,
    }
    .to_string();
    PROVIDERS.iter().find(|p| p.id == id)
}

pub fn name(id: &str) -> &'static str {
    provider(id).map(|p| p.name).unwrap_or("The AI service")
}

/// The key to use and where it came from ("saved in MoodCut" or the variable's name).
pub fn key(id: &str) -> Option<(String, String)> {
    let p = provider(id)?;
    let saved = crate::config::settings()["ai_keys"][p.id].as_str().and_then(unprotect).filter(|k| !k.trim().is_empty());
    if let Some(k) = saved {
        return Some((k, "saved in MoodCut".into()));
    }
    // Higgsfield's documented pair of variables
    if p.id == "higgsfield" {
        if let (Ok(i), Ok(s)) = (std::env::var("HF_API_KEY_ID"), std::env::var("HF_API_KEY_SECRET")) {
            if !i.trim().is_empty() && !s.trim().is_empty() {
                return Some((format!("{}:{}", i.trim(), s.trim()), "HF_API_KEY_ID and HF_API_KEY_SECRET".into()));
            }
        }
    }
    p.env.iter().find_map(|v| std::env::var(v).ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).map(|k| (k, (*v).to_string())))
}

/// The key, or an error the editor explains with a Connect button.
pub fn need(id: &str) -> Result<String> {
    match key(id) {
        Some((k, _)) => Ok(k),
        None => bail!("NO_KEY {id}: {} isn't connected: add its API key in AI Services", name(id)),
    }
}

/// What people paste: the bare key, or with "Bearer " / "Key " / "xi-api-key:" in front.
fn clean(key: &str) -> String {
    let mut k = key.trim().trim_matches('"').trim().to_string();
    for prefix in ["authorization:", "xi-api-key:", "bearer ", "key "] {
        if k.to_ascii_lowercase().starts_with(prefix) {
            k = k[prefix.len()..].trim().to_string();
        }
    }
    k
}

pub fn set(id: &str, key: &str) -> Result<()> {
    let Some(p) = provider(id) else { bail!("unknown service '{id}'") };
    let k = clean(key);
    if k.is_empty() {
        bail!("paste the {} API key", p.name);
    }
    if k.len() < 12 || k.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("that doesn't look like a {} API key: copy it again from {}", p.name, p.keys_url);
    }
    if p.id == "higgsfield" && !k.contains(':') {
        bail!("paste the Higgsfield key as KEY_ID:KEY_SECRET (both parts from {}, joined by a colon)", p.keys_url);
    }
    let mut all = crate::config::settings()["ai_keys"].clone();
    if !all.is_object() {
        all = json!({});
    }
    all[p.id] = Value::String(protect(&k));
    crate::config::save_setting("ai_keys", all);
    Ok(())
}

pub fn remove(id: &str) {
    let Some(p) = provider(id) else { return };
    let mut all = crate::config::settings()["ai_keys"].clone();
    if let Some(o) = all.as_object_mut() {
        o.remove(p.id);
        crate::config::save_setting("ai_keys", all);
    }
}

/// `sk_…a1b2`: enough to recognize a key, not to use it.
pub fn masked(key: &str) -> String {
    let n = key.chars().count();
    if n < 10 {
        return "••••".into();
    }
    let head: String = key.chars().take(3).collect();
    let tail: String = key.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// Every service, for the editor: never the key itself.
pub fn status() -> Value {
    Value::Array(
        PROVIDERS
            .iter()
            .map(|p| {
                let k = key(p.id);
                json!({
                    "id": p.id, "name": p.name, "what": p.what, "keys_url": p.keys_url, "billing_url": p.billing_url, "env": p.env[0],
                    "connected": k.is_some(),
                    "source": k.as_ref().map(|(_, s)| s.clone()),
                    "masked": k.as_ref().map(|(k, _)| masked(k)),
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------- at rest

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn protect(plain: &str) -> String {
    match dpapi(plain.as_bytes(), true) {
        Some(sealed) => format!("dpapi:{}", b64().encode(sealed)),
        None => format!("b64:{}", b64().encode(plain.as_bytes())),
    }
}

fn unprotect(stored: &str) -> Option<String> {
    if let Some(s) = stored.strip_prefix("dpapi:") {
        let sealed = b64().decode(s.trim()).ok()?;
        return String::from_utf8(dpapi(&sealed, false)?).ok();
    }
    if let Some(s) = stored.strip_prefix("b64:") {
        return String::from_utf8(b64().decode(s.trim()).ok()?).ok();
    }
    None
}

/// Encrypts (or decrypts) for the current Windows user with DPAPI.
#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: `input` points at `data`, which outlives the call; DPAPI allocates `out` with
    // LocalAlloc, and it is copied and freed below.
    let ok = unsafe {
        if encrypt {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    // SAFETY: DPAPI returned `cbData` bytes at `pbData`.
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    Some(v)
}

#[cfg(not(windows))]
fn dpapi(_data: &[u8], _encrypt: bool) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_cleaned_masked_and_round_trip() {
        assert_eq!(clean("  Bearer abc123def456ghi  "), "abc123def456ghi");
        assert_eq!(clean("Key abc:def123456789"), "abc:def123456789");
        assert_eq!(clean("\"sk_1234567890abcdef\""), "sk_1234567890abcdef");
        assert_eq!(masked("sk_1234567890abcdef"), "sk_…cdef");
        assert_eq!(masked("short"), "••••");
        let sealed = protect("sk_1234567890abcdef");
        assert!(!sealed.contains("1234567890"), "stored in the clear: {sealed}");
        assert_eq!(unprotect(&sealed).as_deref(), Some("sk_1234567890abcdef"));
        assert_eq!(unprotect("plain-text"), None);
    }

    #[test]
    fn providers_by_any_name() {
        assert_eq!(provider("ElevenLabs").map(|p| p.id), Some("elevenlabs"));
        assert_eq!(provider("fal.ai").map(|p| p.id), Some("fal"));
        assert_eq!(provider("runwayml").map(|p| p.id), Some("runway"));
        assert_eq!(provider("Higgsfield").map(|p| p.id), Some("higgsfield"));
        assert!(provider("openai").is_none());
    }
}
