//! Compact, token-efficient text views of a project for AI clients.

use super::exec::{kind_s, media_summary};
use crate::model::project::*;
use crate::model::schema;
use crate::model::time::fmt_t;
use crate::model::value::fmt_num;
use std::fmt::Write;

fn trunc(s: &str, n: usize) -> String {
    let one_line = s.replace('\n', "⏎");
    if one_line.chars().count() <= n {
        one_line
    } else {
        format!("{}…", one_line.chars().take(n).collect::<String>())
    }
}

pub fn comp_header(p: &Project, c: &Comp, active: &str) -> String {
    let mut s = format!("{} '{}' {}x{}@{} dur={}", c.id, c.name, c.w, c.h, fmt_num(c.fps), fmt_t(c.duration()));
    if c.id == p.main {
        s.push_str(" [main]");
    }
    if c.id == active && c.id != p.main {
        s.push_str(" [active]");
    }
    if c.bg.a < 1.0 {
        s.push_str(" bg=none");
    } else if c.bg.to_hex() != "#000000" {
        s.push_str(&format!(" bg={}", c.bg.to_hex()));
    }
    s
}

fn tr_s(t: &Transition) -> String {
    let mut s = format!("{} {}", t.kind, fmt_num(t.dur));
    for (k, v) in &t.props {
        s.push_str(&format!(" {k}={}", v.display()));
    }
    s
}

/// One-line item summary.
pub fn item_line(p: &Project, it: &Item) -> String {
    let mut s = String::new();
    let _ = write!(s, "{}", it.id);
    if let Some(n) = &it.name {
        let _ = write!(s, "({n})");
    }
    match it.kind {
        ItemKind::Clip => {
            let src = it.src.clone().unwrap_or_default();
            let _ = write!(s, " {src}");
        }
        ItemKind::Text => {
            let _ = write!(s, " text '{}'", trunc(it.text.as_deref().unwrap_or(""), 32));
        }
        ItemKind::Shape => {
            let kinds: Vec<&str> = it.shapes.iter().map(|x| x.kind.as_str()).collect();
            let _ = write!(s, " shape {}", kinds.join("+"));
        }
        ItemKind::Solid => {
            let c = it.props.get("color").map(|a| a.v.display()).unwrap_or_else(|| "#ffffff".into());
            let _ = write!(s, " solid {c}");
        }
        ItemKind::Adjust => s.push_str(" adjust"),
        ItemKind::Comp => {
            let _ = write!(s, " comp {}", it.src.clone().unwrap_or_default());
        }
    }
    let _ = write!(s, " {}-{}", fmt_t(it.start), fmt_t(it.end()));
    if it.src_in > 0.0 {
        let _ = write!(s, " in={}", fmt_t(it.src_in));
    }
    let sp = it.speed();
    if (sp - 1.0).abs() > 1e-9 {
        let _ = write!(s, " x{}", fmt_num(sp));
    }
    if it.reversed() {
        s.push_str(" rev");
    }
    if it.props.get("hold").map(|a| a.v.bool()).unwrap_or(false) {
        s.push_str(" hold");
    }
    if it.off {
        s.push_str(" OFF");
    }
    // static notable props
    for k in ["pos", "scale", "rot", "opacity", "vol", "blend", "fit", "size"] {
        if let Some(a) = it.props.get(k) {
            if !a.is_animated() {
                let _ = write!(s, " {k}={}", a.v.display());
            }
        }
    }
    if it.props.get("mute").map(|a| a.v.bool()).unwrap_or(false) {
        s.push_str(" mute");
    }
    let keyed: Vec<&str> = it.props.iter().filter(|(_, a)| a.is_animated()).map(|(k, _)| k.as_str()).collect();
    if !keyed.is_empty() {
        let _ = write!(s, " kf:{}", keyed.join(","));
    }
    if !it.fx.is_empty() {
        let names: Vec<String> = it.fx.iter().map(|e| format!("{}{}", e.kind, if e.on { "" } else { "(off)" })).collect();
        let _ = write!(s, " fx:{}", names.join(","));
    }
    if !it.afx.is_empty() {
        let names: Vec<&str> = it.afx.iter().map(|e| e.kind.as_str()).collect();
        let _ = write!(s, " afx:{}", names.join(","));
    }
    if !it.masks.is_empty() {
        let _ = write!(s, " masks:{}", it.masks.len());
    }
    if !it.anims.is_empty() {
        let names: Vec<&str> = it.anims.iter().map(|e| e.kind.as_str()).collect();
        let _ = write!(s, " txanim:{}", names.join(","));
    }
    if let Some(t) = &it.tin {
        let _ = write!(s, " <{}", tr_s(t).replace(' ', ":"));
    }
    if let Some(t) = &it.tout {
        let _ = write!(s, " >{}", tr_s(t).replace(' ', ":"));
    }
    if let Some(pr) = &it.parent {
        let _ = write!(s, " parent={pr}");
    }
    if let Some(m) = &it.matte {
        let _ = write!(s, " matte={}:{:?}", m.src, m.mode);
    }
    let fi = it.props.get("fadeIn").map(|a| a.v.num()).unwrap_or(0.0);
    let fo = it.props.get("fadeOut").map(|a| a.v.num()).unwrap_or(0.0);
    if fi > 0.0 || fo > 0.0 {
        let _ = write!(s, " afade={}/{}", fmt_num(fi), fmt_num(fo));
    }
    let _ = p;
    s
}

fn effect_line(e: &Effect, list: &[&schema::P]) -> String {
    let mut s = format!("{} {}", e.id, e.kind);
    if !e.on {
        s.push_str(" (off)");
    }
    for (k, a) in &e.props {
        let _ = write!(s, " {k}={}", a.display());
    }
    let _ = list;
    s
}

