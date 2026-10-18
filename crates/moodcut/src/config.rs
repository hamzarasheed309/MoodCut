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
