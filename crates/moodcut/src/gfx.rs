//! HTML motion graphics. An AI (or a person) writes a web page animated with
//! CSS / Web Animations / JS (GSAP, anime.js, Lottie, canvas, SVG…); MoodCut
//! renders it frame by frame in headless Edge/Chrome and encodes the frames to
//! a lossless video with alpha that is then imported like any other media.
//!
//! Rendering is deterministic: `assets/gfx_shim.js` runs before the page's own
//! scripts and replaces its clocks, then every frame is seeked explicitly
//! before the screenshot, so output never depends on machine speed.
//!
//! The page is served from a throwaway localhost server that exposes only the
//! graphic's folder, the bundled fonts and the project media it references
//! (`mc://m3`), never the MoodCut API.
//!
//! Graphics stay editable without code: their `{{name|default}}` placeholders are
//! fields ([`fields`]) the editor shows as controls and the AI sets with `vars`.
//! The editor previews a graphic live in a sandboxed iframe running the same
//! virtual clock ([`build_page`] with `preview`), and renders only on Apply.

pub mod fields;

use crate::model::project::{Brand, Media, Project};
use crate::render::ffmpeg::{command, Ff};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SHIM: &str = include_str!("../assets/gfx_shim.js");
/// Lets the editor drive the virtual clock of a live preview.
const BRIDGE: &str = include_str!("../assets/gfx_preview.js");

/// Source of a rendered graphic, kept on its media so it can be re-rendered.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Gfx {
    /// The .html file (as written by the AI, before variable substitution). It mirrors `src`.
    pub html: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    /// Seconds; 0 = still image.
    pub dur: f64,
    #[serde(default)]
    pub alpha: bool,
    /// Field values set on this graphic (unset fields render their defaults).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
    /// The page source, kept in the project so undo and redo restore it (the .html
    /// file is rewritten to match). Empty in projects from before it was kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub src: String,
    /// The brand kit it was rendered with, when the page uses the brand variables.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub brand: String,
    /// Stills: the animation time captured (seconds).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub at: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Gfx {
    /// The page source: the .html file (it may have been edited outside MoodCut),
    /// else the copy kept in the project.
    pub fn source(&self) -> Result<String> {
        match std::fs::read_to_string(&self.html) {
            Ok(s) => Ok(s),
            Err(_) if !self.src.is_empty() => Ok(self.src.clone()),
            Err(e) => Err(anyhow!("reading {}: {e}", self.html)),
        }
    }

    /// Rendered with brand colors that have changed since.
    pub fn is_stale(&self, brand: &Brand) -> bool {
        !self.brand.is_empty() && self.brand != brand_key(brand)
    }
}

/// Graphic sources by media id: (html file, source).
pub fn sources(p: &Project) -> BTreeMap<String, (String, String)> {
    p.media.iter().filter_map(|m| m.gfx.as_ref().map(|g| (m.id.clone(), (g.html.clone(), g.src.clone())))).collect()
}

/// After undo / redo: rewrites the .html files whose source changed, so each file
/// always holds what the project has (a re-render reads the file).
pub fn mirror_sources(before: &BTreeMap<String, (String, String)>, after: &BTreeMap<String, (String, String)>) {
    for (id, (html, src)) in after {
        if src.is_empty() || before.get(id).is_some_and(|b| b.0 == *html && b.1 == *src) {
            continue;
        }
        let path = Path::new(html);
        if path.parent().is_some_and(|d| d.is_dir()) && std::fs::read_to_string(path).ok().as_deref() != Some(src.as_str()) {
            let _ = std::fs::write(path, src);
        }
    }
}

fn write_source(g: &Gfx) -> Option<String> {
    if g.src.is_empty() {
        return None;
    }
    std::fs::write(&g.html, &g.src).err().map(|e| format!("! couldn't write {}: {e} (the page is kept in the project)", g.html))
}

pub struct RenderSpec<'a> {
    /// Page source after [`build_page`].
    pub html: &'a str,
    /// Folder served as the page's origin (relative URLs resolve here).
    pub dir: &'a Path,
    /// Media ids the page may load as `mc://m3`.
    pub media: BTreeMap<String, PathBuf>,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    pub dur: f64,
    pub alpha: bool,
    pub vars: &'a BTreeMap<String, String>,
    /// Frames to keep as PNG for the preview sheet (times in seconds).
    pub keep: Vec<f64>,
    /// For stills (dur 0): the animation time to capture, in seconds.
    pub still_at: f64,
}

pub struct RenderOut {
    pub frames: usize,
    /// JS exceptions, console errors and failed requests (deduplicated).
    pub errors: Vec<String>,
    /// (time, png) for the requested preview times.
    pub kept: Vec<(f64, Vec<u8>)>,
    /// Union of visible element boxes at the kept times, in px.
    pub bounds: Option<[f64; 4]>,
    pub empty: bool,
    pub secs: f64,
}

// ------------------------------------------------------------------ browser

/// Edge or Chrome/Chromium (MOODCUT_BROWSER overrides).
pub fn find_browser() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MOODCUT_BROWSER") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let mut cands: Vec<PathBuf> = vec![];
    if cfg!(windows) {
        for base in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Ok(b) = std::env::var(base) {
                cands.push(PathBuf::from(&b).join("Microsoft\\Edge\\Application\\msedge.exe"));
                cands.push(PathBuf::from(&b).join("Google\\Chrome\\Application\\chrome.exe"));
                cands.push(PathBuf::from(&b).join("Chromium\\Application\\chrome.exe"));
            }
        }
        // AI clients may start MoodCut with a minimal environment
        cands.push(PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"));
        cands.push(PathBuf::from(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"));
        cands.push(PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"));
    } else if cfg!(target_os = "macos") {
        for app in ["Google Chrome", "Microsoft Edge", "Chromium", "Brave Browser"] {
            cands.push(PathBuf::from(format!("/Applications/{app}.app/Contents/MacOS/{app}")));
        }
    } else {
        for b in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "microsoft-edge-stable", "brave-browser"] {
            if let Some(p) = crate::setup::which(b) {
                cands.push(p);
            }
        }
        // AI clients may start MoodCut with a minimal environment (no PATH extras)
        for p in ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser", "/snap/bin/chromium", "/opt/google/chrome/chrome", "/opt/microsoft/msedge/msedge"] {
            cands.push(PathBuf::from(p));
        }
        // Playwright's bundled Chromium
        let mut roots = vec![PathBuf::from("/ms-playwright"), PathBuf::from("/opt/pw-browsers")];
        if let Ok(p) = std::env::var("PLAYWRIGHT_BROWSERS_PATH") {
            roots.push(PathBuf::from(p));
        }
        if let Ok(h) = std::env::var("HOME") {
            roots.push(PathBuf::from(h).join(".cache/ms-playwright"));
        }
        for r in roots {
            if let Ok(rd) = std::fs::read_dir(&r) {
                let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("chromium-"))).collect();
                dirs.sort();
                for d in dirs.into_iter().rev() {
                    cands.push(d.join("chrome-linux/chrome"));
                }
            }
        }
    }
    cands.into_iter().find(|p| p.is_file())
}

struct Browser {
    child: Child,
    profile: PathBuf,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // the profile can stay locked for a moment after exit on Windows
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(1) == Some("0"))).unwrap_or(false)
}

