//! `node`: DaVinci Resolve's node editor as script lines.
//!
//! A clip's color effects are its node graph. Consecutive effects with the
//! same `node` label are one corrector node: its tools (primaries, curves,
//! warper ...) run together and are keyed once by the node's window and
//! qualifier, and blended by the node's mix (key output gain). An unlabeled
//! effect is a node of its own. Nodes are numbered 1, 2, ... in order.
//!
//! ```text
//! node c1 [list]                                         the graph: numbers, tools, keys, mix
//! node c1 add [serial|parallel|layer|outside] [after=N] [name=Skin] [composite=screen]
//! node c1 N <tool> [params]                              add or set a tool in node N (primaries gammaR=.02, curves luma='...', ...)
//! node c1 N window=m1+m2 | window add circle|linear|polygon|curve|gradient [props] | window none
//! node c1 N qualify hsl|rgb|luma|3d [params]             qualifier (key) for the node, ANDed with its windows
//! node c1 N mix=0.7 | off | on | del | reset | name=Skin | serial | parallel | layer [composite=] | up | down
//! node c1 N track [mode=pos|rotation|similarity|affine|perspective]   track the node's windows
//! ```

use super::exec::{Exec, A};
use super::exec_props::{SubKind, Target};
use crate::model::anim::Anim;
use crate::model::project::*;
use crate::model::schema;
use crate::model::value::{fmt_num, Val};
use anyhow::{anyhow, bail, Result};

/// One node: its label (None for a lone unlabeled effect) and effect indexes in `fx`.
#[derive(Clone, Debug)]
pub(crate) struct NodeRef {
    pub label: Option<String>,
    pub fx: Vec<usize>,
}

fn label_of(e: &Effect) -> Option<String> {
    e.props.get("node").map(|a| a.v.str()).filter(|s| !s.trim().is_empty())
}

/// True for effects that can sit in a node (in-place native effects).
fn nodable(e: &Effect) -> bool {
    schema::effect(&e.kind).is_some_and(schema::windowable)
}

/// The item's nodes in order (grouped as the renderer groups them).
pub(crate) fn nodes(it: &Item) -> Vec<NodeRef> {
    let mut out: Vec<NodeRef> = vec![];
    let mut prev_label: Option<String> = None;
    let mut prev_idx: Option<usize> = None;
    for (i, e) in it.fx.iter().enumerate() {
        if !nodable(e) {
            prev_label = None;
            prev_idx = None;
            continue;
        }
        let label = label_of(e);
        let joins = label.is_some() && label == prev_label && prev_idx == Some(i.wrapping_sub(1));
        if joins {
            if let Some(n) = out.last_mut() {
                n.fx.push(i);
            }
        } else {
            out.push(NodeRef { label: label.clone(), fx: vec![i] });
        }
        prev_label = label;
        prev_idx = Some(i);
    }
    out
}

/// How a node joins the chain, from its first effect.
fn conn(e: &Effect) -> &'static str {
    if e.props.get("parallel").is_some_and(|a| a.v.bool()) {
        "parallel"
    } else if e.props.get("layer").is_some_and(|a| a.v.bool()) {
        "layer"
    } else {
        "serial"
    }
}

/// One line per node: `2 Skin (parallel) [primaries curves] key m2+m3*m4 mix 0.8`.
pub(crate) fn describe(it: &Item) -> Vec<String> {
    nodes(it)
        .iter()
        .enumerate()
        .map(|(k, n)| {
            let first = &it.fx[n.fx[0]];
            let tools: Vec<String> = n.fx.iter().map(|&i| {
                let e = &it.fx[i];
                let mut s = format!("{}.{}", e.id, e.kind);
                if !e.on {
                    s = format!("({s} off)");
                }
                s
            }).collect();
            let key = n.fx.iter().map(|&i| it.fx[i].props.get("mask").map(|a| a.v.str()).unwrap_or_default()).find(|s| !s.trim().is_empty());
            let mix = first.props.get("mix").map(|a| a.v.num()).filter(|m| *m < 0.999);
            let c = conn(first);
            let comp = if c == "layer" { first.props.get("composite").map(|a| format!(" {}", a.v.str())).unwrap_or_default() } else { String::new() };
            format!(
                "{} {}{} [{}]{}{}",
                k + 1,
                n.label.clone().unwrap_or_else(|| "-".into()),
                if c == "serial" { String::new() } else { format!(" ({c}{comp})") },
                tools.join(" "),
                key.map(|k| format!(" key {k}")).unwrap_or_default(),
                mix.map(|m| format!(" mix {}", fmt_num(m))).unwrap_or_default()
            )
        })
        .collect()
}

