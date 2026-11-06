//! Reading and writing `.mcut` project files without ever losing work.
//!
//! A project file is the `Project` as JSON plus two things only the file needs:
//! - `rel` next to each media `path` (and graphic `html`): the same file relative to
//!   the project, so a project folder moved to another drive or PC finds its media;
//! - `session`: the open comp and the playhead, so reopening lands where you left off.
//!
//! Both are optional: older and newer MoodCut versions ignore what they don't know.
//!
//! Saving first checks that every number can be stored (JSON has no NaN or infinity:
//! they would be written as `null` and make the file unreadable), then writes a
//! temporary file next to the project, flushes it to disk and renames it over the old
//! one, so a crash or power cut mid-save leaves the previous version intact. Saved
//! projects also get rolling backups in the data folder (`autosave/backups`), and
//! untitled projects autosave to a file of their own in `autosave/` until they are
//! saved, so they can be recovered after a crash.
//!
//! Opening explains damaged files (pointing at the newest backup), repairs numbers
//! older versions wrote as `null`, makes ids and comp settings consistent
//! (`Project::repair`) and relinks media that moved together with the project.

use crate::model::project::{Project, FORMAT_VERSION};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// At most one backup of a project this often.
const BACKUP_EVERY: Duration = Duration::from_secs(5 * 60);
/// Backups kept per project.
const BACKUPS_KEPT: usize = 10;
/// Untitled autosaves kept, and how old they may get.
const UNTITLED_KEPT: usize = 12;
const UNTITLED_MAX_AGE: Duration = Duration::from_secs(30 * 86400);
/// Nothing this big is a project (and it must not be read into memory to find out).
const MAX_PROJECT_BYTES: u64 = 512 << 20;

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// Where the editor was when the project was saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub comp: Option<String>,
    pub playhead: Option<f64>,
}

/// A project read from disk.
#[derive(Debug)]
pub struct Loaded {
    pub project: Project,
    /// What was repaired or relinked while opening (worth telling the person or the AI).
    pub notes: Vec<String>,
    /// Media whose files are still missing, as "m3 drone.mp4".
    pub missing: Vec<String>,
    pub session: Session,
    /// The project in memory differs from the file (repairs, relinked media).
    pub changed: bool,
}

// ---------------------------------------------------------------- saving

/// The text of a project file. `file` is where it will be written (for the portable
/// `rel` paths; None for untitled autosaves).
pub fn to_json(p: &Project, file: Option<&Path>, session: &Session) -> Result<String> {
    let mut v = serde_json::to_value(p).context("preparing the project for saving")?;
    if let Some(at) = find_null(&v) {
        bail!("the project holds an invalid number (infinite or NaN) at {at}, which can't be saved: undo the last change or set that value again");
    }
    v["v"] = json!(FORMAT_VERSION);
    if let Some(dir) = file.and_then(|f| absolute(f).parent().map(|d| d.to_path_buf())) {
        if let Some(list) = v.get_mut("media").and_then(|m| m.as_array_mut()) {
            for m in list {
                let rel = m["path"].as_str().and_then(|p| relative(Path::new(p), &dir));
                if let Some(r) = rel {
                    m["rel"] = json!(r);
                }
                if let Some(g) = m.get_mut("gfx").filter(|g| g.is_object()) {
                    let rel = g["html"].as_str().and_then(|p| relative(Path::new(p), &dir));
                    if let Some(r) = rel {
                        g["rel"] = json!(r);
                    }
                }
            }
        }
    }
    let mut s = Map::new();
    if let Some(c) = &session.comp {
        s.insert("comp".into(), json!(c));
    }
    if let Some(t) = session.playhead.filter(|t| t.is_finite() && *t >= 0.0) {
        s.insert("playhead".into(), json!((t * 1000.0).round() / 1000.0));
    }
    if !s.is_empty() {
        v["session"] = Value::Object(s);
    }
    Ok(serde_json::to_string_pretty(&v)?)
}

/// Err when the project holds a number a project file can't store (NaN, infinity).
pub fn check_numbers(p: &Project) -> Result<()> {
    let v = serde_json::to_value(p).context("checking the edit")?;
    match find_null(&v) {
        Some(at) => bail!("an invalid number (infinite or NaN) at {at}"),
        None => Ok(()),
    }
}

