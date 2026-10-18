//! MCP stdio transport. If a MoodCut instance (GUI/server) is already running
//! locally, stdio is proxied to it so edits show up live in the open editor;
//! otherwise an embedded engine is started (plus the local web GUI).

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

fn dechunk(s: &str) -> String {
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

async fn is_moodcut(port: u16) -> bool {
    match tokio::time::timeout(std::time::Duration::from_millis(600), local_http(port, "GET", "/api/ping", None, None)).await {
        Ok(Ok((200, _, body))) => body.contains("moodcut"),
        _ => false,
    }
}

pub async fn run(port: u16, with_http: bool) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    if is_moodcut(port).await {
        eprintln!("moodcut: attached to running instance on port {port}");
        let mut session: Option<String> = None;
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            match local_http(port, "POST", "/mcp", Some(&line), session.as_deref()).await {
                Ok((code, headers, body)) => {
                    if let Some((_, v)) = headers.iter().find(|(k, _)| k == "mcp-session-id") {
                        session = Some(v.clone());
                    }
                    if code == 202 || body.trim().is_empty() {
                        continue;
                    }
                    let payload = if body.starts_with("event:") || body.starts_with("data:") {
                        body.lines().filter_map(|l| l.strip_prefix("data:")).map(|d| d.trim()).collect::<Vec<_>>().join("\n")
                    } else {
                        body
                    };
                    stdout.write_all(payload.trim().as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
                Err(e) => {
                    // the GUI went away: answer with an error so the client isn't stuck
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        if let Some(id) = v.get("id") {
                            let err = serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": format!("MoodCut window closed: {e}. Restart the MCP server.")}});
                            stdout.write_all(format!("{err}\n").as_bytes()).await?;
                            stdout.flush().await?;
                        }
                    }
                }
            }
        }
        return Ok(());
    }
    let eng: Result<Arc<Engine>, String> = Ok(Engine::new(Ff::find().unwrap_or_else(|_| Ff::missing())));
    if with_http {
        if let Ok(e) = &eng {
            let e2 = e.clone();
            tokio::spawn(async move {
                let opts = crate::server::ServeOpts { host: "127.0.0.1".into(), port, token: None, open_window: false, tunnel: false, public_url: None };
                if let Err(err) = crate::server::serve(e2, opts).await {
                    eprintln!("moodcut: web GUI not started ({err})");
                }
            });
        }
    }
    let mcp = Arc::new(Mcp::new(eng));
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}});
                stdout.write_all(format!("{err}\n").as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };
        let m = mcp.clone();
        let resp = tokio::task::spawn_blocking(move || m.handle(&msg)).await?;
        if let Some(r) = resp {
            stdout.write_all(serde_json::to_string(&r)?.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
