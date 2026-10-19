//! HTTP server: embedded web GUI, REST API, WebSocket events, preview video
//! streaming, and the MCP Streamable HTTP endpoint (`/mcp`).

use crate::engine::Engine;
use crate::mcp::Mcp;
use crate::render::encode::{self, RenderOpts};
use anyhow::Result;
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(RustEmbed)]
#[folder = "../../gui/dist"]
struct Assets;

pub struct ServeOpts {
    pub host: String,
    pub port: u16,
    /// Required for every request (serve --token / --public).
    pub token: Option<String>,
    pub open_window: bool,
    /// Start a public tunnel at startup (moodcut connect).
    pub tunnel: bool,
    /// Use this public base URL instead of a tunnel (own domain / Tailscale Funnel).
    pub public_url: Option<String>,
}

struct AppState {
    eng: Arc<Engine>,
    mcp: Arc<Mcp>,
    token: Option<String>,
    /// Token that remote (tunneled / LAN) requests must carry.
    remote_token: parking_lot::RwLock<Option<String>>,
    tunnel: Mutex<Option<crate::tunnel::Tunnel>>,
    public_url: parking_lot::RwLock<Option<String>>,
    port: u16,
    cache: Mutex<HashMap<String, (Vec<u8>, &'static str)>>,
    thumb_slots: tokio::sync::Semaphore,
    /// FFmpeg install from the editor: (running, last status line, error)
    ff_setup: Arc<Mutex<(bool, String, Option<String>)>>,
}

type St = Arc<AppState>;

pub fn random_token() -> String {
    crate::tunnel::random_token()
}

pub async fn serve(eng: Arc<Engine>, o: ServeOpts) -> Result<()> {
    let mcp = Arc::new(Mcp::new(Ok(eng.clone())));
    let remote = if o.tunnel || o.public_url.is_some() { Some(o.token.clone().unwrap_or_else(crate::tunnel::persistent_token)) } else { o.token.clone() };
    let st: St = Arc::new(AppState {
        eng: eng.clone(),
        mcp,
        token: o.token.clone(),
        remote_token: parking_lot::RwLock::new(remote),
        tunnel: Mutex::new(None),
        public_url: parking_lot::RwLock::new(o.public_url.clone()),
        port: o.port,
        cache: Mutex::new(HashMap::new()),
        thumb_slots: tokio::sync::Semaphore::new(4),
        ff_setup: Arc::new(Mutex::new((false, String::new(), None))),
    });
    let app = Router::new()
        .route("/api/ping", get(api_ping))
        .route("/api/state", get(api_state))
        .route("/api/edit", post(api_edit))
        .route("/api/project", post(api_project))
        .route("/api/import", post(api_import))
        .route("/api/frame", get(api_frame))
        .route("/api/thumb", get(api_thumb))
        .route("/api/waveform", get(api_waveform))
        .route("/api/stream", get(api_stream))
        .route("/api/render", post(api_render))
        .route("/api/jobs", get(api_jobs))
        .route("/api/catalog", get(api_catalog))
        .route("/api/fs", get(api_fs))
        .route("/api/dialog", post(api_dialog))
        .route("/api/log", get(api_log))
        .route("/api/analyze", post(api_analyze))
        .route("/api/ws", get(api_ws))
        .route("/fonts/{name}", get(font_file))
        .route("/api/reveal", post(api_reveal))
        .route("/api/graphic", get(api_graphic_get).post(api_graphic))
        .route("/api/check", post(api_check))
        .route("/api/scope", get(api_scope))
        .route("/api/history", get(api_history))
        .route("/api/layer", get(api_layer))
        .route("/api/tunnel", post(api_tunnel))
        .route("/api/setup/ffmpeg", post(api_setup_ffmpeg))
        .route("/api/ai/clients", get(api_ai_clients))
        .route("/api/ai/install", post(api_ai_install))
        .route("/mcp", post(mcp_post).get(mcp_get).delete(mcp_delete))
        .route("/mcp/{token}", post(mcp_post_tok).get(mcp_get).delete(mcp_delete))
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(st.clone(), guard))
        // HTML graphics with embedded images, big imports and scripts are fine
        .layer(axum::extract::DefaultBodyLimit::max(256 << 20))
        .with_state(st.clone());
    let addr = format!("{}:{}", o.host, o.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| anyhow::anyhow!("can't listen on {addr}: {e}"))?;
    write_instance(o.port);
    let url = format!("http://127.0.0.1:{}", o.port);
    eprintln!("MoodCut {} — GUI {url}  |  MCP {url}/mcp", crate::VERSION);
    if let Some(t) = &o.token {
        eprintln!("token auth on. MCP URL with token: {url}/mcp/{t}  (or header Authorization: Bearer {t})");
    }
    if let Some(base) = &o.public_url {
        let tok = st.remote_token.read().clone().unwrap_or_default();
        let c = crate::tunnel::connector_url(base, &tok);
        eprintln!("\n{}\n", crate::tunnel::instructions(&c));
        if crate::tunnel::copy_to_clipboard(&c) {
            eprintln!("(connector URL copied to the clipboard)");
        }
    } else if o.tunnel {
        let st2 = st.clone();
        tokio::task::spawn_blocking(move || match crate::tunnel::start(st2.port) {
            Ok(t) => {
                let tok = st2.remote_token.read().clone().unwrap_or_default();
                let c = crate::tunnel::connector_url(&t.url, &tok);
                eprintln!("\n{}\n", crate::tunnel::instructions(&c));
                if crate::tunnel::copy_to_clipboard(&c) {
                    eprintln!("(connector URL copied to the clipboard)");
                }
                *st2.tunnel.lock() = Some(t);
            }
            Err(e) => eprintln!("! tunnel failed: {e:#}\n  You can still use a tunnel of your own: moodcut connect --url https://your-host"),
        });
    }
    // background decoder cleanup
    let e2 = eng.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            e2.frames.gc(60.0);
        }
    });
    if o.open_window {
        let u = match &o.token {
            Some(t) => format!("{url}/?token={t}"),
            None => url.clone(),
        };
        std::thread::spawn(move || open_window(&u));
    }
    axum::serve(listener, app).await?;
    Ok(())
}