/// Where the first `null` of a serialized project is, e.g. `s1 › V2 › c5 › props › opacity`.
/// Nothing in a project serializes to `null` except a non-finite number. Analysis
/// results are free-form JSON and are skipped.
fn find_null(v: &Value) -> Option<String> {
    fn walk(v: &Value, path: &mut Vec<String>) -> bool {
        match v {
            Value::Null => true,
            Value::Array(a) => a.iter().enumerate().any(|(i, x)| {
                path.push(x.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()).unwrap_or_else(|| format!("[{i}]")));
                let hit = walk(x, path);
                if !hit {
                    path.pop();
                }
                hit
            }),
            Value::Object(m) => m.iter().any(|(k, x)| {
                path.push(k.clone());
                let hit = walk(x, path);
                if !hit {
                    path.pop();
                }
                hit
            }),
            _ => false,
        }
    }
    let mut path = vec![];
    for (k, x) in v.as_object()? {
        if k == "analysis" {
            continue;
        }
        path.push(k.clone());
        if walk(x, &mut path) {
            let shown: Vec<&str> = path.iter().map(|s| s.as_str()).filter(|s| !matches!(*s, "comps" | "tracks" | "items" | "media")).collect();
            return Some(shown.join(" › "));
        }
        path.pop();
    }
    None
}

/// Writes `text` to `target` so the file is always either the old or the new
/// version: a temporary sibling is written and flushed to disk, then renamed over it.
pub fn write_atomic(target: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    if let Some(d) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| anyhow!("can't create the folder {}: {}", d.display(), describe_io(&e)))?;
    }
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project.mcut".into());
    let tmp = target.with_file_name(format!("{name}.{}-{}.tmp", std::process::id(), UNIQUE.fetch_add(1, Ordering::Relaxed)));
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        bail!("can't write {}: {}", target.display(), describe_io(&e));
    }
    // a virus scanner, backup tool or the search indexer can hold the old file for a moment
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, target) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(25 << tries));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                bail!("can't replace {}: {}", target.display(), describe_io(&e));
            }
        }
    }
}

/// A plain-language reason for a file error.
pub fn describe_io(e: &std::io::Error) -> String {
    let code = e.raw_os_error();
    if e.kind() == std::io::ErrorKind::StorageFull || (cfg!(windows) && matches!(code, Some(39) | Some(112))) {
        return "the disk is full".into();
    }
    if cfg!(windows) && matches!(code, Some(32) | Some(33)) {
        return "the file is open in another program".into();
    }
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "permission denied (the file is read-only, open elsewhere, or the folder is protected)".into(),
        std::io::ErrorKind::ReadOnlyFilesystem => "the drive is read-only".into(),
        _ => e.to_string(),
    }
}

