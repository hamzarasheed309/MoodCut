//! Large media libraries: cached parallel probing, recursive / URL imports,
//! filtered listings, disk browsing, search (names, notes, tags, transcripts),
//! poster-frame contact sheets and filmstrips, and batch analysis — so an AI
//! can work through hundreds of clips without reading them one by one.

use crate::engine::{glob_match, is_media, EditOutcome, Engine};
use crate::model::project::{Media, MediaKind, Project};
use crate::model::time::fmt_t;
use crate::ops::exec::kind_s;
use crate::render::encode::{self, Tile};
use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use resvg::tiny_skia::Pixmap;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

/// Safety cap on files gathered by one import / find.
const MAX_FILES: usize = 20_000;
const SKIP_DIRS: &[&str] = &["$recycle.bin", "system volume information", "node_modules", ".git", ".moodcut-cache", "__macosx"];

/// Media kind as shown to people and the AI ("graphic" for rendered HTML).
pub fn kind_name(m: &Media) -> &'static str {
    if m.gfx.is_some() {
        "graphic"
    } else {
        kind_s(m.kind)
    }
}

/// 1:02:03, 3:42 or 12.5s.
pub fn fmt_dur(s: f64) -> String {
    if s >= 3600.0 {
        format!("{}:{:02}:{:02}", (s / 3600.0) as u64, ((s % 3600.0) / 60.0) as u64, (s % 60.0) as u64)
    } else if s >= 60.0 {
        format!("{}:{:02}", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!("{}s", fmt_t((s * 10.0).round() / 10.0))
    }
}

pub fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

fn split_id(s: &str) -> Option<u32> {
    s.strip_prefix('m')?.parse().ok()
}

/// Resolves a media selector to ids: `m3`, `m1..m40`, names, `all`,
/// `tag:broll`, `kind:video`, `q:beach`, comma separated.
pub fn select_media(p: &Project, spec: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    let push = |out: &mut Vec<String>, id: &str| {
        if !out.iter().any(|x| x == id) {
            out.push(id.to_string());
        }
    };
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let lower = part.to_ascii_lowercase();
        if lower == "all" || part == "*" {
            p.media.iter().for_each(|m| push(&mut out, &m.id));
        } else if let Some(t) = lower.strip_prefix("tag:") {
            p.media.iter().filter(|m| m.tags.iter().any(|x| x.eq_ignore_ascii_case(t))).for_each(|m| push(&mut out, &m.id));
        } else if let Some(k) = lower.strip_prefix("kind:") {
            p.media.iter().filter(|m| kind_name(m) == k || (k == "still" && m.is_still())).for_each(|m| push(&mut out, &m.id));
        } else if let Some(q) = lower.strip_prefix("q:") {
            p.media.iter().filter(|m| media_text(m).contains(q)).for_each(|m| push(&mut out, &m.id));
        } else if let Some((a, b)) = part.split_once("..").and_then(|(a, b)| Some((split_id(a)?, split_id(b)?))) {
            p.media.iter().filter(|m| split_id(&m.id).is_some_and(|n| n >= a && n <= b)).for_each(|m| push(&mut out, &m.id));
        } else if let Some(m) = p.media(part) {
            push(&mut out, &m.id.clone());
        } else {
            let names = p.media.iter().flat_map(|m| [m.id.as_str(), m.name.as_str()]);
            let hint = crate::model::schema::suggest(names, part).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
            bail!("unknown media '{part}'.{hint}");
        }
    }
    if out.is_empty() {
        bail!("no media matched '{spec}'");
    }
    Ok(out)
}

/// Lowercased searchable text of a media item.
fn media_text(m: &Media) -> String {
    format!("{} {} {} {}", m.name, m.path, m.note, m.tags.join(" ")).to_lowercase()
}

/// Listing filters (all optional).
#[derive(Default, Clone)]
pub struct MediaFilter {
    pub kind: Option<String>,
    pub q: Option<String>,
    pub tag: Option<String>,
    pub min_dur: Option<f64>,
    pub max_dur: Option<f64>,
    /// landscape | portrait | square
    pub orient: Option<String>,
    /// true = only media used in a comp, false = only unused
    pub used: Option<bool>,
}