fn local_host(h: &str) -> bool {
    let h = h.trim();
    let host = if h.starts_with('[') { h.split(']').next().map(|x| format!("{x}]")).unwrap_or_default() } else { h.split(':').next().unwrap_or(h).to_string() };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "tauri.localhost")
}

/// A request is remote when it came through a proxy/tunnel or names a
/// non-local host (LAN, tunnel hostname): those always need a token.
fn is_remote(headers: &HeaderMap) -> bool {
    let fwd = ["x-forwarded-for", "x-forwarded-host", "forwarded", "cf-connecting-ip", "cf-ray", "x-real-ip"].iter().any(|h| headers.contains_key(*h));
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("127.0.0.1");
    fwd || !local_host(host)
}

/// Token auth + DNS-rebinding / cross-site protection.
async fn guard(State(st): State<St>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let path = req.uri().path().to_string();
    // OAuth discovery probes (claude.ai, ChatGPT): this server is token-in-URL, no OAuth
    if path.starts_with("/.well-known/") {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response();
    }
    let q = req.uri().query().unwrap_or("").to_string();
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(|s| s.to_string());
    let cookie_tok = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).and_then(|c| c.split(';').find_map(|kv| kv.trim().strip_prefix("mc_token=").map(|s| s.to_string())));
    let query_tok = q.split('&').find_map(|kv| kv.strip_prefix("token=").map(|s| s.to_string()));
    let path_tok = path.strip_prefix("/mcp/").map(|s| s.to_string());
    let given = [bearer, cookie_tok, query_tok.clone(), path_tok];
    let matches = |t: &Option<String>| t.as_ref().is_some_and(|t| given.iter().any(|g| g.as_deref() == Some(t.as_str())));
    let remote_tok = st.remote_token.read().clone();
    let remote = is_remote(headers);
    let authed = if remote {
        matches(&st.token) || matches(&remote_tok)
    } else {
        st.token.is_none() || matches(&st.token) || matches(&remote_tok)
    };
    if remote && st.token.is_none() && remote_tok.is_none() {
        return (StatusCode::FORBIDDEN, "remote access is off: start it with `moodcut connect` (or Connect AI > web link)").into_response();
    }
    // browsers: only this app's own local pages may call it without a token;
    // MCP also answers browser tools on this computer (MCP Inspector, local web clients)
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let mcp_path = path == "/mcp" || path.starts_with("/mcp/");
    let mut cors_origin: Option<String> = None;
    if let Some(origin) = &origin {
        let o_host = origin.split("://").nth(1).unwrap_or("").trim_end_matches('/');
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
        let same_local = local_host(o_host) && o_host.eq_ignore_ascii_case(host);
        let token_given = matches(&st.token) || matches(&remote_tok);
        let local_tool = mcp_path && !remote && local_host(o_host);
        if !same_local && !token_given && !local_tool {
            return (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response();
        }
        if mcp_path && !same_local {
            cors_origin = Some(origin.clone());
        }
    }
    if req.method() == axum::http::Method::OPTIONS && mcp_path {
        let mut r = StatusCode::NO_CONTENT.into_response();
        add_cors(&mut r, cors_origin.as_deref());
        return r;
    }
    let public_path = !remote && (path == "/api/ping" || (!path.starts_with("/api/") && !path.starts_with("/mcp")));
    if !authed && !public_path {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "missing or invalid token"}))).into_response();
    }
    let mut resp = next.run(req).await;
    add_cors(&mut resp, cors_origin.as_deref());
    if let Some(qt) = query_tok {
        if matches(&Some(qt.clone())) && (st.token.as_deref() == Some(qt.as_str()) || remote_tok.as_deref() == Some(qt.as_str())) {
            if let Ok(v) = HeaderValue::from_str(&format!("mc_token={qt}; Path=/; SameSite=Strict; HttpOnly")) {
                resp.headers_mut().insert(header::SET_COOKIE, v);
            }
        }
    }
    resp
}

/// CORS headers for MCP requests from browser tools on this computer.
fn add_cors(r: &mut Response, origin: Option<&str>) {
    let Some(o) = origin.and_then(|o| HeaderValue::from_str(o).ok()) else { return };
    let h = r.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
    h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, DELETE, OPTIONS"));
    h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type, authorization, mcp-session-id, mcp-protocol-version, last-event-id"));
    h.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("mcp-session-id"));
    h.insert(header::VARY, HeaderValue::from_static("origin"));
}

/// Where the running editor listens, so `moodcut mcp` and a second launch can find it.
fn instance_file() -> std::path::PathBuf {
    crate::config::ensure_data_dir().join("instance.json")
}

fn write_instance(port: u16) {
    let _ = std::fs::write(instance_file(), json!({"port": port, "pid": std::process::id(), "version": crate::VERSION}).to_string());
}

/// The port in the instance file (may be stale: check it with `running_instance`).
pub fn instance_port() -> Option<u16> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(instance_file()).ok()?).ok()?;
    v["port"].as_u64().map(|p| p as u16)
}