/// A label no node of the item uses yet: N1, N2, ...
fn new_label(it: &Item) -> String {
    let used: Vec<String> = it.fx.iter().filter_map(label_of).collect();
    (1..).map(|k| format!("N{k}")).find(|l| !used.contains(l)).unwrap()
}

/// Masks referenced by an expression, split into shape windows and qualifiers.
fn split_key(it: &Item, expr: &str) -> Result<(Vec<String>, Vec<String>)> {
    let Some(e) = crate::render::matte::parse(expr)? else { return Ok((vec![], vec![])) };
    let (mut w, mut q) = (vec![], vec![]);
    for id in e.ids() {
        let Some(m) = it.masks.iter().find(|m| m.id.eq_ignore_ascii_case(id)) else { continue };
        let list = if schema::is_pixel_mask(&m.kind) { &mut q } else { &mut w };
        if !list.contains(&m.id) {
            list.push(m.id.clone());
        }
    }
    Ok((w, q))
}

/// The canonical node key: (window + window ...) * qualifier * ...
fn join_key(w: &[String], q: &[String]) -> String {
    let wins = match w.len() {
        0 => String::new(),
        1 => w[0].clone(),
        _ => format!("({})", w.join("+")),
    };
    let mut parts: Vec<String> = vec![];
    if !wins.is_empty() {
        parts.push(wins);
    }
    parts.extend(q.iter().cloned());
    parts.join("*")
}

