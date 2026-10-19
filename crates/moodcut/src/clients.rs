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

/// Walks JSON text outside strings: `f(chars, i)` returns how many chars to
/// skip (0 = keep the char at `i`). Strings are copied untouched.
fn scan_json(b: &[char], mut f: impl FnMut(&[char], usize) -> usize) -> String {
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
            in_str = c != '"';
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        let skip = f(b, i);
        if skip == 0 {
            out.push(c);
            i += 1;
        } else {
            i += skip;
        }
    }
    out
}

/// Removes `//` and `/* */` comments outside strings.
fn strip_comments(s: &str) -> String {
    let b: Vec<char> = s.trim_start_matches('\u{feff}').chars().collect();
    scan_json(&b, |b, i| match (b[i], b.get(i + 1)) {
        ('/', Some('/')) => b[i..].iter().position(|c| *c == '\n').unwrap_or(b.len() - i),
        ('/', Some('*')) => b[i + 2..].windows(2).position(|w| w == ['*', '/']).map(|p| p + 4).unwrap_or(b.len() - i),
        _ => 0,
    })
}

/// Removes `//` and `/* */` comments, then trailing commas, outside strings, so
/// JSONC files (VS Code, hand-edited configs) can be read.
pub fn strip_jsonc(s: &str) -> String {
    let b: Vec<char> = strip_comments(s).chars().collect();
    scan_json(&b, |b, i| {
        if b[i] != ',' {
            return 0;
        }
        let next = b[i + 1..].iter().find(|c| !c.is_whitespace());
        usize::from(matches!(next, Some('}') | Some(']')))
    })
}

/// True when the file has comments (they can't be kept when MoodCut rewrites it).
fn has_comments(text: &str) -> bool {
    strip_comments(text).trim() != text.trim_start_matches('\u{feff}').trim()
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

/// Writes atomically next to the real file (following a symlink), with the
/// original's permissions, keeping a backup of the original beside it: made
/// once, or refreshed when `fresh_backup` (the rewrite loses something, e.g. comments).
fn write_file(path: &Path, text: &str, fresh_backup: bool) -> Result<PathBuf> {
    let path = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).with_context(|| format!("following the link {}", path.display()))?,
        _ => path.to_path_buf(),
    };
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let bak = path.with_extension(format!("{}.moodcut-backup", path.extension().and_then(|e| e.to_str()).unwrap_or("bak")));
    let original = std::fs::metadata(&path).ok();
    if original.is_some() && (fresh_backup || !bak.exists()) {
        std::fs::copy(&path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    }
    let tmp = path.with_extension("moodcut-tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    if let Some(m) = &original {
        // a private config (e.g. 0600 with tokens in it) stays private
        let _ = std::fs::set_permissions(&tmp, m.permissions());
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", path.display()));
    }
    Ok(bak)
}

fn json_registered(path: &Path, format: Format) -> Option<String> {
    let v = read_json(path).ok()?;
    v.get(servers_key(format))?.get(NAME)?.get("command")?.as_str().map(|s| s.to_string())
}

/// Adds, updates or removes MoodCut's entry. Returns (changed, note for the user).
fn json_set(path: &Path, format: Format, exe: Option<&str>) -> Result<(bool, Option<String>)> {
    let mut v = read_json(path).map_err(|e| {
        let snippet = exe.map(|x| format!("\nAdd this under \"{}\" yourself:\n  \"{NAME}\": {}", servers_key(format), entry(format, x))).unwrap_or_default();
        anyhow!("{e:#}; left it unchanged.{snippet}")
    })?;
    let obj = v.as_object_mut().expect("object");
    let key = servers_key(format);
    match exe {
        Some(x) => {
            let servers = obj.entry(key).or_insert_with(|| Value::Object(Map::new()));
            let Some(servers) = servers.as_object_mut() else { bail!("\"{key}\" in {} isn't an object; left it unchanged", path.display()) };
            let fresh = entry(format, x);
            match servers.get_mut(NAME).and_then(|e| e.as_object_mut()) {
                // update command/args in place: settings the user added (env, timeouts) stay
                Some(cur) => {
                    let mut changed = false;
                    for (k, val) in fresh.as_object().unwrap() {
                        if cur.get(k) != Some(val) {
                            cur.insert(k.clone(), val.clone());
                            changed = true;
                        }
                    }
                    if cur.remove("url").is_some() {
                        changed = true;
                    }
                    if !changed {
                        return Ok((false, None));
                    }
                }
                None => {
                    servers.insert(NAME.into(), fresh);
                }
            }
        }
        None => {
            let Some(servers) = obj.get_mut(key).and_then(|s| s.as_object_mut()) else { return Ok((false, None)) };
            if servers.remove(NAME).is_none() {
                return Ok((false, None));
            }
        }
    }
    let comments = std::fs::read_to_string(path).map(|t| has_comments(&t)).unwrap_or(false);
    let bak = write_file(path, &(serde_json::to_string_pretty(&v)? + "\n"), comments)?;
    Ok((true, comments.then(|| format!("comments in the file couldn't be kept; the original is saved as {}", bak.display()))))
}