/// Tiny blocking HTTP request to a local port (no async runtime needed).
pub fn local_request(port: u16, method: &str, path: &str, body: Option<&str>, timeout_ms: u64) -> Option<(u16, String)> {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let t = std::time::Duration::from_millis(timeout_ms);
    let mut s = std::net::TcpStream::connect_timeout(&addr, t).ok()?;
    s.set_read_timeout(Some(t * 10)).ok()?;
    s.set_write_timeout(Some(t)).ok()?;
    let body = body.unwrap_or("");
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    s.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, rest) = text.split_once("\r\n\r\n")?;
    let code = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    let chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
    Some((code, if chunked { crate::mcp::stdio::dechunk(rest) } else { rest.to_string() }))
}

/// `/api/ping` of a MoodCut listening on `port`, if one is.
pub fn running_instance(port: u16) -> Option<Value> {
    let (code, body) = local_request(port, "GET", "/api/ping", None, 400)?;
    let v: Value = serde_json::from_str(&body).ok()?;
    (code == 200 && v["app"] == "moodcut").then_some(v)
}

/// A MoodCut already running here: on `port`, or where the instance file says.
pub fn find_instance(port: u16) -> Option<(u16, Value)> {
    if let Some(v) = running_instance(port) {
        return Some((port, v));
    }
    let p = instance_port().filter(|p| *p != port)?;
    running_instance(p).map(|v| (p, v))
}

/// `port` if it is free, else the next free one (up to 20 higher).
pub fn free_port(port: u16) -> u16 {
    (port..port.saturating_add(20)).find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok()).unwrap_or(port)
}

async fn api_ping(State(st): State<St>) -> Response {
    let path = st.eng.st.read().path.as_ref().map(|p| p.to_string_lossy().to_string());
    Json(json!({"app": "moodcut", "version": crate::VERSION, "port": st.port, "path": path})).into_response()
}

/// AI apps on this computer and whether MoodCut is connected to them.
async fn api_ai_clients(headers: HeaderMap) -> Response {
    if is_remote(&headers) {
        return err(StatusCode::FORBIDDEN, "only from this computer");
    }
    let list = blocking(crate::clients::statuses).await;
    Json(json!({"ok": true, "clients": list, "command": crate::clients::server_command().ok()})).into_response()
}

/// One click in the editor: register (or remove) MoodCut in an AI app's settings.
async fn api_ai_install(headers: HeaderMap, Json(a): Json<Value>) -> Response {
    if is_remote(&headers) {
        return err(StatusCode::FORBIDDEN, "AI apps can only be connected from this computer");
    }
    let id = a["client"].as_str().unwrap_or("").to_string();
    let remove = a["remove"].as_bool().unwrap_or(false);
    let r = blocking(move || -> anyhow::Result<(String, Value)> {
        let c = crate::clients::find(&id).ok_or_else(|| anyhow::anyhow!("unknown AI app '{id}'"))?;
        let msg = crate::clients::apply(c, remove)?;
        Ok((msg, json!(crate::clients::status(c))))
    })
    .await;
    match r {
        Ok((msg, status)) => Json(json!({"ok": true, "text": msg, "client": status})).into_response(),
        Err(e) => Json(json!({"ok": false, "error": format!("{e:#}")})).into_response(),
    }
}

async fn static_file(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let file = Assets::get(path).or_else(|| if path.contains('.') { None } else { Assets::get("index.html") });
    match file {
        Some(f) => {
            let mime = mime_guess::from_path(if Assets::get(path).is_some() { path } else { "index.html" }).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, "no-cache".to_string())], f.data.into_owned()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn err(code: StatusCode, msg: impl ToString) -> Response {
    (code, Json(json!({"ok": false, "error": msg.to_string()}))).into_response()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("blocking task panicked")
}

async fn api_state(State(st): State<St>) -> Response {
    let (p, rev, active, path) = st.eng.snapshot();
    let dirty = st.eng.st.read().dirty;
    let undo = st.eng.st.read().undo.len();
    let redo = st.eng.st.read().redo.len();
    Json(json!({
        "project": p, "rev": rev, "active": active, "path": path, "dirty": dirty,
        "undo": undo, "redo": redo, "ffmpeg": st.eng.ff.version(), "version": crate::VERSION
    }))
    .into_response()
}

#[derive(Deserialize)]
struct EditReq {
    script: String,
    #[serde(default)]
    source: Option<String>,
}

async fn api_edit(State(st): State<St>, Json(r): Json<EditReq>) -> Response {
    let eng = st.eng.clone();
    let src = r.source.unwrap_or_else(|| "gui".into());
    match blocking(move || eng.edit(&r.script, &src)).await {
        Ok(o) => Json(json!({"ok": true, "log": o.log, "rev": o.rev})).into_response(),
        Err(e) => Json(json!({"ok": false, "error": format!("{e:#}")})).into_response(),
    }
}

async fn api_project(State(st): State<St>, Json(a): Json<Value>) -> Response {
    let mcp = st.mcp.clone();
    let r = blocking(move || mcp.call("project", &a)).await;
    let txt = r["content"][0]["text"].as_str().unwrap_or("").to_string();
    Json(json!({"ok": r.get("isError").is_none(), "text": txt})).into_response()
}

#[derive(Deserialize)]
struct ImportReq {
    paths: Vec<String>,
}

async fn api_import(State(st): State<St>, Json(r): Json<ImportReq>) -> Response {
    let eng = st.eng.clone();
    match blocking(move || eng.import(&r.paths, "gui")).await {
        Ok(o) => Json(json!({"ok": true, "log": o.log, "rev": o.rev})).into_response(),
        Err(e) => Json(json!({"ok": false, "error": format!("{e:#}")})).into_response(),
    }
}

#[derive(Deserialize)]
struct FrameQ {
    #[serde(default)]
    t: f64,
    #[serde(default)]
    w: Option<u32>,
    #[serde(default)]
    comp: Option<String>,
    #[serde(default)]
    q: Option<u8>,
    #[serde(default)]
    png: Option<bool>,
}

async fn api_frame(State(st): State<St>, Query(q): Query<FrameQ>) -> Response {
    let eng = st.eng.clone();
    let r = blocking(move || -> anyhow::Result<(Vec<u8>, &'static str)> {
        let pm = eng.frame(q.comp.as_deref(), q.t, q.w.unwrap_or(960), true)?;
        if q.png.unwrap_or(false) {
            Ok((encode::encode_png(&pm)?, "image/png"))
        } else {
            Ok((encode::encode_jpeg(&pm, q.q.unwrap_or(80))?, "image/jpeg"))
        }
    })
    .await;
    match r {
        Ok((b, mime)) => ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "no-store")], b).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct ThumbQ {
    media: String,
    #[serde(default)]
    t: Option<f64>,
    #[serde(default)]
    w: Option<u32>,
    #[serde(default)]
    h: Option<u32>,
}

