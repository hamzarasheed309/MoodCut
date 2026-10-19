//! Registers MoodCut with the AI apps on this computer (Claude Desktop,
//! Claude Code, Cursor, VS Code, Windsurf, LM Studio, Gemini CLI, Codex).
//!
//! Each app keeps its MCP servers in a config file. Editing them safely is the
//! whole job: the existing file is merged (other servers are never touched),
//! JSON with comments is understood, a backup is made before the first change,
//! and a file that can't be read is left alone with the snippet to paste.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The server name MoodCut is registered under.
pub const NAME: &str = "moodcut";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Format {
    /// `{"mcpServers": {"moodcut": {"command", "args"}}}` (Claude Desktop, Cursor, Windsurf, LM Studio, Gemini CLI, Claude Code)
    McpServers,
    /// VS Code: `{"servers": {"moodcut": {"type": "stdio", "command", "args"}}}`
    VsCode,
    /// Codex CLI: `[mcp_servers.moodcut]` in config.toml
    CodexToml,
}

pub struct Client {
    pub id: &'static str,
    pub name: &'static str,
    /// What to do after registering.
    pub after: &'static str,
    format: Format,
}

pub const CLIENTS: &[Client] = &[
    Client { id: "claude", name: "Claude Desktop", after: "quit and reopen Claude Desktop", format: Format::McpServers },
    Client { id: "claude-code", name: "Claude Code", after: "start a new Claude Code session", format: Format::McpServers },
    Client { id: "cursor", name: "Cursor", after: "MoodCut appears under Cursor Settings > MCP", format: Format::McpServers },
    Client { id: "vscode", name: "VS Code (Copilot)", after: "run 'MCP: List Servers' in VS Code and start moodcut", format: Format::VsCode },
    Client { id: "windsurf", name: "Windsurf", after: "refresh the MCP servers in Windsurf's Cascade panel", format: Format::McpServers },
    Client { id: "lmstudio", name: "LM Studio", after: "MoodCut appears in LM Studio's Program tab", format: Format::McpServers },
    Client { id: "gemini", name: "Gemini CLI", after: "start a new gemini session", format: Format::McpServers },
    Client { id: "codex", name: "Codex CLI", after: "start a new codex session", format: Format::CodexToml },
];

/// Accepts ids and a few common spellings ("claude-desktop", "vs code", "copilot").
pub fn find(id: &str) -> Option<&'static Client> {
    let k: String = id.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let id = match k.as_str() {
        "claude" | "claudedesktop" | "desktop" => "claude",
        "claudecode" | "cc" => "claude-code",
        "vscode" | "code" | "copilot" | "githubcopilot" => "vscode",
        "lmstudio" | "lms" => "lmstudio",
        "gemini" | "geminicli" => "gemini",
        "codex" | "codexcli" | "openaicodex" => "codex",
        other => other,
    };
    CLIENTS.iter().find(|c| c.id == id)
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

/// Per-user application settings folder (`%APPDATA%`, `~/Library/Application Support`, `~/.config`).
fn app_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        env_dir("APPDATA")
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library/Application Support"))
    } else {
        env_dir("XDG_CONFIG_HOME").or_else(|| home().map(|h| h.join(".config")))
    }
}

/// Claude Desktop from the Microsoft Store / MSIX keeps its config in a
/// virtualized AppData folder: `%LOCALAPPDATA%\Packages\Claude_<id>\LocalCache\Roaming\Claude`.
fn claude_msix_dirs() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return vec![];
    }
    let Some(pk) = env_dir("LOCALAPPDATA").map(|l| l.join("Packages")) else { return vec![] };
    let Ok(rd) = std::fs::read_dir(&pk) else { return vec![] };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with("claude_"))
        .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude"))
        .collect()
}