impl<'a> Exec<'a> {
    /// `node <clip> ...`
    pub fn c_node(&mut self, a: &mut A) -> Result<()> {
        let usage = "node <clip> [list] | node <clip> add [serial|parallel|layer|outside] [after=N] [name=] | node <clip> <N> <tool> [params] | node <clip> <N> window=<expr>|window add <shape> [props] | node <clip> <N> qualify hsl|rgb|luma|3d [params] | node <clip> <N> mix=|off|on|del|reset|name=|serial|parallel|layer|up|down|track";
        let spec = a.shift().ok_or_else(|| anyhow!(usage))?;
        // `c1.2` or `c1.n2` as well as `c1 2`
        let (item, num) = match spec.split_once('.') {
            Some((i, n)) => (i.to_string(), Some(n.trim_start_matches(['n', 'N']).to_string())),
            None => (spec.clone(), None),
        };
        let id = self.item_id(&item)?;
        let l = self.relocate(&id)?;
        if !matches!(self.item_at(l).kind, ItemKind::Clip | ItemKind::Adjust | ItemKind::Comp | ItemKind::Solid | ItemKind::Text | ItemKind::Shape) {
            bail!("{id} has no picture to grade");
        }
        let num = num.or_else(|| a.pos.first().filter(|s| s.parse::<usize>().is_ok()).cloned().inspect(|_| {
            a.pos.remove(0);
        }));
        let Some(num) = num else {
            let sub = a.shift().unwrap_or_else(|| "list".into()).to_ascii_lowercase();
            return match sub.as_str() {
                "list" | "ls" | "graph" | "show" => {
                    let lines = describe(self.item_at(l));
                    self.note(if lines.is_empty() { format!("{id} has no nodes yet: node {id} add") } else { format!("{id} nodes:\n{}", lines.join("\n")) });
                    Ok(())
                }
                "add" | "new" => self.node_add(&id, a),
                o => bail!("node {id} {o}? {usage}"),
            };
        };
        let n: usize = num.parse().map_err(|_| anyhow!("node numbers are 1, 2, ... (node {id} list)"))?;
        let count = nodes(self.item_at(l)).len();
        if n == 0 || n > count {
            bail!("{id} has {count} node(s); node {id} add makes one");
        }
        // a lone unlabeled effect becomes a labeled node before any node edit
        self.label_node(&id, n)?;
        let word = a.pos.first().map(|s| s.to_ascii_lowercase());
        match word.as_deref() {
            Some("off" | "on" | "bypass" | "enable") => {
                let on = matches!(word.as_deref(), Some("on" | "enable"));
                a.shift();
                self.node_each(&id, n, |e| e.on = on)?;
                self.note(format!("~{id} node {n} {}", if on { "on" } else { "off" }));
                Ok(())
            }
            Some("del" | "delete" | "rm" | "remove") => {
                a.shift();
                let l = self.relocate(&id)?;
                let idx = nodes(self.item_at(l))[n - 1].fx.clone();
                let it = self.item_mut_at(l);
                let mut k = 0;
                it.fx.retain(|_| {
                    let keep = !idx.contains(&k);
                    k += 1;
                    keep
                });
                self.note(format!("-{id} node {n} ({} tools; its masks stay)", idx.len()));
                Ok(())
            }
            Some("reset") => {
                a.shift();
                const KEEP: [&str; 6] = ["node", "mask", "mix", "parallel", "layer", "composite"];
                self.node_each(&id, n, |e| e.props.retain(|k, _| KEEP.contains(&k.as_str())))?;
                self.note(format!("~{id} node {n} reset (tools back to neutral; key and mix kept)"));
                Ok(())
            }
            Some("serial" | "parallel" | "layer") => {
                let c = a.shift().unwrap().to_ascii_lowercase();
                let comp = a.take(&["composite", "mode"]);
                if let Some(m) = &comp {
                    if !schema::COMPOSITE_MODES.contains(&m.to_ascii_lowercase().as_str()) {
                        bail!("composite modes: {}", schema::COMPOSITE_MODES.join(" "));
                    }
                }
                if n == 1 && c != "serial" {
                    bail!("node 1 has no node before it to share the input with; add a node first");
                }
                self.node_each(&id, n, |e| {
                    e.props.insert("parallel".into(), Anim::new(Val::B(c == "parallel")));
                    e.props.insert("layer".into(), Anim::new(Val::B(c == "layer")));
                    if let Some(m) = &comp {
                        e.props.insert("composite".into(), Anim::new(Val::S(m.to_ascii_lowercase())));
                    }
                })?;
                self.note(format!("~{id} node {n} {c}{}", comp.map(|m| format!(" composite={m}")).unwrap_or_default()));
                Ok(())
            }
            Some("up" | "down") => {
                let up = a.shift().unwrap().eq_ignore_ascii_case("up");
                self.node_move(&id, n, up)
            }
            Some("window" | "windows" | "win") => {
                a.shift();
                self.node_window(&id, n, a)
            }
            Some("qualify" | "qualifier" | "key") => {
                a.shift();
                let kind = a.shift().ok_or_else(|| anyhow!("node {id} {n} qualify hsl|rgb|luma|3d [params]"))?;
                self.node_add_mask(&id, n, &kind, a, true)
            }
            Some("track") => {
                a.shift();
                let mode = a.take(&["mode", "m"]).unwrap_or_else(|| "similarity".into());
                let l = self.relocate(&id)?;
                let it = self.item_at(l).clone();
                let key = nodes(&it)[n - 1].fx.iter().map(|&i| it.fx[i].props.get("mask").map(|a| a.v.str()).unwrap_or_default()).find(|s| !s.trim().is_empty()).unwrap_or_default();
                let (wins, _) = split_key(&it, &key)?;
                if wins.is_empty() {
                    bail!("node {n} of {id} has no window to track (node {id} {n} window add circle ...)");
                }
                for w in wins {
                    for c in super::lex::parse_script(&format!("track {id}.{w} mode={mode}"))? {
                        self.cmd(&c)?;
                    }
                }
                Ok(())
            }
            _ => {
                // key=value on the node, or a tool with params
                let kvs_first = a.pos.is_empty();
                if kvs_first {
                    let kvs = a.rest();
                    return self.node_set(&id, n, &kvs);
                }
                let tool = a.shift().unwrap();
                self.node_tool(&id, n, &tool, a)
            }
        }
    }