impl MediaFilter {
    fn keep(&self, m: &Media, used: &std::collections::HashSet<&str>) -> bool {
        if let Some(k) = &self.kind {
            let k = k.to_ascii_lowercase();
            if !(kind_name(m) == k || (k == "still" && m.is_still()) || (k == "visual" && m.is_visual())) {
                return false;
            }
        }
        if let Some(q) = &self.q {
            let text = media_text(m);
            if !q.to_lowercase().split_whitespace().all(|w| text.contains(w)) {
                return false;
            }
        }
        if let Some(t) = &self.tag {
            if !m.tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                return false;
            }
        }
        if self.min_dur.is_some_and(|d| m.dur < d) || self.max_dur.is_some_and(|d| m.is_still() || m.dur > d) {
            return false;
        }
        if let Some(o) = &self.orient {
            if !m.is_visual() {
                return false;
            }
            let ok = match o.as_str() {
                "portrait" | "vertical" => m.h > m.w,
                "square" => m.h == m.w,
                _ => m.w > m.h,
            };
            if !ok {
                return false;
            }
        }
        if let Some(u) = self.used {
            if used.contains(m.id.as_str()) != u {
                return false;
            }
        }
        true
    }
}

fn used_media(p: &Project) -> std::collections::HashSet<&str> {
    p.comps.iter().flat_map(|c| c.items()).filter_map(|i| i.src.as_deref()).collect()
}

/// One compact line per media.
pub fn media_line(p: &Project, m: &Media, detail: u8) -> String {
    let mut s = format!("{} {} {}", m.id, m.name, kind_name(m));
    if m.is_visual() {
        let _ = write!(s, " {}x{}", m.w, m.h);
    }
    if m.kind == MediaKind::Video {
        let _ = write!(s, " {}fps", crate::model::value::fmt_num((m.fps * 100.0).round() / 100.0));
    }
    if !m.is_still() {
        let _ = write!(s, " {}", fmt_dur(m.dur));
    }
    if m.kind == MediaKind::Video && !m.has_audio {
        s.push_str(" noaudio");
    }
    if m.alpha {
        s.push_str(" alpha");
    }
    if !m.tags.is_empty() {
        let _ = write!(s, " [{}]", m.tags.join(","));
    }
    if !m.note.is_empty() {
        let note: String = m.note.chars().take(if detail >= 2 { 400 } else { 80 }).collect();
        let _ = write!(s, " \"{note}\"");
    }
    let analyses: Vec<&str> = p.analysis.keys().filter_map(|k| k.strip_prefix(&format!("{}:", m.id))).collect();
    if !analyses.is_empty() {
        let _ = write!(s, " {{{}}}", analyses.join(","));
    }
    if detail >= 2 {
        let _ = write!(s, " path={} {}", m.path, fmt_bytes(m.size));
    }
    s
}

fn walk(dir: &Path, recursive: bool, depth: usize, pat: Option<&str>, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if recursive && depth < 12 && !SKIP_DIRS.contains(&name.to_ascii_lowercase().as_str()) && !name.ends_with(" assets") {
                walk(&p, recursive, depth + 1, pat, out);
            }
        } else if is_media(&p) && pat.is_none_or(|pt| glob_match(pt, &name)) {
            out.push(p);
        }
    }
}

