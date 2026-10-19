//! Paths and persistent configuration.

use std::path::PathBuf;

/// Per-user data directory (fonts, ffmpeg, autosaves, caches).
/// Windows: %APPDATA%\MoodCut, macOS: ~/Library/Application Support/MoodCut, Linux: ~/.local/share/moodcut
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("MOODCUT_HOME") {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join("MoodCut"))
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library/Application Support/MoodCut"))
    } else {
        std::env::var("XDG_DATA_HOME")
            .ok()
            .map(|x| PathBuf::from(x).join("moodcut"))
            .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".local/share/moodcut")))
    }
}

pub fn ensure_data_dir() -> PathBuf {
    let d = data_dir().unwrap_or_else(|| std::env::temp_dir().join("moodcut"));
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cache_dir() -> PathBuf {
    let d = ensure_data_dir().join("cache");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn autosave_dir() -> PathBuf {
    let d = ensure_data_dir().join("autosave");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn settings_file() -> PathBuf {
    ensure_data_dir().join("settings.json")
}

/// User settings kept between sessions (proxies, ...), `{}` when there are none.
pub fn settings() -> serde_json::Value {
    std::fs::read_to_string(settings_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(|v: &serde_json::Value| v.is_object()).unwrap_or_else(|| serde_json::json!({}))
}

/// Sets one top-level setting and saves the file.
pub fn save_setting(key: &str, value: serde_json::Value) {
    let mut s = settings();
    s[key] = value;
    if let Ok(text) = serde_json::to_string_pretty(&s) {
        let _ = std::fs::write(settings_file(), text);
    }
}