    /// Runs `f` on every effect of node `n`.
    fn node_each(&mut self, id: &str, n: usize, mut f: impl FnMut(&mut Effect)) -> Result<()> {
        let l = self.relocate(id)?;
        let idx = nodes(self.item_at(l)).get(n - 1).map(|x| x.fx.clone()).ok_or_else(|| anyhow!("{id} has no node {n}"))?;
        let it = self.item_mut_at(l);
        for i in idx {
            f(&mut it.fx[i]);
        }
        Ok(())
    }

    /// Gives node `n` a label if it is a lone unlabeled effect.
    fn label_node(&mut self, id: &str, n: usize) -> Result<()> {
        let l = self.relocate(id)?;
        let it = self.item_at(l);
        let nd = nodes(it)[n - 1].clone();
        if nd.label.is_some() {
            return Ok(());
        }
        let label = new_label(it);
        let it = self.item_mut_at(l);
        it.fx[nd.fx[0]].props.insert("node".into(), Anim::new(Val::S(label)));
        Ok(())
    }

    /// `node <clip> add [serial|parallel|layer|outside] [after=N] [name=] [composite=]`
    fn node_add(&mut self, id: &str, a: &mut A) -> Result<()> {
        let kind = a.shift().map(|s| s.to_ascii_lowercase()).unwrap_or_else(|| "serial".into());
        if !matches!(kind.as_str(), "serial" | "parallel" | "layer" | "outside" | "corrector") {
            bail!("node {id} add serial|parallel|layer|outside");
        }
        let l = self.relocate(id)?;
        let it = self.item_at(l).clone();
        let list = nodes(&it);
        let after: usize = match a.take(&["after", "to"]) {
            Some(s) => s.trim_start_matches(['n', 'N']).parse().map_err(|_| anyhow!("after=<node number>"))?,
            None => list.len(),
        };
        if after > list.len() {
            bail!("{id} has {} node(s)", list.len());
        }
        if matches!(kind.as_str(), "parallel" | "layer") && after == 0 {
            bail!("a {kind} node shares the input of the node before it; add it after node 1 or later");
        }
        let name = a.take(&["name", "label", "as"]);
        let label = match &name {
            Some(nm) => {
                if it.fx.iter().filter_map(label_of).any(|l| l.eq_ignore_ascii_case(nm)) {
                    bail!("{id} already has a node named {nm}");
                }
                nm.clone()
            }
            None => new_label(&it),
        };
        let mut props = Props::new();
        props.insert("node".into(), Anim::new(Val::S(label.clone())));
        match kind.as_str() {
            "parallel" => {
                props.insert("parallel".into(), Anim::new(Val::B(true)));
            }
            "layer" => {
                props.insert("layer".into(), Anim::new(Val::B(true)));
                let c = a.take(&["composite", "mode"]).unwrap_or_else(|| "normal".into()).to_ascii_lowercase();
                if !schema::COMPOSITE_MODES.contains(&c.as_str()) {
                    bail!("composite modes: {}", schema::COMPOSITE_MODES.join(" "));
                }
                props.insert("composite".into(), Anim::new(Val::S(c)));
            }
            "outside" => {
                // the inverse key of the node it follows (Resolve's Add Outside Node)
                let base = list.get(after.wrapping_sub(1)).ok_or_else(|| anyhow!("an outside node follows a node with a window"))?;
                let key = base.fx.iter().map(|&i| it.fx[i].props.get("mask").map(|a| a.v.str()).unwrap_or_default()).find(|s| !s.trim().is_empty()).ok_or_else(|| anyhow!("node {after} of {id} has no window or qualifier to be outside of"))?;
                props.insert("mask".into(), Anim::new(Val::S(format!("-({key})"))));
            }
            _ => {}
        }
        // the new node's tool: Resolve's corrector starts with neutral primaries
        let pos = if after == 0 { list.first().map(|n| n.fx[0]).unwrap_or(it.fx.len()) } else { list[after - 1].fx.last().map(|i| i + 1).unwrap_or(it.fx.len()) };
        let itm = self.item_mut_at(l);
        let fid = itm.next_sub("f");
        itm.fx.insert(pos, Effect { id: fid.clone(), kind: "primaries".into(), on: true, props });
        let rest = a.rest();
        if !rest.is_empty() {
            self.set_sub(&Target { item: id.to_string(), sub: Some((SubKind::Fx, fid.clone())) }, &rest)?;
        }
        let no = after + 1;
        self.note(format!("+{id} node {no} {label} ({kind}): {id}.{fid} primaries. Grade it: node {id} {no} primaries gainR=1.03 | node {id} {no} curves luma='...' | node {id} {no} window add circle"));
        Ok(())
    }