/// `path` relative to the folder `base` ("footage/a.mp4", "../Music/song.mp3"), with
/// forward slashes so the file works on every OS. None on another drive.
pub fn relative(path: &Path, base: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    let a: Vec<Component> = path.components().collect();
    let b: Vec<Component> = base.components().collect();
    let same = |x: &Component, y: &Component| {
        if cfg!(windows) {
            x.as_os_str().to_string_lossy().eq_ignore_ascii_case(&y.as_os_str().to_string_lossy())
        } else {
            x == y
        }
    };
    let common = a.iter().zip(&b).take_while(|(x, y)| same(x, y)).count();
    let roots = a.iter().take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir)).count();
    if common < roots.max(1) || b[common..].iter().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let mut parts: Vec<String> = vec!["..".to_string(); b.len() - common];
    for c in &a[common..] {
        match c {
            Component::Normal(n) => parts.push(n.to_string_lossy().to_string()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `a/b/../c` -> `a/c` without touching the disk.
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------- backups

/// The folder holding a saved project's backups (in the data folder, not next to
/// the project, so they survive the project folder being deleted or synced).
pub fn backup_dir(file: &Path) -> PathBuf {
    let key = absolute(file).to_string_lossy().to_lowercase();
    let stem: String = file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '_' })
        .take(40)
        .collect();
    crate::config::autosave_dir().join("backups").join(format!("{stem}-{:08x}", crate::render::eval::hash_str(&key) as u32))
}

/// Backups in `dir`, newest first.
fn backups(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut v: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "mcut"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v
}

/// Copies `file` (about to be replaced) into its backups: at most one every few
/// minutes, the newest few kept.
pub fn backup(file: &Path) {
    if !file.is_file() {
        return;
    }
    let dir = backup_dir(file);
    let now = SystemTime::now();
    if backups(&dir).first().is_some_and(|(_, t)| now.duration_since(*t).unwrap_or_default() < BACKUP_EVERY) {
        return;
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let to = dir.join(format!("{}.mcut", crate::render::encode::chrono_like()));
    if std::fs::copy(file, &to).is_ok() {
        // a copy keeps the original's time; the backup's own time paces the next one
        if let Ok(f) = std::fs::File::options().write(true).open(&to) {
            let _ = f.set_modified(now);
        }
    }
    for (old, _) in backups(&dir).into_iter().skip(BACKUPS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
}

/// The newest backup of `file`, and how old it is.
pub fn latest_backup(file: &Path) -> Option<(PathBuf, Duration)> {
    let (p, t) = backups(&backup_dir(file)).into_iter().next()?;
    Some((p, SystemTime::now().duration_since(t).unwrap_or_default()))
}

fn fmt_age(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=89 => format!("{s} s"),
        90..=5399 => format!("{} min", s / 60),
        5400..=172_799 => format!("{} h", s / 3600),
        _ => format!("{} days", s / 86400),
    }
}

// ---------------------------------------------------------------- untitled autosaves

/// A fresh autosave file for a new untitled project (each one gets its own, so a
/// second untitled project never overwrites the first).
pub fn new_untitled() -> PathBuf {
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    crate::config::autosave_dir().join(format!("untitled-{}-{}-{n}.mcut", crate::render::encode::chrono_like(), std::process::id()))
}

/// True for files in the autosave folder that hold untitled projects.
pub fn is_untitled_autosave(p: &Path) -> bool {
    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let in_dir = absolute(p).parent().map(clean) == Some(clean(&absolute(&crate::config::autosave_dir())));
    in_dir && name.starts_with("untitled") && name.ends_with(".mcut")
}

/// An untitled project that was autosaved but never saved to a file of its own.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Unsaved {
    pub path: String,
    pub name: String,
    /// Seconds since 1970.
    pub modified: u64,
    pub items: usize,
    pub media: usize,
}

/// Untitled projects with something in them, newest first (`except`: the one open now).
pub fn unsaved(except: &Path) -> Vec<Unsaved> {
    let mut out = vec![];
    for e in std::fs::read_dir(crate::config::autosave_dir()).into_iter().flatten().flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !(name.starts_with("untitled") && name.ends_with(".mcut")) || p == except {
            continue;
        }
        let Some(modified) = e.metadata().ok().and_then(|m| m.modified().ok()) else { continue };
        let Some(v) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let items: usize = v["comps"].as_array().into_iter().flatten().flat_map(|c| c["tracks"].as_array().into_iter().flatten()).map(|t| t["items"].as_array().map(|i| i.len()).unwrap_or(0)).sum();
        let media = v["media"].as_array().map(|m| m.len()).unwrap_or(0);
        if items == 0 && media == 0 {
            continue;
        }
        out.push(Unsaved {
            path: p.to_string_lossy().to_string(),
            name: v["name"].as_str().unwrap_or("Untitled").to_string(),
            modified: modified.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            items,
            media,
        });
    }
    out.sort_by_key(|u| std::cmp::Reverse(u.modified));
    out
}

/// Removes old untitled autosaves: the newest few stay, none older than a month.
pub fn prune_untitled(keep: &Path) {
    let now = SystemTime::now();
    let mut list: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(crate::config::autosave_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p != keep && p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("untitled") && n.to_string_lossy().ends_with(".mcut")))
        .filter_map(|p| Some((p.clone(), std::fs::metadata(&p).ok()?.modified().ok()?)))
        .collect();
    list.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (i, (p, t)) in list.into_iter().enumerate() {
        if i >= UNTITLED_KEPT || now.duration_since(t).unwrap_or_default() > UNTITLED_MAX_AGE {
            let _ = std::fs::remove_file(p);
        }
    }
}

