//! Model Context Protocol server (JSON-RPC 2.0), transport-agnostic.
//!
//! Token-efficiency design:
//! - 8 small tools; the big reference lives behind `help(topic)`.
//! - `edit` takes a whole MoodCut Script (dozens of edits per call, atomic).
//! - Results are compact diffs/outlines, never full project JSON (unless asked).
//! - `preview` returns one contact-sheet image instead of N frames.

pub mod craft;
pub mod docs;
pub mod stdio;

use crate::engine::Engine;
use crate::ops::outline;
use crate::render::encode::{self, RenderOpts};
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const INSTRUCTIONS: &str = "MoodCut is a local video editor + motion graphics engine. Workflow: media(import) -> view -> edit (send MANY script lines per call; atomic) -> preview (small contact sheet to check) -> render. \
Ids: m# media, c# clips, t# text, g# shapes, s# comps; tracks V1 (bottom) V2.. and A1... Times in seconds. \
Custom motion graphics: write HTML/CSS/JS with the graphic tool (rendered to video with alpha, imported as media). \
For a complete edit from one request follow help('workflow'); run check (with expect) before every render and fix what it reports. \
Work like a senior editor and motion designer, never generic: read help('style') and the closest help('recipes') before building; set a brand kit (brand <preset> or the client's font/colors), one look, 2-3 reusable motion moves, and sound design (sfx whoosh/hit/riser); check target=reels|tiktok|shorts|youtube flags hooks, pacing, platform safe zones and craft issues. \
Premiere/After Effects tools and panels (rolling edits, lift/extract, paste attributes, align, keyframe assistant, cameras, Lumetri, scopes, Essential Sound) are script commands: help('tools'). \
The edit tool description has a cheat sheet; call help(topic) for full references (edit, tools, effects, transitions, anim, 3d, text, templates, macros, graphics, color, scopes, render).";

pub struct Mcp {
    pub eng: Result<Arc<Engine>, String>,
}

fn text_result(s: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": s.into()}]})
}

fn err_result(s: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": s.into()}], "isError": true})
}