async fn api_thumb(State(st): State<St>, Query(q): Query<ThumbQ>) -> Response {
    let (p, ..) = st.eng.snapshot();
    let Some(m) = p.media(&q.media).cloned() else { return err(StatusCode::NOT_FOUND, "no media") };
    let w = q.w.unwrap_or(160).clamp(16, 960);
    let t = q.t.unwrap_or_else(|| Engine::poster_time(&m));
    let key = format!("thumb|{}|{}|{:.2}|{w}", m.path, m.mtime, t);
    if let Some((b, mime)) = st.cache.lock().get(&key).cloned() {
        return ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "max-age=3600")], b).into_response();
    }
    // a big library can request hundreds of thumbnails at once; decode a few at a time
    let _permit = st.thumb_slots.acquire().await;
    let eng = st.eng.clone();
    let r = blocking(move || -> anyhow::Result<Vec<u8>> {
        if m.kind == crate::model::project::MediaKind::Audio {
            anyhow::bail!("audio has no thumbnail");
        }
        let pm = eng.media_frame(&m, t, w)?;
        let pm = if m.alpha { crate::library::on_checker(&pm) } else { (*pm).clone() };
        encode::encode_jpeg(&pm, 75)
    })
    .await;
    match r {
        Ok(b) => {
            let mut c = st.cache.lock();
            if c.len() > 2000 {
                c.clear();
            }
            c.insert(key, (b.clone(), "image/jpeg"));
            ([(header::CONTENT_TYPE, "image/jpeg"), (header::CACHE_CONTROL, "max-age=3600")], b).into_response()
        }
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

async fn api_waveform(State(st): State<St>, Query(q): Query<ThumbQ>) -> Response {
    let (p, ..) = st.eng.snapshot();
    let Some(m) = p.media(&q.media).cloned() else { return err(StatusCode::NOT_FOUND, "no media") };
    if !m.has_audio {
        return err(StatusCode::NOT_FOUND, "no audio");
    }
    let (w, h) = (q.w.unwrap_or(800).clamp(32, 4000), q.h.unwrap_or(64).clamp(16, 400));
    let key = format!("wave|{}|{}|{w}x{h}", m.path, m.mtime);
    if let Some((b, mime)) = st.cache.lock().get(&key).cloned() {
        return ([(header::CONTENT_TYPE, mime)], b).into_response();
    }
    let eng = st.eng.clone();
    let r = blocking(move || {
        eng.ff.run_capture(&[
            "-i".into(), m.path.clone(), "-filter_complex".into(), format!("aformat=channel_layouts=mono,showwavespic=s={w}x{h}:colors=#9be7c4"), "-frames:v".into(), "1".into(), "-f".into(), "image2pipe".into(), "-vcodec".into(), "png".into(), "-".into(),
        ])
    })
    .await;
    match r {
        Ok((b, _)) => {
            st.cache.lock().insert(key, (b.clone(), "image/png"));
            ([(header::CONTENT_TYPE, "image/png")], b).into_response()
        }
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
struct StreamQ {
    #[serde(default)]
    from: f64,
    #[serde(default)]
    scale: Option<f32>,
    #[serde(default)]
    comp: Option<String>,
}

async fn api_stream(State(st): State<St>, Query(q): Query<StreamQ>) -> Response {
    let (p, _, active, _) = st.eng.snapshot();
    let comp = q.comp.unwrap_or(active);
    let cancel = Arc::new(AtomicBool::new(false));
    let rx = match encode::preview_stream(st.eng.ff.clone(), p, st.eng.frames.clone(), comp, q.from.max(0.0), q.scale.unwrap_or(0.5).clamp(0.1, 1.0), cancel.clone()) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    };
    let (tx, trx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        for chunk in rx {
            if tx.blocking_send(chunk).is_err() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
        }
    });
    let stream = futures::stream::unfold(trx, |mut r| async move { r.recv().await.map(|b| (Ok::<Bytes, std::io::Error>(Bytes::from(b)), r)) });
    ([(header::CONTENT_TYPE, "video/mp4"), (header::CACHE_CONTROL, "no-store")], Body::from_stream(stream)).into_response()
}

async fn api_render(State(st): State<St>, Json(a): Json<Value>) -> Response {
    let s = |k: &str| a[k].as_str().map(|x| x.to_string());
    let (t0, t1) = match s("range").filter(|r| !r.trim().is_empty()) {
        Some(r) => match st.eng.render_range(&s("comp").unwrap_or_default(), &r) {
            Ok(x) => x,
            Err(e) => return err(StatusCode::BAD_REQUEST, format!("{e:#}")),
        },
        None => (0.0, 0.0),
    };
    let opts = RenderOpts {
        comp: s("comp").unwrap_or_default(),
        t0,
        t1,
        preset: s("preset").unwrap_or_else(|| "mp4".into()),
        out: s("out").map(std::path::PathBuf::from).unwrap_or_default(),
        scale: a["scale"].as_f64().unwrap_or(1.0) as f32,
        hw: s("hw").unwrap_or_else(|| "off".into()),
    };
    match st.eng.start_render(opts) {
        Ok(j) => Json(j.status()).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

async fn api_jobs(State(st): State<St>) -> Response {
    let jobs: Vec<Value> = st.eng.jobs.lock().iter().map(|j| j.status()).collect();
    Json(json!(jobs)).into_response()
}

async fn api_catalog() -> Response {
    use crate::model::schema;
    let pjson = |l: &[schema::P]| -> Vec<Value> {
        l.iter()
            .map(|p| {
                let ty = match p.t {
                    schema::T::Num => json!("num"),
                    schema::T::Vec2 => json!("vec2"),
                    schema::T::Color => json!("color"),
                    schema::T::Str => json!("str"),
                    schema::T::Bool => json!("bool"),
                    schema::T::Enum(o) => json!(o),
                };
                let def = schema::default_val(p, schema::VCtx { cw: 1920.0, ch: 1080.0 });
                json!({"name": p.name, "type": ty, "def": def, "min": if p.min.is_finite() { json!(p.min) } else { Value::Null }, "max": if p.max.is_finite() { json!(p.max) } else { Value::Null }, "anim": p.anim, "desc": p.desc})
            })
            .collect()
    };
    let fx: Vec<Value> = schema::EFFECTS.iter().map(|e| json!({"name": e.name, "cat": e.cat, "desc": e.desc, "ff": e.engine == schema::Engine::Ff, "params": pjson(e.params)})).collect();
    let afx: Vec<Value> = schema::AUDIO_EFFECTS.iter().map(|e| json!({"name": e.name, "desc": e.desc, "params": pjson(e.params)})).collect();
    let tr: Vec<Value> = schema::TRANSITIONS.iter().map(|t| json!({"name": t.name, "desc": t.desc, "params": pjson(t.params)})).collect();
    let anims: Vec<Value> = crate::ops::presets::LAYER_ENTER.iter().chain(crate::ops::presets::LAYER_EXIT).chain(crate::ops::presets::LAYER_LOOP).chain(crate::ops::presets::LAYER_ONESHOT).map(|(n, d)| json!({"name": n, "desc": d})).collect();
    let text_anims: Vec<Value> = crate::ops::presets::TEXT_ANIMS.iter().map(|(n, d)| json!({"name": n, "desc": d})).collect();
    let templates: Vec<Value> = crate::ops::templates::TEMPLATES.iter().map(|(n, p, d)| json!({"name": n, "params": p, "desc": d})).collect();
    let presets: Vec<Value> = encode::PRESETS.iter().map(|(n, d)| json!({"name": n, "desc": d})).collect();
    let colors: Vec<Value> = crate::ops::presets::COLOR_PRESETS.iter().map(|(n, d)| json!({"name": n, "desc": d})).collect();
    Json(json!({
        "effects": fx, "audio_effects": afx, "transitions": tr, "anims": anims, "text_anims": text_anims, "templates": templates, "presets": presets, "colors": colors,
        "props": {"layer": pjson(schema::LAYER), "clip": pjson(schema::CLIP), "text": pjson(schema::TEXT), "solid": pjson(schema::SOLID), "comp": pjson(schema::NESTED), "camera": pjson(schema::CAMERA), "shape": pjson(schema::SHAPE_COMMON), "mask": pjson(schema::MASK), "animator": pjson(schema::ANIMATOR)},
        "scopes": crate::render::scopes::KINDS,
        "sfx": crate::sfx::KINDS.iter().map(|k| json!({"name": k.name, "desc": k.desc, "dur": k.dur, "sync": k.sync, "vol": k.vol})).collect::<Vec<_>>(),
        "brands": crate::ops::pro::BRAND_PRESETS.iter().map(|(n, u, v)| json!({"name": n, "use": u, "font": v[0], "weight": v[1], "primary": v[2], "accent": v[3], "text": v[4], "dark": v[5]})).collect::<Vec<_>>(),
        "shape_kinds": schema::SHAPE_KINDS.iter().map(|k| json!({"name": k, "params": pjson(schema::shape_props(k).unwrap_or(&[]))})).collect::<Vec<_>>()
    }))
    .into_response()
}

#[derive(Deserialize)]
struct FsQ {
    #[serde(default)]
    path: Option<String>,
}

async fn api_fs(State(st): State<St>, Query(q): Query<FsQ>) -> Response {
    let dir = match q.path.filter(|p| !p.is_empty()) {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            if cfg!(windows) {
                std::env::var("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default()
            } else {
                std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_default()
            }
        }
    };
    if st.eng.check_path(&dir.to_string_lossy()).is_err() {
        return err(StatusCode::FORBIDDEN, "not allowed");
    }
    let mut entries = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = p.is_dir();
            if is_dir || crate::engine::is_media(&p) || name.ends_with(".mcut") || name.ends_with(".cube") {
                entries.push(json!({"name": name, "path": p.to_string_lossy(), "dir": is_dir}));
            }
        }
    }
    entries.sort_by(|a, b| (!a["dir"].as_bool().unwrap_or(false), a["name"].as_str().unwrap_or("").to_lowercase()).cmp(&(!b["dir"].as_bool().unwrap_or(false), b["name"].as_str().unwrap_or("").to_lowercase())));
    let mut roots: Vec<String> = vec![];
    if cfg!(windows) {
        for d in b'C'..=b'Z' {
            let r = format!("{}:\\", d as char);
            if std::path::Path::new(&r).exists() {
                roots.push(r);
            }
        }
    } else {
        roots.push("/".into());
    }
    Json(json!({"path": dir.to_string_lossy(), "parent": dir.parent().map(|p| p.to_string_lossy().to_string()), "entries": entries, "roots": roots})).into_response()
}

#[derive(Deserialize)]
struct DialogReq {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    filter: Option<String>,
}

async fn api_dialog(Json(r): Json<DialogReq>) -> Response {
    let kind = r.kind.unwrap_or_else(|| "open".into());
    let filter = r.filter.unwrap_or_else(|| "media".into());
    match blocking(move || native_dialog(&kind, &filter)).await {
        Ok(paths) => Json(json!({"ok": true, "paths": paths})).into_response(),
        Err(e) => Json(json!({"ok": false, "error": format!("{e:#}")})).into_response(),
    }
}

/// Native OS file dialogs without extra dependencies.
fn native_dialog(kind: &str, filter: &str) -> anyhow::Result<Vec<String>> {
    use std::process::Stdio;
    let run = |exe: &str, args: &[&str]| -> anyhow::Result<String> {
        let out = crate::render::ffmpeg::command(std::path::Path::new(exe)).args(args).stdout(Stdio::piped()).stderr(Stdio::null()).output()?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let s = if cfg!(windows) {
        let flt = match filter {
            "project" => "MoodCut project|*.mcut",
            "lut" => "LUT|*.cube",
            _ => "Media|*.mp4;*.mov;*.mkv;*.avi;*.webm;*.m4v;*.mts;*.mxf;*.gif;*.mp3;*.wav;*.aac;*.m4a;*.flac;*.ogg;*.png;*.jpg;*.jpeg;*.webp;*.svg;*.tif;*.bmp|All files|*.*",
        };
        let ps = match kind {
            "save" => format!("Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.SaveFileDialog; $f.Filter = '{flt}'; if ($f.ShowDialog() -eq 'OK') {{ $f.FileName }}"),
            "folder" => "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; if ($f.ShowDialog() -eq 'OK') { $f.SelectedPath }".to_string(),
            _ => format!("Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.OpenFileDialog; $f.Multiselect = $true; $f.Filter = '{flt}'; if ($f.ShowDialog() -eq 'OK') {{ $f.FileNames -join \"`n\" }}"),
        };
        run("powershell", &["-NoProfile", "-STA", "-Command", &ps])?
    } else if cfg!(target_os = "macos") {
        let script = match kind {
            "save" => "POSIX path of (choose file name)",
            "folder" => "POSIX path of (choose folder)",
            _ => "set fs to choose file with multiple selections allowed\nset out to \"\"\nrepeat with f in fs\nset out to out & POSIX path of f & linefeed\nend repeat\nout",
        };
        run("osascript", &["-e", script])?
    } else {
        let mut args = vec!["--file-selection"];
        match kind {
            "save" => args.push("--save"),
            "folder" => args.push("--directory"),
            _ => {
                args.push("--multiple");
                args.push("--separator=\n");
            }
        }
        run("zenity", &args)?
    };
    Ok(s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

async fn font_file(AxPath(name): AxPath<String>) -> Response {
    use crate::render::text::{INTER_BLACK, INTER_BOLD, INTER_REGULAR};
    let data: &'static [u8] = match name.as_str() {
        "Inter-Regular.otf" => INTER_REGULAR,
        "Inter-Bold.otf" => INTER_BOLD,
        "Inter-Black.otf" => INTER_BLACK,
        _ => return (StatusCode::NOT_FOUND, "no such font").into_response(),
    };
    ([(header::CONTENT_TYPE, "font/otf"), (header::CACHE_CONTROL, "max-age=31536000")], data).into_response()
}

/// Source and settings of an HTML graphic, for the editor dialog.
async fn api_graphic_get(State(st): State<St>, Query(q): Query<HashMap<String, String>>) -> Response {
    let Some(id) = q.get("id") else { return err(StatusCode::BAD_REQUEST, "id required") };
    let (p, ..) = st.eng.snapshot();
    let Some(m) = p.media(id) else { return err(StatusCode::NOT_FOUND, format!("unknown media {id}")) };
    let Some(g) = &m.gfx else { return err(StatusCode::BAD_REQUEST, format!("{id} isn't an HTML graphic")) };
    let html = std::fs::read_to_string(&g.html).unwrap_or_default();
    Json(json!({"ok": true, "id": m.id, "name": m.name, "html": html, "dur": g.dur, "fps": g.fps, "w": g.w, "h": g.h, "alpha": g.alpha, "vars": g.vars})).into_response()
}

/// Renders an HTML graphic (same arguments as the MCP `graphic` tool).
async fn api_graphic(State(st): State<St>, Json(a): Json<Value>) -> Response {
    let eng = st.eng.clone();
    let req = crate::mcp::graphic_req(&a);
    match blocking(move || eng.graphic(req, "gui")).await {
        Ok(o) => {
            use base64::Engine as _;
            let sheet = o.sheet.map(|j| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(j)));
            Json(json!({"ok": true, "id": o.id, "text": o.text, "sheet": sheet})).into_response()
        }
        Err(e) => Json(json!({"ok": false, "error": format!("{e:#}")})).into_response(),
    }
}

/// Starts/stops/reports the public web link (local requests only).
/// One-click FFmpeg install from the editor (portable build into the data
/// folder); the engine switches to it as soon as it's unpacked.
async fn api_setup_ffmpeg(State(st): State<St>, headers: HeaderMap, Json(a): Json<Value>) -> Response {
    if is_remote(&headers) {
        return err(StatusCode::FORBIDDEN, "FFmpeg can only be installed from this computer");
    }
    if a["action"].as_str() == Some("start") && !st.eng.ff.available() {
        let mut g = st.ff_setup.lock();
        if !g.0 {
            *g = (true, "starting".into(), None);
            let (state, eng, st2) = (st.ff_setup.clone(), st.eng.clone(), st.clone());
            std::thread::spawn(move || {
                let r = crate::setup::install_ffmpeg(&|m: String| state.lock().1 = m);
                let mut g = state.lock();
                g.0 = false;
                match r {
                    Ok(_) if eng.ff.reload() => {
                        g.1 = format!("FFmpeg {} ready", eng.ff.version());
                        st2.cache.lock().clear();
                        eng.emit("setup", eng.st.read().rev, "setup", g.1.clone());
                    }
                    Ok(_) => g.2 = Some("FFmpeg was installed but couldn't be started".into()),
                    Err(e) => g.2 = Some(format!("{e:#}")),
                }
            });
        }
    }
    let g = st.ff_setup.lock();
    Json(json!({"ok": true, "installed": st.eng.ff.available(), "version": st.eng.ff.version(), "running": g.0, "status": g.1, "error": g.2})).into_response()
}

async fn api_tunnel(State(st): State<St>, headers: HeaderMap, Json(a): Json<Value>) -> Response {
    if is_remote(&headers) {
        return err(StatusCode::FORBIDDEN, "the web link can only be managed from this computer");
    }
    let action = a["action"].as_str().unwrap_or("status").to_string();
    let st2 = st.clone();
    let r = blocking(move || -> anyhow::Result<Value> {
        if action == "stop" {
            *st2.tunnel.lock() = None;
            *st2.public_url.write() = None;
            return Ok(json!({"ok": true, "running": false}));
        }
        if action == "url" {
            // your own public address (named tunnel, Tailscale Funnel, reverse proxy)
            let u = a["url"].as_str().map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| u.starts_with("https://") || u.starts_with("http://")).ok_or_else(|| anyhow::anyhow!("url must start with https://"))?;
            if st2.remote_token.read().is_none() {
                *st2.remote_token.write() = Some(crate::tunnel::persistent_token());
            }
            *st2.tunnel.lock() = None;
            *st2.public_url.write() = Some(u);
        }
        if action == "start" {
            let running = st2.tunnel.lock().as_mut().map(|t| t.running()).unwrap_or(false);
            if !running {
                if st2.remote_token.read().is_none() {
                    *st2.remote_token.write() = Some(crate::tunnel::persistent_token());
                }
                let t = crate::tunnel::start(st2.port)?;
                *st2.tunnel.lock() = Some(t);
            }
        }
        let mut g = st2.tunnel.lock();
        let mut live = None;
        if let Some(t) = g.as_mut() {
            if t.running() {
                live = Some(t.url.clone());
            }
        }
        drop(g);
        let url = live.or_else(|| st2.public_url.read().clone());
        let tok = st2.remote_token.read().clone();
        let connector = match (&url, &tok) {
            (Some(u), Some(t)) => Some(crate::tunnel::connector_url(u, t)),
            _ => None,
        };
        Ok(json!({"ok": true, "running": url.is_some(), "url": url, "connector": connector}))
    })
    .await;
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => Json(json!({"ok": false, "error": format!("{e:#}")})).into_response(),
    }
}