/// Config files of a client (several for Claude Desktop when both the classic
/// and the Store build are present). Empty when the client can't be located.
fn config_paths(c: &Client) -> Vec<PathBuf> {
    let h = home();
    match c.id {
        "claude" => {
            let mut v: Vec<PathBuf> = app_config_dir().map(|d| d.join("Claude").join("claude_desktop_config.json")).into_iter().collect();
            v.extend(claude_msix_dirs().into_iter().map(|d| d.join("claude_desktop_config.json")));
            v
        }
        "claude-code" => h.map(|h| h.join(".claude.json")).into_iter().collect(),
        "cursor" => h.map(|h| h.join(".cursor").join("mcp.json")).into_iter().collect(),
        "vscode" => app_config_dir().map(|d| d.join("Code").join("User").join("mcp.json")).into_iter().collect(),
        "windsurf" => h.map(|h| h.join(".codeium").join("windsurf").join("mcp_config.json")).into_iter().collect(),
        "lmstudio" => h.map(|h| h.join(".lmstudio").join("mcp.json")).into_iter().collect(),
        "gemini" => h.map(|h| h.join(".gemini").join("settings.json")).into_iter().collect(),
        "codex" => h.map(|h| h.join(".codex").join("config.toml")).into_iter().collect(),
        _ => vec![],
    }
}

/// Signs that the app is installed for this user (its settings folder exists,
/// or its command is on PATH).
fn installed(c: &Client) -> bool {
    let has_dir = |p: &Path| p.parent().is_some_and(|d| d.is_dir());
    match c.id {
        "claude" => {
            config_paths(c).iter().any(|p| has_dir(p))
                || env_dir("LOCALAPPDATA").is_some_and(|l| l.join("AnthropicClaude").is_dir())
                || (cfg!(target_os = "macos") && Path::new("/Applications/Claude.app").is_dir())
        }
        "claude-code" => crate::setup::which("claude").is_some() || home().is_some_and(|h| h.join(".claude").is_dir()),
        "vscode" => config_paths(c).iter().any(|p| has_dir(p)) || crate::setup::which("code").is_some(),
        "gemini" => home().is_some_and(|h| h.join(".gemini").is_dir()) || crate::setup::which("gemini").is_some(),
        "codex" => home().is_some_and(|h| h.join(".codex").is_dir()) || crate::setup::which("codex").is_some(),
        _ => config_paths(c).iter().any(|p| has_dir(p)),
    }
}

/// How AI apps should start MoodCut: this executable with `mcp`.
pub fn server_command() -> Result<String> {
    let exe = std::env::current_exe().context("can't find the MoodCut executable")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    Ok(crate::render::ffmpeg::strip_unc(&exe))
}

fn entry(format: Format, exe: &str) -> Value {
    match format {
        Format::VsCode => json!({"type": "stdio", "command": exe, "args": ["mcp"]}),
        _ => json!({"command": exe, "args": ["mcp"]}),
    }
}

fn servers_key(format: Format) -> &'static str {
    if format == Format::VsCode {
        "servers"
    } else {
        "mcpServers"
    }
}

