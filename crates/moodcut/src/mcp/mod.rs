//! Model Context Protocol server (JSON-RPC 2.0), transport-agnostic.
//!
//! Token-efficiency design:
//! - 8 small tools; the big reference lives behind `help(topic)`.
//! - `edit` takes a whole MoodCut Script (dozens of edits per call, atomic).
//! - Results are compact diffs/outlines, never full project JSON (unless asked).
//! - `preview` returns one contact-sheet image instead of N frames.

pub mod docs;
pub mod stdio;

use crate::engine::Engine;
use crate::ops::outline;
use crate::render::encode::{self, RenderOpts};
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub const INSTRUCTIONS: &str = "MoodCut is a local video editor + motion graphics engine. Workflow: media(import) -> view -> edit (send MANY script lines per call; atomic) -> preview (small contact sheet to check) -> render. \
Ids: m# media, c# clips, t# text, g# shapes, s# comps; tracks V1 (bottom) V2.. and A1... Times in seconds. \
The edit tool description has a cheat sheet; call help(topic) for full references (edit, effects, transitions, anim, text, templates, macros, color, render).";

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
            "description": "Import media (files, folders, globs like C:/clips/*.mp4) or list imported media. Returns ids (m1..) with type, size, fps, duration.",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["import", "list"]},
                "paths": {"type": "array", "items": {"type": "string"}}
            }, "required": ["action"]},
            "annotations": {"title": "Media", "destructiveHint": false, "openWorldHint": false}
        },
        {
            "name": "edit",
            "description": "Apply a MoodCut Script: one command per line, all-or-nothing. Returns a diff (+created ~changed -removed !warning).\n\
Place: add <m|path>[in-out] [V2] [at=T] [dur=] [props] | append m1[0-5] m2 | text 'Hi' at=0 dur=3 size=120 enter=fadeUp exit=fadeDown | shape rect w=400 h=80 fill=#f05 | solid #000 | adjust\n\
Edit: cut <id|V1> at=T | trim <id> start=+1 end=-0.5 | del <ids> [ripple] | move <id> at=T [track=V2] | speed <id> 2x | freeze <id> at=T | dup <id>\n\
Props: set <id> pos=center scale=1.2 rot=10 opacity=50% blend=screen vol=-6dB | kf <id> scale 0:1 2:1.2@outBack | expr <id> pos 'wiggle(2,20)'\n\
Look: fx <id> blur radius=8 | color <id> cinematic | tr <a> <b> dissolve 0.5 | tr V1 dissolve | fade <id> in=0.5 out=1 | mask <id> ellipse feather=40 | anim <id> popIn,fadeOut\n\
Pro: title lowerthird text='Jane' sub='CEO' | captions <clip> | autocut silence <clip> | montage m1,m2 music=m3 beats=2 | reframe 9:16 | duck A1 by=A2 | parent <a> <b> | precomp <ids>\n\
Refs: $ = last created item, $2 = 2nd created in this script. Times: 12.5, 1:02, 90f, end, c3.end+0.5, @marker. undo / redo alone. help('edit') lists everything.",
            "inputSchema": {"type": "object", "properties": {"script": {"type": "string", "description": "MoodCut Script lines"}}, "required": ["script"]},
            "annotations": {"title": "Edit timeline", "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        },
        {
            "name": "view",
            "description": "Read state as compact text. what: timeline (default; detail 0 summary, 1 items, 2 all props) | item (id) | media | comps | markers | transcript (id) | history | log | json (id or comp).",
            "inputSchema": {"type": "object", "properties": {
                "what": {"type": "string", "enum": ["timeline", "item", "media", "comps", "markers", "transcript", "history", "log", "json"]},
                "id": {"type": "string"}, "detail": {"type": "integer", "minimum": 0, "maximum": 2}, "comp": {"type": "string"}
            }},
            "annotations": {"title": "View", "readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "preview",
            "description": "Look at the edit. t=seconds for one frame, or n=frames for a labeled contact sheet across range (default whole comp). Small widths save tokens (default 512 single, 240/thumb).",
            "inputSchema": {"type": "object", "properties": {
                "t": {"type": "number"}, "times": {"type": "array", "items": {"type": "number"}}, "n": {"type": "integer", "minimum": 1, "maximum": 16},
                "range": {"type": "string", "description": "a-b seconds"}, "width": {"type": "integer"}, "comp": {"type": "string"}
            }},
            "annotations": {"title": "Preview frames", "readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "analyze",
            "description": "Analyze media or a clip's media (cached): silence (thresh,min) | scenes (thresh) | loudness | beats | transcript (lang, model, src=.srt) | black | stats (exposure hints).",
            "inputSchema": {"type": "object", "properties": {
                "target": {"type": "string"}, "kind": {"type": "string", "enum": ["silence", "scenes", "loudness", "beats", "transcript", "black", "stats"]},
                "params": {"type": "object", "additionalProperties": {"type": ["string", "number"]}}
            }, "required": ["target", "kind"]},
            "annotations": {"title": "Analyze", "readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "render",
            "description": "Export. action: start (default) | status | list | cancel. preset: mp4 youtube social draft hevc av1 prores prores4444 webm webm-alpha mov-alpha gif png mp3 wav. Options: out, range a-b, scale, hw=auto (GPU), wait=true to block until done.",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["start", "status", "list", "cancel"]},
                "preset": {"type": "string"}, "out": {"type": "string"}, "range": {"type": "string"}, "scale": {"type": "number"},
                "hw": {"type": "string"}, "comp": {"type": "string"}, "id": {"type": "string"}, "wait": {"type": "boolean"}
            }},
            "annotations": {"title": "Render", "destructiveHint": false, "openWorldHint": false}
        },
        {
            "name": "help",
            "description": "MoodCut reference on demand. topic: edit time props effects audio transitions anim text shapes templates macros color expr render analyze examples fonts.",
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
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "moodcut", "title": "MoodCut Video Editor", "version": crate::VERSION},
                    "instructions": INSTRUCTIONS
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                Ok(self.call(name, &args))
            }
            "resources/list" => Ok(json!({"resources": []})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
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
        if action == "import" {
            let paths: Vec<String> = match &a["paths"] {
                Value::Array(v) => v.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
                Value::String(x) => vec![x.clone()],
                _ => vec![],
            };
            if paths.is_empty() {
                anyhow::bail!("import needs paths");
            }
            for p in &paths {
                eng.check_path(p)?;
            }
            let out = eng.import(&paths, "mcp")?;
            return Ok(out.log.join("\n"));
        }
        let (p, ..) = eng.snapshot();
        Ok(outline::media_list(&p, a["detail"].as_u64().unwrap_or(1) as u8))
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
            "timeline" => format!("rev {rev}\n{}", outline::timeline(&p, &comp, &active, detail)),
            "item" => {
                let id = s(a, "id").ok_or_else(|| anyhow::anyhow!("view item needs id"))?;
                if let Some(c) = p.comp(&id) {
                    outline::timeline(&p, &c.id, &active, detail.max(1))
                } else {
                    let it = p.item(&id).ok_or_else(|| anyhow::anyhow!("unknown item {id}"))?;
                    outline::item_detail(&p, it)
                }
            }
            "media" => outline::media_list(&p, detail.max(1)),
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
                    Some(r) => {
                        let (x, y) = r.split_once('-').ok_or_else(|| anyhow::anyhow!("range must be a-b"))?;
                        (x.trim().parse()?, y.trim().parse()?)
                    }
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