fn launch(w: u32, h: u32) -> Result<(Browser, String)> {
    let exe = find_browser().ok_or_else(|| anyhow!("no Edge/Chrome/Chromium found for HTML graphics; install one or set MOODCUT_BROWSER to its executable"))?;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let profile = crate::config::cache_dir().join(format!("browser-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&profile);
    std::fs::create_dir_all(&profile)?;
    let mut cmd = command(&exe);
    cmd.args([
        "--headless=new",
        "--remote-debugging-port=0",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding",
        "--disable-sync",
        "--disable-translate",
        "--disable-default-apps",
        "--hide-scrollbars",
        "--mute-audio",
        "--force-color-profile=srgb",
        "--font-render-hinting=none",
        "--run-all-compositor-stages-before-draw",
        "--enable-unsafe-swiftshader",
        "--autoplay-policy=no-user-gesture-required",
    ]);
    cmd.arg(format!("--user-data-dir={}", profile.display()));
    cmd.arg(format!("--window-size={w},{h}"));
    // frames are captured one by one, so software rendering costs little, gives the
    // same pixels on every PC and leaves the GPU to the editor (MOODCUT_BROWSER_GPU=1 to use it)
    if !std::env::var("MOODCUT_BROWSER_GPU").is_ok_and(|v| matches!(v.as_str(), "1" | "on" | "true")) {
        cmd.arg("--disable-gpu");
    }
    if running_as_root() || std::env::var("MOODCUT_BROWSER_NO_SANDBOX").is_ok() {
        cmd.arg("--no-sandbox");
    }
    cmd.arg("about:blank");
    let child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().with_context(|| format!("starting {}", exe.display()))?;
    let br = Browser { child, profile: profile.clone() };
    // the browser writes its DevTools port and browser target path here
    let port_file = profile.join("DevToolsActivePort");
    let t0 = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(&port_file) {
            let mut l = s.lines();
            if let (Some(port), Some(path)) = (l.next(), l.next()) {
                if !port.trim().is_empty() && path.starts_with('/') {
                    return Ok((br, format!("ws://127.0.0.1:{}{}", port.trim(), path.trim())));
                }
            }
        }
        if t0.elapsed() > Duration::from_secs(20) {
            bail!("{} didn't start (no DevTools port after 20s)", exe.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ------------------------------------------------------------------ DevTools protocol

struct Cdp {
    ws: tungstenite::WebSocket<TcpStream>,
    next: u64,
    session: Option<String>,
    errors: Vec<String>,
}

impl Cdp {
    fn connect(url: &str) -> Result<Cdp> {
        let addr = url.trim_start_matches("ws://").split('/').next().unwrap_or("").to_string();
        let stream = TcpStream::connect(&addr).with_context(|| format!("connecting to browser at {addr}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        stream.set_nodelay(true)?;
        let (ws, _) = tungstenite::client(url, stream).map_err(|e| anyhow!("DevTools handshake: {e}"))?;
        Ok(Cdp { ws, next: 0, session: None, errors: vec![] })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = &self.session {
            msg["sessionId"] = json!(s);
        }
        self.ws.send(tungstenite::Message::text(msg.to_string())).map_err(|e| anyhow!("DevTools send: {e}"))?;
        loop {
            let m = self.ws.read().map_err(|e| anyhow!("DevTools ({method}): {e}"))?;
            let txt = match m {
                tungstenite::Message::Text(t) => t.to_string(),
                tungstenite::Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                tungstenite::Message::Close(_) => bail!("the browser closed the DevTools connection"),
                _ => continue,
            };
            let v: Value = serde_json::from_str(&txt)?;
            if v["id"].as_u64() == Some(id) {
                if let Some(e) = v.get("error") {
                    bail!("{method}: {}", e["message"].as_str().unwrap_or("error"));
                }
                return Ok(v["result"].clone());
            }
            self.event(&v);
        }
    }

    fn event(&mut self, v: &Value) {
        let p = &v["params"];
        let msg = match v["method"].as_str() {
            Some("Runtime.exceptionThrown") => {
                let d = &p["exceptionDetails"];
                let desc = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("exception");
                Some(format!("JS error: {} (line {})", desc.lines().next().unwrap_or(desc), d["lineNumber"].as_i64().unwrap_or(0) + 1))
            }
            Some("Runtime.consoleAPICalled") if p["type"] == "error" => {
                let args: Vec<String> = p["args"].as_array().map(|a| a.iter().map(|x| x["value"].as_str().map(|s| s.to_string()).or_else(|| x["description"].as_str().map(|s| s.to_string())).unwrap_or_default()).collect()).unwrap_or_default();
                Some(format!("console.error: {}", args.join(" ").lines().next().unwrap_or("")))
            }
            Some("Log.entryAdded") if p["entry"]["level"] == "error" => {
                let e = &p["entry"];
                Some(format!("{} {}", e["text"].as_str().unwrap_or(""), e["url"].as_str().unwrap_or("")).trim().to_string())
            }
            _ => None,
        };
        if let Some(m) = msg {
            let m: String = m.chars().take(300).collect();
            if !self.errors.contains(&m) && self.errors.len() < 20 {
                self.errors.push(m);
            }
        }
    }

    /// Evaluates an expression in the page, awaiting promises.
    fn eval(&mut self, expr: &str) -> Result<Value> {
        let r = self.call("Runtime.evaluate", json!({"expression": expr, "awaitPromise": true, "returnByValue": true}))?;
        if let Some(d) = r.get("exceptionDetails") {
            let desc = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("error");
            bail!("page script error: {}", desc.lines().next().unwrap_or(desc));
        }
        Ok(r["result"]["value"].clone())
    }
}

// ------------------------------------------------------------------ asset server

/// Serves the graphic's folder, bundled fonts and referenced media on a random
/// localhost port for the duration of one render.
struct AssetServer {
    port: u16,
    stop: Arc<AtomicBool>,
}

impl Drop for AssetServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port)); // wake the accept loop
    }
}

struct Site {
    index: String,
    dir: PathBuf,
    media: BTreeMap<String, PathBuf>,
}

fn serve_assets(site: Site) -> Result<AssetServer> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    let site = Arc::new(site);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            if st.load(Ordering::Relaxed) {
                break;
            }
            let Ok(conn) = conn else { continue };
            let site = site.clone();
            std::thread::spawn(move || {
                let _ = handle_http(conn, &site);
            });
        }
    });
    Ok(AssetServer { port, stop })
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn handle_http(mut conn: TcpStream, site: &Site) -> Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut rd = BufReader::new(conn.try_clone()?);
    let mut line = String::new();
    rd.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut range: Option<String> = None;
    loop {
        let mut h = String::new();
        if rd.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("range") {
                range = Some(v.trim().to_string());
            }
        }
    }
    let path = pct_decode(target.split(['?', '#']).next().unwrap_or("/"));
    if path == "/favicon.ico" {
        return respond(&mut conn, 204, "image/x-icon", b"", None, &method);
    }
    let (body, mime): (Vec<u8>, String) = if path == "/" || path == "/index.html" {
        (site.index.clone().into_bytes(), "text/html; charset=utf-8".into())
    } else {
        match site_file(&site.dir, &site.media, path.trim_start_matches('/')) {
            Some(SiteFile::Bytes(data, mime)) => (data.to_vec(), mime.to_string()),
            Some(SiteFile::File(f, mime)) => (std::fs::read(&f)?, mime),
            None => return respond(&mut conn, 404, "text/plain", b"not found", None, &method),
        }
    };
    // byte ranges, so <video> can seek
    if let Some(r) = range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
        let (a, b) = r.split_once('-').unwrap_or((r, ""));
        let len = body.len();
        let start = a.parse::<usize>().unwrap_or(0).min(len);
        let end = b.parse::<usize>().map(|e| e + 1).unwrap_or(len).clamp(start, len);
        return respond(&mut conn, 206, &mime, &body[start..end], Some((start, end, len)), &method);
    }
    respond(&mut conn, 200, &mime, &body, None, &method)
}