/// Removes `//` and `/* */` comments and trailing commas outside strings, so
/// JSONC files (VS Code, hand-edited configs) can be read.
pub fn strip_jsonc(s: &str) -> String {
    let b: Vec<char> = s.trim_start_matches('\u{feff}').chars().collect();
    let mut out = String::with_capacity(b.len());
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                // drop it when only whitespace/comments lead to } or ]
                let mut j = i + 1;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if !matches!(b.get(j), Some('}') | Some(']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Reads a JSON(C) config. Missing or empty file = `{}`. Unreadable = error (never overwritten).
fn read_json(path: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).or_else(|_| serde_json::from_str(&strip_jsonc(&text))).map_err(|e| anyhow!("{} isn't valid JSON ({e})", path.display()))?;
    if !v.is_object() {
        bail!("{} doesn't hold a JSON object", path.display());
    }
    Ok(v)
}

/// Writes atomically, keeping a one-time backup of the original next to it.
fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    if path.is_file() {
        let bak = path.with_extension(format!("{}.moodcut-backup", path.extension().and_then(|e| e.to_str()).unwrap_or("bak")));
        if !bak.exists() {
            let _ = std::fs::copy(path, &bak);
        }
    }
    let tmp = path.with_extension("moodcut-tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).or_else(|_| std::fs::write(path, text)).with_context(|| format!("writing {}", path.display()))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

fn json_registered(path: &Path, format: Format) -> Option<String> {
    let v = read_json(path).ok()?;
    v.get(servers_key(format))?.get(NAME)?.get("command")?.as_str().map(|s| s.to_string())
}

fn json_set(path: &Path, format: Format, exe: Option<&str>) -> Result<bool> {
    let mut v = read_json(path).map_err(|e| {
        let snippet = exe.map(|x| format!("\nAdd this under \"{}\" yourself:\n  \"{NAME}\": {}", servers_key(format), entry(format, x))).unwrap_or_default();
        anyhow!("{e:#}; left it unchanged.{snippet}")
    })?;
    let obj = v.as_object_mut().expect("object");
    let key = servers_key(format);
    match exe {
        Some(x) => {
            let servers = obj.entry(key).or_insert_with(|| Value::Object(Map::new()));
            if !servers.is_object() {
                bail!("\"{key}\" in {} isn't an object; left it unchanged", path.display());
            }
            let new = entry(format, x);
            if servers.get(NAME) == Some(&new) {
                return Ok(false);
            }
            servers.as_object_mut().unwrap().insert(NAME.into(), new);
        }
        None => {
            let Some(servers) = obj.get_mut(key).and_then(|s| s.as_object_mut()) else { return Ok(false) };
            if servers.remove(NAME).is_none() {
                return Ok(false);
            }
        }
    }
    write_file(path, &(serde_json::to_string_pretty(&v)? + "\n"))?;
    Ok(true)
}

/// TOML literal string (no escapes needed for Windows paths).
fn toml_str(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{s}'")
    }
}

/// The `[mcp_servers.moodcut]` table's line range in a Codex config.
fn toml_table(text: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| {
        let t = l.trim();
        t == format!("[mcp_servers.{NAME}]") || t == format!("[mcp_servers.\"{NAME}\"]")
    })?;
    let end = lines.iter().enumerate().skip(start + 1).find(|(_, l)| l.trim_start().starts_with('[')).map(|(i, _)| i).unwrap_or(lines.len());
    Some((start, end))
}

fn toml_registered(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let (a, b) = toml_table(&text)?;
    text.lines().skip(a + 1).take(b - a - 1).find_map(|l| l.trim().strip_prefix("command").map(|r| r.trim_start_matches([' ', '=']).trim().trim_matches(['\'', '"']).to_string()))
}

fn toml_set(path: &Path, exe: Option<&str>) -> Result<bool> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    if let Some((a, b)) = toml_table(&text) {
        // drop trailing blank lines of the old table too
        lines.drain(a..b);
    } else if exe.is_none() {
        return Ok(false);
    }
    if let Some(x) = exe {
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("[mcp_servers.{NAME}]"));
        lines.push(format!("command = {}", toml_str(x)));
        lines.push("args = ['mcp']".into());
    }
    let new = lines.join("\n") + "\n";
    if new == text {
        return Ok(false);
    }
    write_file(path, &new)?;
    Ok(true)
}

/// Status of one client on this computer.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Status {
    pub id: &'static str,
    pub name: &'static str,
    pub installed: bool,
    /// MoodCut is registered (pointing at some executable).
    pub registered: bool,
    /// Registered, but pointing at another (moved or old) executable.
    pub stale: bool,
    pub config: Option<String>,
    pub after: &'static str,
}

pub fn status(c: &'static Client) -> Status {
    let paths = config_paths(c);
    let me = server_command().ok();
    let found: Vec<String> = paths
        .iter()
        .filter_map(|p| match c.format {
            Format::CodexToml => toml_registered(p),
            f => json_registered(p, f),
        })
        .collect();
    let stale = !found.is_empty() && me.as_ref().is_some_and(|m| found.iter().any(|f| !crate::ops::exec::same_path(f, m)));
    Status {
        id: c.id,
        name: c.name,
        installed: installed(c),
        registered: !found.is_empty(),
        stale,
        config: paths.first().map(|p| p.display().to_string()),
        after: c.after,
    }
}

pub fn statuses() -> Vec<Status> {
    CLIENTS.iter().map(status).collect()
}