/// TOML literal string (no escapes needed for Windows paths).
fn toml_str(s: &str) -> String {
    if s.contains('\'') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{s}'")
    }
}

/// The `[mcp_servers.moodcut]` table's line range in a Codex config, with its
/// sub-tables (`[mcp_servers.moodcut.env]`).
fn toml_table(text: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let is_head = |t: &str| t == format!("[mcp_servers.{NAME}]") || t == format!("[mcp_servers.\"{NAME}\"]");
    let is_sub = |t: &str| t.starts_with(&format!("[mcp_servers.{NAME}.")) || t.starts_with(&format!("[mcp_servers.\"{NAME}\"."));
    let start = lines.iter().position(|l| is_head(l.trim()))?;
    let end = lines.iter().enumerate().skip(start + 1).find(|(_, l)| {
        let t = l.trim_start();
        t.starts_with('[') && !is_sub(t.trim())
    }).map(|(i, _)| i).unwrap_or(lines.len());
    Some((start, end))
}

/// A TOML string value (basic "..." with escapes, or literal '...').
fn toml_unquote(v: &str) -> String {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        let mut out = String::new();
        let mut it = inner.chars();
        while let Some(c) = it.next() {
            if c == '\\' {
                match it.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(o) => out.push(o),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        v.trim_matches('\'').to_string()
    }
}

/// Reads config.toml; only a missing file counts as empty (anything unreadable is left alone).
fn read_toml_text(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("can't read {}; left it unchanged", path.display())),
    }
}

fn toml_registered(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let (a, b) = toml_table(&text)?;
    text.lines().skip(a + 1).take(b - a - 1).take_while(|l| !l.trim_start().starts_with('[')).find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "command").then(|| toml_unquote(v))
    })
}

fn toml_set(path: &Path, exe: Option<&str>) -> Result<(bool, Option<String>)> {
    let text = read_toml_text(path).map_err(|e| {
        let snippet = exe.map(|x| format!("\nAdd this to it yourself:\n[mcp_servers.{NAME}]\ncommand = {}\nargs = ['mcp']", toml_str(x))).unwrap_or_default();
        anyhow!("{e:#}{snippet}")
    })?;
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    match (toml_table(&text), exe) {
        (Some((a, b)), None) => {
            lines.drain(a..b);
        }
        (None, None) => return Ok((false, None)),
        (Some((a, b)), Some(x)) => {
            // rewrite command/args in the table's own keys; other keys and sub-tables stay
            let own_end = (a + 1..b).find(|i| lines[*i].trim_start().starts_with('[')).unwrap_or(b);
            let set = |key: &str, val: String, lines: &mut Vec<String>| {
                let line = format!("{key} = {val}");
                match (a + 1..own_end).find(|i| lines[*i].split_once('=').is_some_and(|(k, _)| k.trim() == key)) {
                    Some(i) => lines[i] = line,
                    None => lines.insert(a + 1, line),
                }
            };
            set("args", "['mcp']".into(), &mut lines);
            set("command", toml_str(x), &mut lines);
        }
        (None, Some(x)) => {
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
    }
    let new = lines.join("\n") + "\n";
    if new == text {
        return Ok((false, None));
    }
    write_file(path, &new, false)?;
    Ok((true, None))
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
    let r = apply_here(c, remove);
    if let (Ok(_), Some(dir)) = (&r, profiles_dir()) {
        if !remove {
            remember_profile(&dir);
        } else if CLIENTS.iter().all(|c| !status(c).registered) {
            forget_profile(&dir);
        }
    }
    r
}

fn apply_here(c: &'static Client, remove: bool) -> Result<String> {
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
    let mut notes = vec![];
    for p in &paths {
        let (did, note) = match c.format {
            Format::CodexToml => toml_set(p, exe.as_deref())?,
            f => json_set(p, f, exe.as_deref())?,
        };
        if did {
            changed.push(p.display().to_string());
        }
        notes.extend(note);
    }
    let mut msg = match (remove, changed.is_empty()) {
        (true, true) => format!("{}: wasn't registered", c.name),
        (true, false) => format!("{}: removed from {}", c.name, changed.join(", ")),
        (false, true) => format!("{}: already registered ({})", c.name, paths[0].display()),
        (false, false) => format!("{}: registered in {} ({})", c.name, changed.join(", "), c.after),
    };
    for n in notes {
        msg.push_str(&format!("; {n}"));
    }
    Ok(msg)
}

// ---- profiles connected on this PC
//
// On Windows an all-users install is removed by an elevated uninstaller, often
// running as another (admin) account than the person who connected their AI
// apps. Each profile MoodCut registers in is therefore noted in a shared folder
// (one small file per profile, so every user can add theirs), and
// `uninstall-ai --all-profiles` cleans all of them.

/// Environment variables that locate a profile's AI app settings (everything `config_paths` reads).
const PROFILE_VARS: &[&str] = if cfg!(windows) { &["USERPROFILE", "APPDATA", "LOCALAPPDATA"] } else { &["HOME", "XDG_CONFIG_HOME"] };

/// `%ProgramData%\MoodCut\ai-profiles` on Windows; elsewhere only when `MOODCUT_AI_PROFILES` names a folder.
pub fn profiles_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("MOODCUT_AI_PROFILES").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        env_dir("ProgramData").map(|d| d.join("MoodCut").join("ai-profiles"))
    } else {
        None
    }
}