fn respond(conn: &mut TcpStream, code: u16, mime: &str, body: &[u8], range: Option<(usize, usize, usize)>, method: &str) -> Result<()> {
    let reason = match code {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        _ => "Not Found",
    };
    let mut head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n", body.len());
    if let Some((s, e, n)) = range {
        head.push_str(&format!("Content-Range: bytes {s}-{}/{n}\r\n", e.saturating_sub(1)));
    }
    head.push_str("\r\n");
    conn.write_all(head.as_bytes())?;
    if method != "HEAD" {
        conn.write_all(body)?;
    }
    conn.flush()?;
    Ok(())
}

// ------------------------------------------------------------------ page source

/// What a page may load besides itself.
pub enum SiteFile {
    Bytes(&'static [u8], &'static str),
    /// A file and its media type.
    File(PathBuf, String),
}

/// Resolves a path a graphic's page asks for (decoded, without the leading slash):
/// the bundled fonts (`mc-fonts/…`), library media (`mc-media/<id>`) or a file next
/// to the page. Nothing outside the page's folder is reachable.
pub fn site_file(dir: &Path, media: &BTreeMap<String, PathBuf>, path: &str) -> Option<SiteFile> {
    if let Some(name) = path.strip_prefix("mc-fonts/") {
        use crate::render::text::{INTER_BLACK, INTER_BOLD, INTER_REGULAR};
        let data: &'static [u8] = match name {
            "Inter-Regular.otf" => INTER_REGULAR,
            "Inter-Bold.otf" => INTER_BOLD,
            "Inter-Black.otf" => INTER_BLACK,
            _ => return None,
        };
        return Some(SiteFile::Bytes(data, "font/otf"));
    }
    let file = match path.strip_prefix("mc-media/") {
        Some(id) => media.get(id).cloned()?,
        None => {
            let rel = Path::new(path);
            if path.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
                return None;
            }
            dir.join(rel)
        }
    };
    file.is_file().then(|| {
        let mime = mime_guess::from_path(&file).first_or_octet_stream().to_string();
        SiteFile::File(file, mime)
    })
}

fn base_css(base: &str) -> String {
    format!(
        "@font-face{{font-family:Inter;src:url({base}/mc-fonts/Inter-Regular.otf);font-weight:100 500}}\
         @font-face{{font-family:Inter;src:url({base}/mc-fonts/Inter-Bold.otf);font-weight:600 800}}\
         @font-face{{font-family:Inter;src:url({base}/mc-fonts/Inter-Black.otf);font-weight:900}}"
    )
}

/// The project's brand kit as CSS variables (`var(--brand-accent)` ...), so graphics match the titles.
pub fn brand_css(b: &Brand) -> String {
    let weight = b.weight.as_deref().map(|w| match w.to_ascii_lowercase().as_str() {
        "thin" => "100".to_string(),
        "extralight" | "ultralight" => "200".into(),
        "light" => "300".into(),
        "regular" | "normal" => "400".into(),
        "medium" => "500".into(),
        "semibold" | "demibold" => "600".into(),
        "bold" => "700".into(),
        "extrabold" | "ultrabold" => "800".into(),
        "black" | "heavy" => "900".into(),
        other => other.to_string(),
    });
    let mut v = vec![format!("--brand-font:{}", b.font.as_deref().map(|f| format!("'{}',Inter,system-ui,sans-serif", f.replace('\'', ""))).unwrap_or_else(|| "Inter,system-ui,sans-serif".into()))];
    v.push(format!("--brand-weight:{}", weight.unwrap_or_else(|| "800".into())));
    for (k, val, def) in [("primary", &b.primary, "#111111"), ("accent", &b.accent, "#ff3b6b"), ("text", &b.text, "#ffffff"), ("dark", &b.dark, "#0b0b10")] {
        v.push(format!("--brand-{k}:{}", val.as_deref().unwrap_or(def)));
    }
    format!(":root{{{}}}", v.join(";"))
}

/// Identifies a brand kit's look, to tell when a graphic was rendered with other colors.
pub fn brand_key(b: &Brand) -> String {
    format!("{:08x}", crate::render::eval::hash_str(&brand_css(b)) as u32)
}

/// How a page is built.
pub struct PageOpts<'a> {
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    /// Seconds (0 = still).
    pub dur: f64,
    /// More CSS for the head (the brand variables).
    pub extra_css: &'a str,
    /// URL prefix the page is served under ("" for renders, which serve it at the root).
    pub base: &'a str,
    /// A live preview in the editor: the virtual clock and the editor bridge run
    /// before the page's own scripts (renders inject the clock over DevTools instead).
    pub preview: bool,
}

pub struct Page {
    pub html: String,
    /// `MC.vars`: every field's value or default, plus the other vars.
    pub vars: BTreeMap<String, String>,
    /// Placeholders with no value and no default (they render empty).
    pub missing: Vec<String>,
}

/// JSON that can sit inside `<script>`.
fn script_json(v: &Value) -> String {
    v.to_string().replace('<', "\\u003c")
}

/// Starts with `<!doctype` or `<html` (after comments and whitespace).
fn is_document(s: &str) -> bool {
    let mut rest = s.trim_start();
    while let Some(after) = rest.strip_prefix("<!--") {
        rest = after.find("-->").map(|i| after[i + 3..].trim_start()).unwrap_or("");
    }
    let l = rest.get(..9).unwrap_or(rest).to_ascii_lowercase();
    l.starts_with("<!doctype") || l.starts_with("<html")
}

/// Position of `<name` as a whole tag name (`<head` but not `<header`).
fn find_open_tag(lower: &str, name: &str) -> Option<usize> {
    let pat = format!("<{name}");
    let mut from = 0;
    while let Some(i) = lower[from..].find(&pat).map(|x| x + from) {
        let next = lower.as_bytes().get(i + pat.len()).copied();
        if matches!(next, Some(b'>') | Some(b'/')) || next.is_some_and(|c| c.is_ascii_whitespace()) {
            return Some(i);
        }
        from = i + 1;
    }
    None
}

/// Puts `head` at the start of a full document's head (making one if it has none).
fn insert_head(mut s: String, head: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let after_tag = |i: usize| lower[i..].find('>').map(|x| i + x + 1).unwrap_or(lower.len());
    if let Some(i) = find_open_tag(&lower, "head") {
        s.insert_str(after_tag(i), head);
    } else if let Some(i) = find_open_tag(&lower, "html") {
        s.insert_str(after_tag(i), &format!("<head>{head}</head>"));
    } else {
        let at = lower.find("<!doctype").map(after_tag).unwrap_or(0);
        s.insert_str(at, &format!("<head>{head}</head>"));
    }
    s
}