/// Claude Code through its own CLI when available (it owns ~/.claude.json).
fn claude_code_cli(exe: Option<&str>) -> Option<Result<bool>> {
    let claude = crate::setup::which("claude")?;
    let run = |args: &[&str]| crate::render::ffmpeg::command(&claude).args(args).stdin(std::process::Stdio::null()).output();
    let _ = run(&["mcp", "remove", "--scope", "user", NAME]);
    let Some(x) = exe else { return Some(Ok(true)) };
    Some(match run(&["mcp", "add", "--scope", "user", NAME, "--", x, "mcp"]) {
        Ok(o) if o.status.success() => Ok(true),
        Ok(o) => Err(anyhow!("claude mcp add failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(anyhow!("couldn't run claude: {e}")),
    })
}

/// Registers MoodCut with a client (or removes it when `remove`). Returns a one-line report.
pub fn apply(c: &'static Client, remove: bool) -> Result<String> {
    let exe = if remove { None } else { Some(server_command()?) };
    if c.id == "claude-code" {
        if let Some(r) = claude_code_cli(exe.as_deref()) {
            if r.is_ok() || !home().is_some_and(|h| h.join(".claude.json").is_file()) {
                r?;
                return Ok(if remove { format!("{}: removed", c.name) } else { format!("{}: registered with `claude mcp add` ({})", c.name, c.after) });
            }
        }
    }
    let mut paths = config_paths(c);
    if paths.is_empty() {
        bail!("can't find where {} keeps its settings", c.name);
    }
    if c.id == "claude" {
        // the Store build reads only its own copy; write every location that exists (or the classic one)
        let existing: Vec<PathBuf> = paths.iter().filter(|p| p.parent().is_some_and(|d| d.is_dir())).cloned().collect();
        paths = if existing.is_empty() { paths.into_iter().take(1).collect() } else { existing };
    } else {
        paths.truncate(1);
    }
    let mut changed = vec![];
    for p in &paths {
        let did = match c.format {
            Format::CodexToml => toml_set(p, exe.as_deref())?,
            f => json_set(p, f, exe.as_deref())?,
        };
        if did {
            changed.push(p.display().to_string());
        }
    }
    Ok(match (remove, changed.is_empty()) {
        (true, true) => format!("{}: wasn't registered", c.name),
        (true, false) => format!("{}: removed from {}", c.name, changed.join(", ")),
        (false, true) => format!("{}: already registered ({})", c.name, paths[0].display()),
        (false, false) => format!("{}: registered in {} ({})", c.name, changed.join(", "), c.after),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let s = "{\n // servers\n \"servers\": {\"a\": {\"url\": \"http://x/*y*/\"},},\n /* more */ \"k\": [1,2,],\n}";
        let v: Value = serde_json::from_str(&strip_jsonc(s)).unwrap();
        assert_eq!(v["servers"]["a"]["url"], "http://x/*y*/");
        assert_eq!(v["k"], json!([1, 2]));
    }

    #[test]
    fn merges_without_touching_other_servers_and_refuses_broken_files() {
        let dir = std::env::temp_dir().join(format!("moodcut-clients-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("cfg.json");
        std::fs::write(&f, "{\"mcpServers\": {\"other\": {\"command\": \"x\"}}, \"theme\": \"dark\",}").unwrap();
        assert!(json_set(&f, Format::McpServers, Some("C:\\Tools\\moodcut.exe")).unwrap());
        let v = read_json(&f).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["moodcut"]["args"], json!(["mcp"]));
        assert_eq!(v["theme"], "dark");
        assert!(dir.join("cfg.json.moodcut-backup").is_file(), "original kept");
        assert!(!json_set(&f, Format::McpServers, Some("C:\\Tools\\moodcut.exe")).unwrap(), "second run changes nothing");
        assert!(json_set(&f, Format::McpServers, None).unwrap());
        assert!(read_json(&f).unwrap()["mcpServers"].get("moodcut").is_none());
        // a file that isn't JSON is never overwritten
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ this is not json").unwrap();
        let e = json_set(&bad, Format::VsCode, Some("/usr/bin/moodcut")).unwrap_err().to_string();
        assert!(e.contains("left it unchanged") && e.contains("\"type\":\"stdio\""), "{e}");
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{ this is not json");
        // codex toml
        let t = dir.join("config.toml");
        std::fs::write(&t, "model = 'o4'\n\n[mcp_servers.other]\ncommand = 'x'\n").unwrap();
        assert!(toml_set(&t, Some("C:\\Tools\\moodcut.exe")).unwrap());
        assert_eq!(toml_registered(&t).as_deref(), Some("C:\\Tools\\moodcut.exe"));
        assert!(!toml_set(&t, Some("C:\\Tools\\moodcut.exe")).unwrap());
        assert!(toml_set(&t, None).unwrap());
        let text = std::fs::read_to_string(&t).unwrap();
        assert!(text.contains("[mcp_servers.other]") && !text.contains(NAME), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_clients_by_common_names() {
        assert_eq!(find("Claude Desktop").unwrap().id, "claude");
        assert_eq!(find("claude_code").unwrap().id, "claude-code");
        assert_eq!(find("VS Code").unwrap().id, "vscode");
        assert!(find("notepad").is_none());
    }
}