/// Multi-line detail of one item.
pub fn item_detail(p: &Project, it: &Item) -> String {
    let mut s = item_line(p, it);
    s.push('\n');
    if let Some(t) = &it.text {
        let _ = writeln!(s, "  text: {:?}", t);
    }
    if let Some(m) = it.src.as_deref().and_then(|x| p.media(x)) {
        let _ = writeln!(s, "  media: {} {} {}", m.name, kind_s(m.kind), media_summary(m));
    }
    for (k, a) in &it.props {
        let _ = writeln!(s, "  {k}: {}", a.display());
    }
    for e in &it.fx {
        let _ = writeln!(s, "  fx {}", effect_line(e, &[]));
    }
    for e in &it.afx {
        let _ = writeln!(s, "  afx {}", effect_line(e, &[]));
    }
    for e in &it.masks {
        let _ = writeln!(s, "  mask {}", effect_line(e, &[]));
    }
    for e in &it.shapes {
        let _ = writeln!(s, "  shape {}", effect_line(e, &[]));
    }
    for e in &it.anims {
        let _ = writeln!(s, "  txanim {}", effect_line(e, &[]));
    }
    s
}

/// Timeline outline of a comp. detail: 0 = summary, 1 = items, 2 = items + all props.
pub fn timeline(p: &Project, comp_id: &str, active: &str, detail: u8) -> String {
    let Some(c) = p.comp(comp_id) else { return format!("no comp '{comp_id}'") };
    let mut s = comp_header(p, c, active);
    s.push('\n');
    let mut tracks: Vec<&Track> = c.vtracks().collect();
    tracks.reverse(); // top track first, like an NLE
    tracks.extend(c.atracks());
    for t in tracks {
        let mut flags = String::new();
        if t.hidden {
            flags.push_str(" hidden");
        }
        if t.locked {
            flags.push_str(" locked");
        }
        if t.muted {
            flags.push_str(" muted");
        }
        if t.solo {
            flags.push_str(" solo");
        }
        if t.vol != 0.0 {
            let _ = write!(flags, " vol={}", fmt_num(t.vol));
        }
        if let Some(d) = &t.duck {
            let _ = write!(flags, " duck={}by{}", fmt_num(d.amount), d.by);
        }
        if !t.afx.is_empty() {
            let _ = write!(flags, " afx:{}", t.afx.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(","));
        }
        match detail {
            0 => {
                if t.items.is_empty() {
                    let _ = writeln!(s, "{}{}: empty", t.id, flags);
                } else {
                    let _ = writeln!(s, "{}{}: {} item(s) {}-{}", t.id, flags, t.items.len(), fmt_t(t.items.first().unwrap().start), fmt_t(t.end()));
                }
            }
            1 => {
                if t.items.is_empty() {
                    let _ = writeln!(s, "{}{}: -", t.id, flags);
                    continue;
                }
                let parts: Vec<String> = t.items.iter().map(|i| item_line(p, i)).collect();
                let _ = writeln!(s, "{}{}: {}", t.id, flags, parts.join(" | "));
            }
            _ => {
                let _ = writeln!(s, "{}{}:", t.id, flags);
                for i in &t.items {
                    for (n, l) in item_detail(p, i).lines().enumerate() {
                        let _ = writeln!(s, "{}{}", if n == 0 { "  " } else { "  " }, l);
                    }
                }
            }
        }
    }
    if !c.markers.is_empty() {
        let ms: Vec<String> = c.markers.iter().take(64).map(|m| if m.name.is_empty() { fmt_t(m.t) } else { format!("{}:{}", fmt_t(m.t), m.name) }).collect();
        let more = if c.markers.len() > 64 { format!(" …+{}", c.markers.len() - 64) } else { String::new() };
        let _ = writeln!(s, "markers: {}{more}", ms.join(" "));
    }
    let m = &c.master;
    if m.vol != 0.0 || m.lufs.is_some() || m.limiter || !m.afx.is_empty() {
        let _ = writeln!(
            s,
            "master: vol={} lufs={} limiter={}{}",
            fmt_num(m.vol),
            m.lufs.map(fmt_num).unwrap_or("off".into()),
            m.limiter,
            if m.afx.is_empty() { String::new() } else { format!(" afx:{}", m.afx.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(",")) }
        );
    }
    s.trim_end().to_string()
}

pub fn media_list(p: &Project, detail: u8) -> String {
    if p.media.is_empty() {
        return "no media (import files with the media tool)".into();
    }
    let mut s = String::new();
    for m in &p.media {
        let _ = write!(s, "{} {} {} {}", m.id, m.name, kind_s(m.kind), media_summary(m));
        if detail >= 2 {
            let _ = write!(s, " path={}", m.path);
        }
        let analyses: Vec<&str> = p.analysis.keys().filter_map(|k| k.strip_prefix(&format!("{}:", m.id))).collect();
        if !analyses.is_empty() {
            let _ = write!(s, " [analyzed: {}]", analyses.join(","));
        }
        s.push('\n');
    }
    s.trim_end().to_string()
}

pub fn comps_list(p: &Project, active: &str) -> String {
    p.comps.iter().map(|c| comp_header(p, c, active)).collect::<Vec<_>>().join("\n")
}

pub fn project_summary(p: &Project, path: Option<&str>, active: &str, rev: u64) -> String {
    let main = p.main_comp();
    let mut s = format!("project '{}' rev={} media={} comps={}", p.name, rev, p.media.len(), p.comps.len());
    if let Some(path) = path {
        let _ = write!(s, " file={path}");
    }
    if let Some(m) = main {
        let _ = write!(s, "\n{}", comp_header(p, m, active));
    }
    s
}