/// The page to load: fields filled, `mc://m3` mapped to served media, the bundled
/// fonts and `--mc-dur` / `--mc-w` / `--mc-h` / `--mc-fps` (stills never reach their
/// end, so an outro timed from --mc-dur doesn't play in them), and a body fragment
/// wrapped in a full-frame transparent page.
pub fn build_page(src: &str, vars: &BTreeMap<String, String>, o: &PageOpts) -> Page {
    let f = fields::fill(src, vars);
    let media = format!("{}/mc-media/", o.base);
    let body = f.html.replace("mc://", &media);
    let page_vars: BTreeMap<String, String> = f.vars.into_iter().map(|(k, v)| (k, v.replace("mc://", &media))).collect();
    let dur = if o.dur > 0.0 { format!("{}s", fmt_num(o.dur)) } else { "3600s".into() };
    let mc_css = format!(":root{{--mc-dur:{dur};--mc-w:{}px;--mc-h:{}px;--mc-fps:{}}}", o.w, o.h, fmt_num(o.fps));
    let script = if o.preview {
        let cfg = json!({"duration": o.dur, "fps": o.fps, "width": o.w, "height": o.h, "frames": frame_count(o.fps, o.dur), "vars": page_vars});
        format!("<script>window.MC = Object.assign(window.MC || {{}}, {});\n{SHIM}\n{BRIDGE}</script>", script_json(&cfg))
    } else {
        String::new()
    };
    let css = format!("{}{mc_css}{}", base_css(o.base), o.extra_css);
    let html = if is_document(&body) {
        insert_head(body, &format!("{script}<style>{css}</style>"))
    } else {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">{script}<style>{css}\
             *{{box-sizing:border-box}}html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:transparent}}\
             body{{font-family:Inter,system-ui,sans-serif;color:#fff;-webkit-font-smoothing:antialiased}}</style></head><body>{body}</body></html>"
        )
    };
    Page { html, vars: page_vars, missing: f.missing }
}

/// Fields and authoring hints of a page, for the editor.
pub fn info_json(src: &str) -> Value {
    let p = fields::parse(src);
    json!({"fields": p.fields, "warnings": p.warnings, "uses_duration": fields::uses_duration(src), "hardcoded": fields::hardcoded(src)})
}

/// Frame times (seconds) for a graphic.
pub fn frame_count(fps: f64, dur: f64) -> usize {
    if dur <= 0.0 {
        1
    } else {
        ((dur * fps).round() as usize).max(1)
    }
}

// ------------------------------------------------------------------ render

/// Renders the page to `out` (.mov PNG with alpha, or .png for stills).
/// `ff` is only needed for animated graphics (stills are written as PNG directly).
pub fn render(ff: Option<&Ff>, spec: &RenderSpec, out: &Path, cancel: Option<&AtomicBool>) -> Result<RenderOut> {
    let t_start = Instant::now();
    let (w, h) = (spec.w.clamp(16, 7680), spec.h.clamp(16, 7680));
    let site = Site { index: spec.html.to_string(), dir: spec.dir.to_path_buf(), media: spec.media.clone() };
    let server = serve_assets(site)?;
    let (_browser, ws_url) = launch(w, h)?;
    let mut cdp = Cdp::connect(&ws_url)?;
    let target = cdp.call("Target.createTarget", json!({"url": "about:blank"}))?;
    let target_id = target["targetId"].as_str().ok_or_else(|| anyhow!("no target id"))?.to_string();
    let att = cdp.call("Target.attachToTarget", json!({"targetId": target_id, "flatten": true}))?;
    cdp.session = att["sessionId"].as_str().map(|s| s.to_string());
    cdp.call("Page.enable", json!({}))?;
    cdp.call("Runtime.enable", json!({}))?;
    let _ = cdp.call("Log.enable", json!({}));
    cdp.call("Emulation.setDeviceMetricsOverride", json!({"width": w, "height": h, "deviceScaleFactor": 1, "mobile": false}))?;
    if spec.alpha {
        cdp.call("Emulation.setDefaultBackgroundColorOverride", json!({"color": {"r": 0, "g": 0, "b": 0, "a": 0}}))?;
    }
    let n = frame_count(spec.fps, spec.dur);
    let cfg = json!({"duration": spec.dur, "fps": spec.fps, "width": w, "height": h, "frames": n, "vars": spec.vars});
    let init = format!("window.MC = Object.assign(window.MC || {{}}, {cfg});\n{SHIM}");
    cdp.call("Page.addScriptToEvaluateOnNewDocument", json!({"source": init}))?;
    let url = format!("http://127.0.0.1:{}/", server.port);
    cdp.call("Page.navigate", json!({"url": url}))?;
    // wait until the new document (with the shim) is live
    let t0 = Instant::now();
    loop {
        if cdp.eval("typeof window.__mc === 'object' && location.href").map(|v| v.as_str() == Some(url.as_str())).unwrap_or(false) {
            break;
        }
        if t0.elapsed() > Duration::from_secs(20) {
            bail!("the page didn't load within 20s");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    cdp.eval("window.__mc.ready(20000)")?;

    let still = spec.dur <= 0.0;
    let mut enc = if still {
        None
    } else {
        let ff = ff.filter(|f| f.available()).ok_or_else(|| anyhow!("animated graphics need FFmpeg (moodcut setup); a still (dur 0) doesn't"))?;
        let mut c = command(&ff.ffmpeg_path());
        c.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "image2pipe", "-framerate"]).arg(format!("{}", spec.fps)).args(["-c:v", "png", "-i", "-", "-c:v", "png", "-pix_fmt"]).arg(if spec.alpha { "rgba" } else { "rgb24" }).arg(out);
        Some(c.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("starting ffmpeg")?)
    };
    let keep_idx: Vec<usize> = spec.keep.iter().map(|t| ((t * spec.fps).round() as usize).min(n - 1)).collect();
    let mut kept = vec![];
    let mut empty = true;
    let mut bounds: Option<[f64; 4]> = None;
    if still && spec.still_at > 0.0 {
        // play up to the capture time so frame-by-frame (incremental) animations are right too
        let steps = (spec.still_at * spec.fps).floor() as usize;
        for k in 0..steps {
            cdp.eval(&format!("window.__mc.seek({})", k as f64 * 1000.0 / spec.fps))?;
        }
    }
    for i in 0..n {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            bail!("cancelled");
        }
        let ms = if still { spec.still_at.max(0.0) * 1000.0 } else { i as f64 * 1000.0 / spec.fps };
        cdp.eval(&format!("window.__mc.seek({ms})"))?;
        let shot = cdp.call("Page.captureScreenshot", json!({"format": "png", "fromSurface": true, "captureBeyondViewport": false, "optimizeForSpeed": true}))?;
        let png = base64::engine::general_purpose::STANDARD.decode(shot["data"].as_str().unwrap_or("")).context("decoding screenshot")?;
        let keep_this = keep_idx.contains(&i) || still;
        if keep_this {
            if let Ok(b) = cdp.eval("window.__mc.bounds()") {
                if let Some(a) = b.as_array().filter(|a| a.len() == 4) {
                    let v: Vec<f64> = a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect();
                    bounds = Some(match bounds {
                        None => [v[0], v[1], v[2], v[3]],
                        Some(o) => [o[0].min(v[0]), o[1].min(v[1]), o[2].max(v[2]), o[3].max(v[3])],
                    });
                }
            }
            if empty && !png_is_blank(&png, spec.alpha) {
                empty = false;
            }
            kept.push((i as f64 / spec.fps, png.clone()));
        }
        match enc.as_mut() {
            Some(child) => {
                let stdin = child.stdin.as_mut().ok_or_else(|| anyhow!("ffmpeg stdin"))?;
                if stdin.write_all(&png).is_err() {
                    break;
                }
            }
            None => std::fs::write(out, &png)?,
        }
    }
    if let Some(mut child) = enc {
        drop(child.stdin.take());
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let st = child.wait()?;
        if !st.success() {
            bail!("encoding the graphic failed: {}", err.lines().last().unwrap_or("ffmpeg error"));
        }
    }
    let _ = cdp.call("Browser.close", json!({}));
    Ok(RenderOut { frames: n, errors: std::mem::take(&mut cdp.errors), kept, bounds, empty, secs: t_start.elapsed().as_secs_f64() })
}