// ---------------------------------------------------------------- opening

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}

/// "can't open X: why", pointing at the newest backup when there is one.
fn damaged(path: &Path, why: &str) -> anyhow::Error {
    let mut msg = format!("can't open {}: {why}", file_name(path));
    if let Some((b, age)) = latest_backup(path) {
        msg.push_str(&format!(". A backup from {} ago is at {} (open that instead)", fmt_age(age), b.display()));
    }
    anyhow!(msg)
}

/// A JSON error with the line it happened on.
fn describe_json(e: &serde_json::Error, text: &str) -> String {
    if e.is_eof() {
        return "the file ends early: it was cut off while being written".into();
    }
    let line: String = text.lines().nth(e.line().saturating_sub(1)).map(|l| l.trim()).unwrap_or("").chars().take(70).collect();
    if line.is_empty() {
        e.to_string()
    } else {
        format!("{e}: `{line}`")
    }
}

/// `null`s outside the free-form analysis results.
fn count_nulls(v: &Value) -> usize {
    fn n(v: &Value) -> usize {
        match v {
            Value::Null => 1,
            Value::Array(a) => a.iter().map(n).sum(),
            Value::Object(m) => m.values().map(n).sum(),
            _ => 0,
        }
    }
    v.as_object().map(|m| m.iter().filter(|(k, _)| *k != "analysis").map(|(_, x)| n(x)).sum()).unwrap_or(0)
}

/// Replaces `null`s (numbers older versions couldn't store): in lists with 0; in
/// objects the field is dropped (its default applies), or set to 0 with `zero`.
fn fill_nulls(v: &mut Value, zero: bool, top: bool) {
    match v {
        Value::Array(a) => {
            for x in a {
                if x.is_null() {
                    *x = json!(0);
                } else {
                    fill_nulls(x, zero, false);
                }
            }
        }
        Value::Object(m) => {
            let nulls: Vec<String> = m.iter().filter(|(_, x)| x.is_null()).map(|(k, _)| k.clone()).collect();
            for k in nulls {
                if zero {
                    m.insert(k, json!(0));
                } else {
                    m.remove(&k);
                }
            }
            for (k, x) in m.iter_mut() {
                if !(top && k == "analysis") {
                    fill_nulls(x, zero, false);
                }
            }
        }
        _ => {}
    }
}

