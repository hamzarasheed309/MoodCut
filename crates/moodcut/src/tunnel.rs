//! Public HTTPS access for web AI clients — claude.ai custom connectors and
//! ChatGPT developer-mode connectors run in the cloud and need an https URL.
//! A Cloudflare quick tunnel (no account needed) forwards to the local server;
//! the URL carries a persistent secret token (`/mcp/<token>`), and tunneled
//! requests are rejected without it.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Unpredictable token from the OS-seeded hasher keys (no extra dependency).
pub fn random_token() -> String {
    let alphabet = b"abcdefghijkmnopqrstuvwxyz23456789";
    let mut out = String::new();
    let mut i = 0u64;
    while out.len() < 32 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let mut v = h.finish();
        for _ in 0..8 {
            out.push(alphabet[(v % alphabet.len() as u64) as usize] as char);
            v /= alphabet.len() as u64;
        }
        i += 1;
    }
    out.truncate(32);
    out
}

/// The remote-access token, created once and kept in the data folder so a
/// connector URL stays valid across restarts (with a stable tunnel hostname).
pub fn persistent_token() -> String {
    let f = crate::config::ensure_data_dir().join("remote-token");
    if let Some(t) = std::fs::read_to_string(&f).ok().map(|s| s.trim().to_string()).filter(|s| s.len() >= 24) {
        return t;
    }
    let t = random_token();
    let _ = std::fs::write(&f, &t);
    t
}

