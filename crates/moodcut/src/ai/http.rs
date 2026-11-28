//! HTTP for the AI services through curl (built into Windows 10 and later, everywhere else
//! already), so MoodCut needs no TLS stack of its own. The URL and the headers (API keys) go to
//! curl on its stdin as a config file, never on its command line where other programs could
//! read them. Requests can be cancelled, downloads report their progress, and an HTTP error
//! becomes an `ApiError` with the provider's own code and message, so `tasks::explain` can say
//! what happened in plain words.

use crate::render::ffmpeg::command;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// A provider answered with an HTTP error.
#[derive(Debug)]
pub struct ApiError {
    pub provider: &'static str,
    pub status: u16,
    /// The provider's machine-readable code (`insufficient_credits`, `voice_not_found`...).
    pub code: String,
    pub message: String,
    pub body: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} answered HTTP {}", super::keys::name(self.provider), self.status)?;
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        if !self.code.is_empty() && !self.message.contains(&self.code) {
            write!(f, " ({})", self.code)?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

pub enum Body<'a> {
    None,
    Json(&'a Value),
    /// multipart/form-data: text fields, then files as (field, path, mime type)
    Form(Vec<(String, String)>, Vec<(String, PathBuf, String)>),
    /// A file sent as the raw body (presigned uploads); set its Content-Type in the headers.
    Raw(PathBuf),
}

pub struct Call<'a> {
    pub provider: &'static str,
    pub method: &'a str,
    pub url: String,
    /// "Name: value" lines; they travel on curl's stdin (API keys).
    pub headers: Vec<String>,
    pub body: Body<'a>,
    /// Seconds for the whole request.
    pub timeout: u64,
}