/// True when a screenshot has nothing visible (fully transparent, or one flat color).
fn png_is_blank(png: &[u8], alpha: bool) -> bool {
    let Ok(img) = image::load_from_memory_with_format(png, image::ImageFormat::Png) else { return false };
    let rgba = img.to_rgba8();
    let px = rgba.as_raw();
    if alpha {
        return px.chunks_exact(4).step_by(7).all(|p| p[3] == 0);
    }
    let first = &px[..4.min(px.len())];
    px.chunks_exact(4).step_by(7).all(|p| p == first)
}

// ------------------------------------------------------------------ engine integration

/// A request from the `graphic` tool / API.
#[derive(Default)]
pub struct GraphicReq {
    /// Page source (full document or body fragment). Optional when re-rendering.
    pub html: Option<String>,
    pub name: Option<String>,
    pub dur: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub fps: Option<f64>,
    pub alpha: Option<bool>,
    /// Field values, merged into the graphic's own.
    pub vars: BTreeMap<String, String>,
    /// Fields to reset to their defaults.
    pub unset: Vec<String>,
    /// Re-render this graphic in place (every clip using it updates).
    pub id: Option<String>,
    /// Make a new graphic from this one's source (usually with other vars).
    pub from: Option<String>,
    /// Also place it: `V3 at=2 dur=4` (anything `add` accepts after the media).
    pub place: Option<String>,
    /// Point this clip at the result (a variant for one clip, made with `from`).
    pub replace: Option<String>,
    /// Frames in the returned preview sheet (0 = none).
    pub preview: usize,
    /// For stills: the animation time to capture (seconds).
    pub at: Option<f64>,
    /// New html without fields gets its texts and colors made into fields (default true).
    pub editable: Option<bool>,
}

pub struct GraphicOut {
    pub id: String,
    pub text: String,
    /// JPEG contact sheet of the rendered graphic.
    pub sheet: Option<Vec<u8>>,
}

/// A live preview page (nothing rendered, nothing changed).
pub struct PreviewPage {
    pub html: String,
    /// The page's folder (its relative URLs).
    pub dir: PathBuf,
    pub media: BTreeMap<String, PathBuf>,
    /// Fields, hints and the settings used.
    pub info: Value,
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').chars().take(40).collect::<String>();
    if out.is_empty() {
        "graphic".into()
    } else {
        out
    }
}

/// Where graphics of a project live: next to the project file, else in the data dir.
pub fn graphics_dir(project_path: Option<&Path>) -> PathBuf {
    match project_path.and_then(|p| Some((p.parent()?.to_path_buf(), p.file_stem()?.to_string_lossy().to_string()))) {
        Some((dir, stem)) => dir.join(format!("{stem} assets")).join("graphics"),
        None => crate::config::ensure_data_dir().join("graphics"),
    }
}

fn unique_path(dir: &Path, slug: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{slug}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{slug}-{n}.{ext}"));
        n += 1;
    }
    p
}

/// A graphic request resolved against the project: what to render and where.
struct Prepared {
    src: String,
    /// Fields made automatically from a page that had none.
    auto: Vec<(String, fields::Kind)>,
    name: String,
    w: u32,
    h: u32,
    fps: f64,
    dur: f64,
    alpha: bool,
    at: f64,
    /// Field values set on the graphic (not the defaults).
    vars: BTreeMap<String, String>,
    dir: PathBuf,
    html_path: PathBuf,
}

impl Prepared {
    fn page_dir(&self) -> PathBuf {
        self.html_path.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| self.dir.clone())
    }
}

fn media_paths(p: &Project) -> BTreeMap<String, PathBuf> {
    p.media.iter().map(|m| (m.id.clone(), PathBuf::from(&m.path))).collect()
}

fn uses_of(p: &Project, id: &str) -> usize {
    p.comps.iter().flat_map(|c| c.items()).filter(|i| i.src.as_deref() == Some(id)).count()
}

