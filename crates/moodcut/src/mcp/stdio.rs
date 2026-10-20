//! MCP stdio transport. If a MoodCut instance (GUI/server) is already running
//! locally, stdio is proxied to it so edits show up live in the open editor;
//! otherwise an embedded engine is started (plus the local web GUI). When the
//! editor closes mid-conversation, the proxy switches to an embedded engine
//! with the same project instead of failing, so the AI app never gets stuck.

use super::Mcp;
use crate::engine::Engine;
use crate::render::ffmpeg::Ff;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Minimal HTTP/1.1 request to the local MoodCut server.
pub async fn local_http(port: u16, method: &str, path: &str, body: Option<&str>, session: Option<&str>) -> Result<(u16, Vec<(String, String)>, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await?;
    let body = body.unwrap_or("");
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    if let Some(sid) = session {
        req.push_str(&format!("Mcp-Session-Id: {sid}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, rest) = text.split_once("\r\n\r\n").ok_or_else(|| anyhow!("bad http response"))?;
    let mut lines = head.lines();
    let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))).collect();
    let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v.contains("chunked"));
    let body = if chunked { dechunk(rest) } else { rest.to_string() };
    Ok((status, headers, body))
}

pub fn dechunk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    loop {
        let Some((size_line, after)) = rest.split_once("\r\n") else { break };
        let size = usize::from_str_radix(size_line.trim(), 16).unwrap_or(0);
        if size == 0 || after.len() < size {
            break;
        }
        out.push_str(&after[..size]);
        rest = after[size..].trim_start_matches("\r\n");
    }
    out
}

async fn write_line(stdout: &mut tokio::io::Stdout, v: &str) -> Result<()> {
    stdout.write_all(v.trim().as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

/// Starts MoodCut's own engine (with the web GUI on a free port), opening `path` if given.
fn embedded(port: u16, with_http: bool, path: Option<&str>) -> Arc<Mcp> {
    let eng = Engine::new(Ff::find().unwrap_or_else(|_| Ff::missing()));
    eng.proxies.set_active();
    if let Some(p) = path.filter(|p| std::path::Path::new(p).is_file()) {
        if let Err(e) = eng.open(p) {
            eprintln!("moodcut: couldn't reopen {p}: {e:#}");
        }
    }
    if with_http {
        let e2 = eng.clone();
        let port = crate::server::free_port(port);
        tokio::spawn(async move {
            let opts = crate::server::ServeOpts { host: "127.0.0.1".into(), port, token: None, open_window: false, tunnel: false, public_url: None };
            if let Err(err) = crate::server::serve(e2, opts).await {
                eprintln!("moodcut: web GUI not started ({err})");
            }
        });
    }
    Arc::new(Mcp::new(Ok(eng)))
}

async fn handle_embedded(mcp: &Arc<Mcp>, line: &str, stdout: &mut tokio::io::Stdout) -> Result<()> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            let err = serde_json::json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}});
            return write_line(stdout, &err.to_string()).await;
        }
    };
    let m = mcp.clone();
    let id = msg.get("id").cloned();
    let resp = match tokio::task::spawn_blocking(move || m.handle(&msg)).await {
        Ok(r) => r,
        // keep serving: answer this request with an error instead of exiting
        Err(e) => id.map(|id| serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": format!("internal error: {e}")}})),
    };
    if let Some(r) = resp {
        write_line(stdout, &serde_json::to_string(&r)?).await?;
    }
    Ok(())
}

pub async fn run(port: u16, with_http: bool) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    let found = tokio::task::spawn_blocking(move || crate::server::find_instance(port)).await.ok().flatten();
    // proxying to an open editor until it goes away; then our own engine
    let mut target: Option<(u16, Option<String>)> = found.map(|(p, v)| (p, v["path"].as_str().map(|s| s.to_string())));
    let mut local: Option<Arc<Mcp>> = None;
    match &target {
        Some((p, _)) => eprintln!("moodcut: attached to the MoodCut editor on port {p}"),
        None => local = Some(embedded(port, with_http, None)),
    }
    let mut session: Option<String> = None;
    let mut first = true;
    while let Some(raw) = lines.next_line().await? {
        // a byte-order mark from some Windows shells would break the first message
        let line = if first { raw.trim_start_matches('\u{feff}').to_string() } else { raw };
        first = false;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(mcp) = &local {
            handle_embedded(mcp, &line, &mut stdout).await?;
            continue;
        }
        let (p, last_path) = target.clone().expect("proxy target");
        match local_http(p, "POST", "/mcp", Some(&line), session.as_deref()).await {
            Ok((code, headers, body)) => {
                if let Some((_, v)) = headers.iter().find(|(k, _)| k == "mcp-session-id") {
                    session = Some(v.clone());
                }
                if code != 202 && !body.trim().is_empty() {
                    let payload = if body.starts_with("event:") || body.starts_with("data:") {
                        body.lines().filter_map(|l| l.strip_prefix("data:")).map(|d| d.trim()).collect::<Vec<_>>().join("\n")
                    } else {
                        body
                    };
                    write_line(&mut stdout, &payload).await?;
                }
                // remember the open project, to carry on with it if the editor closes
                if line.contains("\"tools/call\"") {
                    if let Ok(Ok((200, _, b))) = tokio::time::timeout(std::time::Duration::from_millis(500), local_http(p, "GET", "/api/ping", None, None)).await {
                        // the editor's answer is the truth: null = an unsaved project
                        if let Ok(v) = serde_json::from_str::<Value>(&b) {
                            target = Some((p, v["path"].as_str().map(|s| s.to_string())));
                        }
                    }
                }
            }
            Err(e) => {
                // the editor went away: continue with our own engine and the same project
                eprintln!("moodcut: the editor on port {p} closed ({e}); continuing with MoodCut's own engine{}", last_path.as_deref().map(|x| format!(" and {x}")).unwrap_or_default());
                let mcp = embedded(port, with_http, last_path.as_deref());
                handle_embedded(&mcp, &line, &mut stdout).await?;
                local = Some(mcp);
            }
        }
    }
    Ok(())
}