pub struct Resp {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

fn curl() -> Result<PathBuf> {
    crate::setup::sys_tool("curl").context("curl is needed to reach the AI services (it is built into Windows 10 and later)")
}

/// A fresh file in MoodCut's temporary folder for AI requests.
pub fn tmp_file(ext: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = crate::config::ensure_data_dir().join("ai-tmp");
    let _ = std::fs::create_dir_all(&dir);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    dir.join(format!("{}-{}-{nanos:x}.{ext}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

/// A value in a curl config file: quoted, with `\` and `"` escaped.
fn conf_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Runs one request. With `out`, the body is saved there (downloads) and `progress` hears the
/// bytes so far; otherwise it is returned. `cancel` stops it at once.
pub fn send(c: &Call, out: Option<&Path>, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<Resp> {
    let curl = curl()?;
    let mut cfg = format!("url = {}\n", conf_quote(&c.url));
    let mut headers = c.headers.clone();
    let mut cleanup: Vec<PathBuf> = vec![];
    let mut cmd = command(&curl);
    cmd.args(["-sS", "-X", c.method, "--connect-timeout", "20", "--max-time"]).arg(c.timeout.max(10).to_string());
    match &c.body {
        Body::None => {}
        Body::Json(v) => {
            let f = tmp_file("json");
            std::fs::write(&f, serde_json::to_vec(v)?).with_context(|| format!("writing {}", f.display()))?;
            headers.push("Content-Type: application/json".into());
            cmd.arg("--data-binary").arg(format!("@{}", f.display()));
            cleanup.push(f);
        }
        Body::Form(fields, files) => {
            for (k, v) in fields {
                cmd.arg("--form-string").arg(format!("{k}={v}"));
            }
            for (k, p, mime) in files {
                cmd.arg("-F").arg(format!("{k}=@\"{}\";type={mime}", p.display()));
            }
        }
        Body::Raw(p) => {
            cmd.arg("--data-binary").arg(format!("@{}", p.display()));
        }
    }
    for h in &headers {
        if h.contains('\n') || h.contains('\r') {
            bail!("a header has a line break");
        }
        cfg.push_str(&format!("header = {}\n", conf_quote(h)));
    }
    let body_file = tmp_file("body");
    let target = out.map(Path::to_path_buf).unwrap_or_else(|| body_file.clone());
    if let Some(d) = target.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    cmd.arg("-o").arg(&target).args(["-w", "%{http_code}", "-K", "-"]);
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("starting curl")?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(cfg.as_bytes());
    }
    let mut so = child.stdout.take().ok_or_else(|| anyhow!("curl has no stdout"))?;
    let mut se = child.stderr.take().ok_or_else(|| anyhow!("curl has no stderr"))?;
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = so.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = se.read_to_string(&mut s);
        s
    });
    let done = |files: &[PathBuf]| {
        for f in files {
            let _ = std::fs::remove_file(f);
        }
    };
    let st = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {}
            Err(e) => {
                done(&cleanup);
                return Err(anyhow!("curl: {e}"));
            }
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            done(&cleanup);
            let _ = std::fs::remove_file(&target);
            bail!("cancelled");
        }
        if out.is_some() {
            progress(super::file_len(&target));
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    done(&cleanup);
    let code_text = t_out.join().unwrap_or_default();
    let err_text = t_err.join().unwrap_or_default();
    if !st.success() {
        let _ = std::fs::remove_file(&target);
        let why = err_text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no answer").to_string();
        bail!("couldn't reach {}: {why}", super::keys::name(c.provider));
    }
    let status = code_text.trim().parse::<u16>().unwrap_or(0);
    let ok = (200..300).contains(&status);
    // a download that failed wrote the error body where the file should go
    let body = if out.is_none() || !ok { std::fs::read(&target).unwrap_or_default() } else { vec![] };
    if out.is_none() || !ok {
        let _ = std::fs::remove_file(&target);
    }
    Ok(Resp { status, body })
}

/// The provider's error code and message from an error body, whatever its shape:
/// `{"detail": {"code", "message"}}` (ElevenLabs), `{"error": "..."}` (Runway),
/// `{"detail": [{"loc", "msg"}]}` (validation), `{"error": {"message", "code"}}`, plain text.
pub fn error_parts(body: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(body).trim().to_string();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let s = |v: &Value| v.as_str().map(str::to_string);
    let validation = |list: &Vec<Value>| {
        list.iter()
            .map(|e| {
                let loc: Vec<String> = e["loc"].as_array().map(|l| l.iter().filter(|x| x.as_str() != Some("body")).map(|x| x.to_string().trim_matches('"').to_string()).collect()).unwrap_or_default();
                let msg = s(&e["msg"]).or_else(|| s(&e["message"])).unwrap_or_else(|| e.to_string());
                if loc.is_empty() {
                    msg
                } else {
                    format!("{}: {msg}", loc.join("."))
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let d = &v["detail"];
    let (mut code, mut msg) = (String::new(), String::new());
    if d.is_object() {
        code = s(&d["code"]).or_else(|| s(&d["status"])).or_else(|| s(&d["type"])).unwrap_or_default();
        msg = s(&d["message"]).unwrap_or_default();
    } else if let Some(t) = d.as_str() {
        msg = t.to_string();
    } else if let Some(list) = d.as_array() {
        msg = validation(list);
        code = "invalid_parameters".into();
    }
    let e = &v["error"];
    if msg.is_empty() {
        if let Some(t) = e.as_str() {
            msg = t.to_string();
        } else if e.is_object() {
            msg = s(&e["message"]).unwrap_or_default();
            if code.is_empty() {
                code = s(&e["code"]).or_else(|| s(&e["type"])).unwrap_or_default();
            }
        }
    }
    if let Some(list) = v["issues"].as_array() {
        let more = validation(list);
        if !more.is_empty() {
            msg = if msg.is_empty() { more } else { format!("{msg}: {more}") };
        }
    }
    if msg.is_empty() {
        msg = s(&v["message"]).or_else(|| s(&v["msg"])).unwrap_or_default();
    }
    if code.is_empty() {
        code = s(&v["error_type"]).or_else(|| s(&v["code"])).unwrap_or_default();
    }
    if msg.is_empty() && v.is_null() && !text.starts_with('<') {
        msg = text.chars().take(300).collect();
    }
    (code, msg.chars().take(600).collect())
}

pub fn api_error(provider: &'static str, r: &Resp) -> ApiError {
    let (code, message) = error_parts(&r.body);
    ApiError { provider, status: r.status, code, message, body: String::from_utf8_lossy(&r.body).chars().take(2000).collect() }
}

/// A JSON request; an HTTP error becomes an `ApiError`. An empty body is `null`.
pub fn json(provider: &'static str, method: &str, url: &str, headers: Vec<String>, body: Body, cancel: &AtomicBool) -> Result<Value> {
    let c = Call { provider, method, url: url.to_string(), headers, body, timeout: 300 };
    let r = send(&c, None, cancel, &|_| {})?;
    if !r.ok() {
        return Err(api_error(provider, &r).into());
    }
    Ok(r.json())
}

/// Downloads a result (a signed or public URL: no keys are sent, redirects are followed) to
/// `dest` through `dest.part`, reporting the bytes received.
pub fn download(provider: &'static str, url: &str, dest: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> Result<u64> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        bail!("{} returned no downloadable result", super::keys::name(provider));
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let curl = curl()?;
    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2 * attempt as u64));
        }
        let mut cmd = command(&curl);
        cmd.args(["-sS", "-L", "--connect-timeout", "20", "--max-time", "1800", "--retry", "2", "-o"]).arg(&part).args(["-w", "%{http_code}", "-K", "-"]);
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("starting curl")?;
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(format!("url = {}\n", conf_quote(url)).as_bytes());
        }
        let mut so = child.stdout.take().ok_or_else(|| anyhow!("curl has no stdout"))?;
        let mut se = child.stderr.take().ok_or_else(|| anyhow!("curl has no stderr"))?;
        let t_out = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = so.read_to_string(&mut s);
            s
        });
        let t_err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = se.read_to_string(&mut s);
            s
        });
        let st = loop {
            if let Some(st) = child.try_wait()? {
                break st;
            }
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&part);
                bail!("cancelled");
            }
            progress(super::file_len(&part));
            std::thread::sleep(Duration::from_millis(200));
        };
        let code = t_out.join().unwrap_or_default().trim().parse::<u16>().unwrap_or(0);
        let err = t_err.join().unwrap_or_default();
        if st.success() && (200..300).contains(&code) && super::file_len(&part) > 0 {
            std::fs::rename(&part, dest).with_context(|| format!("saving {}", dest.display()))?;
            return Ok(super::file_len(dest));
        }
        let _ = std::fs::remove_file(&part);
        last_err = if st.success() { format!("HTTP {code}") } else { err.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no answer").to_string() };
        // an expired or forbidden link won't get better by asking again
        if (400..500).contains(&code) {
            break;
        }
    }
    bail!("downloading the result from {} failed: {last_err}", super::keys::name(provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_errors_are_read_from_every_shape() {
        let el = br#"{"detail":{"type":"payment_required","code":"insufficient_credits","message":"Not enough credits.","status":"insufficient_credits"}}"#;
        assert_eq!(error_parts(el), ("insufficient_credits".into(), "Not enough credits.".into()));
        let legacy = br#"{"detail":{"status":"quota_exceeded","message":"This request exceeds your quota."}}"#;
        assert_eq!(error_parts(legacy).0, "quota_exceeded");
        let runway = br#"{"error":"promptText is too long"}"#;
        assert_eq!(error_parts(runway).1, "promptText is too long");
        let val = br#"{"detail":[{"loc":["body","duration"],"msg":"value is not a valid enumeration member","type":"type_error.enum"}]}"#;
        assert_eq!(error_parts(val), ("invalid_parameters".into(), "duration: value is not a valid enumeration member".into()));
        let openai = br#"{"error":{"message":"Bad key","code":"invalid_api_key"}}"#;
        assert_eq!(error_parts(openai), ("invalid_api_key".into(), "Bad key".into()));
        assert_eq!(error_parts(b"Service Unavailable").1, "Service Unavailable");
        assert_eq!(error_parts(b"<html>gateway</html>").1, "");
    }

    #[test]
    fn config_values_are_quoted() {
        assert_eq!(conf_quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