impl crate::engine::Engine {
    fn prepare_graphic(&self, p: &Project, active: &str, path: Option<&Path>, r: &GraphicReq) -> Result<Prepared> {
        let comp = p.comp(active).or_else(|| p.main_comp()).ok_or_else(|| anyhow!("no composition"))?;
        let base_id = r.id.clone().or_else(|| r.from.clone());
        let base = match &base_id {
            Some(id) => {
                let m = p.media(id).ok_or_else(|| anyhow!("unknown media {id}"))?;
                let g = m.gfx.clone().ok_or_else(|| anyhow!("{id} isn't an HTML graphic"))?;
                Some((m.clone(), g))
            }
            None => None,
        };
        let mut src = match (&r.html, &base) {
            (Some(h), _) => h.clone(),
            (None, Some((_, g))) => g.source()?,
            (None, None) => bail!("graphic needs html (or id/from to re-render an existing graphic)"),
        };
        if src.trim().is_empty() {
            bail!("html is empty");
        }
        let mut auto = vec![];
        if r.html.is_some() && r.editable.unwrap_or(true) && fields::parse(&src).fields.is_empty() {
            let e = fields::make_editable(&src, true, true);
            if !e.added.is_empty() {
                src = e.src;
                auto = e.added;
            }
        }
        let g0 = base.as_ref().map(|b| &b.1);
        let (w, h) = r.size.or(g0.map(|g| (g.w, g.h))).unwrap_or((comp.w, comp.h));
        let fps = r.fps.or(g0.map(|g| g.fps)).unwrap_or(comp.fps).clamp(1.0, 120.0);
        let dur = r.dur.or(g0.map(|g| g.dur)).unwrap_or(5.0).clamp(0.0, 600.0);
        let alpha = r.alpha.or(g0.map(|g| g.alpha)).unwrap_or(true);
        let at = r.at.or(g0.map(|g| g.at)).unwrap_or(0.0).clamp(0.0, 600.0);
        let parsed = fields::parse(&src);
        let mut vars = g0.map(|g| g.vars.clone()).unwrap_or_default();
        for k in &r.unset {
            vars.remove(k);
        }
        for (k, v) in &r.vars {
            let v = match parsed.fields.iter().find(|f| &f.name == k) {
                Some(f) => fields::normalize(f, v).map_err(|e| anyhow!("field {k}: {e}"))?,
                None => v.clone(),
            };
            vars.insert(k.clone(), v);
        }
        let name = r
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| if r.id.is_some() { base.as_ref().map(|b| b.0.name.clone()) } else { None })
            .unwrap_or_else(|| format!("graphic {}", p.media.iter().filter(|m| m.gfx.is_some()).count() + 1));
        let dir = graphics_dir(path);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.check_path(&dir.to_string_lossy())?;
        // a re-render keeps its file, unless another graphic shares it (variants made
        // before every graphic had its own file); new graphics and variants get their own
        let html_path = match (&r.id, &base) {
            (Some(id), Some((_, g))) if Path::new(&g.html).parent().is_some_and(|d| d.is_dir()) && !p.media.iter().any(|m| &m.id != id && m.gfx.as_ref().is_some_and(|o| o.html == g.html)) => PathBuf::from(&g.html),
            _ => unique_path(&dir, &slugify(&name), "html"),
        };
        Ok(Prepared { src, auto, name, w, h, fps, dur, alpha, at, vars, dir, html_path })
    }

    /// Renders a prepared graphic into its folder and probes it (not yet in the project).
    fn render_prepared(&self, p: &Project, g: &Prepared, keep: Vec<f64>) -> Result<(Media, RenderOut, PathBuf)> {
        let page = build_page(&g.src, &g.vars, &PageOpts { w: g.w, h: g.h, fps: g.fps, dur: g.dur, extra_css: &brand_css(&p.brand), base: "", preview: false });
        let vars_json = serde_json::to_string(&page.vars).unwrap_or_default();
        // scripts read MC.vars directly, so the values are part of the key too
        let key = crate::render::eval::hash_str(&format!("{}|{}x{}|{}|{}|{}|{}|{vars_json}", page.html, g.w, g.h, g.fps, g.dur, g.alpha, g.at));
        let slug = slugify(&g.name);
        let ext = if g.dur <= 0.0 { "png" } else { "mov" };
        let out = g.dir.join(format!("{slug}-{:08x}.{ext}", key as u32));
        let tmp = g.dir.join(format!(".{slug}-{:08x}.tmp.{ext}", key as u32));
        let page_dir = g.page_dir();
        let spec = RenderSpec { html: &page.html, dir: &page_dir, media: media_paths(p), w: g.w, h: g.h, fps: g.fps, dur: g.dur, alpha: g.alpha, vars: &page.vars, keep, still_at: g.at };
        let ro = render(Some(&self.ff), &spec, &tmp, None)?;
        if out.exists() {
            let _ = std::fs::remove_file(&tmp); // identical render already on disk (and maybe in use)
        } else {
            std::fs::rename(&tmp, &out)?;
        }
        let mut m = self.ff.probe(&out.to_string_lossy())?;
        m.name = g.name.clone();
        // it follows the brand kit if the page as filled uses a brand variable (a field may set or override one)
        let brand = if fields::fill(&g.src, &g.vars).html.contains("--brand-") { brand_key(&p.brand) } else { String::new() };
        m.gfx = Some(Gfx { html: g.html_path.to_string_lossy().to_string(), w: g.w, h: g.h, fps: g.fps, dur: g.dur, alpha: g.alpha, vars: g.vars.clone(), src: g.src.clone(), brand, at: g.at });
        Ok((m, ro, out))
    }

    /// Renders an HTML graphic, imports it into the library (or re-renders an
    /// existing one) and optionally places it, as one undo step.
    pub fn graphic(&self, r: GraphicReq, source: &str) -> Result<GraphicOut> {
        use crate::model::time::fmt_t;
        let (p, _, active, path) = self.snapshot();
        let g = self.prepare_graphic(&p, &active, path.as_deref(), &r)?;
        let (w, h, fps, dur, alpha) = (g.w, g.h, g.fps, g.dur, g.alpha);
        let name = g.name.clone();
        let n_prev = r.preview.min(12);
        // frames kept for the checks (and the sheet): spread over the animation
        let n_keep = if n_prev == 0 { 6 } else { n_prev };
        let keep: Vec<f64> = if dur <= 0.0 { vec![0.0] } else { (0..n_keep).map(|i| (i as f64 + 0.5) * dur / n_keep as f64).collect() };
        let (mut m, ro, out) = self.render_prepared(&p, &g, keep)?;
        let gfx = m.gfx.clone();

        // commit against the current project (edits may have happened while rendering)
        let (mut work, _, active, _) = self.snapshot();
        let mut log = vec![];
        let shape = format!("{w}x{h} {}{}", if dur > 0.0 { format!("{}fps {}s", fmt_num(fps), fmt_t(dur)) } else { "still".into() }, if alpha { " alpha" } else { "" });
        let id = match &r.id {
            Some(id) => {
                let slot = work.media.iter_mut().find(|x| &x.id == id).ok_or_else(|| anyhow!("{id} was removed while rendering"))?;
                m.id = id.clone();
                m.tags = slot.tags.clone();
                m.note = slot.note.clone();
                *slot = m;
                log.push(format!("~{id} graphic '{name}' re-rendered {shape}"));
                id.clone()
            }
            None => {
                m.id = work.next_media_id();
                let id = m.id.clone();
                log.push(format!("+{id} graphic '{name}' {shape}"));
                work.media.push(m);
                id
            }
        };
        if let Some(pl) = r.place.as_deref().filter(|s| !s.trim().is_empty()) {
            let o = self.run_ops(&mut work, &active, &format!("add {id} {pl}"))?;
            log.extend(o.log);
        }
        if let Some(item) = r.replace.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let src_in = work.comps.iter().flat_map(|c| c.items()).find(|i| i.id == item).map(|i| i.src_in).ok_or_else(|| anyhow!("no clip {item}"))?;
            let o = self.run_ops(&mut work, &active, &format!("replace {item} {id} in={}", fmt_num(src_in)))?;
            log.extend(o.log);
        }
        self.commit(work, None, &format!("graphic {name}"), source, &log)?;

        // report: what the AI needs to judge the result without guessing
        let mut text = log.join("\n");
        if let Some(warn) = gfx.as_ref().and_then(write_source) {
            text.push_str(&format!("\n{warn}"));
        }
        text.push_str(&format!("\nrendered {} frame{} in {:.1}s -> {}", ro.frames, if ro.frames == 1 { "" } else { "s" }, ro.secs, out.display()));
        if ro.empty {
            text.push_str("\n! nothing visible was rendered (check selectors, colors and that elements are on screen)");
        }
        if let Some([x0, y0, x1, y1]) = ro.bounds {
            let (fw, fh) = (w as f64, h as f64);
            text.push_str(&format!("\ncontent box {:.0},{:.0}-{:.0},{:.0}", x0, y0, x1, y1));
            if x0 < -1.0 || y0 < -1.0 || x1 > fw + 1.0 || y1 > fh + 1.0 {
                text.push_str(" ! extends past the frame (may be intentional for slide-ins)");
            } else if x0 < fw * 0.05 || y0 < fh * 0.05 || x1 > fw * 0.95 || y1 > fh * 0.95 {
                text.push_str(" (outside title-safe 5% margin)");
            }
        }
        for e in &ro.errors {
            text.push_str(&format!("\n! {e}"));
        }
        let unset = fields::fill(&g.src, &g.vars).missing;
        if !unset.is_empty() {
            text.push_str(&format!("\n! unset fields (rendered empty): {} (pass vars, or write {{{{name|default}}}})", unset.join(", ")));
        }
        text.push_str(&fields_report(&g));
        let sheet = if n_prev > 0 {
            // small graphics (lower thirds, bugs) are cropped to their content so details stay legible
            let crop = ro.bounds.and_then(|[x0, y0, x1, y1]| {
                let (fw, fh) = (w as f64, h as f64);
                let (mx, my) = (fw * 0.04, fh * 0.04);
                let (cx0, cy0, cx1, cy1) = ((x0 - mx).max(0.0), (y0 - my).max(0.0), (x1 + mx).min(fw), (y1 + my).min(fh));
                ((cx1 - cx0) * (cy1 - cy0) < fw * fh * 0.35 && cx1 - cx0 > 8.0 && cy1 - cy0 > 8.0).then_some((cx0 as u32, cy0 as u32, (cx1 - cx0) as u32, (cy1 - cy0) as u32))
            });
            let tiles: Vec<crate::render::encode::Tile> = ro
                .kept
                .iter()
                .filter_map(|(t, png)| {
                    let mut img = crate::render::encode::decode_image(png).ok()?;
                    if let Some((x, y, cw, ch)) = crop {
                        img = img.clone_rect(resvg::tiny_skia::IntRect::from_xywh(x as i32, y as i32, cw, ch)?)?;
                    }
                    Some(crate::render::encode::Tile { label: if dur > 0.0 { format!("{}s", fmt_t(*t)) } else { String::new() }, img })
                })
                .collect();
            let (tw, th) = crop.map(|c| (c.2, c.3)).unwrap_or((w, h));
            if crop.is_some() {
                text.push_str("\npreview tiles are cropped to the content box");
            }
            let cols = if th > tw { tiles.len().min(6) } else { (tiles.len() as f64).sqrt().ceil() as usize };
            let cell_w = if tiles.len() <= 4 { 320 } else { 240 };
            let cell_h = ((cell_w as f64 * th as f64 / tw as f64).round() as u32).clamp(16, 640);
            let pm = crate::render::encode::tile_sheet(&tiles, cell_w, cell_h.max(16), cols, alpha)?;
            Some(crate::render::encode::encode_jpeg(&pm, 80)?)
        } else {
            None
        };
        Ok(GraphicOut { id, text, sheet })
    }

    /// Re-renders graphics as one undo step: the given ones, or every graphic
    /// rendered with an older brand kit.
    pub fn refresh_graphics(&self, ids: Option<Vec<String>>, source: &str) -> Result<String> {
        let (p, _, active, path) = self.snapshot();
        let ids: Vec<String> = match ids {
            Some(v) if !v.is_empty() => v,
            _ => p.media.iter().filter(|m| m.gfx.as_ref().is_some_and(|g| g.is_stale(&p.brand))).map(|m| m.id.clone()).collect(),
        };
        if ids.is_empty() {
            return Ok("every graphic already matches the brand kit".into());
        }
        let mut done = vec![];
        for id in &ids {
            let r = GraphicReq { id: Some(id.clone()), editable: Some(false), ..Default::default() };
            let g = self.prepare_graphic(&p, &active, path.as_deref(), &r)?;
            let (m, ro, _) = self.render_prepared(&p, &g, vec![])?;
            done.push((id.clone(), m, ro.errors.len()));
        }
        let (mut work, ..) = self.snapshot();
        let mut log = vec![];
        let mut written = vec![];
        for (id, mut m, errors) in done {
            let Some(slot) = work.media.iter_mut().find(|x| x.id == id) else { continue };
            m.id = id.clone();
            m.tags = slot.tags.clone();
            m.note = slot.note.clone();
            let errs = if errors > 0 { format!(" ! {errors} JS error{} (graphic id={id} shows them)", if errors == 1 { "" } else { "s" }) } else { String::new() };
            log.push(format!("~{id} graphic '{}' re-rendered{errs}", m.name));
            written.extend(m.gfx.clone());
            *slot = m;
        }
        let n = log.len();
        self.commit(work, None, &format!("re-render {n} graphic{}", if n == 1 { "" } else { "s" }), source, &log)?;
        for g in &written {
            if let Some(w) = write_source(g) {
                log.push(w);
            }
        }
        Ok(log.join("\n"))
    }

    /// A live preview page of a graphic request: nothing is rendered or changed.
    /// `base` is the URL prefix it is served under.
    pub fn graphic_preview(&self, r: &GraphicReq, base: &str) -> Result<PreviewPage> {
        let (p, _, active, path) = self.snapshot();
        let g = self.prepare_graphic(&p, &active, path.as_deref(), r)?;
        let page = build_page(&g.src, &g.vars, &PageOpts { w: g.w, h: g.h, fps: g.fps, dur: g.dur, extra_css: &brand_css(&p.brand), base, preview: true });
        let mut info = info_json(&g.src);
        if let Some(o) = info.as_object_mut() {
            for (k, v) in [
                ("w", json!(g.w)),
                ("h", json!(g.h)),
                ("fps", json!(g.fps)),
                ("dur", json!(g.dur)),
                ("alpha", json!(g.alpha)),
                ("at", json!(g.at)),
                ("missing", json!(page.missing)),
                ("auto", json!(g.auto.iter().map(|(n, _)| n).collect::<Vec<_>>())),
            ] {
                o.insert(k.into(), v);
            }
        }
        Ok(PreviewPage { html: page.html, dir: g.page_dir(), media: media_paths(&p), info })
    }

    /// Source, settings and fields of a graphic (the editor's dialog and inspector).
    pub fn graphic_detail(&self, id: &str) -> Result<Value> {
        let (p, ..) = self.snapshot();
        let m = p.media(id).ok_or_else(|| anyhow!("unknown media {id}"))?;
        let g = m.gfx.as_ref().ok_or_else(|| anyhow!("{id} isn't an HTML graphic"))?;
        let html = g.source()?;
        let mut v = info_json(&html);
        if let Some(o) = v.as_object_mut() {
            for (k, x) in [
                ("ok", json!(true)),
                ("id", json!(m.id)),
                ("name", json!(m.name)),
                ("html", json!(html)),
                ("file", json!(g.html)),
                ("dur", json!(g.dur)),
                ("fps", json!(g.fps)),
                ("w", json!(g.w)),
                ("h", json!(g.h)),
                ("alpha", json!(g.alpha)),
                ("at", json!(g.at)),
                ("vars", json!(g.vars)),
                ("uses", json!(uses_of(&p, id))),
                ("stale", json!(g.is_stale(&p.brand))),
            ] {
                o.insert(k.into(), x);
            }
        }
        Ok(v)
    }

    /// A graphic's source and fields as text (the AI reading back a graphic, e.g. after people edited it).
    pub fn graphic_source(&self, id: &str) -> Result<String> {
        let (p, ..) = self.snapshot();
        let m = p.media(id).ok_or_else(|| anyhow!("unknown media {id}"))?;
        let g = m.gfx.as_ref().ok_or_else(|| anyhow!("{id} isn't an HTML graphic"))?;
        let html = g.source()?;
        let parsed = fields::parse(&html);
        let uses = uses_of(&p, id);
        let shape = if g.dur > 0.0 { format!("{}fps {}s", fmt_num(g.fps), fmt_num(g.dur)) } else { format!("still at {}s", fmt_num(g.at)) };
        let mut s = format!("{id} '{}' {}x{} {shape}{}, used by {uses} clip{}\nfile: {}", m.name, g.w, g.h, if g.alpha { " alpha" } else { "" }, if uses == 1 { "" } else { "s" }, g.html);
        if g.is_stale(&p.brand) {
            s.push_str("\n! rendered with an older brand kit (graphic refresh=true re-renders it)");
        }
        s.push_str(&format!("\nfields: {}", if parsed.fields.is_empty() { "none".to_string() } else { fields::summary(&parsed.fields, &g.vars) }));
        for w in &parsed.warnings {
            s.push_str(&format!("\n! {w}"));
        }
        s.push_str(&format!("\n---\n{html}"));
        Ok(s)
    }
}

