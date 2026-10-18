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
    pub token: Option<String>,
    pub open_window: bool,
}

struct AppState {
    eng: Arc<Engine>,
    mcp: Arc<Mcp>,
    token: Option<String>,
    cache: Mutex<HashMap<String, (Vec<u8>, &'static str)>>,
    thumb_slots: tokio::sync::Semaphore,
}

type St = Arc<AppState>;

pub fn random_token() -> String {
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) ^ (std::process::id() as u64).wrapping_mul(0x9E3779B97F4A7C15);
    let mut s = String::new();
    for _ in 0..32 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let c = b"abcdefghijkmnopqrstuvwxyz23456789"[(seed % 33) as usize] as char;
        s.push(c);
    }
    s
}

pub async fn serve(eng: Arc<Engine>, o: ServeOpts) -> Result<()> {
    let mcp = Arc::new(Mcp::new(Ok(eng.clone())));
    let st: St = Arc::new(AppState { eng: eng.clone(), mcp, token: o.token.clone(), cache: Mutex::new(HashMap::new()), thumb_slots: tokio::sync::Semaphore::new(4) });
    let app = Router::new()
        .route("/api/ping", get(|| async { Json(json!({"app": "moodcut", "version": crate::VERSION})) }))
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
        .route("/mcp", post(mcp_post).get(mcp_get).delete(mcp_delete))
        .route("/mcp/{token}", post(mcp_post_tok).get(mcp_get).delete(mcp_delete))
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(st.clone(), guard))
        .with_state(st.clone());
    let addr = format!("{}:{}", o.host, o.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| anyhow::anyhow!("can't listen on {addr}: {e}"))?;
    let url = format!("http://127.0.0.1:{}", o.port);
    eprintln!("MoodCut {} — GUI {url}  |  MCP {url}/mcp", crate::VERSION);
    if let Some(t) = &o.token {
        eprintln!("token auth on. MCP URL with token: {url}/mcp/{t}  (or header Authorization: Bearer {t})");
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

fn is_local_origin(o: &str) -> bool {
    let o = o.trim_end_matches('/');
    let host = o.split("://").nth(1).unwrap_or(o);
    let host = host.split(':').next().unwrap_or(host);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "tauri.localhost") || o == "null"
}

/// Token auth + DNS-rebinding protection.
async fn guard(State(st): State<St>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let path = req.uri().path().to_string();
    let q = req.uri().query().unwrap_or("").to_string();
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(|s| s.to_string());
    let cookie_tok = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).and_then(|c| c.split(';').find_map(|kv| kv.trim().strip_prefix("mc_token=").map(|s| s.to_string())));
    let query_tok = q.split('&').find_map(|kv| kv.strip_prefix("token=").map(|s| s.to_string()));
    let path_tok = path.strip_prefix("/mcp/").map(|s| s.to_string());
    let authed = match &st.token {
        None => true,
        Some(t) => [bearer, cookie_tok, query_tok.clone(), path_tok].iter().any(|x| x.as_deref() == Some(t.as_str())),
    };
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if !is_local_origin(origin) && (st.token.is_none() || !authed) {
            return (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response();
        }
    }
    let public_path = path == "/api/ping" || (!path.starts_with("/api/") && !path.starts_with("/mcp"));
    if !authed && !public_path {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "missing or invalid token"}))).into_response();
    }
    let mut resp = next.run(req).await;
    if let (Some(t), Some(qt)) = (&st.token, query_tok) {
        if &qt == t {
            if let Ok(v) = HeaderValue::from_str(&format!("mc_token={t}; Path=/; SameSite=Strict; HttpOnly")) {
                resp.headers_mut().insert(header::SET_COOKIE, v);
            }
        }
    }
    resp
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
        "undo": undo, "redo": redo, "ffmpeg": st.eng.ff.version, "version": crate::VERSION
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
    let (t0, t1) = s("range").and_then(|r| r.split_once('-').and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))).unwrap_or((0.0, 0.0));
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
        "props": {"layer": pjson(schema::LAYER), "clip": pjson(schema::CLIP), "text": pjson(schema::TEXT), "solid": pjson(schema::SOLID), "comp": pjson(schema::NESTED), "shape": pjson(schema::SHAPE_COMMON), "mask": pjson(schema::MASK), "animator": pjson(schema::ANIMATOR)},
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