/// Runs the edit quality check (same arguments as the MCP `check` tool).
#[derive(Deserialize)]
struct ScopeQ {
    #[serde(default)]
    t: f64,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    comp: Option<String>,
    #[serde(default)]
    w: Option<u32>,
    #[serde(default)]
    h: Option<u32>,
}

/// Lumetri scopes for the editor: a JPEG data URL plus the exposure summary.
async fn api_scope(State(st): State<St>, Query(q): Query<ScopeQ>) -> Response {
    let eng = st.eng.clone();
    let r = blocking(move || -> anyhow::Result<(Vec<u8>, String)> {
        let kind = q.kind.unwrap_or_else(|| "waveform".into());
        let w = q.w.unwrap_or(384).clamp(96, 1600);
        let h = q.h.unwrap_or(if kind == "vectorscope" { w } else { w * 9 / 16 }).clamp(96, 1600);
        let (pm, text) = eng.scope(q.comp.as_deref(), q.t, &kind, w, h)?;
        Ok((encode::encode_jpeg(&pm, 85)?, text))
    })
    .await;
    match r {
        Ok((jpg, text)) => {
            use base64::Engine as _;
            Json(json!({"ok": true, "img": format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(&jpg)), "text": text})).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// Undo labels (newest first) and redo labels (next first) for the History panel.
async fn api_history(State(st): State<St>) -> Response {
    let (undo, redo) = st.eng.history_both(100);
    Json(json!({"undo": undo, "redo": redo})).into_response()
}

#[derive(Deserialize)]
struct LayerQ {
    id: String,
    #[serde(default)]
    t: f64,
}

/// On-screen corners and anchor of a layer (comp px) for the monitor's transform overlay.
async fn api_layer(State(st): State<St>, Query(q): Query<LayerQ>) -> Response {
    let (p, ..) = st.eng.snapshot();
    let Some((ci, ti, ii)) = p.locate(&q.id) else { return err(StatusCode::NOT_FOUND, format!("no item {}", q.id)) };
    let c = &p.comps[ci];
    let it = &c.tracks[ti].items[ii];
    Json(json!({"ok": true, "comp": c.id, "geom": crate::render::comp::layer_geom(&p, c, it, q.t)})).into_response()
}

async fn api_check(State(st): State<St>, Json(a): Json<Value>) -> Response {
    let mcp = st.mcp.clone();
    let r = blocking(move || mcp.call("check", &a)).await;
    let mut text = String::new();
    let mut sheet = None;
    for c in r["content"].as_array().into_iter().flatten() {
        match c["type"].as_str() {
            Some("image") => sheet = c["data"].as_str().map(|d| format!("data:image/jpeg;base64,{d}")),
            _ => text = c["text"].as_str().unwrap_or("").to_string(),
        }
    }
    Json(json!({"ok": r.get("isError").is_none(), "text": text, "sheet": sheet})).into_response()
}

/// Shows a rendered file in the OS file manager.
async fn api_reveal(State(st): State<St>, Json(a): Json<Value>) -> Response {
    let Some(path) = a["path"].as_str().map(|s| s.to_string()) else { return err(StatusCode::BAD_REQUEST, "path required") };
    if st.eng.check_path(&path).is_err() {
        return err(StatusCode::FORBIDDEN, "not allowed");
    }
    let p = std::path::PathBuf::from(&path);
    let r = if cfg!(windows) {
        crate::render::ffmpeg::command(std::path::Path::new("explorer")).arg(format!("/select,{}", path)).spawn()
    } else if cfg!(target_os = "macos") {
        crate::render::ffmpeg::command(std::path::Path::new("open")).args(["-R", &path]).spawn()
    } else {
        crate::render::ffmpeg::command(std::path::Path::new("xdg-open")).arg(p.parent().unwrap_or(&p)).spawn()
    };
    match r {
        Ok(_) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn api_log(State(st): State<St>) -> Response {
    let l: Vec<_> = st.eng.log.lock().iter().rev().take(100).cloned().collect();
    Json(json!(l)).into_response()
}

async fn api_analyze(State(st): State<St>, Json(a): Json<Value>) -> Response {
    let mcp = st.mcp.clone();
    let r = blocking(move || mcp.call("analyze", &a)).await;
    Json(json!({"ok": r.get("isError").is_none(), "text": r["content"][0]["text"]})).into_response()
}

async fn api_ws(State(st): State<St>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |sock| ws_loop(st, sock))
}

async fn ws_loop(st: St, mut sock: WebSocket) {
    let mut rx = st.eng.events.subscribe();
    let hello = json!({"kind": "hello", "rev": st.eng.st.read().rev});
    if sock.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    // the editor's selection and playhead count only while its socket is open
    st.eng.gui_clients.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ws_events(&st, &mut sock, &mut rx).await;
    if st.eng.gui_clients.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) == 1 {
        st.eng.set_ui(Default::default());
    }
}

async fn ws_events(st: &St, sock: &mut WebSocket, rx: &mut tokio::sync::broadcast::Receiver<crate::engine::Event>) {
    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(e) => {
                        let txt = serde_json::to_string(&e).unwrap_or_default();
                        if sock.send(Message::Text(txt.into())).await.is_err() { break; }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            msg = sock.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&t) {
                            if v["kind"] == "ui" {
                                if let Ok(ui) = serde_json::from_value::<crate::ops::Ui>(v["ui"].clone()) {
                                    st.eng.set_ui(ui);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------- MCP over HTTP

async fn mcp_handle(st: St, headers: HeaderMap, body: String) -> Response {
    let msg: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}))).into_response(),
    };
    let is_init = msg["method"] == "initialize";
    let mcp = st.mcp.clone();
    let resp = blocking(move || mcp.handle(&msg)).await;
    let session = if is_init { random_token() } else { headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).unwrap_or("").to_string() };
    match resp {
        None => {
            let mut r = StatusCode::ACCEPTED.into_response();
            if let Ok(v) = HeaderValue::from_str(&session) {
                if !session.is_empty() {
                    r.headers_mut().insert("mcp-session-id", v);
                }
            }
            r
        }
        Some(v) => {
            let mut r = Json(v).into_response();
            if !session.is_empty() {
                if let Ok(h) = HeaderValue::from_str(&session) {
                    r.headers_mut().insert("mcp-session-id", h);
                }
            }
            r
        }
    }
}