/// What the AI hears about a graphic's fields after a render.
fn fields_report(g: &Prepared) -> String {
    let mut s = String::new();
    let parsed = fields::parse(&g.src);
    if !g.auto.is_empty() {
        s.push_str(&format!(
            "\nmade editable: the page had no {{{{fields}}}}, so its texts and colors became fields ({}); name them yourself next time, or pass editable=false to keep the html exactly as written",
            g.auto.iter().map(|(n, k)| format!("{n} {}", k.as_str())).collect::<Vec<_>>().join(", ")
        ));
    }
    if !parsed.fields.is_empty() {
        s.push_str(&format!("\nfields (people edit these in the editor; change them later with vars={{...}}, no html rewrite): {}", fields::summary(&parsed.fields, &g.vars)));
    }
    let fixed: Vec<String> = fields::hardcoded(&g.src).into_iter().map(|t| format!("'{t}'")).collect();
    if !fixed.is_empty() {
        if parsed.fields.is_empty() {
            s.push_str(&format!("\n! no editable fields: {} written into the html can only be changed in code; put user-facing text and colors in {{{{name|default}}}} fields (help graphics)", fixed.join(", ")));
        } else {
            s.push_str(&format!("\nfixed text (changing it needs code): {}; make it a {{{{field}}}} if it may change", fixed.join(", ")));
        }
    }
    for w in &parsed.warnings {
        s.push_str(&format!("\n! {w}"));
    }
    s
}