impl Engine {
    /// ffprobe with a memory + disk cache keyed by path, size and mtime.
    pub fn probe_cached(&self, path: &str) -> Result<Media> {
        let meta = std::fs::metadata(path).with_context(|| format!("file not found: {path}"))?;
        let mtime = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let key = format!("{path}|{}|{mtime}", meta.len());
        if let Some(m) = self.probes.lock().get(&key) {
            return Ok(m.clone());
        }
        let file = crate::config::cache_dir().join("probe").join(format!("{:016x}.json", crate::render::eval::hash_str(&key)));
        if let Some(m) = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Media>(&t).ok()) {
            self.probes.lock().insert(key, m.clone());
            return Ok(m);
        }
        let m = self.ff.probe(path)?;
        if let Some(d) = file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&file, serde_json::to_string(&m).unwrap_or_default());
        self.probes.lock().insert(key, m.clone());
        Ok(m)
    }

    /// Downloads a URL into the project's assets (via curl, which ships with Windows 10+).
    fn download(&self, url: &str) -> Result<PathBuf> {
        let path = self.st.read().path.clone();
        let dir = crate::gfx::graphics_dir(path.as_deref()).parent().map(|d| d.join("downloads")).unwrap_or_else(|| crate::config::ensure_data_dir().join("downloads"));
        std::fs::create_dir_all(&dir)?;
        self.check_path(&dir.to_string_lossy())?;
        let clean = url.split(['?', '#']).next().unwrap_or(url);
        let mut name: String = clean.rsplit('/').next().unwrap_or("").chars().map(|c| if c.is_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect();
        if name.is_empty() || !name.contains('.') {
            name = format!("download-{:08x}.mp4", crate::render::eval::hash_str(url) as u32);
        }
        let out = dir.join(&name);
        if !out.exists() {
            let curl = crate::setup::which("curl").ok_or_else(|| anyhow!("downloading needs curl (built into Windows 10+)"))?;
            let tmp = dir.join(format!(".{name}.part"));
            let st = crate::render::ffmpeg::command(&curl).args(["-L", "--fail", "--silent", "--show-error", "--max-time", "1800", "-o"]).arg(&tmp).arg(url).stdout(Stdio::null()).stderr(Stdio::piped()).output()?;
            if !st.status.success() {
                let _ = std::fs::remove_file(&tmp);
                bail!("download failed for {url}: {}", String::from_utf8_lossy(&st.stderr).trim());
            }
            std::fs::rename(&tmp, &out)?;
        }
        Ok(out)
    }

    /// Expands folders (recursively), globs and URLs into media files.
    pub fn collect_files(&self, paths: &[String], recursive: bool) -> Result<Vec<PathBuf>> {
        let mut files = vec![];
        for p in paths {
            if p.starts_with("http://") || p.starts_with("https://") {
                files.push(self.download(p)?);
                continue;
            }
            let pb = PathBuf::from(p);
            if pb.is_dir() {
                walk(&pb, recursive, 0, None, &mut files);
            } else if p.contains('*') || p.contains('?') {
                let (dir, pat) = match pb.parent() {
                    Some(d) if !d.as_os_str().is_empty() => (d.to_path_buf(), pb.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()),
                    _ => (PathBuf::from("."), p.clone()),
                };
                walk(&dir, recursive && p.contains("**"), 0, Some(&pat.replace("**", "*")), &mut files);
            } else {
                files.push(pb);
            }
        }
        files.dedup();
        Ok(files)
    }

    pub fn import(&self, paths: &[String], source: &str) -> Result<EditOutcome> {
        self.import_with(paths, true, &[], source)
    }

    /// Imports files, folders (recursive), globs and URLs. Files are probed in
    /// parallel; unreadable ones are skipped and reported instead of failing
    /// the whole import. Big imports reply with a summary, not one line each.
    pub fn import_with(&self, paths: &[String], recursive: bool, tags: &[String], source: &str) -> Result<EditOutcome> {
        let files = self.collect_files(paths, recursive)?;
        if files.is_empty() {
            bail!("no media files found in {}", paths.join(", "));
        }
        let probed: Vec<(PathBuf, Result<Media>)> = files
            .par_iter()
            .map(|f| {
                let s = f.to_string_lossy().to_string();
                let r = self.check_path(&s).and_then(|_| self.probe_cached(&s));
                (f.clone(), r)
            })
            .collect();
        let mut ok = vec![];
        let mut skipped = vec![];
        for (f, r) in probed {
            match r {
                Ok(_) => ok.push(f),
                Err(e) => {
                    // keep just the reason: drop the path and FFmpeg's "[demuxer @ 0x..]" prefix
                    let msg = e.to_string().lines().next().unwrap_or("").replace(&*f.to_string_lossy(), "").to_string();
                    let reason = msg.rsplit("] ").next().unwrap_or(&msg).trim_start_matches(|c: char| c == ':' || c == ' ').to_string();
                    skipped.push(format!("{} ({})", f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), reason))
                }
            }
        }
        if ok.is_empty() {
            bail!("nothing could be imported: {}", skipped.join("; "));
        }
        let before: std::collections::HashSet<String> = self.st.read().p.media.iter().map(|m| m.id.clone()).collect();
        let mut script = format!("import {}", ok.iter().map(|f| crate::ops::lex::quote(&f.to_string_lossy())).collect::<Vec<_>>().join(" "));
        if !tags.is_empty() {
            script.push_str(&format!(" tag={}", tags.join(",")));
        }
        let mut out = self.edit(&script, source)?;
        let p = self.st.read().p.clone();
        let new: Vec<&Media> = p.media.iter().filter(|m| !before.contains(&m.id)).collect();
        let existing = ok.len() - new.len();
        if new.len() > 25 || !skipped.is_empty() || existing > 0 {
            let mut lines = vec![];
            if !new.is_empty() {
                let by = |k: &str| new.iter().filter(|m| kind_name(m) == k).collect::<Vec<_>>();
                let mut parts = vec![];
                for k in ["video", "audio", "image", "svg", "graphic"] {
                    let v = by(k);
                    if !v.is_empty() {
                        let total: f64 = v.iter().map(|m| m.dur).sum();
                        parts.push(if total > 0.0 { format!("{} {k} ({})", v.len(), fmt_dur(total)) } else { format!("{} {k}", v.len()) });
                    }
                }
                let first = &new[0].id;
                let last = &new[new.len() - 1].id;
                lines.push(format!("+{}{} imported {} media: {}", first, if new.len() > 1 { format!("..{last}") } else { String::new() }, new.len(), parts.join(", ")));
                if new.len() <= 25 {
                    lines.extend(new.iter().map(|m| format!("+{}", media_line(&p, m, 1))));
                } else {
                    lines.push("browse with media list (filters, paging), preview media=m1..m48 (poster frames) or view what=search".into());
                }
            }
            if existing > 0 {
                lines.push(format!("{existing} file(s) were already in the project"));
            }
            if !skipped.is_empty() {
                lines.push(format!("! skipped {} unreadable file(s): {}", skipped.len(), skipped.iter().take(8).cloned().collect::<Vec<_>>().join("; ")));
            }
            out.log = lines;
        }
        Ok(out)
    }

    /// Lists media files on disk (without importing), grouped by folder.
    pub fn find_files(&self, root: &str, pattern: Option<&str>, recursive: bool, list_files: bool, limit: usize) -> Result<String> {
        self.check_path(root)?;
        let rp = Path::new(root);
        if !rp.is_dir() {
            bail!("{root} is not a folder");
        }
        let mut files = vec![];
        walk(rp, recursive, 0, pattern, &mut files);
        if files.is_empty() {
            return Ok(format!("no media files in {root}{}", pattern.map(|p| format!(" matching {p}")).unwrap_or_default()));
        }
        let mut by_dir: BTreeMap<PathBuf, Vec<&PathBuf>> = BTreeMap::new();
        for f in &files {
            by_dir.entry(f.parent().map(|d| d.to_path_buf()).unwrap_or_default()).or_default().push(f);
        }
        let total: u64 = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
        let mut s = format!("{} media files, {} in {} folder(s){}", files.len(), fmt_bytes(total), by_dir.len(), if files.len() >= MAX_FILES { " (stopped at the limit)" } else { "" });
        let mut shown = 0;
        for (d, fs) in &by_dir {
            let size: u64 = fs.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
            let mut exts: BTreeMap<String, usize> = BTreeMap::new();
            for f in fs {
                *exts.entry(f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()).or_default() += 1;
            }
            let _ = write!(s, "\n{}  ({} files, {}: {})", d.display(), fs.len(), fmt_bytes(size), exts.iter().map(|(e, n)| format!("{n} {e}")).collect::<Vec<_>>().join(", "));
            if list_files {
                for f in fs.iter() {
                    if shown >= limit {
                        break;
                    }
                    let _ = write!(s, "\n  {}", f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                    shown += 1;
                }
            }
        }
        if list_files && shown < files.len() {
            let _ = write!(s, "\n… {} more (raise limit or narrow pattern)", files.len() - shown);
        }
        Ok(s)
    }

    /// Filtered, sorted, paged media listing with a totals header.
    pub fn media_listing(&self, f: &MediaFilter, sort: &str, offset: usize, limit: usize, detail: u8) -> String {
        let (p, ..) = self.snapshot();
        if p.media.is_empty() {
            return "no media (import files or folders with the media tool)".into();
        }
        let used = used_media(&p);
        let mut v: Vec<&Media> = p.media.iter().filter(|m| f.keep(m, &used)).collect();
        match sort {
            "name" => v.sort_by_key(|m| m.name.to_lowercase()),
            "dur" | "duration" => v.sort_by(|a, b| b.dur.partial_cmp(&a.dur).unwrap_or(std::cmp::Ordering::Equal)),
            "date" | "mtime" => v.sort_by_key(|m| std::cmp::Reverse(m.mtime)),
            "size" => v.sort_by_key(|m| std::cmp::Reverse(m.size)),
            "path" => v.sort_by_key(|m| m.path.to_lowercase()),
            _ => {}
        }
        let total_dur: f64 = v.iter().map(|m| m.dur).sum();
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for m in &v {
            *kinds.entry(kind_name(m)).or_default() += 1;
        }
        let filtered = v.len() != p.media.len();
        let mut s = format!(
            "{} media{} ({}){}",
            v.len(),
            if filtered { format!(" of {}", p.media.len()) } else { String::new() },
            kinds.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", "),
            if total_dur > 0.0 { format!(", {} total", fmt_dur(total_dur)) } else { String::new() }
        );
        let end = (offset + limit).min(v.len());
        if offset > 0 || end < v.len() {
            let _ = write!(s, " | showing {}-{}", offset + 1, end);
            if end < v.len() {
                let _ = write!(s, ", next offset={end}");
            }
        }
        for m in v.iter().skip(offset).take(limit) {
            s.push('\n');
            s.push_str(&media_line(&p, m, detail));
        }
        s
    }

    /// One frame of a media file (premultiplied RGBA, `w` px wide), cached.
    /// Audio gives a waveform picture.
    pub fn media_frame(&self, m: &Media, t: f64, w: u32) -> Result<Arc<Pixmap>> {
        let w = w.clamp(16, 1920) & !1;
        let key = format!("{}|{}|{:.2}|{w}", m.path, m.mtime, t);
        if let Some(pm) = self.thumbs.lock().get(&key) {
            return Ok(pm.clone());
        }
        let pm = match m.kind {
            MediaKind::Image => self.frames.image(&m.path, w, ((m.h as f32 / m.w.max(1) as f32) * w as f32).round().max(2.0) as u32)?,
            MediaKind::Svg => self.frames.svg(&m.path, w, ((m.h as f32 / m.w.max(1) as f32) * w as f32).round().max(2.0) as u32)?,
            MediaKind::Video => {
                let (raw, _) = self.ff.run_capture(&[
                    "-ss".into(),
                    format!("{:.3}", t.clamp(0.0, (m.dur - 0.05).max(0.0))),
                    "-i".into(),
                    m.path.clone(),
                    "-frames:v".into(),
                    "1".into(),
                    "-vf".into(),
                    format!("scale={w}:-2"),
                    "-f".into(),
                    "rawvideo".into(),
                    "-pix_fmt".into(),
                    "rgba".into(),
                    "-".into(),
                ])?;
                let h = (raw.len() / (w as usize * 4)) as u32;
                if h == 0 {
                    bail!("no frame at {t}s in {}", m.name);
                }
                Arc::new(crate::render::decode::pixmap_from_rgba(raw[..(w * h * 4) as usize].to_vec(), w, h, true).ok_or_else(|| anyhow!("bad frame"))?)
            }
            MediaKind::Audio => {
                let h = (w * 9 / 16).max(8);
                let (raw, _) = self.ff.run_capture(&[
                    "-i".into(),
                    m.path.clone(),
                    "-filter_complex".into(),
                    format!("aformat=channel_layouts=mono,showwavespic=s={w}x{h}:colors=0x9b8cff:scale=sqrt"),
                    "-frames:v".into(),
                    "1".into(),
                    "-f".into(),
                    "rawvideo".into(),
                    "-pix_fmt".into(),
                    "rgba".into(),
                    "-".into(),
                ])?;
                if raw.len() < (w * h * 4) as usize {
                    bail!("no waveform for {}", m.name);
                }
                Arc::new(crate::render::decode::pixmap_from_rgba(raw[..(w * h * 4) as usize].to_vec(), w, h, true).ok_or_else(|| anyhow!("bad waveform"))?)
            }
        };
        let mut c = self.thumbs.lock();
        if c.len() > 600 {
            c.clear();
        }
        c.insert(key, pm.clone());
        Ok(pm)
    }

    /// A representative time for a media's poster frame.
    pub fn poster_time(m: &Media) -> f64 {
        if m.is_still() || m.dur <= 0.0 {
            0.0
        } else if m.gfx.is_some() {
            m.dur * 0.6
        } else {
            (m.dur * 0.33).min(m.dur - 0.05).max(0.0)
        }
    }

    /// Contact sheet of poster frames, one cell per media, labelled with id and length.
    pub fn media_sheet(&self, ids: &[String], cell_w: u32) -> Result<(Pixmap, String)> {
        let (p, ..) = self.snapshot();
        let ms: Vec<Media> = ids.iter().filter_map(|id| p.media(id).cloned()).collect();
        if ms.is_empty() {
            bail!("no media to show");
        }
        let cell_w = cell_w.clamp(96, 480);
        let cell_h = cell_w * 9 / 16;
        let tiles: Vec<Tile> = ms
            .par_iter()
            .map(|m| {
                let img = self.media_frame(m, Self::poster_time(m), cell_w).map(|a| (*a).clone()).unwrap_or_else(|_| Pixmap::new(cell_w, cell_h).unwrap());
                let short: String = m.name.chars().take(14).collect();
                let label = if m.is_still() { format!("{} {short}", m.id) } else { format!("{} {} {short}", m.id, fmt_dur(m.dur)) };
                Tile { label, img: if m.alpha { on_checker(&img) } else { img } }
            })
            .collect();
        let cols = ((tiles.len() as f64).sqrt() * 1.25).ceil().max(1.0) as usize;
        let pm = encode::tile_sheet(&tiles, cell_w, cell_h, cols.min(8), false)?;
        let cap = format!("{} media: {}", ms.len(), ms.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(" "));
        Ok((pm, cap))
    }

    /// Filmstrip of one media: `n` frames across `range` (source seconds), labelled with times.
    pub fn media_strip(&self, id: &str, n: usize, range: Option<(f64, f64)>, cell_w: u32) -> Result<(Pixmap, String)> {
        let (p, ..) = self.snapshot();
        let m = p.media(id).cloned().ok_or_else(|| anyhow!("unknown media {id}"))?;
        if m.is_still() {
            let (pm, _) = self.media_sheet(&[m.id.clone()], cell_w.max(320))?;
            return Ok((pm, format!("{} still {}x{}", m.id, m.w, m.h)));
        }
        let (a, b) = range.unwrap_or((0.0, m.dur));
        let (a, b) = (a.clamp(0.0, m.dur), b.clamp(0.0, m.dur));
        if b <= a {
            bail!("empty range for {id} ({} long)", fmt_dur(m.dur));
        }
        let n = n.clamp(1, 24);
        let times: Vec<f64> = (0..n).map(|i| a + (i as f64 + 0.5) * (b - a) / n as f64).collect();
        let cell_w = cell_w.clamp(96, 640);
        let aspect = if m.is_visual() && m.w > 0 { m.h as f64 / m.w as f64 } else { 9.0 / 16.0 };
        let cell_h = ((cell_w as f64 * aspect).round() as u32).max(16);
        let tiles: Vec<Tile> = times
            .par_iter()
            .map(|t| {
                let img = self.media_frame(&m, *t, cell_w).map(|a| (*a).clone()).unwrap_or_else(|_| Pixmap::new(cell_w, cell_h).unwrap());
                Tile { label: format!("{}s", fmt_t((t * 10.0).round() / 10.0)), img: if m.alpha { on_checker(&img) } else { img } }
            })
            .collect();
        let cols = if m.kind == MediaKind::Audio { 1 } else if cell_h > cell_w { n.min(8) } else { (n as f64).sqrt().ceil() as usize };
        let pm = encode::tile_sheet(&tiles, cell_w, cell_h, cols, false)?;
        Ok((pm, format!("{} {} frames {}-{}s (source time)", m.id, n, fmt_t(a), fmt_t(b))))
    }

    /// Runs one analysis over many media (bounded parallelism); one line each.
    pub fn analyze_many(&self, ids: &[String], kind: &str, opts: &BTreeMap<String, String>) -> Result<String> {
        let threads = if matches!(kind, "transcript" | "transcribe" | "speech" | "stt") { 1 } else { 4 };
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
        let lines: Vec<String> = pool.install(|| {
            ids.par_iter()
                .map(|id| match self.analyze(id, kind, opts) {
                    Ok((mid, v)) => format!("{mid} {}", crate::analyze::summarize(kind, &v, 8)),
                    Err(e) => format!("{id} ! {}", e.to_string().lines().next().unwrap_or("")),
                })
                .collect()
        });
        Ok(format!("{kind} for {} media\n{}", ids.len(), lines.join("\n")))
    }

    /// Searches media names, paths, notes and tags, plus every cached transcript.
    pub fn search(&self, q: &str, limit: usize) -> String {
        let (p, ..) = self.snapshot();
        let words: Vec<String> = q.to_lowercase().split_whitespace().map(|s| s.to_string()).collect();
        if words.is_empty() {
            return "search needs q".into();
        }
        let mut out = vec![];
        for m in &p.media {
            let text = media_text(m);
            if words.iter().all(|w| text.contains(w.as_str())) {
                out.push(media_line(&p, m, 1));
            }
        }
        let mut with_tx = 0;
        let mut hits = 0;
        for m in &p.media {
            let Some(v) = self.cached_analysis(m, "transcript") else { continue };
            with_tx += 1;
            for seg in v["segments"].as_array().into_iter().flatten() {
                let text = seg["text"].as_str().unwrap_or("");
                let lower = text.to_lowercase();
                if words.iter().all(|w| lower.contains(w.as_str())) {
                    hits += 1;
                    if out.len() < limit {
                        let (s, e) = (seg["s"].as_f64().unwrap_or(0.0), seg["e"].as_f64().unwrap_or(0.0));
                        out.push(format!("{} {}-{} \"{}\"", m.id, fmt_t((s * 10.0).round() / 10.0), fmt_t((e * 10.0).round() / 10.0), text.trim()));
                    }
                }
            }
        }
        let mut s = if out.is_empty() { format!("no matches for '{q}'") } else { out.join("\n") };
        if hits > 0 && out.len() >= limit {
            let _ = write!(s, "\n… more transcript matches (raise limit)");
        }
        if with_tx == 0 && p.media.iter().any(|m| m.has_audio) {
            s.push_str("\n(no transcripts yet: analyze target=all kind=transcript to search speech)");
        }
        s
    }
}

