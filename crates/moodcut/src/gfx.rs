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

/// Source of a rendered graphic, kept on its media so it can be re-rendered.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Gfx {
    /// The .html file (as written by the AI, before variable substitution).
    pub html: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    /// Seconds; 0 = still image.
    pub dur: f64,
    #[serde(default)]
    pub alpha: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
}

pub struct RenderSpec<'a> {
    /// Page source after `prepare_html`.
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
        // Playwright's bundled Chromium
        let mut roots = vec![];
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
    } else if let Some(name) = path.strip_prefix("/mc-fonts/") {
        use crate::render::text::{INTER_BLACK, INTER_BOLD, INTER_REGULAR};
        let data: &[u8] = match name {
            "Inter-Regular.otf" => INTER_REGULAR,
            "Inter-Bold.otf" => INTER_BOLD,
            "Inter-Black.otf" => INTER_BLACK,
            _ => return respond(&mut conn, 404, "text/plain", b"not found", None, &method),
        };
        (data.to_vec(), "font/otf".into())
    } else {
        let file = if let Some(id) = path.strip_prefix("/mc-media/") {
            site.media.get(id).cloned()
        } else {
            let rel = Path::new(path.trim_start_matches('/'));
            if rel.components().all(|c| matches!(c, Component::Normal(_))) {
                Some(site.dir.join(rel))
            } else {
                None
            }
        };
        match file.filter(|f| f.is_file()) {
            Some(f) => (std::fs::read(&f)?, mime_guess::from_path(&f).first_or_octet_stream().to_string()),
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const BASE_CSS: &str = "@font-face{font-family:Inter;src:url(/mc-fonts/Inter-Regular.otf);font-weight:100 500}\
@font-face{font-family:Inter;src:url(/mc-fonts/Inter-Bold.otf);font-weight:600 800}\
@font-face{font-family:Inter;src:url(/mc-fonts/Inter-Black.otf);font-weight:900}";

/// Replaces `{{name}}` / `{{name|default}}` placeholders; unset ones without a
/// default become empty. Returns the text and the names left unset.
pub fn fill_vars(src: &str, vars: &BTreeMap<String, String>) -> (String, Vec<String>) {
    // inside <script>, an unknown {{x}} may be real code: leave it alone
    let lower = src.to_ascii_lowercase();
    let mut scripts = vec![];
    let mut from = 0;
    while let Some(a) = lower[from..].find("<script").map(|x| x + from) {
        let b = lower[a..].find("</script").map(|x| x + a).unwrap_or(src.len());
        scripts.push((a, b));
        from = b.max(a + 7);
    }
    let in_script = |pos: usize| scripts.iter().any(|(a, b)| pos >= *a && pos < *b);
    let mut out = String::with_capacity(src.len());
    let mut missing = vec![];
    let mut rest = src;
    while let Some(i) = rest.find("{{") {
        let at = src.len() - rest.len() + i;
        let Some(j) = rest[i + 2..].find("}}") else { break };
        let inner = &rest[i + 2..i + 2 + j];
        let (name, def) = match inner.split_once('|') {
            Some((n, d)) => (n.trim(), Some(d)),
            None => (inner.trim(), None),
        };
        // only identifier-like names are placeholders (leave JS/CSS braces alone)
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            out.push_str(&rest[..i + 2]);
            rest = &rest[i + 2..];
            continue;
        }
        out.push_str(&rest[..i]);
        match (vars.get(name), def) {
            (Some(v), _) => out.push_str(&html_escape(v)),
            (None, Some(d)) => out.push_str(d),
            (None, None) if in_script(at) => out.push_str(&rest[i..i + 2 + j + 2]),
            (None, None) => {
                if !missing.iter().any(|m| m == name) {
                    missing.push(name.to_string());
                }
            }
        }
        rest = &rest[i + 2 + j + 2..];
    }
    out.push_str(rest);
    (out, missing)
}

/// Substitutes placeholders, maps `mc://m3` to served media, and wraps a body
/// fragment in a full-frame transparent page.
pub fn prepare_html(src: &str, vars: &BTreeMap<String, String>) -> String {
    let mut s = fill_vars(src, vars).0;
    s = s.replace("mc://", "/mc-media/");
    let lower = s.trim_start().to_ascii_lowercase();
    if lower.starts_with("<!doctype") || lower.starts_with("<html") {
        // full document: add the bundled font faces
        let fonts = format!("<style>{BASE_CSS}</style>");
        if let Some(i) = s.to_ascii_lowercase().find("<head>") {
            s.insert_str(i + 6, &fonts);
        } else if let Some(i) = s.to_ascii_lowercase().find("<html") {
            if let Some(j) = s[i..].find('>') {
                s.insert_str(i + j + 1, &format!("<head>{fonts}</head>"));
            }
        }
        s
    } else {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>{BASE_CSS}\
             *{{box-sizing:border-box}}html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:transparent}}\
             body{{font-family:Inter,system-ui,sans-serif;color:#fff;-webkit-font-smoothing:antialiased}}</style></head><body>{s}</body></html>"
        )
    }
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
        let ff = ff.ok_or_else(|| anyhow!("animated graphics need FFmpeg (moodcut setup); a still (dur 0) doesn't"))?;
        let mut c = command(&ff.ffmpeg);
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
    pub vars: BTreeMap<String, String>,
    /// Re-render this graphic in place (every clip using it updates).
    pub id: Option<String>,
    /// Make a new graphic from this one's source (usually with other vars).
    pub from: Option<String>,
    /// Also place it: `V3 at=2 dur=4` (anything `add` accepts after the media).
    pub place: Option<String>,
    /// Frames in the returned preview sheet (0 = none).
    pub preview: usize,
    /// For stills: the animation time to capture (seconds).
    pub at: Option<f64>,
}