pub fn tools() -> Value {
    json!([
        {
            "name": "project",
            "description": "Create/open/save a MoodCut project, undo/redo. action: new | open | save | info | undo | redo | history. new takes name, size (1920x1080, 9:16, 4k...), fps, optional path (.mcut).",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["new", "open", "save", "info", "undo", "redo", "history"]},
                "path": {"type": "string"}, "name": {"type": "string"}, "size": {"type": "string"}, "fps": {"type": "number"}, "n": {"type": "integer"}
            }, "required": ["action"]},
            "annotations": {"title": "Project", "destructiveHint": false, "openWorldHint": false}
        },
        {
            "name": "media",
            "description": "The media library (scales to thousands of files). action:\n\
import: paths = files, folders (recursive by default), globs (D:/shoot/**/*.mp4) or http(s) URLs; tag='day1' tags them. Unreadable files are skipped and reported.\n\
list: compact lines, filters kind (video|audio|image|graphic|still) q (words in name/path/note/tags) tag min_dur max_dur orientation (landscape|portrait|square) used (bool), sort (name|dur|date|size|path), offset/limit paging, detail=2 adds paths.\n\
find: browse a disk folder without importing (path, pattern, recursive, files=true lists names).\n\
proxy: preview proxies (small fast copies of 4K/HEVC footage, built in the background; exports always use the originals): ids=m1..m9|all to build, op=delete|cancel, mode=auto|all|off for new imports, enabled=false to preview the originals. No ids = status.\n\
Organise with edit lines: tag m1..m40 broll | note m3 'drone over beach at sunset' | untag | relink m3 <path> | unimport m5. See footage with preview media=...",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["import", "list", "find", "proxy"]},
                "ids": {"type": "string"}, "op": {"type": "string"}, "mode": {"type": "string"}, "enabled": {"type": "boolean"},
                "paths": {"type": "array", "items": {"type": "string"}}, "recursive": {"type": "boolean"}, "tag": {"type": "string"},
                "kind": {"type": "string"}, "q": {"type": "string"}, "min_dur": {"type": "number"}, "max_dur": {"type": "number"},
                "orientation": {"type": "string"}, "used": {"type": "boolean"}, "sort": {"type": "string"},
                "offset": {"type": "integer"}, "limit": {"type": "integer"}, "detail": {"type": "integer"},
                "path": {"type": "string"}, "pattern": {"type": "string"}, "files": {"type": "boolean"}
            }, "required": ["action"]},
            "annotations": {"title": "Media library", "destructiveHint": false, "openWorldHint": true}
        },
        {
            "name": "edit",
            "description": "Apply a MoodCut Script: one command per line, all-or-nothing. Returns a diff (+created ~changed -removed !warning).\n\
Place: add <m|path>[in-out] [V2] [at=T] [dur=] [props] | append m1[0-5] m2 | text 'Hi' at=0 dur=3 size=120 enter=fadeUp exit=fadeDown | shape rect w=400 h=80 fill=#f05 | solid #000 | adjust\n\
Edit: cut <id|V1> at=T | trim <id> start=+1 end=-0.5 | del <ids> [ripple] | move <id> at=T [track=V2] | speed <id> 2x | freeze <id> at=T | dup <id>\n\
Props: set <id> pos=center scale=1.2 rot=10 opacity=50% blend=screen vol=-6dB | kf <id> scale 0:1 2:1.2@outBack | expr <id> pos 'wiggle(2,20)'\n\
Look: fx <id> blur radius=8 | fx c1.f1 radius=4|off|del | color <id> cinematic (a new look replaces the old) | color <id> exposure=.3 temp=-10 | tr <a> <b> dissolve 0.5 | tr V1 dissolve | fade <id> in=0.5 out=1 | mask <id> ellipse feather=40 | anim <id> popIn,fadeOut\n\
Sound: afx <clip|A1|master> eq bass=3 | afx A1.a1 off|del|ratio=4 | sound <clip> dialogue | sfx whoosh at=2 | duck A2 by=A1 | master lufs=-14 limiter=1 | kf c1 vol 0:-60 1:0\n\
Pro: title lowerthird text='Jane' sub='CEO' | captions <clip> | autocut silence <clip> | montage m1,m2 music=m3 beats=2 | reframe 9:16 | duck A1 by=A2 | parent <a> <b> | precomp <ids>\n\
Tools (help tools): roll <id> +0.3 | lift|extract 2 5 | swap a b | paste <from> <to> fx | stagger <ids> by=0.1 | align <ids> center | ease <id> all easy | kf <id> all reverse | null | camera move=pushin layers=V2 | sound <clip> dialogue | workarea 2 8\n\
Finish (help style, sfx, brand): brand bold | sfx whoosh on=transitions | sfx hit at=0.1 | speedramp V1 at=3 to=4x | punchin V1 | jcut c4 by=0.4 | captions V1 style=punch | anim t1 slamIn|wipeInLeft|whipInRight\n\
Selectors: sel (editor selection) after:T at:T kind:text V2:after:5; time playhead. Scopes: preview scope=waveform|vectorscope|all.\n\
Refs: $ = last created item, $2 = 2nd created in this script. Times: 12.5, 1:02, 90f, end, c3.end+0.5, @marker. undo / redo alone. help('edit') lists everything.",
            "inputSchema": {"type": "object", "properties": {"script": {"type": "string", "description": "MoodCut Script lines"}}, "required": ["script"]},
            "annotations": {"title": "Edit timeline", "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        },
        {
            "name": "view",
            "description": "Read state as compact text. what: timeline (default; detail 0 summary, 1 items, 2 all props) | item (id) | media | comps | markers | transcript (id) | search (q: words in media names/notes/tags and every analyzed transcript, with times) | history | log | json (id or comp).",
            "inputSchema": {"type": "object", "properties": {
                "what": {"type": "string", "enum": ["timeline", "item", "media", "comps", "markers", "transcript", "search", "history", "log", "json"]},
                "id": {"type": "string"}, "detail": {"type": "integer", "minimum": 0, "maximum": 2}, "comp": {"type": "string"},
                "q": {"type": "string"}, "limit": {"type": "integer"}
            }},
            "annotations": {"title": "View", "readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "preview",
            "description": "Look at the edit or the footage. Timeline: t=seconds for one frame, or n=frames for a labeled contact sheet across range (default whole comp). Footage: media=m1..m48 | all | tag:broll | kind:video gives poster frames labeled id+length (48 per sheet, offset pages); media=m5 n=8 [range=a-b] gives a filmstrip of that file with source times (pick in/out points). Scopes: scope=waveform|parade|vectorscope|histogram|all at t gives the Lumetri scope plus measured levels, clipping and color cast (grade by numbers). Small widths save tokens.",
            "inputSchema": {"type": "object", "properties": {
                "t": {"type": "number"}, "times": {"type": "array", "items": {"type": "number"}}, "n": {"type": "integer", "minimum": 1, "maximum": 24},
                "range": {"type": "string", "description": "a-b seconds"}, "width": {"type": "integer"}, "comp": {"type": "string"},
                "media": {"type": "string"}, "offset": {"type": "integer"},
                "scope": {"type": "string", "enum": ["waveform", "parade", "vectorscope", "histogram", "all"]}
            }},
            "annotations": {"title": "Preview frames", "readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "analyze",
            "description": "Analyze media or a clip's media (cached): silence (thresh,min) | scenes (thresh) | loudness | beats | transcript (lang, model, src=.srt) | black | stats (exposure hints). target can be many: m1..m40, all, tag:interviews, kind:video (runs in parallel, one line each).",
            "inputSchema": {"type": "object", "properties": {
                "target": {"type": "string"}, "kind": {"type": "string", "enum": ["silence", "scenes", "loudness", "beats", "transcript", "black", "stats"]},
                "params": {"type": "object", "additionalProperties": {"type": ["string", "number"]}}
            }, "required": ["target", "kind"]},
            "annotations": {"title": "Analyze", "readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "render",
            "description": "Export. action: start (default) | status | list | cancel. preset: mp4 youtube social draft hevc av1 prores prores4444 webm webm-alpha mov-alpha gif png mp3 wav; deliverables without encoding: srt vtt (captions), chapters (YouTube, from markers), xml (Premiere Pro / Resolve timeline), edl. Options: out, range a-b or work (the work area), scale, hw=auto (GPU), wait=true to block until done.",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["start", "status", "list", "cancel"]},
                "preset": {"type": "string"}, "out": {"type": "string"}, "range": {"type": "string"}, "scale": {"type": "number"},
                "hw": {"type": "string"}, "comp": {"type": "string"}, "id": {"type": "string"}, "wait": {"type": "boolean"}
            }},
            "annotations": {"title": "Render", "destructiveHint": false, "openWorldHint": false}
        },
        {
            "name": "graphic",
            "description": "Design motion graphics as HTML/CSS/JS (like a web page or artifact) and import them as media. CSS animations/transitions, Web Animations, GSAP/anime.js/Lottie, canvas and SVG render frame-accurately in a headless browser, transparent by default. Returns the media id, layout checks, JS errors and a contact sheet.\n\
html: full page or body fragment (Inter built in; {{key}} = vars; library media as src='mc://m3'; window.MC = {duration,fps,width,height,vars}). Optional: name, dur (s; 0 = still, captured at `at` s; default 5), size (default comp), fps, alpha (default true), vars, place ('V3 at=2' also adds a clip), id (re-render that graphic in place, e.g. fixed html), from (new variant, e.g. other vars), preview (frames in sheet, default 6). help('graphics') for patterns.",
            "inputSchema": {"type": "object", "properties": {
                "html": {"type": "string"}, "name": {"type": "string"}, "dur": {"type": "number"}, "size": {"type": "string"}, "fps": {"type": "number"},
                "alpha": {"type": "boolean"}, "vars": {"type": "object", "additionalProperties": {"type": ["string", "number"]}},
                "place": {"type": "string"}, "id": {"type": "string"}, "from": {"type": "string"}, "preview": {"type": "integer", "minimum": 0, "maximum": 12},
                "at": {"type": "number", "description": "stills: animation time to capture (s)"}
            }},
            "annotations": {"title": "HTML motion graphic", "destructiveHint": false, "openWorldHint": true}
        },
        {
            "name": "check",
            "description": "Quality-check the edit before rendering (and after big changes). Finds black gaps, 1-2 frame gaps, flash frames, layers hidden under opaque clips, offline media, upscaled/soft footage, fps judder, transitions without handles, text/graphics outside title-safe, off-screen or overlapping, small or low-contrast text, text too short to read, fast captions, audio past the picture, music under dialogue without ducking, and the real mix loudness/true peak vs the delivery target. Each problem has an id/time and usually a fix line; fix=true applies them as one undoable edit. Craft notes (C#: weak hook, slow pacing, static shots, linear easing, too many transition types, silent transitions/titles, inconsistent type, missing brand or captions) come with `try:` commands that are never auto-applied: act on them as an editor would. expect verifies the request: {dur, dur_tol, size:'1080x1920', aspect, fps, lufs, captions, music, max_shot, min_shot}. target: web (default, -14 LUFS) | reels | tiktok | shorts (vertical UI zones, hook, pacing) | youtube | social | podcast (-16) | broadcast (-23). Returns frames of visual problems with title-safe guides (and the app UI zones for short-form).",
            "inputSchema": {"type": "object", "properties": {
                "comp": {"type": "string"}, "target": {"type": "string"}, "fix": {"type": "boolean"}, "audio": {"type": "boolean"}, "frames": {"type": "boolean"},
                "safe": {"type": "number", "description": "title-safe margin fraction, default 0.05"},
                "expect": {"type": "object", "properties": {
                    "dur": {"type": "number"}, "dur_tol": {"type": "number"}, "size": {"type": "string"}, "aspect": {"type": "string"}, "fps": {"type": "number"},
                    "lufs": {"type": "number"}, "captions": {"type": "boolean"}, "music": {"type": "boolean"}, "max_shot": {"type": "number"}, "min_shot": {"type": "number"}
                }}
            }},
            "annotations": {"title": "Check edit quality", "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false}
        },
        {
            "name": "help",
            "description": "MoodCut reference on demand. topic: workflow style recipes platforms sfx brand edit tools time props effects audio transitions anim 3d text shapes templates macros color scopes expr render analyze graphics examples fonts.",
            "inputSchema": {"type": "object", "properties": {"topic": {"type": "string"}}},
            "annotations": {"title": "Help", "readOnlyHint": true, "openWorldHint": false}
        }
    ])
}