/// This process's profile: the values of `PROFILE_VARS` that are set.
pub fn current_profile() -> Map<String, Value> {
    PROFILE_VARS.iter().filter_map(|v| std::env::var(v).ok().filter(|x| !x.is_empty()).map(|x| (v.to_string(), Value::String(x)))).collect()
}

/// One file per profile, named after its home folder.
fn profile_file(dir: &Path, profile: &Map<String, Value>) -> Option<PathBuf> {
    let home = profile.get(PROFILE_VARS[0])?.as_str()?;
    let name: String = home.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let hash = home.to_lowercase().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let tail: String = name.chars().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
    Some(dir.join(format!("{tail}-{:08x}.json", hash as u32)))
}

/// Notes this profile (best effort: a read-only shared folder just means no note).
fn remember_profile(dir: &Path) {
    let me = current_profile();
    let Some(f) = profile_file(dir, &me) else { return };
    if f.is_file() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&f, serde_json::to_string_pretty(&Value::Object(me)).unwrap_or_default());
}

fn forget_profile(dir: &Path) {
    if let Some(f) = profile_file(dir, &current_profile()) {
        let _ = std::fs::remove_file(f);
    }
}

/// The profiles noted in `dir`, with their files.
pub fn recorded_profiles(dir: &Path) -> Vec<(PathBuf, Map<String, Value>)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<(PathBuf, Map<String, Value>)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| match std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(Value::Object(m)) if m.get(PROFILE_VARS[0]).is_some_and(|h| h.is_string()) => Some((p, m)),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// A profile's home folder.
pub fn profile_home(p: &Map<String, Value>) -> Option<&str> {
    p.get(PROFILE_VARS[0]).and_then(|v| v.as_str())
}

/// Same home folder (case-insensitive on Windows).
pub fn same_profile(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    match (profile_home(a), profile_home(b)) {
        (Some(x), Some(y)) => crate::ops::exec::same_path(x, y),
        _ => false,
    }
}