/// Reads a project file: explains damage, repairs what it safely can, relinks moved media.
pub fn load(path: &Path) -> Result<Loaded> {
    let shown = path.display().to_string();
    if crate::engine::is_media(path) {
        bail!("{} is a media file, not a project: import it into a project instead", file_name(path));
    }
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => bail!("{shown} is a folder, not a project file"),
        Ok(m) if m.len() > MAX_PROJECT_BYTES => bail!("{} is too large to be a MoodCut project", file_name(path)),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("there's no project at {shown}"),
        Err(e) => bail!("can't read {shown}: {}", describe_io(&e)),
    }
    let bytes = std::fs::read(path).map_err(|e| anyhow!("can't read {shown}: {}", describe_io(&e)))?;
    if bytes.iter().all(|b| b.is_ascii_whitespace() || *b == 0) {
        return Err(damaged(path, "the file is empty"));
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(text).map_err(|e| damaged(path, &format!("it isn't readable project data ({})", describe_json(&e, text))))?;
    if !(v.is_object() && (v.get("comps").is_some_and(|c| c.is_array()) || v.get("main").is_some_and(|m| m.is_string()))) {
        bail!("{} isn't a MoodCut project", file_name(path));
    }
    let ver = v["v"].as_u64().unwrap_or(0);
    let session = Session {
        comp: v["session"]["comp"].as_str().map(|s| s.to_string()),
        playhead: v["session"]["playhead"].as_f64().filter(|t| t.is_finite() && *t >= 0.0),
    };
    let rels: Vec<(Option<String>, Option<String>)> =
        v["media"].as_array().map(|a| a.iter().map(|m| (m["rel"].as_str().map(String::from), m["gfx"]["rel"].as_str().map(String::from))).collect()).unwrap_or_default();
    let mut notes = vec![];
    if ver > FORMAT_VERSION as u64 {
        notes.push(format!("it was saved by a newer MoodCut (project format {ver}); saving it here drops anything this version doesn't know"));
    }
    let mut changed = false;
    let mut p = match serde_json::from_value::<Project>(v.clone()) {
        Ok(p) => p,
        Err(first) => {
            // older versions could store an invalid number as `null`
            let nulls = count_nulls(&v);
            let fixed = if nulls > 0 {
                [false, true].into_iter().find_map(|zero| {
                    let mut w = v.clone();
                    fill_nulls(&mut w, zero, true);
                    serde_json::from_value::<Project>(w).ok()
                })
            } else {
                None
            };
            match fixed {
                Some(p) => {
                    notes.push(format!("repaired {nulls} invalid number{} (set to 0 or the default)", if nulls == 1 { "" } else { "s" }));
                    changed = true;
                    p
                }
                None => {
                    // the typed parse of the text says where it went wrong
                    let why = serde_json::from_str::<Project>(text).err().map(|e| describe_json(&e, text)).unwrap_or_else(|| first.to_string());
                    return Err(damaged(path, &format!("part of it can't be read ({why})")));
                }
            }
        }
    };
    let repairs = p.repair();
    if !repairs.is_empty() {
        changed = true;
        notes.extend(repairs);
    }
    let (relinked, missing) = relink(&mut p, path, &rels);
    if relinked > 0 {
        changed = true;
        notes.push(format!("found {relinked} moved media file{} next to the project and relinked {}", if relinked == 1 { "" } else { "s" }, if relinked == 1 { "it" } else { "them" }));
    }
    remove_stale_temps(path);
    Ok(Loaded { project: p, notes, missing, session, changed })
}