pub struct GraphicOut {
    pub id: String,
    pub text: String,
    /// JPEG contact sheet of the rendered graphic.
    pub sheet: Option<Vec<u8>>,
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

impl crate::engine::Engine {
    /// Renders an HTML graphic, imports it into the library (or re-renders an
    /// existing one) and optionally places it, as one undo step.
    pub fn graphic(&self, r: GraphicReq, source: &str) -> Result<GraphicOut> {
        use crate::model::time::fmt_t;
        let (p, _, active, path) = self.snapshot();
        let comp = p.comp(&active).or_else(|| p.main_comp()).ok_or_else(|| anyhow!("no composition"))?;
        let base_id = r.id.clone().or_else(|| r.from.clone());
        let base = match &base_id {
            Some(id) => {
                let m = p.media(id).ok_or_else(|| anyhow!("unknown media {id}"))?;
                let g = m.gfx.clone().ok_or_else(|| anyhow!("{id} isn't an HTML graphic"))?;
                Some((m.clone(), g))
            }
            None => None,
        };
        let src = match (&r.html, &base) {
            (Some(h), _) => h.clone(),
            (None, Some((_, g))) => std::fs::read_to_string(&g.html).with_context(|| format!("reading {}", g.html))?,
            (None, None) => bail!("graphic needs html (or id/from to re-render an existing graphic)"),
        };
        if src.trim().is_empty() {
            bail!("html is empty");
        }
        let g0 = base.as_ref().map(|b| b.1.clone());
        let (w, h) = r.size.or(g0.as_ref().map(|g| (g.w, g.h))).unwrap_or((comp.w, comp.h));
        let fps = r.fps.or(g0.as_ref().map(|g| g.fps)).unwrap_or(comp.fps).clamp(1.0, 120.0);
        let dur = r.dur.or(g0.as_ref().map(|g| g.dur)).unwrap_or(5.0).clamp(0.0, 600.0);
        let alpha = r.alpha.or(g0.as_ref().map(|g| g.alpha)).unwrap_or(true);
        let mut vars = g0.as_ref().map(|g| g.vars.clone()).unwrap_or_default();
        vars.extend(r.vars.clone());
        let name = r
            .name
            .clone()
            .or_else(|| if r.id.is_some() { base.as_ref().map(|b| b.0.name.clone()) } else { None })
            .unwrap_or_else(|| format!("graphic {}", p.media.iter().filter(|m| m.gfx.is_some()).count() + 1));
        let dir = graphics_dir(path.as_deref());
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        self.check_path(&dir.to_string_lossy())?;
        let slug = slugify(&name);
        let html_path = match (&r.html, &base) {
            (None, Some((_, g))) => PathBuf::from(&g.html),
            (Some(_), Some((_, g))) if r.id.is_some() => PathBuf::from(&g.html),
            _ => unique_path(&dir, &slug, "html"),
        };
        if r.html.is_some() {
            std::fs::write(&html_path, &src).with_context(|| format!("writing {}", html_path.display()))?;
        }
        let page = prepare_html(&src, &vars);
        let key = crate::render::eval::hash_str(&format!("{page}|{w}x{h}|{fps}|{dur}|{alpha}|{:?}", r.at));
        let ext = if dur <= 0.0 { "png" } else { "mov" };
        let out = dir.join(format!("{slug}-{:08x}.{ext}", key as u32));
        let tmp = dir.join(format!(".{slug}-{:08x}.tmp.{ext}", key as u32));
        let media: BTreeMap<String, PathBuf> = p.media.iter().map(|m| (m.id.clone(), PathBuf::from(&m.path))).collect();
        let n_prev = r.preview.min(12);
        let keep: Vec<f64> = if dur <= 0.0 || n_prev == 0 { vec![0.0] } else { (0..n_prev).map(|i| (i as f64 + 0.5) * dur / n_prev as f64).collect() };
        let page_dir = html_path.parent().map(|d| d.to_path_buf()).unwrap_or(dir.clone());
        let spec = RenderSpec { html: &page, dir: &page_dir, media, w, h, fps, dur, alpha, vars: &vars, keep, still_at: r.at.unwrap_or(0.0) };
        let ro = render(Some(&self.ff), &spec, &tmp, None)?;
        if out.exists() {
            let _ = std::fs::remove_file(&tmp); // identical render already on disk (and maybe in use)
        } else {
            std::fs::rename(&tmp, &out)?;
        }
        let mut m = self.ff.probe(&out.to_string_lossy())?;
        m.name = name.clone();
        m.gfx = Some(Gfx { html: html_path.to_string_lossy().to_string(), w, h, fps, dur, alpha, vars: vars.clone() });

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
        self.commit(work, None, &format!("graphic {name}"), source, &log);

        // report: what the AI needs to judge the result without guessing
        let mut text = log.join("\n");
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
        let unset = fill_vars(&src, &vars).1;
        if !unset.is_empty() {
            text.push_str(&format!("\n! unset variables (rendered empty): {} (pass vars, or write {{{{name|default}}}})", unset.join(", ")));
        }
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
}

fn fmt_num(v: f64) -> String {
    crate::model::value::fmt_num(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_are_wrapped_and_vars_escaped() {
        let vars = BTreeMap::from([("name".to_string(), "Tom & <Jerry>".to_string())]);
        let page = prepare_html("<h1>{{name}}</h1><img src='mc://m3'><p>{{role|CEO}}{{gone}}</p><script>if(a){{b}}</script>", &vars);
        assert!(page.contains("<p>CEO</p>"), "{page}");
        assert!(page.contains("if(a){{b}}"), "JS braces must survive: {page}");
        assert_eq!(fill_vars("{{a}} {{b|x}} {{c}}", &BTreeMap::new()).1, vec!["a", "c"]);
        assert!(page.starts_with("<!doctype html>"));
        assert!(page.contains("<h1>Tom &amp; &lt;Jerry&gt;</h1>"));
        assert!(page.contains("src='/mc-media/m3'"));
        assert!(page.contains("background:transparent"));
        let full = prepare_html("<!DOCTYPE html><html><head><title>x</title></head><body>hi</body></html>", &BTreeMap::new());
        assert!(full.contains("<head><style>@font-face"), "{full}");
    }

    #[test]
    fn names_and_frames() {
        assert_eq!(slugify("Lower third: Jane Doé!"), "lower-third-jane-doé");
        assert_eq!(slugify("***"), "graphic");
        assert_eq!(frame_count(30.0, 2.5), 75);
        assert_eq!(frame_count(30.0, 0.0), 1);
    }
}