fn s(v: &Value, k: &str) -> Option<String> {
    match &v[k] {
        Value::String(x) => Some(x.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl Mcp {
    pub fn new(eng: Result<Arc<Engine>, String>) -> Mcp {
        Mcp { eng }
    }

    /// Handles one JSON-RPC message. Returns None for notifications.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        if let Value::Array(batch) = msg {
            let out: Vec<Value> = batch.iter().filter_map(|m| self.handle(m)).collect();
            return if out.is_empty() { None } else { Some(Value::Array(out)) };
        }
        let id = msg.get("id").cloned();
        let method = msg["method"].as_str().unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let id = id?;
        if method.is_empty() {
            return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32600, "message": "invalid request"}}));
        }
        let result = match method {
            "initialize" => {
                let want = params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSIONS[0]);
                let ver = if PROTOCOL_VERSIONS.contains(&want) { want } else { PROTOCOL_VERSIONS[0] };
                Ok(json!({
                    "protocolVersion": ver,
                    "capabilities": {"tools": {"listChanged": false}, "prompts": {"listChanged": false}},
                    "serverInfo": {"name": "moodcut", "title": "MoodCut Video Editor", "version": crate::VERSION},
                    "instructions": INSTRUCTIONS
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                // a bug in one tool must never take the connection down with it
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.call(name, &args))) {
                    Ok(r) => Ok(r),
                    Err(e) => {
                        let why = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into());
                        Ok(err_result(format!("MoodCut hit an internal error in {name} ({why}); nothing was changed. Please report it; you can carry on.")))
                    }
                }
            }
            "resources/list" => Ok(json!({"resources": []})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "prompts/list" => Ok(json!({"prompts": prompts().iter().map(|p| json!({"name": p.name, "title": p.title, "description": p.desc, "arguments": p.args.iter().map(|(n, d, r)| json!({"name": n, "description": d, "required": r})).collect::<Vec<_>>()})).collect::<Vec<_>>()})),
            "prompts/get" => {
                let name = params["name"].as_str().unwrap_or("");
                match prompts().into_iter().find(|p| p.name == name) {
                    Some(p) => {
                        let args = params["arguments"].as_object().cloned().unwrap_or_default();
                        let get = |k: &str| args.get(k).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                        let missing: Vec<&str> = p.args.iter().filter(|(n, _, r)| *r && get(n).is_none()).map(|(n, ..)| *n).collect();
                        if missing.is_empty() {
                            Ok(json!({"description": p.desc, "messages": [{"role": "user", "content": {"type": "text", "text": (p.build)(&get)}}]}))
                        } else {
                            Err((-32602, format!("missing argument(s): {}", missing.join(", "))))
                        }
                    }
                    None => Err((-32602, format!("unknown prompt '{name}'"))),
                }
            }
            "logging/setLevel" => Ok(json!({})),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err((code, m)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}}),
        })
    }

    pub fn call(&self, name: &str, a: &Value) -> Value {
        if name == "help" {
            return text_result(docs::help(&s(a, "topic").unwrap_or_default()));
        }
        let eng = match &self.eng {
            Ok(e) => e.clone(),
            Err(e) => return err_result(format!("MoodCut can't start: {e}")),
        };
        let r = match name {
            "project" => self.t_project(&eng, a),
            "media" => self.t_media(&eng, a),
            "edit" => self.t_edit(&eng, a),
            "view" => self.t_view(&eng, a),
            "preview" => return self.t_preview(&eng, a),
            "graphic" => return self.t_graphic(&eng, a),
            "check" | "qa" | "review" => return self.t_check(&eng, a),
            "analyze" => self.t_analyze(&eng, a),
            "render" => self.t_render(&eng, a),
            other => Err(anyhow::anyhow!("unknown tool '{other}'")),
        };
        match r {
            Ok(t) => text_result(t),
            Err(e) => err_result(format!("{e:#}")),
        }
    }

    fn t_project(&self, eng: &Arc<Engine>, a: &Value) -> anyhow::Result<String> {
        let action = s(a, "action").unwrap_or_else(|| "info".into());
        match action.as_str() {
            "new" => {
                let name = s(a, "name").unwrap_or_else(|| "Untitled".into());
                let (w, h) = s(a, "size").and_then(|x| crate::ops::exec::parse_size(&x)).unwrap_or((1920, 1080));
                let fps = a["fps"].as_f64().unwrap_or(30.0);
                let path = s(a, "path").map(|p| {
                    let mut pb = std::path::PathBuf::from(p);
                    if pb.extension().is_none() {
                        pb.set_extension("mcut");
                    }
                    pb
                });
                eng.new_project(&name, w, h, fps, path)
            }
            "open" => eng.open(&s(a, "path").ok_or_else(|| anyhow::anyhow!("open needs path"))?),
            "save" => eng.save(s(a, "path").as_deref()),
            "undo" => Ok(eng.undo(a["n"].as_u64().unwrap_or(1) as usize, "mcp")?.log.join("\n")),
            "redo" => Ok(eng.redo(a["n"].as_u64().unwrap_or(1) as usize, "mcp")?.log.join("\n")),
            "history" => {
                let h = eng.history();
                Ok(if h.is_empty() { "no history".into() } else { h.join("\n") })
            }
            _ => {
                let (p, rev, active, path) = eng.snapshot();
                let mut out = outline::project_summary(&p, path.as_ref().map(|x| x.to_string_lossy().to_string()).as_deref(), &active, rev);
                out.push('\n');
                out.push_str(&outline::media_list(&p, 1));
                Ok(out)
            }
        }
    }

    fn t_media(&self, eng: &Arc<Engine>, a: &Value) -> anyhow::Result<String> {
        let action = s(a, "action").unwrap_or_else(|| "list".into());
        let paths: Vec<String> = match &a["paths"] {
            Value::Array(v) => v.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
            Value::String(x) => vec![x.clone()],
            _ => s(a, "path").into_iter().collect(),
        };
        match action.as_str() {
            "import" => {
                if paths.is_empty() {
                    anyhow::bail!("import needs paths");
                }
                for p in paths.iter().filter(|p| !p.starts_with("http")) {
                    eng.check_path(p)?;
                }
                let tags: Vec<String> = s(a, "tag").map(|t| t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
                let out = eng.import_with(&paths, a["recursive"].as_bool().unwrap_or(true), &tags, "mcp")?;
                Ok(out.log.join("\n"))
            }
            "proxy" | "proxies" => {
                use crate::render::proxy::Mode;
                let mut notes = vec![];
                // check everything first, so a bad call changes nothing
                let op = s(a, "op").map(|o| o.to_ascii_lowercase());
                let ids_given = s(a, "ids").or_else(|| s(a, "q")).is_some();
                if let Some(o) = op.as_deref() {
                    if !matches!(o, "create" | "make" | "build" | "delete" | "del" | "remove" | "cancel" | "stop") {
                        anyhow::bail!("unknown op '{o}' (create, delete, cancel)");
                    }
                    if matches!(o, "delete" | "del" | "remove") && !ids_given {
                        anyhow::bail!("op=delete needs ids (e.g. ids=m3 or ids=all)");
                    }
                }
                let mode = match s(a, "mode") {
                    Some(m) => Some(Mode::parse(&m).ok_or_else(|| anyhow::anyhow!("mode must be auto, all or off"))?),
                    None => None,
                };
                if let Some(mode) = mode {
                    eng.proxies.set_mode(mode);
                    notes.push(format!("new imports: {}", match mode { Mode::Off => "no proxies", Mode::Auto => "proxies for heavy footage", Mode::All => "proxies for every video" }));
                }
                if let Some(on) = a["enabled"].as_bool() {
                    eng.proxies.set_enabled(on);
                    notes.push(format!("previews use {}", if on { "proxies where ready" } else { "the original files" }));
                }
                let (p, ..) = eng.snapshot();
                if matches!(op.as_deref(), Some("cancel" | "stop")) {
                    eng.proxies.cancel_all();
                    notes.push("stopped making proxies (the queue is empty)".into());
                } else if let Some(spec) = s(a, "ids").or_else(|| s(a, "q")) {
                    let ids = crate::library::select_media(&p, &spec)?;
                    let ms: Vec<crate::model::project::Media> = ids.iter().filter_map(|i| p.media(i).cloned()).collect();
                    if matches!(op.as_deref(), Some("delete" | "del" | "remove")) {
                        notes.push(format!("deleted {} prox(ies)", eng.proxies.delete(&ms)));
                    } else {
                        let n = eng.proxies.request(&ms, true);
                        let skipped = ms.iter().filter(|m| !crate::render::proxy::eligible(m)).count();
                        notes.push(format!("queued {n} prox(ies){}", if skipped > 0 { format!(" ({skipped} skipped: not video, or with transparency)") } else { String::new() }));
                    }
                }
                notes.push(eng.proxies.summary(&p.media));
                Ok(notes.join("\n"))
            }
            "find" | "browse" | "scan" => {
                let root = paths.first().cloned().ok_or_else(|| anyhow::anyhow!("find needs path (a folder)"))?;
                eng.find_files(&root, s(a, "pattern").as_deref(), a["recursive"].as_bool().unwrap_or(true), a["files"].as_bool().unwrap_or(false), a["limit"].as_u64().unwrap_or(200) as usize)
            }
            _ => {
                let f = crate::library::filter_from_json(a);
                let limit = a["limit"].as_u64().unwrap_or(80).clamp(1, 1000) as usize;
                Ok(eng.media_listing(&f, &s(a, "sort").unwrap_or_default(), a["offset"].as_u64().unwrap_or(0) as usize, limit, a["detail"].as_u64().unwrap_or(1) as u8))
            }
        }
    }

    fn t_edit(&self, eng: &Arc<Engine>, a: &Value) -> anyhow::Result<String> {
        let script = s(a, "script").ok_or_else(|| anyhow::anyhow!("edit needs script"))?;
        let out = eng.edit(&script, "mcp")?;
        let (p, _, active, _) = eng.snapshot();
        let dur = p.comp(&active).map(|c| c.duration()).unwrap_or(0.0);
        let mut s = format!("ok rev {} ({} op{})", out.rev, out.ops, if out.ops == 1 { "" } else { "s" });
        for l in out.log.iter().take(120) {
            s.push('\n');
            s.push_str(l);
        }
        if out.log.len() > 120 {
            s.push_str(&format!("\n… {} more lines", out.log.len() - 120));
        }
        s.push_str(&format!("\n{} dur={}s", active, crate::model::time::fmt_t(dur)));
        Ok(s)
    }

    fn t_view(&self, eng: &Arc<Engine>, a: &Value) -> anyhow::Result<String> {
        let (p, rev, active, _) = eng.snapshot();
        let what = s(a, "what").unwrap_or_else(|| "timeline".into());
        let detail = a["detail"].as_u64().unwrap_or(1).min(2) as u8;
        let comp = s(a, "comp").unwrap_or(active.clone());
        Ok(match what.as_str() {
            "timeline" => {
                let ui = eng.ui();
                let mut head = format!("rev {rev}");
                if let Some(t) = ui.playhead {
                    head.push_str(&format!(" | editor: playhead={}", crate::model::time::fmt_t(t)));
                    if !ui.selection.is_empty() {
                        head.push_str(&format!(" selected={} (script: sel)", ui.selection.join(",")));
                    }
                }
                format!("{head}\n{}", outline::timeline(&p, &comp, &active, detail))
            }
            "item" => {
                let id = s(a, "id").ok_or_else(|| anyhow::anyhow!("view item needs id"))?;
                if let Some(c) = p.comp(&id) {
                    outline::timeline(&p, &c.id, &active, detail.max(1))
                } else {
                    let it = p.item(&id).ok_or_else(|| anyhow::anyhow!("unknown item {id}"))?;
                    outline::item_detail(&p, it)
                }
            }
            "media" => eng.media_listing(&Default::default(), "", 0, 200, detail.max(1)),
            "search" | "find" => {
                let q = s(a, "q").or_else(|| s(a, "id")).ok_or_else(|| anyhow::anyhow!("view search needs q"))?;
                eng.search(&q, a["limit"].as_u64().unwrap_or(40).clamp(1, 500) as usize)
            }
            "comps" => outline::comps_list(&p, &active),
            "markers" => {
                let c = p.comp(&comp).ok_or_else(|| anyhow::anyhow!("no comp {comp}"))?;
                if c.markers.is_empty() {
                    "no markers".into()
                } else {
                    c.markers.iter().map(|m| format!("{} {}", crate::model::time::fmt_t(m.t), m.name)).collect::<Vec<_>>().join("\n")
                }
            }
            "transcript" => {
                let id = s(a, "id").ok_or_else(|| anyhow::anyhow!("view transcript needs a media or clip id"))?;
                let (_, v) = eng.analyze(&id, "transcript", &BTreeMap::new())?;
                crate::analyze::summarize("transcript", &v, 400)
            }
            "history" => {
                let h = eng.history();
                if h.is_empty() {
                    "no history".into()
                } else {
                    h.join("\n")
                }
            }
            "log" => {
                let l = eng.log.lock();
                l.iter().rev().take(20).map(|e| format!("[{} {}{}] {}", e.rev, e.source, if e.ok { "" } else { " ERR" }, e.script.lines().next().unwrap_or(""))).collect::<Vec<_>>().join("\n")
            }
            "json" => {
                let id = s(a, "id").unwrap_or(comp.clone());
                if let Some(it) = p.item(&id) {
                    serde_json::to_string(it)?
                } else if let Some(c) = p.comp(&id) {
                    serde_json::to_string(c)?
                } else {
                    anyhow::bail!("unknown id {id}")
                }
            }
            other => anyhow::bail!("unknown view '{other}'"),
        })
    }

    fn t_preview(&self, eng: &Arc<Engine>, a: &Value) -> Value {
        let r = (|| -> anyhow::Result<(Vec<u8>, String)> {
            let (p, _, active, _) = eng.snapshot();
            if let Some(kind) = s(a, "scope") {
                let comp = s(a, "comp").unwrap_or_else(|| active.clone());
                let c = p.comp(&comp).ok_or_else(|| anyhow::anyhow!("no comp {comp}"))?;
                let t = a["t"].as_f64().or(eng.ui().playhead).unwrap_or(c.duration() / 2.0).clamp(0.0, (c.duration() - c.frame_dur()).max(0.0));
                let w = a["width"].as_u64().unwrap_or(if kind == "all" { 640 } else { 384 }).clamp(128, 1280) as u32;
                let h = if kind == "vectorscope" { w } else { w * 9 / 16 };
                let (pm, text) = eng.scope(Some(&c.id), t, &kind, w, h)?;
                return Ok((encode::encode_jpeg(&pm, 82)?, format!("{kind} t={}s\n{text}", crate::model::time::fmt_t(t))));
            }
            if let Some(spec) = s(a, "media") {
                let ids = crate::library::select_media(&p, &spec)?;
                let parse_range = |r: String| r.split_once('-').and_then(|(x, y)| Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?)));
                let (pm, cap) = if ids.len() == 1 && (a["n"].as_u64().unwrap_or(6) > 1 || a.get("range").is_some()) {
                    eng.media_strip(&ids[0], a["n"].as_u64().unwrap_or(6) as usize, s(a, "range").and_then(parse_range), a["width"].as_u64().unwrap_or(240) as u32)?
                } else {
                    let off = a["offset"].as_u64().unwrap_or(0) as usize;
                    let page: Vec<String> = ids.iter().skip(off).take(48).cloned().collect();
                    if page.is_empty() {
                        anyhow::bail!("offset {off} is past the {} matching media", ids.len());
                    }
                    let (pm, mut cap) = eng.media_sheet(&page, a["width"].as_u64().unwrap_or(if page.len() > 24 { 160 } else { 200 }) as u32)?;
                    if ids.len() > off + page.len() {
                        cap.push_str(&format!(" (of {}; next offset={})", ids.len(), off + page.len()));
                    }
                    (pm, cap)
                };
                return Ok((encode::encode_jpeg(&pm, 78)?, cap));
            }
            let comp = s(a, "comp").unwrap_or(active);
            let c = p.comp(&comp).ok_or_else(|| anyhow::anyhow!("no comp {comp}"))?;
            // default range: where the pictures are (a long music bed shouldn't stretch the sheet)
            let vis_end = c.vtracks().map(|t| t.end()).fold(0.0, f64::max);
            let dur = if vis_end > 0.0 { vis_end } else { c.duration().max(c.frame_dur()) };
            let times: Vec<f64> = if let Some(arr) = a["times"].as_array() {
                arr.iter().filter_map(|x| x.as_f64()).take(16).collect()
            } else if let Some(t) = a["t"].as_f64() {
                vec![t]
            } else {
                let n = a["n"].as_u64().unwrap_or(6).clamp(1, 16) as usize;
                let (r0, r1) = match s(a, "range").and_then(|r| r.split_once('-').map(|(x, y)| (x.trim().parse::<f64>().ok(), y.trim().parse::<f64>().ok()))) {
                    Some((Some(x), Some(y))) if y > x => (x, y),
                    _ => (0.0, dur),
                };
                (0..n).map(|i| r0 + (i as f64 + 0.5) * (r1 - r0) / n as f64).collect()
            };
            let full = c.duration().max(c.frame_dur());
            let times: Vec<f64> = times.into_iter().map(|t| t.clamp(0.0, (full - c.frame_dur()).max(0.0))).collect();
            if times.len() == 1 {
                let w = a["width"].as_u64().unwrap_or(512).clamp(64, 1920) as u32;
                let pm = eng.frame(Some(&c.id), times[0], w, false)?;
                let jpg = encode::encode_jpeg(&pm, 82)?;
                return Ok((jpg, format!("frame t={}s {}x{}", crate::model::time::fmt_t(times[0]), pm.width(), pm.height())));
            }
            let n = times.len();
            let tw = a["width"].as_u64().map(|w| w as u32).unwrap_or(if n <= 4 { 320 } else if n <= 9 { 240 } else { 200 }).clamp(64, 640);
            let cols = if c.h > c.w { n.min(6) } else { (n as f64).sqrt().ceil() as usize };
            let pm = eng.contact_sheet(Some(&c.id), &times, tw, cols)?;
            let jpg = encode::encode_jpeg(&pm, 78)?;
            Ok((jpg, format!("{n} frames at {}", times.iter().map(|t| crate::model::time::fmt_t(*t)).collect::<Vec<_>>().join(", "))))
        })();
        match r {
            Ok((jpg, caption)) => json!({"content": [
                {"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&jpg), "mimeType": "image/jpeg"},
                {"type": "text", "text": caption}
            ]}),
            Err(e) => err_result(format!("{e:#}")),
        }
    }

    fn t_graphic(&self, eng: &Arc<Engine>, a: &Value) -> Value {
        let req = graphic_req(a);
        match eng.graphic(req, "mcp") {
            Ok(o) => {
                let mut content = vec![];
                if let Some(jpg) = o.sheet {
                    content.push(json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&jpg), "mimeType": "image/jpeg"}));
                }
                content.push(json!({"type": "text", "text": o.text}));
                json!({"content": content})
            }
            Err(e) => err_result(format!("{e:#}")),
        }
    }

    fn t_check(&self, eng: &Arc<Engine>, a: &Value) -> Value {
        let e = &a["expect"];
        let aspect = e["aspect"].as_str().and_then(|x| {
            let (w, h) = x.split_once([':', 'x', '/'])?;
            Some(w.trim().parse::<f64>().ok()? / h.trim().parse::<f64>().ok()?)
        }).or_else(|| e["aspect"].as_f64());
        let o = crate::check::CheckOpts {
            comp: s(a, "comp"),
            target: s(a, "target"),
            expect: crate::check::Expect {
                dur: e["dur"].as_f64(),
                dur_tol: e["dur_tol"].as_f64(),
                size: e["size"].as_str().and_then(crate::ops::exec::parse_size),
                aspect,
                fps: e["fps"].as_f64(),
                lufs: e["lufs"].as_f64(),
                captions: e["captions"].as_bool(),
                music: e["music"].as_bool(),
                max_shot: e["max_shot"].as_f64(),
                min_shot: e["min_shot"].as_f64(),
            },
            audio: a["audio"].as_bool().unwrap_or(true),
            frames: a["frames"].as_bool().unwrap_or(true),
            fix: a["fix"].as_bool().unwrap_or(false),
            safe: a["safe"].as_f64(),
        };
        match eng.check(&o, "mcp") {
            Ok(r) => {
                let mut content = vec![];
                if let Some(jpg) = r.sheet {
                    content.push(json!({"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&jpg), "mimeType": "image/jpeg"}));
                }
                content.push(json!({"type": "text", "text": r.text}));
                json!({"content": content})
            }
            Err(e) => err_result(format!("{e:#}")),
        }
    }

    fn t_analyze(&self, eng: &Arc<Engine>, a: &Value) -> anyhow::Result<String> {
        let target = s(a, "target").ok_or_else(|| anyhow::anyhow!("analyze needs target"))?;
        let kind = s(a, "kind").ok_or_else(|| anyhow::anyhow!("analyze needs kind"))?;
        let mut opts = BTreeMap::new();
        if let Some(o) = a["params"].as_object() {
            for (k, v) in o {
                let vs = match v {
                    Value::String(x) => x.clone(),
                    other => other.to_string(),
                };
                opts.insert(k.clone(), vs);
            }
        }
        let (p, ..) = eng.snapshot();
        let single = p.media(&target).is_some() || p.item(&target).is_some();
        if !single {
            let ids = crate::library::select_media(&p, &target)?;
            if ids.len() > 1 {
                return eng.analyze_many(&ids, &kind, &opts);
            }
        }
        let (mid, v) = eng.analyze(&target, &kind, &opts)?;
        Ok(format!("{mid} {kind}: {}", crate::analyze::summarize(&kind, &v, 60)))
    }

    fn t_render(&self, eng: &Arc<Engine>, a: &Value) -> anyhow::Result<String> {
        let action = s(a, "action").unwrap_or_else(|| "start".into());
        match action.as_str() {
            "list" => {
                let jobs = eng.jobs.lock();
                if jobs.is_empty() {
                    return Ok("no renders".into());
                }
                Ok(jobs.iter().map(|j| j.status().to_string()).collect::<Vec<_>>().join("\n"))
            }
            "status" | "cancel" => {
                let id = s(a, "id").or_else(|| eng.jobs.lock().last().map(|j| j.id.clone())).ok_or_else(|| anyhow::anyhow!("no render jobs"))?;
                let j = eng.job(&id).ok_or_else(|| anyhow::anyhow!("no job {id}"))?;
                if action == "cancel" {
                    j.prog.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(j.status().to_string())
            }
            _ => {
                let (t0, t1) = match s(a, "range") {
                    Some(r) => eng.render_range(&s(a, "comp").unwrap_or_default(), &r)?,
                    None => (0.0, 0.0),
                };
                let opts = RenderOpts {
                    comp: s(a, "comp").unwrap_or_default(),
                    t0,
                    t1,
                    preset: s(a, "preset").unwrap_or_else(|| "mp4".into()),
                    out: s(a, "out").map(std::path::PathBuf::from).unwrap_or_default(),
                    scale: a["scale"].as_f64().unwrap_or(1.0) as f32,
                    hw: s(a, "hw").unwrap_or_else(|| "off".into()),
                };
                let job = eng.start_render(opts)?;
                if a["wait"].as_bool().unwrap_or(false) {
                    let start = std::time::Instant::now();
                    while job.result.lock().is_none() && start.elapsed().as_secs() < 600 {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                }
                Ok(job.status().to_string())
            }
        }
    }
}

/// Parses `graphic` tool / API arguments.
pub fn graphic_req(a: &Value) -> crate::gfx::GraphicReq {
    let vars = a["vars"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    let vs = match v {
                        Value::String(x) => x.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), vs)
                })
                .collect()
        })
        .unwrap_or_default();
    crate::gfx::GraphicReq {
        html: s(a, "html"),
        name: s(a, "name"),
        dur: a["dur"].as_f64(),
        size: s(a, "size").and_then(|x| crate::ops::exec::parse_size(&x)),
        fps: a["fps"].as_f64(),
        alpha: a["alpha"].as_bool(),
        vars,
        id: s(a, "id"),
        from: s(a, "from"),
        place: s(a, "place"),
        preview: a["preview"].as_u64().unwrap_or(6) as usize,
        at: a["at"].as_f64(),
    }
}