    /// `node <clip> N key=value ...`: node-level settings on every tool.
    fn node_set(&mut self, id: &str, n: usize, kvs: &[(String, String)]) -> Result<()> {
        if kvs.is_empty() {
            bail!("node {id} {n} needs a tool (primaries, curves ...) or key=value (window=, mix=, name=, composite=)");
        }
        let l = self.relocate(id)?;
        let it = self.item_at(l).clone();
        let idx = nodes(&it)[n - 1].fx.clone();
        for (k, v) in kvs {
            match k.to_ascii_lowercase().as_str() {
                "name" | "label" => {
                    if it.fx.iter().enumerate().any(|(i, e)| !idx.contains(&i) && label_of(e).is_some_and(|x| x.eq_ignore_ascii_case(v))) {
                        bail!("{id} already has a node named {v}");
                    }
                    let v = v.clone();
                    self.node_each(id, n, |e| {
                        e.props.insert("node".into(), Anim::new(Val::S(v.clone())));
                    })?;
                }
                "window" | "key" | "mask" | "windows" => {
                    let expr = if matches!(v.to_ascii_lowercase().as_str(), "none" | "off" | "") { String::new() } else { v.clone() };
                    if !expr.is_empty() {
                        super::exec_props::check_mask_expr(&it, id, &expr)?;
                    }
                    self.node_each(id, n, |e| {
                        e.props.insert("mask".into(), Anim::new(Val::S(expr.clone())));
                    })?;
                }
                "mix" | "keygain" | "opacity" => {
                    let m: f64 = v.trim_end_matches('%').parse::<f64>().map(|x| if v.ends_with('%') { x / 100.0 } else { x }).map_err(|_| anyhow!("mix=0..1"))?;
                    let m = m.clamp(0.0, 1.0);
                    self.node_each(id, n, |e| {
                        e.props.insert("mix".into(), Anim::new(Val::N(m)));
                    })?;
                }
                "composite" => {
                    let c = v.to_ascii_lowercase();
                    if !schema::COMPOSITE_MODES.contains(&c.as_str()) {
                        bail!("composite modes: {}", schema::COMPOSITE_MODES.join(" "));
                    }
                    self.node_each(id, n, |e| {
                        e.props.insert("composite".into(), Anim::new(Val::S(c.clone())));
                    })?;
                }
                o => bail!("node {id} {n} {o}=? (window= mix= name= composite=, or a tool: node {id} {n} primaries {o}=...)"),
            }
            self.note(format!("~{id} node {n} {k}={v}"));
        }
        Ok(())
    }