/// Sets a command's environment to another profile's (variables it lacks are removed).
pub fn profile_env(cmd: &mut std::process::Command, profile: &Map<String, Value>) {
    for v in PROFILE_VARS {
        match profile.get(*v).and_then(|x| x.as_str()) {
            Some(x) => cmd.env(v, x),
            None => cmd.env_remove(v),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_noted_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("moodcut-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        remember_profile(&dir);
        remember_profile(&dir);
        let me = current_profile();
        let got = recorded_profiles(&dir);
        if me.contains_key(PROFILE_VARS[0]) {
            assert_eq!(got.len(), 1);
            assert!(same_profile(&got[0].1, &me));
            forget_profile(&dir);
            assert!(recorded_profiles(&dir).is_empty());
        } else {
            assert!(got.is_empty());
        }
        // names are safe file names and differ per home
        let a: Map<String, Value> = [(PROFILE_VARS[0].to_string(), json!("C:\\Users\\Ana Lía"))].into_iter().collect();
        let b: Map<String, Value> = [(PROFILE_VARS[0].to_string(), json!("C:\\Users\\bob"))].into_iter().collect();
        let (fa, fb) = (profile_file(&dir, &a).unwrap(), profile_file(&dir, &b).unwrap());
        assert_ne!(fa, fb);
        assert!(fa.file_name().unwrap().to_str().unwrap().chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        assert!(json_set(&f, Format::McpServers, Some("C:\\Tools\\moodcut.exe")).unwrap().0);
        let v = read_json(&f).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["moodcut"]["args"], json!(["mcp"]));
        assert_eq!(v["theme"], "dark");
        assert!(dir.join("cfg.json.moodcut-backup").is_file(), "original kept");
        assert!(!json_set(&f, Format::McpServers, Some("C:\\Tools\\moodcut.exe")).unwrap().0, "second run changes nothing");
        assert!(json_set(&f, Format::McpServers, None).unwrap().0);
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
        assert!(toml_set(&t, Some("C:\\Tools\\moodcut.exe")).unwrap().0);
        assert_eq!(toml_registered(&t).as_deref(), Some("C:\\Tools\\moodcut.exe"));
        assert!(!toml_set(&t, Some("C:\\Tools\\moodcut.exe")).unwrap().0);
        assert!(toml_set(&t, None).unwrap().0);
        let text = std::fs::read_to_string(&t).unwrap();
        assert!(text.contains("[mcp_servers.other]") && !text.contains(NAME), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_user_settings_permissions_and_links() {
        let dir = std::env::temp_dir().join(format!("moodcut-clients2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // env the user added to MoodCut's entry survives a re-register (e.g. an upgrade)
        let f = dir.join("c.json");
        std::fs::write(&f, r#"{"mcpServers": {"moodcut": {"command": "old.exe", "args": ["mcp"], "env": {"MOODCUT_FFMPEG": "C:\\ff"}}}}"#).unwrap();
        assert!(json_set(&f, Format::McpServers, Some("new.exe")).unwrap().0);
        let v = read_json(&f).unwrap();
        assert_eq!(v["mcpServers"]["moodcut"]["command"], "new.exe");
        assert_eq!(v["mcpServers"]["moodcut"]["env"]["MOODCUT_FFMPEG"], "C:\\ff");
        // a trailing comma before a comment, and comments are reported (the original is kept)
        let j = dir.join("mcp.json");
        std::fs::write(&j, "{\n \"servers\": {\"a\": {\"command\": \"x\"}, // \"b\": {}\n },\n}").unwrap();
        let (did, note) = json_set(&j, Format::VsCode, Some("m.exe")).unwrap();
        assert!(did && note.unwrap().contains("moodcut-backup"));
        assert!(std::fs::read_to_string(dir.join("mcp.json.moodcut-backup")).unwrap().contains("// \"b\""));
        // Codex: sub-tables go with the table, other keys stay, escaped paths read back
        let t = dir.join("config.toml");
        std::fs::write(&t, "[mcp_servers.moodcut]\ncommand = 'old'\nargs = ['mcp']\nstartup_timeout_sec = 60\n\n[mcp_servers.moodcut.env]\nA = '1'\n\n[other]\nx = 1\n").unwrap();
        assert!(toml_set(&t, Some("C:\\Users\\O'Brien\\moodcut.exe")).unwrap().0);
        let text = std::fs::read_to_string(&t).unwrap();
        assert!(text.contains("startup_timeout_sec = 60") && text.contains("[mcp_servers.moodcut.env]"), "{text}");
        assert_eq!(toml_registered(&t).as_deref(), Some("C:\\Users\\O'Brien\\moodcut.exe"));
        assert!(toml_set(&t, None).unwrap().0);
        let text = std::fs::read_to_string(&t).unwrap();
        assert!(!text.contains("moodcut") && text.contains("[other]"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // a private file stays private, and a symlinked config keeps its link
            let real = dir.join("real.json");
            std::fs::write(&real, "{}").unwrap();
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
            let link = dir.join("link.json");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(json_set(&link, Format::McpServers, Some("m")).unwrap().0);
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
            assert!(read_json(&real).unwrap()["mcpServers"]["moodcut"].is_object());
            // an unreadable config.toml is left alone
            let bad = dir.join("bad.toml");
            std::fs::write(&bad, [0xffu8, 0xfe, 0x00, 0x41]).unwrap();
            assert!(toml_set(&bad, Some("m")).is_err());
            assert_eq!(std::fs::read(&bad).unwrap(), vec![0xffu8, 0xfe, 0x00, 0x41]);
        }
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
