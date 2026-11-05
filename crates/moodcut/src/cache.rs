//! Keeps the data folder's cache (proxies, thumbnails, waveforms, media info,
//! analysis results) from growing without limit.
//!
//! Every process that edits (the editor, `serve`, `mcp`) trims once at start, in
//! the background: files not used for `MOODCUT_CACHE_DAYS` (default 14) go, then
//! the oldest go until the cache is under `MOODCUT_CACHE_GB` (default 5). Proxies
//! are marked as used whenever a project picks them up, so the footage you're
//! working on keeps its proxies. Anything written in the last hour is left alone,
//! since another MoodCut process may be making it. Everything here can be
//! rebuilt, so a trimmed file only costs the time to make it again.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Never removed: may still be in use by this or another MoodCut process.
const RECENT: Duration = Duration::from_secs(3600);
/// Scratch folders (browser profiles, FFmpeg unpacking) left by a process that was killed.
const SCRATCH_PREFIXES: &[&str] = &["browser-", "ffmpeg-unpack-"];
const SCRATCH_AGE: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_age: Duration,
    pub max_bytes: u64,
}

impl Limits {
    pub fn from_env() -> Limits {
        let num = |var: &str, default: f64| std::env::var(var).ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v >= 0.0).unwrap_or(default);
        Limits {
            max_age: Duration::from_secs_f64(num("MOODCUT_CACHE_DAYS", 14.0) * 86400.0),
            max_bytes: (num("MOODCUT_CACHE_GB", 5.0) * 1024.0 * 1024.0 * 1024.0) as u64,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Trimmed {
    pub files: usize,
    pub bytes: u64,
}

/// Trims the cache on a background thread (once per process is enough).
pub fn trim_in_background() {
    let _ = std::thread::Builder::new().name("cache-trim".into()).spawn(|| {
        let data = crate::config::ensure_data_dir();
        // an FFmpeg download that was interrupted (a finished one cleans up after itself)
        remove_stale_dir(&data.join("ffmpeg-download"), SystemTime::now());
        let t = trim(&crate::config::cache_dir(), Limits::from_env(), SystemTime::now());
        if t.files > 0 {
            eprintln!("moodcut: cache trimmed: {} old files, {:.1} MB", t.files, t.bytes as f64 / 1048576.0);
        }
    });
}

/// Marks a cache file as just used, so trimming keeps it.
pub fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

struct Entry {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

pub fn trim(dir: &Path, limits: Limits, now: SystemTime) -> Trimmed {
    let mut out = Trimmed::default();
    let mut files = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if path.is_dir() && SCRATCH_PREFIXES.iter().any(|p| name.starts_with(p)) {
                remove_stale_dir(&path, now);
            } else {
                collect(&path, &mut files, 0);
            }
        }
    }
    let age = |e: &Entry| now.duration_since(e.modified).unwrap_or_default();
    files.retain(|e| age(e) >= RECENT); // in use or just made: never a candidate, and not counted
    files.sort_by_key(|e| e.modified); // oldest first
    let mut total: u64 = files.iter().map(|e| e.len).sum();
    for e in &files {
        if age(e) <= limits.max_age && total <= limits.max_bytes {
            continue;
        }
        if std::fs::remove_file(&e.path).is_ok() {
            total = total.saturating_sub(e.len);
            out.files += 1;
            out.bytes += e.len;
            // a proxy's timing sidecar is useless without it
            if e.path.extension().is_some_and(|x| x == "mp4") {
                let side = e.path.with_extension("json");
                let len = std::fs::metadata(&side).map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&side).is_ok() {
                    // its own entry (if listed) then fails to remove and isn't counted twice
                    total = total.saturating_sub(len);
                    out.files += 1;
                    out.bytes += len;
                }
            }
        }
    }
    out
}

fn collect(path: &Path, out: &mut Vec<Entry>, depth: usize) {
    let Ok(m) = std::fs::symlink_metadata(path) else { return };
    if m.is_dir() {
        if depth < 4 {
            if let Ok(rd) = std::fs::read_dir(path) {
                for e in rd.flatten() {
                    collect(&e.path(), out, depth + 1);
                }
            }
        }
    } else if m.is_file() {
        out.push(Entry { path: path.to_path_buf(), len: m.len(), modified: m.modified().unwrap_or_else(|_| SystemTime::now()) });
    }
}

fn remove_stale_dir(dir: &Path, now: SystemTime) {
    let stale = std::fs::metadata(dir).and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|d| d > SCRATCH_AGE);
    if stale {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(86400);

    fn file(path: &Path, bytes: usize, age: Duration) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
        std::fs::File::options().write(true).open(path).unwrap().set_modified(SystemTime::now() - age).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("moodcut-cache-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn removes_old_files_and_their_sidecars() {
        let d = scratch("age");
        file(&d.join("proxies/old_1280x720.mp4"), 100, 20 * DAY);
        file(&d.join("proxies/old_1280x720.json"), 10, 20 * DAY);
        file(&d.join("proxies/new_1280x720.mp4"), 100, DAY);
        file(&d.join("thumbs/old.jpg"), 10, 30 * DAY);
        let t = trim(&d, Limits { max_age: 14 * DAY, max_bytes: u64::MAX }, SystemTime::now());
        assert_eq!(t.files, 3, "old proxy, its sidecar (counted as old itself) and the old thumbnail");
        assert!(!d.join("proxies/old_1280x720.mp4").exists());
        assert!(!d.join("proxies/old_1280x720.json").exists());
        assert!(!d.join("thumbs/old.jpg").exists());
        assert!(d.join("proxies/new_1280x720.mp4").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn size_cap_removes_oldest_first_but_never_recent_files() {
        let d = scratch("size");
        file(&d.join("proxies/a.mp4"), 1000, 5 * DAY);
        file(&d.join("proxies/b.mp4"), 1000, 3 * DAY);
        file(&d.join("proxies/c.mp4"), 1000, 2 * DAY);
        file(&d.join("proxies/live.part.mp4"), 5000, Duration::from_secs(60));
        let t = trim(&d, Limits { max_age: 14 * DAY, max_bytes: 1500 }, SystemTime::now());
        assert_eq!(t, Trimmed { files: 2, bytes: 2000 });
        assert!(!d.join("proxies/a.mp4").exists() && !d.join("proxies/b.mp4").exists());
        assert!(d.join("proxies/c.mp4").exists());
        assert!(d.join("proxies/live.part.mp4").exists(), "being written by another process");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn touch_keeps_a_file() {
        let d = scratch("touch");
        let p = d.join("proxies/used.mp4");
        file(&p, 100, 20 * DAY);
        touch(&p);
        let t = trim(&d, Limits { max_age: 14 * DAY, max_bytes: u64::MAX }, SystemTime::now());
        assert_eq!(t.files, 0);
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stale_scratch_folders_go_live_ones_stay() {
        let d = scratch("dirs");
        let live = d.join("browser-1-0");
        std::fs::create_dir_all(&live).unwrap();
        trim(&d, Limits { max_age: 14 * DAY, max_bytes: u64::MAX }, SystemTime::now());
        assert!(live.exists());
        trim(&d, Limits { max_age: 14 * DAY, max_bytes: u64::MAX }, SystemTime::now() + 2 * DAY);
        assert!(!live.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