/// Ready-made requests (MCP prompts) for common jobs; clients show them as commands.
pub struct Prompt {
    pub name: &'static str,
    pub title: &'static str,
    pub desc: &'static str,
    pub args: &'static [(&'static str, &'static str, bool)],
    pub build: fn(&dyn Fn(&str) -> Option<String>) -> String,
}

fn opt(v: Option<String>, d: &str) -> String {
    v.unwrap_or_else(|| d.to_string())
}

pub fn prompts() -> Vec<Prompt> {
    vec![
        Prompt {
            name: "edit-folder",
            title: "Edit a video from a folder",
            desc: "Turn a folder of footage into a finished edit, start to finish.",
            args: &[("folder", "folder with the footage", true), ("brief", "what to make (topic, tone, audience)", true), ("length", "target length in seconds", false), ("format", "16:9, 9:16 or 1:1", false), ("music", "music file or folder (optional)", false)],
            build: |g| {
                format!(
                    "Use MoodCut to make this video: {brief}.\nFootage: {folder}. Target length: {len}. Format: {fmt}. Music: {music}.\n\
                     Work like a senior editor and motion designer: read help('style') and the closest help('recipes'), then follow help('workflow') \
                     end to end: import and look at the footage (poster sheets, transcripts), plan a structure with a hook in the first 2 seconds, \
                     set a brand kit and one look, assemble, polish (titles with one consistent entrance, captions if there is speech), add sound \
                     design (help sfx), mix to -14 LUFS, then run check with the platform target and expect (duration and size) and fix everything \
                     it reports, including craft notes. Render with the matching preset. Don't ask me questions unless something essential is \
                     missing; make confident creative choices and tell me what you made and why.",
                    brief = opt(g("brief"), "a short video"),
                    folder = opt(g("folder"), "?"),
                    len = g("length").map(|l| format!("{l} s")).unwrap_or_else(|| "whatever suits the material".into()),
                    fmt = opt(g("format"), "16:9"),
                    music = opt(g("music"), "choose from the footage folder if there is any, otherwise none"),
                )
            },
        },
        Prompt {
            name: "talking-head",
            title: "Clean up a talking-head video",
            desc: "Remove pauses, grade, add captions, name tag, music bed and loudness for a speech-led video.",
            args: &[("file", "the video file", true), ("name", "speaker name for a lower third", false), ("captions", "caption style: bold, box, minimal or none", false), ("music", "music file (optional)", false)],
            build: |g| {
                format!(
                    "In MoodCut, edit the talking-head video {file}: import it, remove silences (autocut silence), color correct, \
                     add {cap} captions, {name}add the music {music} ducked under the voice, set loudness to -14 LUFS, \
                     run check (fix what it finds), and render for YouTube. Tell me the final length and what changed.",
                    file = opt(g("file"), "?"),
                    cap = opt(g("captions"), "bold"),
                    name = g("name").map(|n| format!("a lower third for {n} near the start (graphic tool), ")).unwrap_or_default(),
                    music = opt(g("music"), "(none given: skip music)"),
                )
            },
        },
        Prompt {
            name: "social-cutdown",
            title: "Vertical social cut-down",
            desc: "Make a 9:16 short from the current edit: tighter pacing, captions, hook.",
            args: &[("length", "max length in seconds (default 45)", false), ("platform", "tiktok, reels or shorts", false)],
            build: |g| {
                format!(
                    "In MoodCut, make a vertical {plat} version of the current edit, at most {len} s: keep the strongest moments \
                     (hook in the first 1-2 seconds, read help('platforms')), reframe 9:16 with a blurred fill, tighten the pacing (shots \
                     0.8-2 s, something new every 3-5 s), keep text and faces out of the app's UI zones, add punchy captions if there is speech \
                     (captions V1 style=punch), add sound design on the cuts that move, then check target=reels expect={{size: '1080x1920', \
                     dur: {len}}} with fix and render preset=social.",
                    plat = opt(g("platform"), "short-form"),
                    len = opt(g("length"), "45"),
                )
            },
        },
        Prompt {
            name: "reel",
            title: "Instagram Reel / TikTok / Short",
            desc: "A scroll-stopping vertical video from footage: hook, punchy captions, pattern interrupts, sound design.",
            args: &[("footage", "folder or files", true), ("brief", "topic, tone, audience, call to action", true), ("length", "seconds (default 30)", false), ("platform", "reels, tiktok or shorts", false), ("brand", "brand colors/font, or a vibe", false), ("music", "music file (optional)", false)],
            build: |g| {
                format!(
                    "Make a {plat} in MoodCut: {brief}. Footage: {footage}. Length about {len} s. Brand: {brand}. Music: {music}.\n\
                     Read help('style'), help('platforms') and the reel-talk recipe in help('recipes') first. Build a 1080x1920 comp. \
                     Open with the strongest moment and an on-screen hook within the first 6 frames (slamIn or wordPop, with a hit). \
                     Keep shots 0.8-2 s and change something every 3-5 s (punch-ins on jump cuts, b-roll with whooshes, text pops). \
                     Captions last: captions V1 style=punch words=2 inside the safe zone. One look, the brand kit, one transition type, \
                     music ducked under speech, -14 LUFS. End on a loop or a 1-2 s CTA. Then check target={plat} with expect size 1080x1920, \
                     fix everything (craft notes too), look at a preview n=12 like a viewer, and render preset=social.",
                    plat = opt(g("platform"), "reels"),
                    brief = opt(g("brief"), "?"),
                    footage = opt(g("footage"), "?"),
                    len = opt(g("length"), "30"),
                    brand = opt(g("brand"), "choose a brand preset that fits the brief"),
                    music = opt(g("music"), "use one from the footage folder if there is any"),
                )
            },
        },
        Prompt {
            name: "youtube-video",
            title: "YouTube video",
            desc: "A polished YouTube edit: cold open, tight talking head with b-roll, lower thirds, chapters, sound design.",
            args: &[("footage", "folder or files", true), ("brief", "topic and audience", true), ("length", "target minutes or seconds", false), ("music", "music file or folder (optional)", false)],
            build: |g| {
                format!(
                    "Edit a YouTube video in MoodCut: {brief}. Footage: {footage}. Target length: {len}. Music: {music}.\n\
                     Read help('style') and the youtube-talk recipe in help('recipes'). Transcribe the talking head and cut the story from the \
                     transcript (strongest claim first as a 5-15 s cold open), autocut silence and punch in on every few jump cuts, b-roll or a \
                     graphic every 5-10 s starting on the word it shows, a lower third for each speaker, chapter markers and chapter titles, one \
                     look and one transition style, subtle music ducked under speech, sound design on b-roll entries and titles, -14 LUFS. Keep \
                     the last 15-20 s clear for end-screen elements. check target=youtube with expect, fix everything, then render preset=youtube \
                     and preset=chapters.",
                    brief = opt(g("brief"), "?"),
                    footage = opt(g("footage"), "?"),
                    len = opt(g("length"), "whatever the story needs (tight)"),
                    music = opt(g("music"), "optional: only if it helps"),
                )
            },
        },
        Prompt {
            name: "promo",
            title: "Product / brand promo",
            desc: "A launch film or ad with kinetic typography, motion design and a CTA end card.",
            args: &[("footage", "product shots / footage", true), ("brief", "product, the one message, call to action", true), ("brand", "brand font and colors (or a vibe)", false), ("length", "6, 15 or 30 s (default 15)", false), ("format", "16:9, 9:16, 1:1 or 4:5", false), ("music", "music file", false)],
            build: |g| {
                format!(
                    "Make a {len} s {fmt} promo in MoodCut: {brief}. Footage: {footage}. Brand: {brand}. Music: {music}.\n\
                     Read help('style'), help('brand') and the promo recipe in help('recipes'). Set the brand kit first. Product or claim within \
                     3 s, one message, kinetic typography on the beat (one word or short line per beat, the same entrance style throughout), \
                     product shots with camera moves or parallax, a designed end card with logo and CTA for the last 2-3 s. Use the graphic tool \
                     (HTML with var(--brand-*) colors) for anything the templates can't do. Sound design on every typographic hit and \
                     transition, a riser into the logo. check with expect (duration, size), fix, render.",
                    len = opt(g("length"), "15"),
                    fmt = opt(g("format"), "16:9"),
                    brief = opt(g("brief"), "?"),
                    footage = opt(g("footage"), "?"),
                    brand = opt(g("brand"), "pick a preset that fits"),
                    music = opt(g("music"), "choose from the footage folder if any"),
                )
            },
        },
        Prompt {
            name: "montage",
            title: "Cinematic music montage",
            desc: "Travel, event or brand-film montage cut to the music with ramps, whips and a film look.",
            args: &[("footage", "folder or files", true), ("music", "the track", true), ("length", "seconds (default: the music phrase that fits)", false), ("look", "kodak, fuji, warm, moody...", false), ("title", "title text (optional)", false)],
            build: |g| {
                format!(
                    "Cut a cinematic montage in MoodCut from {footage} to {music}. Length {len}. Look: {look}. {title}\n\
                     Read help('style') and the montage recipe in help('recipes'). Look at every clip (poster sheets, filmstrips) and pick the \
                     best 1-3 s of each (action, light, faces); order them as a story (establishing -> details -> people -> peak -> ending). Find \
                     the beats and the drop; cut every 2-4 beats, faster after the drop, speed ramps into the drop with a riser ending on it and \
                     a boom on it, whips with whooshes between locations, one look with grain and letterbox, end on the music's ending. check, \
                     fix, render preset=youtube (and a 9:16 version with reframe if useful).",
                    footage = opt(g("footage"), "?"),
                    music = opt(g("music"), "?"),
                    len = opt(g("length"), "a natural phrase of the music"),
                    look = opt(g("look"), "kodak"),
                    title = g("title").map(|t| format!("Title: '{t}' (cinematic, wide tracking, trackIn).")).unwrap_or_default(),
                )
            },
        },
        Prompt {
            name: "motion-graphic",
            title: "Design a motion graphic",
            desc: "Design an animated graphic in HTML/CSS/JS, render it with alpha and place it.",
            args: &[("brief", "what the graphic shows and its style", true), ("dur", "seconds (default 4)", false), ("at", "timeline time to place it (optional)", false)],
            build: |g| {
                format!(
                    "Design this motion graphic for my MoodCut edit with the graphic tool: {brief}. Duration {dur} s. \
                     Read help('graphics') first. Use a polished, modern style with eased entrance and exit animations, keep text in the \
                     title-safe area, and check the returned contact sheet; fix anything off (overflow, timing, JS errors) with id= and \
                     render again. {place}",
                    brief = opt(g("brief"), "?"),
                    dur = opt(g("dur"), "4"),
                    place = g("at").map(|t| format!("Place it at {t} s on a track above the video.")).unwrap_or_else(|| "Then place it where it fits best in the edit.".into()),
                )
            },
        },
        Prompt {
            name: "color-grade",
            title: "Color grade with scopes",
            desc: "Balance and grade the edit shot by shot, measuring with the Lumetri scopes.",
            args: &[("look", "the look to aim for (natural, cinematic, warm, teal-orange, moody...)", false), ("target", "web or broadcast (broadcast keeps levels legal)", false)],
            build: |g| {
                format!(
                    "Color grade my MoodCut edit like a colorist, aiming for a {look} look. Read help('scopes') and help('color'). For every \
                     distinct shot on the main video track: preview scope=all at its middle, then correct first (exposure, whites/blacks, \
                     temp/tint until the cast is neutral and luma sits about 2-98% without clipping) with color <clip> key=value, then apply \
                     the look consistently (color preset, colorwheels, huecurves; secondary hue=25 to protect skin), and check the scope \
                     again. Match shots that cut together (paste <graded> <others> color). {legal}Summarize the grade per shot.",
                    look = opt(g("look"), "natural, polished"),
                    legal = if g("target").is_some_and(|t| t.contains("broad")) { "Finish with fx <all clips> legalize and a scope check (outside 16-235 must be ~0%). " } else { "" },
                )
            },
        },
        Prompt {
            name: "review",
            title: "Review and fix the edit",
            desc: "Quality-check the current edit like a finishing editor and fix the problems.",
            args: &[("target", "web, podcast or broadcast (default web)", false)],
            build: |g| {
                format!(
                    "Review my current MoodCut edit like a finishing editor: run check target={t}, look at the problem frames and a \
                     preview n=8 contact sheet, apply the safe fixes, fix the rest by hand (pacing, continuity, text, audio), then check \
                     again until clean. Summarize what you changed.",
                    t = opt(g("target"), "web")
                )
            },
        },
    ]
}