    /// `node <clip> N <tool> [params]`: sets the node's tool of that kind,
    /// adding it to the node (after its other tools) if it isn't there.
    fn node_tool(&mut self, id: &str, n: usize, tool: &str, a: &mut A) -> Result<()> {
        let def = schema::effect(tool).ok_or_else(|| anyhow!("unknown tool '{tool}' (primaries logwheels hdrwheels colorwheels curves huecurves colorwarper colorslice lut cst secondary channelmixer blur sharpen ...; help grade)"))?;
        if !schema::windowable(def) {
            bail!("{} can't go in a node (it changes geometry or time)", def.name);
        }
        let l = self.relocate(id)?;
        let it = self.item_at(l).clone();
        let nd = nodes(&it)[n - 1].clone();
        // key and mix belong to the node, not to one of its tools
        let (node_kv, kvs): (Vec<(String, String)>, Vec<(String, String)>) =
            a.rest().into_iter().partition(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "mix" | "mask" | "window" | "key" | "composite" | "name" | "label"));
        if let Some(&i) = nd.fx.iter().find(|&&i| it.fx[i].kind == def.name) {
            let fid = it.fx[i].id.clone();
            if !kvs.is_empty() {
                self.set_sub(&Target { item: id.to_string(), sub: Some((SubKind::Fx, fid.clone())) }, &kvs)?;
            }
            if !node_kv.is_empty() {
                self.node_set(id, n, &node_kv)?;
            }
            self.note(format!("~{id} node {n} {} ({id}.{fid})", def.name));
            return Ok(());
        }
        // new tool: shares the node's label, key, mix and connection
        let first = &it.fx[nd.fx[0]];
        let mut props = Props::new();
        for k in ["node", "mask", "mix", "parallel", "layer", "composite"] {
            if let Some(v) = first.props.get(k) {
                props.insert(k.to_string(), v.clone());
            }
        }
        if def.name == "lut" {
            let f = kvs.iter().find(|(k, _)| k.eq_ignore_ascii_case("file") || k.eq_ignore_ascii_case("path")).map(|(_, v)| v.clone()).unwrap_or_default();
            if f.is_empty() || !std::path::Path::new(&f).exists() {
                bail!("lut needs file=<existing .cube file>");
            }
        }
        let pos = nd.fx.last().map(|i| i + 1).unwrap_or(it.fx.len());
        let itm = self.item_mut_at(l);
        let fid = itm.next_sub("f");
        itm.fx.insert(pos, Effect { id: fid.clone(), kind: def.name.into(), on: true, props });
        if !kvs.is_empty() {
            self.set_sub(&Target { item: id.to_string(), sub: Some((SubKind::Fx, fid.clone())) }, &kvs)?;
        }
        if !node_kv.is_empty() {
            self.node_set(id, n, &node_kv)?;
        }
        self.note(format!("+{id} node {n} {} ({id}.{fid})", def.name));
        Ok(())
    }

    /// `node <clip> N window ...`
    fn node_window(&mut self, id: &str, n: usize, a: &mut A) -> Result<()> {
        let sub = a.shift().map(|s| s.to_ascii_lowercase());
        match sub.as_deref() {
            Some("add" | "new") => {
                let kind = a.shift().ok_or_else(|| anyhow!("node {id} {n} window add circle|linear|polygon|curve|gradient [props]"))?;
                self.node_add_mask(id, n, &kind, a, false)
            }
            Some("none" | "off" | "clear") => self.node_set(id, n, &[("window".into(), "none".into())]),
            Some(expr) => self.node_set(id, n, &[("window".into(), expr.to_string())]),
            None => {
                let kvs = a.rest();
                match kvs.iter().find(|(k, _)| k.eq_ignore_ascii_case("window") || k.eq_ignore_ascii_case("key")) {
                    Some((_, v)) => self.node_set(id, n, &[("window".into(), v.clone())]),
                    None => bail!("node {id} {n} window add <shape> | window=m1+m2 | window none"),
                }
            }
        }
    }

    /// Makes a window (shape) or qualifier mask on the clip and adds it to
    /// node `n`'s key: windows join as a union, qualifiers intersect it.
    fn node_add_mask(&mut self, id: &str, n: usize, kind: &str, a: &mut A, qualifier: bool) -> Result<()> {
        let k = kind.to_ascii_lowercase();
        let is_q = matches!(k.as_str(), "hsl" | "rgb" | "luma" | "3d" | "key3d" | "color" | "colorkey");
        if qualifier && !is_q {
            bail!("qualifiers: hsl rgb luma 3d (windows go in with: node {id} {n} window add {k})");
        }
        if !qualifier && is_q {
            bail!("{k} is a qualifier: node {id} {n} qualify {k} ...");
        }
        let l = self.relocate(id)?;
        let it = self.item_at(l).clone();
        let key = nodes(&it)[n - 1].fx.iter().map(|&i| it.fx[i].props.get("mask").map(|a| a.v.str()).unwrap_or_default()).find(|s| !s.trim().is_empty()).unwrap_or_default();
        if key.contains(['-', '!', '~']) {
            bail!("node {n}'s key '{key}' has an outside (-) part; set it whole with node {id} {n} window=<expr>");
        }
        let (mut wins, mut quals) = split_key(&it, &key)?;
        // the mask itself, through the mask command (Resolve window names work there)
        let rest: Vec<String> = a.rest().iter().map(|(k, v)| format!("{k}={}", super::lex::quote(v))).collect();
        let before: Vec<String> = it.masks.iter().map(|m| m.id.clone()).collect();
        let script = format!("mask {id} {k} {} window", rest.join(" "));
        for c in super::lex::parse_script(&script)? {
            self.cmd(&c)?;
        }
        let l = self.relocate(id)?;
        let mid = self.item_at(l).masks.iter().map(|m| m.id.clone()).find(|m| !before.contains(m)).ok_or_else(|| anyhow!("couldn't make the mask"))?;
        if is_q {
            quals.push(mid.clone());
        } else {
            wins.push(mid.clone());
        }
        let expr = join_key(&wins, &quals);
        self.node_set(id, n, &[("window".into(), expr.clone())])?;
        self.note(format!("  node {n} key is now {expr} (look at it: preview view=matte id={id}.{expr})"));
        Ok(())
    }

    /// Moves node `n` before the node above it or after the one below.
    fn node_move(&mut self, id: &str, n: usize, up: bool) -> Result<()> {
        let l = self.relocate(id)?;
        let list = nodes(self.item_at(l));
        let other = if up { n.checked_sub(1).filter(|x| *x >= 1) } else { Some(n + 1).filter(|x| *x <= list.len()) };
        let Some(o) = other else { bail!("node {n} is already {}", if up { "first" } else { "last" }) };
        let (a, b) = if up { (o, n) } else { (n, o) };
        // swap the two adjacent groups (a before b)
        let (ra, rb) = (list[a - 1].fx.clone(), list[b - 1].fx.clone());
        let it = self.item_mut_at(l);
        let ea: Vec<Effect> = ra.iter().map(|&i| it.fx[i].clone()).collect();
        let eb: Vec<Effect> = rb.iter().map(|&i| it.fx[i].clone()).collect();
        let (lo, hi) = (*ra.first().unwrap(), *rb.last().unwrap());
        // anything between the two groups (non-node effects) stays between them
        let mid: Vec<Effect> = (lo..=hi).filter(|i| !ra.contains(i) && !rb.contains(i)).map(|i| it.fx[i].clone()).collect();
        let mut seq = eb;
        seq.extend(mid);
        seq.extend(ea);
        let _old: Vec<Effect> = it.fx.splice(lo..=hi, seq).collect();
        self.note(format!("~{id} node {n} moved {}", if up { "up" } else { "down" }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_join_windows_and_qualifiers() {
        assert_eq!(join_key(&["m1".into()], &[]), "m1");
        assert_eq!(join_key(&["m1".into(), "m3".into()], &["m2".into()]), "(m1+m3)*m2");
        assert_eq!(join_key(&[], &["m2".into()]), "m2");
    }
}