async fn mcp_post(State(st): State<St>, headers: HeaderMap, body: String) -> Response {
    mcp_handle(st, headers, body).await
}

async fn mcp_post_tok(State(st): State<St>, AxPath(_tok): AxPath<String>, headers: HeaderMap, body: String) -> Response {
    mcp_handle(st, headers, body).await
}

async fn mcp_get() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST, DELETE")], "SSE stream not offered; POST JSON-RPC to this endpoint").into_response()
}

async fn mcp_delete() -> Response {
    StatusCode::OK.into_response()
}

/// Opens the GUI in a chromeless app window (Edge/Chrome --app), falling back to the default browser.
pub fn open_window(url: &str) {
    use std::process::Stdio;
    let try_run = |exe: &str, args: &[&str]| -> bool {
        crate::render::ffmpeg::command(std::path::Path::new(exe)).args(args).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
    };
    let app = format!("--app={url}");
    if cfg!(windows) {
        let mut cands: Vec<String> = vec![];
        for base in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Ok(b) = std::env::var(base) {
                cands.push(format!("{b}\\Microsoft\\Edge\\Application\\msedge.exe"));
                cands.push(format!("{b}\\Google\\Chrome\\Application\\chrome.exe"));
            }
        }
        for c in cands {
            if std::path::Path::new(&c).exists() && try_run(&c, &[&app, "--window-size=1600,960"]) {
                return;
            }
        }
        let _ = try_run("cmd", &["/C", "start", "", url]);
    } else if cfg!(target_os = "macos") {
        if !try_run("open", &["-na", "Google Chrome", "--args", &app]) {
            let _ = try_run("open", &[url]);
        }
    } else {
        for b in ["chromium", "chromium-browser", "google-chrome", "microsoft-edge"] {
            if try_run(b, &[&app]) {
                return;
            }
        }
        let _ = try_run("xdg-open", &[url]);
    }
}