/// Temporary files of saves that never finished (MoodCut was killed mid-write).
fn remove_stale_temps(file: &Path) {
    let Some(name) = file.file_name().map(|n| n.to_string_lossy().to_string()) else { return };
    let dir = file.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let now = SystemTime::now();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        let old = || e.metadata().ok().and_then(|m| m.modified().ok()).is_some_and(|t| now.duration_since(t).unwrap_or_default() > Duration::from_secs(600));
        if n.len() > name.len() + 1 && n.starts_with(&name) && n[name.len()..].starts_with('.') && n.ends_with(".tmp") && old() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Points media whose files are gone at copies that moved along with the project.
/// Returns how many were relinked and the ones still missing.
fn relink(p: &mut Project, file: &Path, rels: &[(Option<String>, Option<String>)]) -> (usize, Vec<String>) {
    let file = absolute(file);
    let dir = file.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut relinked = 0;
    let mut missing = vec![];
    for (i, m) in p.media.iter_mut().enumerate() {
        let (rel, html_rel) = rels.get(i).cloned().unwrap_or_default();
        if !Path::new(&m.path).exists() {
            match find_moved(&m.path, rel.as_deref(), &dir, &stem, Some(&m.name)) {
                Some(found) => {
                    m.path = found;
                    relinked += 1;
                }
                None => missing.push(format!("{} {}", m.id, m.name)),
            }
        }
        if let Some(g) = m.gfx.as_mut() {
            if !Path::new(&g.html).exists() {
                // the page is kept next to its rendered video
                let beside = Path::new(&m.path).parent().map(|d| d.join(base_name(&g.html))).filter(|b| b.is_file());
                if let Some(f) = beside.map(|b| b.to_string_lossy().to_string()).or_else(|| find_moved(&g.html, html_rel.as_deref(), &dir, &stem, None)) {
                    g.html = f;
                } else if !g.src.is_empty() {
                    // the project keeps the page: put the file back next to its video
                    if let Some(d) = Path::new(&m.path).parent().filter(|d| d.is_dir()) {
                        let f = d.join(base_name(&g.html));
                        if std::fs::write(&f, &g.src).is_ok() {
                            g.html = f.to_string_lossy().to_string();
                        }
                    }
                }
            }
        }
    }
    (relinked, missing)
}

fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// Where a file that was at `old` may be now that the project is in `dir`: its saved
/// relative path, the same layout below the project ("<name> assets/graphics/a.mov",
/// "footage/a.mp4"), the project folder itself, or the project's assets folders.
fn find_moved(old: &str, rel: Option<&str>, dir: &Path, stem: &str, name: Option<&str>) -> Option<String> {
    if dir.as_os_str().is_empty() {
        return None;
    }
    // split by hand: a Windows path opened on macOS/Linux is one "component" there
    let parts: Vec<&str> = old.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let file = *parts.last()?;
    let mut cands: Vec<PathBuf> = vec![];
    if let Some(r) = rel {
        cands.push(clean(&dir.join(r)));
    }
    for k in (2..=3).rev() {
        if parts.len() > k {
            let tail = &parts[parts.len() - k..];
            // never a drive ("D:") or anything that would leave the folder
            if tail.iter().all(|s| !s.contains(':') && *s != "..") {
                cands.push(tail.iter().fold(dir.to_path_buf(), |d, s| d.join(s)));
            }
        }
    }
    cands.push(dir.join(file));
    for sub in ["graphics", "sfx", "downloads"] {
        cands.push(dir.join(format!("{stem} assets")).join(sub).join(file));
    }
    if let Some(n) = name.filter(|n| *n != file && !n.contains(['/', '\\', ':'])) {
        cands.push(dir.join(n));
    }
    cands.into_iter().find(|c| c.is_file()).map(|c| c.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::anim::{Anim, Ease, Key};
    use crate::model::project::*;
    use crate::model::value::Val;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("moodcut-projfile-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn media(id: &str, path: &Path) -> Media {
        serde_json::from_value(json!({"id": id, "path": path.to_string_lossy(), "name": base_name(&path.to_string_lossy()), "kind": "video", "dur": 10.0, "w": 1920, "h": 1080, "fps": 30.0, "has_video": true})).unwrap()
    }

    fn clip(p: &mut Project, src: &str, start: f64) -> String {
        let id = p.next_item_id(ItemKind::Clip);
        let mut it = Item::new(id.clone(), ItemKind::Clip, start, 2.0);
        it.src = Some(src.into());
        p.comps[0].tracks[0].items.push(it);
        id
    }

    #[test]
    fn invalid_numbers_are_refused_with_where_they_are() {
        let mut p = Project::new("t", 1920, 1080, 30.0);
        p.media.push(media("m1", Path::new("/x/a.mp4")));
        let id = clip(&mut p, "m1", 0.0);
        assert!(check_numbers(&p).is_ok());
        p.item_mut(&id).unwrap().props.insert("opacity".into(), Anim::new(Val::N(f64::NAN)));
        let e = check_numbers(&p).unwrap_err().to_string();
        assert!(e.contains("c1") && e.contains("opacity"), "{e}");
        assert!(to_json(&p, None, &Session::default()).is_err());
        // analysis results are free-form: a null there is fine
        p.item_mut(&id).unwrap().props.clear();
        p.analysis.insert("m1:x".into(), json!({"peak": null}));
        assert!(check_numbers(&p).is_ok());
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_files() {
        let d = scratch("atomic");
        let f = d.join("a.mcut");
        write_atomic(&f, "one").unwrap();
        write_atomic(&f, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        let names: Vec<String> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["a.mcut".to_string()]);
        // a folder in the way is an error, not a panic
        assert!(write_atomic(&d, "x").is_err());
    }

    #[test]
    fn relative_paths() {
        let base = if cfg!(windows) { PathBuf::from(r"C:\Edits\Promo") } else { PathBuf::from("/edits/promo") };
        let j = |s: &str| if cfg!(windows) { PathBuf::from(format!(r"C:\{}", s.replace('/', "\\"))) } else { PathBuf::from(format!("/{s}")) };
        let lower = if cfg!(windows) { "Edits/Promo" } else { "edits/promo" };
        assert_eq!(relative(&j(&format!("{lower}/footage/a.mp4")), &base).as_deref(), Some("footage/a.mp4"));
        assert_eq!(relative(&j("Music/song.mp3"), &base).as_deref(), Some("../../Music/song.mp3"));
        if cfg!(windows) {
            assert_eq!(relative(Path::new(r"D:\a.mp4"), &base), None, "another drive");
        }
        assert_eq!(relative(Path::new("relative/a.mp4"), &base), None);
    }

    #[test]
    fn moved_project_folder_relinks_media_and_graphics() {
        let d = scratch("moved");
        let old = d.join("old");
        let new = d.join("new");
        for root in [&old, &new] {
            std::fs::create_dir_all(root.join("footage")).unwrap();
            std::fs::create_dir_all(root.join("promo assets").join("graphics")).unwrap();
        }
        let mut p = Project::new("promo", 1920, 1080, 30.0);
        let shot = old.join("footage").join("a.mp4");
        let gfx = old.join("promo assets").join("graphics").join("title.mov");
        std::fs::write(&shot, "v").unwrap();
        std::fs::write(&gfx, "v").unwrap();
        p.media.push(media("m1", &shot));
        let mut g = media("m2", &gfx);
        g.gfx = Some(serde_json::from_value(json!({"html": old.join("promo assets/graphics/title.html").to_string_lossy(), "w": 1920, "h": 1080, "fps": 30.0, "dur": 3.0})).unwrap());
        p.media.push(g);
        clip(&mut p, "m1", 0.0);
        let text = to_json(&p, Some(&old.join("promo.mcut")), &Session::default()).unwrap();
        assert!(text.contains("\"rel\": \"footage/a.mp4\""), "{text}");
        // the whole folder moves; the old one is gone
        std::fs::write(new.join("promo.mcut"), &text).unwrap();
        std::fs::write(new.join("footage").join("a.mp4"), "v").unwrap();
        std::fs::write(new.join("promo assets").join("graphics").join("title.mov"), "v").unwrap();
        std::fs::write(new.join("promo assets").join("graphics").join("title.html"), "<p>").unwrap();
        std::fs::remove_dir_all(&old).unwrap();
        let l = load(&new.join("promo.mcut")).unwrap();
        assert!(l.missing.is_empty(), "{:?}", l.missing);
        assert!(l.changed);
        assert!(Path::new(&l.project.media[0].path).is_file());
        assert!(Path::new(&l.project.media[1].path).is_file());
        assert!(Path::new(&l.project.media[1].gfx.as_ref().unwrap().html).is_file());
    }

    #[test]
    fn a_lost_graphic_page_comes_back_from_the_project() {
        let d = scratch("lost-html");
        let dir = d.join("promo assets").join("graphics");
        std::fs::create_dir_all(&dir).unwrap();
        let mov = dir.join("title.mov");
        std::fs::write(&mov, "v").unwrap();
        let mut p = Project::new("promo", 1920, 1080, 30.0);
        let mut g = media("m1", &mov);
        g.gfx = Some(serde_json::from_value(json!({"html": dir.join("title.html").to_string_lossy(), "w": 1920, "h": 1080, "fps": 30.0, "dur": 3.0, "src": "<h1>{{t|Hi}}</h1>"})).unwrap());
        p.media.push(g);
        let file = d.join("promo.mcut");
        std::fs::write(&file, to_json(&p, Some(&file), &Session::default()).unwrap()).unwrap();
        let l = load(&file).unwrap();
        let html = &l.project.media[0].gfx.as_ref().unwrap().html;
        assert_eq!(std::fs::read_to_string(html).unwrap(), "<h1>{{t|Hi}}</h1>");
    }

    #[test]
    fn damaged_and_wrong_files_explain_themselves() {
        let d = scratch("damaged");
        let f = d.join("x.mcut");
        assert!(load(&f).unwrap_err().to_string().contains("there's no project"));
        std::fs::write(&f, "").unwrap();
        assert!(load(&f).unwrap_err().to_string().contains("empty"));
        let full = to_json(&Project::new("t", 1920, 1080, 30.0), Some(&f), &Session::default()).unwrap();
        std::fs::write(&f, &full[..full.len() / 2]).unwrap();
        assert!(load(&f).unwrap_err().to_string().contains("cut off"));
        std::fs::write(&f, r#"{"hello": 1}"#).unwrap();
        assert!(load(&f).unwrap_err().to_string().contains("isn't a MoodCut project"));
        let video = d.join("clip.mp4");
        std::fs::write(&video, "x").unwrap();
        assert!(load(&video).unwrap_err().to_string().contains("media file"));
        // a backup is offered when there is one
        std::fs::write(&f, &full).unwrap();
        backup(&f);
        std::fs::write(&f, "{ not json").unwrap();
        let e = load(&f).unwrap_err().to_string();
        assert!(e.contains("backup") && e.contains("open that instead"), "{e}");
        let _ = std::fs::remove_dir_all(backup_dir(&f));
    }

    #[test]
    fn nulls_from_older_versions_are_repaired() {
        let d = scratch("nulls");
        let f = d.join("n.mcut");
        let mut p = Project::new("t", 1920, 1080, 30.0);
        p.media.push(media("m1", Path::new("/x/a.mp4")));
        clip(&mut p, "m1", 1.0);
        let mut v = serde_json::to_value(&p).unwrap();
        v["comps"][0]["tracks"][0]["items"][0]["start"] = Value::Null;
        v["comps"][0]["tracks"][0]["items"][0]["props"] = json!({"opacity": null, "pos": [null, 3]});
        std::fs::write(&f, v.to_string()).unwrap();
        let l = load(&f).unwrap();
        assert!(l.notes.iter().any(|n| n.contains("repaired 3 invalid numbers")), "{:?}", l.notes);
        let it = &l.project.comps[0].tracks[0].items[0];
        assert_eq!(it.start, 0.0);
        assert_eq!(it.props["pos"].v, Val::V(0.0, 3.0));
    }

    #[test]
    fn keyframes_eases_and_session_survive_a_save() {
        let d = scratch("fidelity");
        let f = d.join("k.mcut");
        let mut p = Project::new("t", 1920, 1080, 30.0);
        p.media.push(media("m1", Path::new("/x/a.mp4")));
        let id = clip(&mut p, "m1", 0.0);
        let mut a = Anim::new(Val::N(0.0));
        a.set_keys(vec![Key { t: 0.0, v: Val::N(10.0), e: Ease::Bezier(0.123456, 0.0, 0.654321, 1.0) }, Key { t: 1.0, v: Val::N(90.0), e: Ease::Linear }]);
        a.v = Val::N(42.0); // the static value, used if the keys are removed
        a.x = Some("value + time".into());
        p.item_mut(&id).unwrap().props.insert("opacity".into(), a.clone());
        let session = Session { comp: Some("s1".into()), playhead: Some(1.5) };
        write_atomic(&f, &to_json(&p, Some(&f), &session).unwrap()).unwrap();
        let l = load(&f).unwrap();
        assert_eq!(l.project.item(&id).unwrap().props["opacity"], a);
        assert_eq!(l.session, session);
        assert!(l.notes.is_empty() && !l.changed, "{:?}", l.notes);
    }

    #[test]
    fn untitled_autosaves_are_listed_and_pruned() {
        let mine = new_untitled();
        assert!(is_untitled_autosave(&mine));
        assert!(!is_untitled_autosave(&std::env::temp_dir().join("untitled-x.mcut")));
        let mut p = Project::new("Lost work", 1920, 1080, 30.0);
        p.media.push(media("m1", Path::new("/x/a.mp4")));
        clip(&mut p, "m1", 0.0);
        let other = new_untitled();
        write_atomic(&other, &to_json(&p, None, &Session::default()).unwrap()).unwrap();
        let list = unsaved(&mine);
        assert!(list.iter().any(|u| u.path == other.to_string_lossy() && u.name == "Lost work" && u.items == 1), "{list:?}");
        assert!(unsaved(&other).iter().all(|u| u.path != other.to_string_lossy()), "the open one isn't offered");
        let _ = std::fs::remove_file(other);
    }
}