/// Composites onto a dark checkerboard so transparent media are visible.
pub fn on_checker(src: &Pixmap) -> Pixmap {
    let (w, h) = (src.width(), src.height());
    let mut bg = Pixmap::new(w, h).unwrap();
    let sq = (h / 8).max(4);
    for y in 0..h {
        for x in 0..w {
            let v = if ((x / sq) + (y / sq)) % 2 == 0 { 70u8 } else { 48u8 };
            let o = ((y * w + x) * 4) as usize;
            bg.data_mut()[o..o + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    bg.draw_pixmap(0, 0, src.as_ref(), &resvg::tiny_skia::PixmapPaint::default(), resvg::tiny_skia::Transform::identity(), None);
    bg
}

/// Parses the `media list` / `preview media` filter arguments.
pub fn filter_from_json(a: &Value) -> MediaFilter {
    let s = |k: &str| a[k].as_str().map(|x| x.to_string()).filter(|x| !x.is_empty());
    MediaFilter {
        kind: s("kind"),
        q: s("q").or_else(|| s("query")),
        tag: s("tag"),
        min_dur: a["min_dur"].as_f64(),
        max_dur: a["max_dur"].as_f64(),
        orient: s("orientation").or_else(|| s("orient")),
        used: a["used"].as_bool(),
    }
}