pub fn connector_url(base: &str, token: &str) -> String {
    format!("{}/mcp/{token}", base.trim_end_matches('/'))
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

pub fn find_cloudflared() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MOODCUT_CLOUDFLARED") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let local = crate::config::ensure_data_dir().join("bin").join(exe("cloudflared"));
    if local.is_file() {
        return Some(local);
    }
    if let Some(p) = crate::setup::which("cloudflared") {
        return Some(p);
    }
    if cfg!(windows) {
        for base in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Ok(b) = std::env::var(base) {
                let p = PathBuf::from(b).join("cloudflared").join("cloudflared.exe");
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Downloads cloudflared from Cloudflare's GitHub releases into the data folder.
pub fn install_cloudflared() -> Result<PathBuf> {
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "amd64" };
    let base = "https://github.com/cloudflare/cloudflared/releases/latest/download";
    let dir = crate::config::ensure_data_dir().join("bin");
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(exe("cloudflared"));
    let curl = crate::setup::which("curl").ok_or_else(|| anyhow!("curl is needed to download cloudflared (built into Windows 10+)"))?;
    let (asset, tgz) = if cfg!(windows) {
        (format!("cloudflared-windows-{arch}.exe"), false)
    } else if cfg!(target_os = "macos") {
        (format!("cloudflared-darwin-{arch}.tgz"), true)
    } else {
        (format!("cloudflared-linux-{arch}"), false)
    };
    let dl = if tgz { dir.join(&asset) } else { dir.join(format!("{}.part", exe("cloudflared"))) };
    eprintln!("downloading {asset} ...");
    let st = crate::render::ffmpeg::command(&curl).args(["-L", "--fail", "--silent", "--show-error", "-o"]).arg(&dl).arg(format!("{base}/{asset}")).status().context("running curl")?;
    if !st.success() {
        bail!("couldn't download cloudflared; install it yourself (Windows: winget install Cloudflare.cloudflared) and retry");
    }
    if tgz {
        let st = crate::render::ffmpeg::command(std::path::Path::new("tar")).arg("-xzf").arg(&dl).arg("-C").arg(&dir).status()?;
        let _ = std::fs::remove_file(&dl);
        if !st.success() {
            bail!("couldn't unpack {asset}");
        }
    } else {
        std::fs::rename(&dl, &target)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    Ok(target)
}

/// A running quick tunnel; stopped when dropped.
pub struct Tunnel {
    child: Child,
    pub url: String,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Tunnel {
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

/// Starts `cloudflared tunnel --url http://127.0.0.1:<port>` and waits for its
/// public https URL.
pub fn start(port: u16) -> Result<Tunnel> {
    let exe = match find_cloudflared() {
        Some(p) => p,
        None => install_cloudflared()?,
    };
    let mut child = crate::render::ffmpeg::command(&exe)
        .args(["tunnel", "--no-autoupdate", "--url"])
        .arg(format!("http://127.0.0.1:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    // cloudflared logs to stderr; keep draining both pipes for its whole life
    for pipe in [child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>), child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if let Some(u) = find_url(&line) {
                    let _ = tx.send(Ok(u));
                } else if line.contains("failed to request quick Tunnel") || line.contains(" ERR ") && line.to_ascii_lowercase().contains("quick tunnel") {
                    let _ = tx.send(Err(line.trim().to_string()));
                }
            }
        });
    }
    drop(tx);
    match rx.recv_timeout(Duration::from_secs(45)) {
        Ok(Ok(url)) => Ok(Tunnel { child, url }),
        Ok(Err(why)) => {
            let _ = child.kill();
            bail!("cloudflared couldn't create a tunnel: {why}")
        }
        Err(_) => {
            let _ = child.kill();
            bail!("cloudflared didn't report a public URL within 45s (is the internet reachable?)")
        }
    }
}

/// Extracts the public `https://<name>.trycloudflare.com` from cloudflared's
/// banner line (not API URLs from its error messages).
pub fn find_url(line: &str) -> Option<String> {
    if line.contains("failed") || line.contains("error") || line.contains(" ERR ") {
        return None;
    }
    let i = line.find("https://")?;
    let rest = &line[i + 8..];
    let end = rest.find(|c: char| c.is_whitespace() || c == '|' || c == '"').unwrap_or(rest.len());
    let host = rest[..end].trim_end_matches('/');
    let sub = host.strip_suffix(".trycloudflare.com")?;
    let ok = !sub.is_empty() && sub != "api" && sub.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| format!("https://{host}"))
}

/// Best-effort copy to the system clipboard.
pub fn copy_to_clipboard(s: &str) -> bool {
    use std::io::Write;
    let tools: &[(&str, &[&str])] = if cfg!(windows) {
        &[("clip", &[])]
    } else if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
    };
    for (cmd, args) in tools {
        let Some(path) = crate::setup::which(cmd) else { continue };
        if let Ok(mut c) = crate::render::ffmpeg::command(&path).args(*args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            if let Some(mut i) = c.stdin.take() {
                let _ = i.write_all(s.as_bytes());
            }
            return c.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    false
}

/// Human instructions for adding the connector.
pub fn instructions(connector: &str) -> String {
    format!(
        "MoodCut is reachable from the web at:\n\n  {connector}\n\n\
         claude.ai (Pro/Max/Team/Enterprise): Settings > Connectors > Add custom connector > paste the URL > Add.\n\
         ChatGPT: Settings > Connectors > Advanced > Developer mode on > Create > paste the URL (no authentication).\n\
         Then ask in a chat, e.g. \"Use MoodCut to cut my D:/Footage/Trip folder into a 60s recap\".\n\
         Keep this window open while you work. The URL contains a secret: share it only with your AI client."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cloudflared_url_and_tokens_differ() {
        let l = "2026-10-18T10:00:00Z INF |  https://quiet-river-1234.trycloudflare.com                               |";
        assert_eq!(find_url(l).as_deref(), Some("https://quiet-river-1234.trycloudflare.com"));
        assert_eq!(find_url("INF Requesting new quick Tunnel on trycloudflare.com..."), None);
        assert_eq!(find_url("failed to request quick Tunnel: Post \"https://api.trycloudflare.com/tunnel\": dial tcp"), None);
        assert_eq!(find_url("INF see https://api.trycloudflare.com/tunnel"), None);
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert_eq!(connector_url("https://x.trycloudflare.com/", &a), format!("https://x.trycloudflare.com/mcp/{a}"));
    }
}