fn fmt_num(v: f64) -> String {
    crate::model::value::fmt_num(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(dur: f64, base: &str, preview: bool) -> PageOpts<'_> {
        PageOpts { w: 1920, h: 1080, fps: 30.0, dur, extra_css: "", base, preview }
    }

    #[test]
    fn fragments_are_wrapped_and_vars_escaped() {
        let vars = BTreeMap::from([("name".to_string(), "Tom & <Jerry>".to_string())]);
        let page = build_page("<h1>{{name}}</h1><img src='mc://m3'><p>{{role|CEO}}{{gone}}</p><script>if(a){{b}}</script>", &vars, &opts(4.0, "", false)).html;
        assert!(page.contains("<p>CEO</p>"), "{page}");
        assert!(page.contains("if(a){{b}}"), "JS braces must survive: {page}");
        assert_eq!(fields::fill("{{a}} {{b|x}} {{c}}", &BTreeMap::new()).missing, vec!["a", "c"]);
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("<h1>Tom &amp; &lt;Jerry&gt;</h1>"));
        assert!(page.contains("src='/mc-media/m3'"));
        assert!(page.contains("background:transparent"));
        assert!(page.contains("--mc-dur:4s;--mc-w:1920px;--mc-h:1080px"), "{page}");
        let full = build_page("<!DOCTYPE html><html><head><title>x</title></head><body>hi</body></html>", &BTreeMap::new(), &opts(4.0, "", false)).html;
        assert!(full.contains("<head><style>@font-face"), "{full}");
        let attrs = build_page("<!-- a graphic -->\n<!doctype html><html lang=en><HEAD lang=\"en\"></HEAD><body><header>x</header></body></html>", &BTreeMap::new(), &opts(4.0, "", false)).html;
        assert!(attrs.contains("<HEAD lang=\"en\"><style>@font-face"), "{attrs}");
        assert!(attrs.contains("<header>x</header>"), "{attrs}");
        let headless = build_page("<!doctype html><html><body><header>x</header></body></html>", &BTreeMap::new(), &opts(4.0, "", false)).html;
        assert!(headless.contains("<html><head><style>@font-face"), "{headless}");
        let still = build_page("<p>x</p>", &BTreeMap::new(), &opts(0.0, "", false)).html;
        assert!(still.contains("--mc-dur:3600s"), "a still never reaches its outro: {still}");
    }

    #[test]
    fn previews_run_the_clock_before_the_page_and_load_from_their_prefix() {
        let vars = BTreeMap::from([("logo".to_string(), "mc://m2".to_string())]);
        let src = "<!doctype html><html><head><script>window.x=1</script></head><body><img src=\"{{logo|mc://m1}}\"><p>{{t|a</script>b}}</p></body></html>";
        let page = build_page(src, &vars, &opts(2.0, "/gfx-preview/abc", true));
        let h = &page.html;
        let shim = h.find("window.__mc").expect("the clock shim");
        assert!(shim < h.find("window.x=1").unwrap(), "the clock runs before the page's scripts");
        assert!(h.contains("__mcPreview"), "the editor bridge");
        assert!(h.contains("src=\"/gfx-preview/abc/mc-media/m2\""), "{h}");
        assert!(h.contains("url(/gfx-preview/abc/mc-fonts/Inter-Regular.otf)"));
        assert!(h.contains("\"logo\":\"/gfx-preview/abc/mc-media/m2\""), "MC.vars hold served URLs: {h}");
        assert!(h.contains("\"t\":\"a\\u003c/script>b\""), "values can't close the script: {h}");
        assert!(!build_page(src, &vars, &opts(2.0, "", false)).html.contains("__mcPreview"), "renders inject the clock over DevTools");
    }

    #[test]
    fn site_files_stay_inside_the_folder() {
        let dir = std::env::temp_dir().join(format!("mc-site-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("img").join("a.png"), b"x").unwrap();
        let media = BTreeMap::from([("m1".to_string(), dir.join("img").join("a.png"))]);
        assert!(matches!(site_file(&dir, &media, "img/a.png"), Some(SiteFile::File(..))));
        assert!(matches!(site_file(&dir, &media, "mc-media/m1"), Some(SiteFile::File(..))));
        assert!(matches!(site_file(&dir, &media, "mc-fonts/Inter-Bold.otf"), Some(SiteFile::Bytes(..))));
        assert!(site_file(&dir, &media, "../img/a.png").is_none());
        assert!(site_file(&dir, &media, "img/../../x").is_none());
        assert!(site_file(&dir, &media, "mc-media/m9").is_none());
        assert!(site_file(&dir, &media, "").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn html_files_follow_undo_but_keep_outside_edits() {
        let dir = std::env::temp_dir().join(format!("mc-mirror-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.html").to_string_lossy().to_string();
        std::fs::write(&f, "new").unwrap();
        let newer = BTreeMap::from([("m1".to_string(), (f.clone(), "new".to_string()))]);
        let older = BTreeMap::from([("m1".to_string(), (f.clone(), "old".to_string()))]);
        mirror_sources(&newer, &older);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "old");
        std::fs::write(&f, "edited outside").unwrap();
        mirror_sources(&older, &older);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "edited outside", "graphics the undo didn't touch keep their file");
        let g = Gfx { html: f.clone(), src: "kept".into(), ..Default::default() };
        assert_eq!(g.source().unwrap(), "edited outside", "the file wins");
        std::fs::remove_file(&f).unwrap();
        assert_eq!(g.source().unwrap(), "kept", "a lost file falls back to the project's copy");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn brand_changes_make_graphics_stale() {
        let b = Brand { accent: Some("#00ff00".into()), ..Default::default() };
        let g = Gfx { brand: brand_key(&b), ..Default::default() };
        assert!(!g.is_stale(&b));
        assert!(g.is_stale(&Brand::default()));
        assert!(!Gfx::default().is_stale(&b), "graphics without brand variables never go stale");
    }

    #[test]
    fn names_and_frames() {
        assert_eq!(slugify("Lower third: Jane Doé!"), "lower-third-jane-doé");
        assert_eq!(slugify("***"), "graphic");
        assert_eq!(frame_count(30.0, 2.5), 75);
        assert_eq!(frame_count(30.0, 0.0), 1);
    }
}
