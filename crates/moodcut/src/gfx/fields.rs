//! Editable fields of HTML motion graphics.
//!
//! A graphic exposes the values people may change (texts, colors, numbers,
//! choices, media) as `{{name|default}}` placeholders, optionally described by a
//! `<script type="application/json" id="mc-fields">` manifest (labels, ranges,
//! choices). The editor builds controls from them, the way After Effects'
//! Essential Properties work, and the AI changes them with `vars` instead of
//! rewriting the page, so people and the AI edit the same values.
//!
//! Values are escaped for where each placeholder stands: HTML text (newlines
//! become `<br>`), attributes, CSS (inside or outside a string) and JavaScript
//! (inside a string literal, or as a JSON literal in code). What is written
//! inside `{{name|default}}` is the page's own code and goes in unchanged.
//!
//! [`make_editable`] turns the texts and colors of a page without fields into
//! fields whose defaults are the original values, so the render doesn't change.

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

// ------------------------------------------------------------------ a small HTML scanner

/// Content of a raw-text element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Raw {
    Script,
    Style,
    /// `<script type="application/json" id="mc-fields">`
    Manifest,
}

#[derive(Debug)]
pub(crate) struct Attr {
    pub name: String,
    /// Byte range of the value (without quotes).
    pub val: Option<(usize, usize)>,
}

#[derive(Debug)]
pub(crate) enum Tok {
    Text(usize, usize),
    Open { start: usize, end: usize, name: String, attrs: Vec<Attr>, closed: bool },
    Close { start: usize, end: usize, name: String },
    Comment(usize, usize),
    /// Byte range of a `<script>` / `<style>` element's content.
    Raw(usize, usize, Raw),
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

/// Index after the `>` that ends a tag (quoted attribute values may hold `>`).
fn tag_end(b: &[u8], mut i: usize) -> usize {
    let mut quote = 0u8;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return i + 1;
        }
        i += 1;
    }
    b.len()
}

fn parse_attrs(src: &str, mut i: usize, to: usize) -> Vec<Attr> {
    let b = src.as_bytes();
    let mut out = vec![];
    while i < to {
        while i < to && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let ns = i;
        while i < to && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        if i == ns {
            i += 1;
            continue;
        }
        let name = src[ns..i].to_ascii_lowercase();
        let mut j = i;
        while j < to && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let mut val = None;
        if j < to && b[j] == b'=' {
            j += 1;
            while j < to && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < to && (b[j] == b'"' || b[j] == b'\'') {
                let q = b[j];
                let vs = j + 1;
                let mut k = vs;
                while k < to && b[k] != q {
                    k += 1;
                }
                val = Some((vs, k));
                i = (k + 1).min(to);
            } else {
                let mut k = j;
                while k < to && !b[k].is_ascii_whitespace() && b[k] != b'>' {
                    k += 1;
                }
                val = Some((j, k));
                i = k;
            }
        }
        out.push(Attr { name, val });
    }
    out
}

/// Splits a page into text, tags, comments and raw `<script>` / `<style>` content.
pub(crate) fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let lower = src.to_ascii_lowercase();
    let n = b.len();
    let mut out = vec![];
    let (mut i, mut text) = (0usize, 0usize);
    let flush = |out: &mut Vec<Tok>, a: usize, z: usize| {
        if z > a {
            out.push(Tok::Text(a, z));
        }
    };
    while i < n {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if lower[i..].starts_with("<!--") {
            flush(&mut out, text, i);
            let end = lower[i + 4..].find("-->").map(|x| i + 4 + x + 3).unwrap_or(n);
            out.push(Tok::Comment(i, end));
            i = end;
            text = i;
            continue;
        }
        let next = b.get(i + 1).copied().unwrap_or(0);
        if next == b'!' || next == b'?' {
            flush(&mut out, text, i);
            i = tag_end(b, i + 1);
            text = i;
            continue;
        }
        let closing = next == b'/';
        let ns = i + 1 + closing as usize;
        if ns >= n || !b[ns].is_ascii_alphabetic() {
            i += 1; // a lone '<' in text
            continue;
        }
        let mut j = ns;
        while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b':') {
            j += 1;
        }
        flush(&mut out, text, i);
        let name = lower[ns..j].to_string();
        let end = tag_end(b, j);
        if closing {
            out.push(Tok::Close { start: i, end, name });
            i = end;
            text = i;
            continue;
        }
        let gt = if end > j && b[end - 1] == b'>' { end - 1 } else { end };
        let attrs = parse_attrs(src, j, gt);
        let closed = gt > j && b[gt - 1] == b'/';
        let raw = match name.as_str() {
            "script" => Some(Raw::Script),
            "style" => Some(Raw::Style),
            _ => None,
        };
        let manifest = raw == Some(Raw::Script) && attrs.iter().any(|a| a.name == "id" && a.val.is_some_and(|(x, y)| src[x..y].trim() == "mc-fields"));
        out.push(Tok::Open { start: i, end, name: name.clone(), attrs, closed });
        i = end;
        text = i;
        if let Some(kind) = raw {
            // raw text runs to the closing tag (`<script/>` doesn't close itself in HTML)
            let close = format!("</{name}");
            let cend = lower[i..].find(&close).map(|x| i + x).unwrap_or(n);
            out.push(Tok::Raw(i, cend, if manifest { Raw::Manifest } else { kind }));
            i = cend;
            text = i;
        }
    }
    flush(&mut out, text, n);
    out
}

fn attr_val<'a>(src: &'a str, attrs: &[Attr], name: &str) -> Option<&'a str> {
    attrs.iter().find(|a| a.name == name).and_then(|a| a.val).map(|(x, y)| &src[x..y])
}

/// Where a placeholder stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ctx {
    Text,
    Attr,
    Css,
    Js,
    Manifest,
    Comment,
}

/// Sorted spans of everything that isn't plain text.
fn spans(toks: &[Tok]) -> Vec<(usize, usize, Ctx)> {
    let mut v: Vec<(usize, usize, Ctx)> = toks
        .iter()
        .filter_map(|t| match t {
            Tok::Open { start, end, .. } | Tok::Close { start, end, .. } => Some((*start, *end, Ctx::Attr)),
            Tok::Comment(a, z) => Some((*a, *z, Ctx::Comment)),
            Tok::Raw(a, z, Raw::Script) => Some((*a, *z, Ctx::Js)),
            Tok::Raw(a, z, Raw::Style) => Some((*a, *z, Ctx::Css)),
            Tok::Raw(a, z, Raw::Manifest) => Some((*a, *z, Ctx::Manifest)),
            Tok::Text(..) => None,
        })
        .collect();
    v.sort_by_key(|s| s.0);
    v
}

fn ctx_at(spans: &[(usize, usize, Ctx)], pos: usize) -> (Ctx, usize) {
    let i = spans.partition_point(|s| s.0 <= pos);
    match i.checked_sub(1).map(|k| spans[k]) {
        Some((a, z, c)) if pos < z => (c, a),
        _ => (Ctx::Text, 0),
    }
}

/// A `{{name|default}}` in the page.
pub(crate) struct Ph<'a> {
    pub start: usize,
    pub end: usize,
    pub name: &'a str,
    pub default: Option<&'a str>,
    pub ctx: Ctx,
    /// Start of the tag / raw element it stands in.
    pub span: usize,
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn placeholders<'a>(src: &'a str, spans: &[(usize, usize, Ctx)]) -> Vec<Ph<'a>> {
    let mut out = vec![];
    let mut pos = 0;
    while let Some(i) = src[pos..].find("{{").map(|x| x + pos) {
        let Some(j) = src[i + 2..].find("}}").map(|x| x + i + 2) else { break };
        let inner = &src[i + 2..j];
        let (name, default) = match inner.split_once('|') {
            Some((n, d)) => (n.trim(), Some(d)),
            None => (inner.trim(), None),
        };
        // only identifier-like names are placeholders (JS/CSS braces stay as they are)
        if !valid_name(name) {
            pos = i + 1;
            continue;
        }
        let (ctx, span) = ctx_at(spans, i);
        out.push(Ph { start: i, end: j + 2, name, default, ctx, span });
        pos = j + 2;
    }
    out
}

// ------------------------------------------------------------------ escaping

fn esc_html(s: &str, text: bool) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            '\r' => {}
            '\n' if text => o.push_str("<br>"),
            '\n' => o.push_str("&#10;"),
            c => o.push(c),
        }
    }
    o
}

fn css_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\'' => o.push_str("\\'"),
            '<' => o.push_str("\\3c "),
            '\n' => o.push_str("\\a "),
            '\r' => {}
            c => o.push(c),
        }
    }
    o
}

fn js_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\'' => o.push_str("\\'"),
            '`' => o.push_str("\\`"),
            '$' => o.push_str("\\$"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '<' => o.push_str("\\x3c"),
            '\u{2028}' => o.push_str("\\u2028"),
            '\u{2029}' => o.push_str("\\u2029"),
            c => o.push(c),
        }
    }
    o
}

/// A JSON string literal that is safe inside `<script>`.
pub(crate) fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()).replace('<', "\\u003c")
}

fn is_js_literal(v: &str) -> bool {
    matches!(v, "true" | "false" | "null") || (v.parse::<f64>().is_ok_and(f64::is_finite) && v.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | 'E')))
}

/// Is `pos` inside a quoted CSS string? Scans the declaration it stands in.
fn css_in_string(src: &str, from: usize, pos: usize) -> bool {
    let b = src.as_bytes();
    let mut a = pos;
    while a > from && !matches!(b[a - 1], b';' | b'{' | b'}') {
        a -= 1;
    }
    let (mut q, mut i) = (0u8, a);
    while i < pos {
        let c = b[i];
        if q != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == q {
                q = 0;
            }
        } else if c == b'"' || c == b'\'' {
            q = c;
        }
        i += 1;
    }
    q != 0
}

/// Is `pos` inside a JS string or template literal? Skips comments; regex literals aren't tracked.
fn js_in_string(src: &str, from: usize, pos: usize) -> bool {
    let b = src.as_bytes();
    let (mut q, mut i) = (0u8, from);
    while i < pos {
        let c = b[i];
        if q != 0 {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                q = 0;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < pos && b[i] != b'\n' {
                i += 1;
            }
            continue;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            match src[i + 2..].find("*/") {
                Some(x) => {
                    i += 2 + x + 2;
                    continue;
                }
                None => return false,
            }
        } else if c == b'"' || c == b'\'' || c == b'`' {
            q = c;
        }
        i += 1;
    }
    q != 0
}

/// A value from vars (data, not code) written where the placeholder stands.
fn encode(v: &str, ph: &Ph, src: &str) -> String {
    match ph.ctx {
        Ctx::Text | Ctx::Comment => esc_html(v, true),
        Ctx::Attr => esc_html(v, false),
        Ctx::Css if css_in_string(src, ph.span, ph.start) => css_str(v),
        // outside a string a value can't end the declaration or the stylesheet
        Ctx::Css => v.chars().filter(|c| !matches!(c, '<' | '>' | '{' | '}' | ';' | '\n' | '\r')).collect(),
        Ctx::Js if js_in_string(src, ph.span, ph.start) => js_str(v),
        Ctx::Js if is_js_literal(v.trim()) => v.trim().to_string(),
        Ctx::Js => json_str(v),
        Ctx::Manifest => v.to_string(),
    }
}

// ------------------------------------------------------------------ fields

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Longtext,
    Color,
    Number,
    Toggle,
    Select,
    Media,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Longtext => "longtext",
            Kind::Color => "color",
            Kind::Number => "number",
            Kind::Toggle => "toggle",
            Kind::Select => "select",
            Kind::Media => "media",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "text" | "string" | "str" | "line" => Kind::Text,
            "longtext" | "textarea" | "multiline" | "paragraph" => Kind::Longtext,
            "color" | "colour" => Kind::Color,
            "number" | "num" | "int" | "integer" | "float" | "range" | "slider" => Kind::Number,
            "toggle" | "bool" | "boolean" | "checkbox" | "switch" => Kind::Toggle,
            "select" | "choice" | "enum" | "dropdown" | "options" => Kind::Select,
            "media" | "image" | "video" | "img" | "file" => Kind::Media,
            _ => return None,
        })
    }
}

/// One value of a graphic that can be changed without touching its code.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub label: String,
    /// What renders while the field isn't set.
    pub default: String,
    /// The default as plain text (entities decoded, `<br>` as a newline), when that differs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// The brand kit color it follows by default (primary, accent, text, dark).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub brand: String,
    /// Where the page uses it: html, attr, css, js (none: read by scripts as MC.vars).
    pub used: Vec<&'static str>,
    #[serde(skip)]
    has_default: bool,
    /// The default came from the manifest (data, escaped like a value).
    #[serde(skip)]
    data_default: bool,
    #[serde(skip)]
    given: Option<Kind>,
}

impl Field {
    fn new(name: &str) -> Field {
        Field {
            name: name.to_string(),
            kind: Kind::Text,
            label: humanize(name),
            default: String::new(),
            plain: None,
            min: None,
            max: None,
            step: None,
            unit: String::new(),
            options: vec![],
            group: String::new(),
            help: String::new(),
            brand: String::new(),
            used: vec![],
            has_default: false,
            data_default: false,
            given: None,
        }
    }

    /// What it shows while unset.
    pub fn shown_default(&self) -> &str {
        self.plain.as_deref().unwrap_or(&self.default)
    }
}

/// The fields of a page, and problems with its manifest.
#[derive(Debug, Default)]
pub struct Parsed {
    pub fields: Vec<Field>,
    pub warnings: Vec<String>,
}

fn ctx_name(c: Ctx) -> &'static str {
    match c {
        Ctx::Text | Ctx::Comment => "html",
        Ctx::Attr => "attr",
        Ctx::Css => "css",
        Ctx::Js => "js",
        Ctx::Manifest => "manifest",
    }
}

/// "speaker_name" / "speakerName" -> "Speaker name".
fn humanize(name: &str) -> String {
    let mut words: Vec<String> = vec![];
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || c == '-' {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let s = words.join(" ");
    let mut ch = s.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().chain(ch).collect(),
        None => name.to_string(),
    }
}

pub fn parse(src: &str) -> Parsed {
    let toks = tokenize(src);
    let sp = spans(&toks);
    let phs = placeholders(src, &sp);
    parse_with(src, &toks, &phs)
}

fn parse_with(src: &str, toks: &[Tok], phs: &[Ph]) -> Parsed {
    let mut p = Parsed::default();
    for t in toks {
        if let Tok::Raw(a, z, Raw::Manifest) = t {
            manifest(&src[*a..*z], &mut p);
        }
    }
    let mut uses: BTreeMap<&str, BTreeSet<&'static str>> = BTreeMap::new();
    for ph in phs {
        if matches!(ph.ctx, Ctx::Comment | Ctx::Manifest) {
            continue;
        }
        uses.entry(ph.name).or_default().insert(ctx_name(ph.ctx));
        let existing = p.fields.iter().position(|f| f.name == ph.name);
        // in a script a bare {{x}} may be code (an object literal in a block): a field needs a default there
        if existing.is_none() && ph.ctx == Ctx::Js && ph.default.is_none() {
            continue;
        }
        let f = match existing {
            Some(i) => &mut p.fields[i],
            None => {
                p.fields.push(Field::new(ph.name));
                p.fields.last_mut().unwrap()
            }
        };
        if !f.has_default {
            if let Some(d) = ph.default {
                f.default = d.to_string();
                f.has_default = true;
            }
        }
    }
    for f in p.fields.iter_mut() {
        f.used = uses.get(f.name.as_str()).map(|s| s.iter().copied().collect()).unwrap_or_default();
        let (kind, unit) = infer(f);
        f.kind = f.given.unwrap_or(kind);
        if f.unit.is_empty() && f.kind == Kind::Number {
            f.unit = unit;
        }
        if f.kind == Kind::Select {
            if f.options.is_empty() {
                f.kind = Kind::Text;
            } else if !f.has_default {
                f.default = f.options[0].clone();
                f.has_default = true;
                f.data_default = true;
            }
        }
        f.brand = brand_of(&f.default).unwrap_or("").to_string();
        if matches!(f.kind, Kind::Text | Kind::Longtext) && !f.data_default && f.used.iter().all(|u| *u == "html") {
            f.plain = plain_text(&f.default);
            if f.plain.as_deref().is_some_and(|t| t.contains('\n')) {
                f.kind = Kind::Longtext;
            }
        }
    }
    p
}

fn manifest(text: &str, p: &mut Parsed) {
    let v: Value = match serde_json::from_str(text.trim()) {
        Ok(v) => v,
        Err(e) => {
            p.warnings.push(format!("the mc-fields manifest isn't valid JSON ({e}); its labels and types are ignored"));
            return;
        }
    };
    let entries: Vec<(String, Value)> = match v {
        Value::Object(m) => m.into_iter().collect(),
        Value::Array(a) => a
            .into_iter()
            .filter_map(|e| {
                let name = e.get("name")?.as_str()?.to_string();
                Some((name, e))
            })
            .collect(),
        _ => {
            p.warnings.push("the mc-fields manifest should be a JSON object: {\"name\": {\"label\": ..., \"type\": ...}}".into());
            return;
        }
    };
    for (name, e) in entries {
        if !valid_name(&name) {
            p.warnings.push(format!("mc-fields: '{name}' isn't a valid field name (letters, digits, _ and -)"));
            continue;
        }
        if p.fields.iter().any(|f| f.name == name) {
            continue;
        }
        let mut f = Field::new(&name);
        match &e {
            Value::String(label) => f.label = label.clone(),
            Value::Object(o) => {
                let s = |k: &str| {
                    o.get(k).and_then(|x| match x {
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        Value::Bool(b) => Some(b.to_string()),
                        _ => None,
                    })
                };
                if let Some(l) = s("label").or_else(|| s("title")) {
                    f.label = l;
                }
                if let Some(t) = s("type") {
                    match Kind::parse(&t) {
                        Some(k) => f.given = Some(k),
                        None => p.warnings.push(format!("mc-fields: {name} has an unknown type '{t}' (text longtext color number toggle select media)")),
                    }
                }
                if let Some(d) = s("default").or_else(|| s("value")) {
                    f.default = d;
                    f.has_default = true;
                    f.data_default = true;
                }
                f.min = o.get("min").and_then(Value::as_f64);
                f.max = o.get("max").and_then(Value::as_f64);
                f.step = o.get("step").and_then(Value::as_f64).filter(|s| *s > 0.0);
                f.unit = s("unit").unwrap_or_default();
                f.options = match o.get("options").or_else(|| o.get("choices")) {
                    Some(Value::Array(a)) => a
                        .iter()
                        .filter_map(|x| match x {
                            Value::String(s) => Some(s.clone()),
                            Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .collect(),
                    Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
                    _ => vec![],
                };
                f.group = s("group").unwrap_or_default();
                f.help = s("help").or_else(|| s("hint")).or_else(|| s("description")).unwrap_or_default();
            }
            _ => {}
        }
        p.fields.push(f);
    }
}

const NAMED_COLORS: &[&str] = &[
    "black", "white", "red", "green", "blue", "yellow", "orange", "purple", "pink", "gray", "grey", "silver", "gold", "navy", "teal", "cyan", "magenta", "maroon", "olive", "lime", "aqua", "fuchsia", "brown", "beige", "coral",
    "crimson", "indigo", "ivory", "khaki", "lavender", "salmon", "tomato", "turquoise", "violet", "transparent", "currentcolor",
];

/// `#hex`, rgb()/hsl()/… or a brand variable.
pub fn is_color(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    if let Some(h) = l.strip_prefix('#') {
        return matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit());
    }
    if ["rgb(", "rgba(", "hsl(", "hsla(", "hwb(", "lab(", "lch(", "oklab(", "oklch(", "color(", "color-mix("].iter().any(|f| l.starts_with(f)) && l.ends_with(')') {
        return true;
    }
    brand_of(&l).is_some()
}

/// The brand color a `var(--brand-…)` value refers to.
pub fn brand_of(s: &str) -> Option<&'static str> {
    let l = s.trim().to_ascii_lowercase();
    let rest = l.strip_prefix("var(--brand-")?;
    ["primary", "accent", "text", "dark"].into_iter().find(|k| rest.starts_with(k) && matches!(rest.as_bytes().get(k.len()), Some(b')' | b',' | b' ')))
}

fn name_says_color(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["color", "colour", "accent", "bg", "background", "fill", "stroke", "tint", "shade"].iter().any(|k| n.contains(k))
}

/// A number with an optional CSS unit: (value, unit).
pub fn num_unit(s: &str) -> Option<(f64, String)> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))).unwrap_or(s.len());
    let (n, u) = s.split_at(split);
    let v: f64 = n.parse().ok().filter(|v: &f64| v.is_finite())?;
    let unit = u.trim();
    const UNITS: &[&str] = &["", "px", "%", "s", "ms", "deg", "turn", "rad", "vh", "vw", "vmin", "vmax", "em", "rem", "ch", "ex", "fr", "x", "pt", "svh", "dvh", "lvh"];
    UNITS.contains(&unit.to_ascii_lowercase().as_str()).then(|| (v, unit.to_string()))
}

fn infer(f: &Field) -> (Kind, String) {
    let d = f.default.trim();
    if !f.options.is_empty() {
        return (Kind::Select, String::new());
    }
    if d.starts_with("mc://") {
        return (Kind::Media, String::new());
    }
    let named = NAMED_COLORS.contains(&d.to_ascii_lowercase().as_str()) && (name_says_color(&f.name) || (!f.used.is_empty() && f.used.iter().all(|u| *u == "css")));
    if is_color(d) || named {
        return (Kind::Color, String::new());
    }
    if d == "true" || d == "false" {
        return (Kind::Toggle, String::new());
    }
    if let Some((_, unit)) = num_unit(d) {
        return (Kind::Number, unit);
    }
    if d.contains('\n') || d.chars().count() > 80 {
        return (Kind::Longtext, String::new());
    }
    (Kind::Text, String::new())
}

/// Decodes the common HTML entities (unknown ones are kept).
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find(';').filter(|e| *e <= 10);
        let dec = end.and_then(|e| {
            let ent = &tail[1..e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => ent.strip_prefix('#').and_then(|n| if let Some(h) = n.strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok() } else { n.parse().ok() }).and_then(char::from_u32),
            };
            c.map(|c| (c, e + 1))
        });
        match dec {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A text default as people read it, when that differs from its HTML.
fn plain_text(d: &str) -> Option<String> {
    if !d.contains('&') && !d.contains('<') {
        return None;
    }
    let mut s = d.to_string();
    for br in ["<br>", "<br/>", "<br />", "<BR>", "<Br>"] {
        s = s.replace(br, "\n");
    }
    if s.contains('<') {
        return None;
    }
    let t = decode_entities(&s);
    (t != d).then_some(t)
}

/// Checks and tidies a value for a field: colors must be colors, numbers get the field's unit, toggles are true/false.
pub fn normalize(f: &Field, v: &str) -> Result<String, String> {
    let t = v.trim();
    Ok(match f.kind {
        Kind::Color => {
            if is_color(t) || (!t.is_empty() && t.chars().all(|c| c.is_ascii_alphabetic())) {
                t.to_string()
            } else {
                return Err(format!("'{t}' isn't a color (#rrggbb, rgb(), a CSS color name or var(--brand-accent))"));
            }
        }
        // a bare number gets the unit the default is written with (`{{y|10px}}`); a unit that only
        // labels the field (`{{lift|12}}%` with "unit": "%") is already in the page
        Kind::Number => match num_unit(t) {
            Some((_, u)) if u.is_empty() => format!("{t}{}", num_unit(&f.default).map(|d| d.1).unwrap_or_default()),
            Some(_) => t.to_string(),
            None => return Err(format!("'{t}' isn't a number{}", if f.unit.is_empty() { String::new() } else { format!(" (in {})", f.unit) })),
        },
        Kind::Toggle => match t.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => "true".into(),
            "false" | "0" | "no" | "off" | "" => "false".into(),
            _ => return Err(format!("'{t}' isn't true or false")),
        },
        Kind::Select => {
            if f.options.is_empty() || f.options.iter().any(|o| o == t) {
                t.to_string()
            } else {
                return Err(format!("'{t}' isn't one of: {}", f.options.join(", ")));
            }
        }
        Kind::Media => {
            let id_like = t.len() > 1 && t.starts_with('m') && t[1..].chars().all(|c| c.is_ascii_digit());
            if id_like {
                format!("mc://{t}")
            } else {
                t.to_string()
            }
        }
        Kind::Text | Kind::Longtext => v.replace("\r\n", "\n"),
    })
}

/// One line per graphic for the AI: `title text="Big launch" | accent color=brand accent | lift number=12% (4..40)`.
pub fn summary(fields: &[Field], vars: &BTreeMap<String, String>) -> String {
    fields
        .iter()
        .map(|f| {
            let v = vars.get(&f.name).map(|s| s.as_str()).unwrap_or_else(|| f.shown_default());
            let shown = match brand_of(v) {
                Some(b) => format!("brand {b}"),
                None if matches!(f.kind, Kind::Text | Kind::Longtext) => {
                    let one: String = v.replace('\n', "⏎").chars().take(40).collect();
                    format!("\"{one}{}\"", if v.chars().count() > 40 { "…" } else { "" })
                }
                None => v.to_string(),
            };
            let mut s = format!("{} {}={shown}", f.name, f.kind.as_str());
            match (f.min, f.max) {
                (Some(a), Some(b)) => s.push_str(&format!(" ({}..{})", crate::model::value::fmt_num(a), crate::model::value::fmt_num(b))),
                (Some(a), None) => s.push_str(&format!(" (>= {})", crate::model::value::fmt_num(a))),
                (None, Some(b)) => s.push_str(&format!(" (<= {})", crate::model::value::fmt_num(b))),
                _ => {}
            }
            if f.kind == Kind::Select {
                s.push_str(&format!(" ({})", f.options.join("|")));
            }
            s
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Does the page time itself from the graphic's length (so changing it keeps the outro)?
pub fn uses_duration(src: &str) -> bool {
    src.contains("--mc-dur") || src.contains("MC.duration")
}

// ------------------------------------------------------------------ filling

pub struct Filled {
    pub html: String,
    /// Every field's value (set, else its default) plus the other vars: `MC.vars` for scripts.
    pub vars: BTreeMap<String, String>,
    /// Placeholders with no value and no default (they render empty).
    pub missing: Vec<String>,
}

/// Replaces the placeholders with their values (escaped for where they stand) or defaults.
pub fn fill(src: &str, vars: &BTreeMap<String, String>) -> Filled {
    let toks = tokenize(src);
    let sp = spans(&toks);
    let phs = placeholders(src, &sp);
    let parsed = parse_with(src, &toks, &phs);
    let mut out = String::with_capacity(src.len() + 64);
    let mut last = 0;
    let mut missing: Vec<String> = vec![];
    for ph in &phs {
        if ph.ctx == Ctx::Manifest {
            continue;
        }
        out.push_str(&src[last..ph.start]);
        let field = parsed.fields.iter().find(|f| f.name == ph.name);
        if let Some(v) = vars.get(ph.name) {
            out.push_str(&encode(v, ph, src));
        } else if let Some(d) = ph.default {
            out.push_str(d);
        } else if let Some(f) = field.filter(|f| f.has_default) {
            if f.data_default {
                out.push_str(&encode(&f.default, ph, src));
            } else {
                out.push_str(&f.default);
            }
        } else if ph.ctx == Ctx::Js {
            // an unknown {{x}} inside a script may be real code
            out.push_str(&src[ph.start..ph.end]);
        } else if !missing.iter().any(|m| m == ph.name) {
            missing.push(ph.name.to_string());
        }
        last = ph.end;
    }
    out.push_str(&src[last..]);
    let mut eff: BTreeMap<String, String> = parsed.fields.iter().filter(|f| f.has_default).map(|f| (f.name.clone(), f.shown_default().to_string())).collect();
    eff.extend(vars.iter().map(|(k, v)| (k.clone(), v.clone())));
    Filled { html: out, vars: eff, missing }
}

// ------------------------------------------------------------------ making a page editable

struct TextRun {
    start: usize,
    end: usize,
    /// Tag, id and class of the element holding it.
    parent: Option<(String, Option<String>, Option<String>)>,
}

/// Visible text runs (trimmed) with the element they're in.
fn text_runs(src: &str, toks: &[Tok]) -> Vec<TextRun> {
    let mut stack: Vec<(String, Option<String>, Option<String>)> = vec![];
    let mut out = vec![];
    for t in toks {
        match t {
            Tok::Open { name, attrs, closed, .. } => {
                if name == "body" {
                    stack.retain(|s| s.0 != "head");
                }
                if !*closed && !VOID.contains(&name.as_str()) && name != "script" && name != "style" {
                    stack.push((name.clone(), attr_val(src, attrs, "id").map(String::from), attr_val(src, attrs, "class").map(String::from)));
                }
            }
            Tok::Close { name, .. } => {
                if let Some(k) = stack.iter().rposition(|s| &s.0 == name) {
                    stack.truncate(k);
                }
            }
            Tok::Text(a, z) => {
                let raw = &src[*a..*z];
                let t = raw.trim();
                if t.is_empty() || stack.iter().any(|s| matches!(s.0.as_str(), "title" | "head" | "noscript")) {
                    continue;
                }
                let start = a + (raw.len() - raw.trim_start().len());
                out.push(TextRun { start, end: start + t.len(), parent: stack.last().cloned() });
            }
            _ => {}
        }
    }
    out
}

fn visible_text(t: &str) -> bool {
    !t.contains("{{") && !t.contains("}}") && decode_entities(t).chars().any(char::is_alphanumeric)
}

/// Visible texts written into the page instead of a field (short, for messages).
pub fn hardcoded(src: &str) -> Vec<String> {
    let toks = tokenize(src);
    text_runs(src, &toks)
        .into_iter()
        .map(|r| &src[r.start..r.end])
        .filter(|t| visible_text(t))
        .map(|t| {
            let one = decode_entities(t).split_whitespace().collect::<Vec<_>>().join(" ");
            if one.chars().count() > 40 {
                format!("{}…", one.chars().take(39).collect::<String>())
            } else {
                one
            }
        })
        .take(12)
        .collect()
}

/// A field name from a class, id or tag: lower snake case.
fn sanitize(s: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && prev_lower && !out.ends_with('_') {
                out.push('_');
            }
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
            out.push(c.to_ascii_lowercase());
        } else {
            prev_lower = false;
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
        }
    }
    let mut out: String = out.trim_matches('_').chars().take(32).collect::<String>().trim_end_matches('_').to_string();
    if out.is_empty() {
        out = "field".into();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out = format!("f_{out}");
    }
    out
}

fn unique(taken: &mut BTreeSet<String>, base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

fn element_word(tag: &str, id: Option<&str>, class: Option<&str>) -> String {
    if let Some(c) = class.and_then(|c| c.split_whitespace().next()) {
        return sanitize(c);
    }
    if let Some(i) = id.filter(|i| i.trim().len() > 2) {
        return sanitize(i);
    }
    match tag {
        "h1" => "title".into(),
        "h2" | "h3" => "heading".into(),
        "h4" | "h5" | "h6" => "subheading".into(),
        "li" => "item".into(),
        "button" | "a" | "label" => "label".into(),
        "p" | "span" | "div" | "b" | "strong" | "em" | "i" | "small" | "text" | "tspan" | "td" | "th" => "text".into(),
        other => sanitize(other),
    }
}

/// The last simple selector's class, id or tag (keyframe steps give "").
fn selector_word(sel: &str) -> String {
    let last = sel.rsplit(',').next().unwrap_or("").trim();
    let comp = last.rsplit(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~')).find(|s| !s.is_empty()).unwrap_or("");
    let comp = comp.split(':').next().unwrap_or("").split('[').next().unwrap_or("");
    let w = if comp.contains(['.', '#']) { comp.rsplit(['.', '#']).next().unwrap_or("") } else { comp };
    if matches!(w, "from" | "to" | "*") || w.ends_with('%') || w.starts_with('@') {
        String::new()
    } else {
        sanitize(w)
    }
}

fn color_field_name(sel: &str, prop: &str) -> String {
    let p = match prop {
        p if p.starts_with("background") => "background",
        p if p.starts_with("border") => "border",
        p if p.contains("stroke") => "stroke",
        p if p.ends_with("shadow") => "shadow",
        p if p.starts_with("outline") => "outline",
        p => p,
    };
    if sel.is_empty() {
        sanitize(p)
    } else {
        sanitize(&format!("{sel}_{p}"))
    }
}

/// The property (and the rule's selector word) of a value at `pos..end`, if it is in a declaration value.
fn declaration(src: &str, a: usize, z: usize, pos: usize, end: usize, sheet: bool) -> Option<(String, String)> {
    let b = src.as_bytes();
    let mut k = pos;
    let mut colon = None;
    while k > a {
        k -= 1;
        match b[k] {
            b':' => {
                colon = Some(k);
                break;
            }
            b';' | b'{' | b'}' => break,
            _ => {}
        }
    }
    let colon = colon?;
    if sheet {
        // a selector (`a:hover #fade {`) runs into a '{'; a value into ';' or '}'
        let mut f = end;
        while f < z && !matches!(b[f], b';' | b'{' | b'}') {
            f += 1;
        }
        if f < z && b[f] == b'{' {
            return None;
        }
    }
    let mut ps = colon;
    while ps > a && !matches!(b[ps - 1], b';' | b'{' | b'}') {
        ps -= 1;
    }
    let prop = src[ps..colon].trim().to_ascii_lowercase();
    if prop.is_empty() || !prop.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let sel = if sheet {
        let mut o = ps;
        while o > a && b[o - 1] != b'{' {
            o -= 1;
        }
        if o == a {
            String::new()
        } else {
            let open = o - 1;
            let mut s = open;
            while s > a && !matches!(b[s - 1], b'{' | b'}' | b';') {
                s -= 1;
            }
            selector_word(src[s..open].trim())
        }
    } else {
        String::new()
    };
    Some((prop, sel))
}

/// Color literals in CSS (a stylesheet, or a style attribute's declarations): (start, end, selector word, property).
fn css_colors(src: &str, a: usize, z: usize, sheet: bool, out: &mut Vec<(usize, usize, String, String)>) {
    let b = src.as_bytes();
    let mut i = a;
    let word_byte = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'_';
    while i < z {
        let c = b[i];
        if c == b'/' && i + 1 < z && b[i + 1] == b'*' {
            i = src[i + 2..z].find("*/").map(|x| i + 2 + x + 2).unwrap_or(z);
            continue;
        }
        if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < z && b[j] != c {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            i = (j + 1).min(z);
            continue;
        }
        if c == b'{' && i + 1 < z && b[i + 1] == b'{' {
            i = src[i + 2..z].find("}}").map(|x| i + 2 + x + 2).unwrap_or(z);
            continue;
        }
        let prev = if i > a { b[i - 1] } else { b' ' };
        if c == b'#' {
            let mut j = i + 1;
            while j < z && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            let len = j - i - 1;
            let ok = matches!(len, 3 | 4 | 6 | 8) && !(j < z && word_byte(b[j])) && !word_byte(prev) && prev != b'&' && !src[a..i].trim_end().to_ascii_lowercase().ends_with("url(");
            if ok {
                if let Some((prop, sel)) = declaration(src, a, z, i, j, sheet) {
                    out.push((i, j, sel, prop));
                }
            }
            i = j.max(i + 1);
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut j = i;
            while j < z && word_byte(b[j]) {
                j += 1;
            }
            let word = src[i..j].to_ascii_lowercase();
            if !word_byte(prev) && matches!(word.as_str(), "rgb" | "rgba" | "hsl" | "hsla") && j < z && b[j] == b'(' {
                if let Some(k) = src[j..z].find(')') {
                    let e = j + k + 1;
                    let inner = &src[j + 1..e - 1];
                    if !inner.contains("var(") && !inner.contains("calc(") && !inner.contains("{{") && !inner.contains('(') {
                        if let Some((prop, sel)) = declaration(src, a, z, i, e, sheet) {
                            out.push((i, e, sel, prop));
                        }
                    }
                    i = e;
                    continue;
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
}

/// Lowercase, no spaces, short hex expanded: equal colors compare equal.
fn norm_color(lit: &str) -> String {
    let l: String = lit.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    match l.strip_prefix('#') {
        Some(h) if h.len() == 3 || h.len() == 4 => format!("#{}", h.chars().flat_map(|c| [c, c]).collect::<String>()),
        _ => l,
    }
}

fn is_transparent(norm: &str) -> bool {
    (norm.starts_with('#') && norm.len() == 9 && norm.ends_with("00")) || norm.ends_with(",0)") || norm.ends_with("/0)")
}

pub struct Editable {
    pub src: String,
    /// The fields made, in page order.
    pub added: Vec<(String, Kind)>,
}

/// Turns the page's visible texts (and the colors of its CSS, up to 8) into fields
/// whose defaults are the values as written, so it renders exactly as before.
pub fn make_editable(src: &str, texts: bool, colors: bool) -> Editable {
    let toks = tokenize(src);
    let sp = spans(&toks);
    let phs = placeholders(src, &sp);
    let parsed = parse_with(src, &toks, &phs);
    let mut taken: BTreeSet<String> = parsed.fields.iter().map(|f| f.name.clone()).collect();
    let in_placeholder = |a: usize, z: usize| phs.iter().any(|p| a < p.end && z > p.start);
    let mut reps: Vec<(usize, usize, String, Option<(String, Kind)>)> = vec![];
    if texts {
        for r in text_runs(src, &toks) {
            let t = &src[r.start..r.end];
            if !visible_text(t) || in_placeholder(r.start, r.end) {
                continue;
            }
            let base = match &r.parent {
                Some((tag, id, class)) => element_word(tag, id.as_deref(), class.as_deref()),
                None => "text".into(),
            };
            let name = unique(&mut taken, &base);
            let kind = if t.contains('\n') || t.chars().count() > 80 { Kind::Longtext } else { Kind::Text };
            reps.push((r.start, r.end, format!("{{{{{name}|{t}}}}}"), Some((name, kind))));
        }
    }
    if colors {
        let mut sites = vec![];
        for t in &toks {
            match t {
                Tok::Raw(a, z, Raw::Style) => css_colors(src, *a, *z, true, &mut sites),
                Tok::Open { name, attrs, .. } => {
                    for at in attrs.iter().filter(|x| x.name == "style") {
                        if let Some((x, y)) = at.val {
                            let owner = element_word(name, attr_val(src, attrs, "id"), attr_val(src, attrs, "class"));
                            let mut found = vec![];
                            css_colors(src, x, y, false, &mut found);
                            sites.extend(found.into_iter().map(|(s, e, _, prop)| (s, e, owner.clone(), prop)));
                        }
                    }
                }
                _ => {}
            }
        }
        sites.sort_by_key(|s| s.0);
        let mut by_value: Vec<(String, String)> = vec![];
        for (a, z, sel, prop) in sites {
            if in_placeholder(a, z) {
                continue;
            }
            let lit = &src[a..z];
            let norm = norm_color(lit);
            let (name, new) = match by_value.iter().find(|(v, _)| *v == norm) {
                Some((_, n)) => (n.clone(), false),
                None => {
                    if by_value.len() >= 8 || is_transparent(&norm) {
                        continue;
                    }
                    let n = unique(&mut taken, &color_field_name(&sel, &prop));
                    by_value.push((norm, n.clone()));
                    (n, true)
                }
            };
            reps.push((a, z, format!("{{{{{name}|{lit}}}}}"), new.then(|| (name, Kind::Color))));
        }
    }
    reps.sort_by_key(|r| r.0);
    let mut out = String::with_capacity(src.len() + reps.len() * 16);
    let mut added = vec![];
    let mut last = 0;
    for (a, z, rep, field) in reps {
        if a < last {
            continue;
        }
        out.push_str(&src[last..a]);
        out.push_str(&rep);
        last = z;
        added.extend(field);
    }
    out.push_str(&src[last..]);
    Editable { src: out, added }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field<'a>(p: &'a Parsed, name: &str) -> &'a Field {
        p.fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no field {name}: {:?}", p.fields.iter().map(|f| &f.name).collect::<Vec<_>>()))
    }

    #[test]
    fn fields_get_types_labels_and_order() {
        let src = r#"<style>.bar{background:{{accent|var(--brand-accent)}};bottom:{{lift|12}}%;color:{{ink|white}}}</style>
<h1 class="t">{{title|Big &amp; bold<br>launch}}</h1><img src="{{logo|mc://m3}}"><p>{{show|true}}{{who}}</p><!-- {{note}} -->
<script type="application/json" id="mc-fields">{"lift":{"label":"Height","min":4,"max":40,"unit":"%"},"style":{"type":"select","options":["bar","box"]},"speed":{"type":"number","default":1.5}}</script>"#;
        let p = parse(src);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let names: Vec<&str> = p.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["lift", "style", "speed", "accent", "ink", "title", "logo", "show", "who"], "manifest first, then page order; comments don't count");
        let lift = field(&p, "lift");
        assert_eq!((lift.kind, lift.label.as_str(), lift.default.as_str(), lift.min, lift.max, lift.unit.as_str()), (Kind::Number, "Height", "12", Some(4.0), Some(40.0), "%"));
        assert_eq!((field(&p, "style").kind, field(&p, "style").default.as_str()), (Kind::Select, "bar"));
        assert_eq!(field(&p, "speed").used, Vec::<&str>::new(), "read by scripts only");
        let accent = field(&p, "accent");
        assert_eq!((accent.kind, accent.brand.as_str()), (Kind::Color, "accent"));
        assert_eq!(field(&p, "ink").kind, Kind::Color, "a color name used in CSS");
        let title = field(&p, "title");
        assert_eq!((title.kind, title.plain.as_deref()), (Kind::Longtext, Some("Big & bold\nlaunch")));
        assert_eq!(field(&p, "logo").kind, Kind::Media);
        assert_eq!(field(&p, "show").kind, Kind::Toggle);
        assert_eq!((field(&p, "who").kind, field(&p, "who").label.as_str()), (Kind::Text, "Who"));
        assert_eq!(humanize("speakerName"), "Speaker name");
        assert_eq!(humanize("lower_third-bg"), "Lower third bg");
        assert!(!parse("<script type=\"application/json\" id=\"mc-fields\">{oops</script>").warnings.is_empty());
    }

    #[test]
    fn values_are_escaped_for_where_they_stand() {
        let src = "<p title='{{t}}'>{{t}}</p><style>.a::after{content:\"{{t}}\"}.b{color:{{c}}}</style>\
                   <script>const a='{{t}}', b={{t}}, n={{n}}; if(x){{y}}</script>";
        let vars = BTreeMap::from([("t".to_string(), "It's \"x\" <b>\nok".to_string()), ("c".to_string(), "red;}body{x".to_string()), ("n".to_string(), "2.5".to_string())]);
        let f = fill(src, &vars);
        let h = &f.html;
        assert!(h.contains("title='It&#39;s &quot;x&quot; &lt;b&gt;&#10;ok'"), "{h}");
        assert!(h.contains(">It&#39;s &quot;x&quot; &lt;b&gt;<br>ok</p>"), "{h}");
        assert!(h.contains(r#"content:"It\'s \"x\" \3c b>\a ok""#), "{h}");
        assert!(h.contains("color:redbodyx}"), "{h}");
        assert!(h.contains(r#"const a='It\'s \"x\" \x3cb>\nok'"#), "{h}");
        assert!(h.contains(r#"b="It's \"x\" <b>\nok""#), "{h}");
        assert!(h.contains("n=2.5;"), "{h}");
        assert!(h.contains("if(x){{y}}"), "unknown braces in scripts stay: {h}");
        assert!(f.missing.is_empty(), "{:?}", f.missing);
    }

    #[test]
    fn defaults_fill_every_use_and_scripts_see_them() {
        let src = "<h1>{{name|Jane}}</h1><p data-n='{{name}}'>{{role}}</p><script type=\"application/json\" id=\"mc-fields\">{\"tag\":{\"default\":\"A & B\"}}</script><i>{{tag}}</i>";
        let f = fill(src, &BTreeMap::new());
        assert!(f.html.contains("<h1>Jane</h1><p data-n='Jane'>"), "{}", f.html);
        assert!(f.html.contains("<i>A &amp; B</i>"), "manifest defaults are data: {}", f.html);
        assert!(f.html.contains("{\"tag\":{\"default\":\"A & B\"}}"), "the manifest is left alone");
        assert_eq!(f.missing, ["role"]);
        assert_eq!(f.vars.get("name").map(String::as_str), Some("Jane"));
        assert_eq!(f.vars.get("tag").map(String::as_str), Some("A & B"));
    }

    #[test]
    fn normalizing_values() {
        let p = parse("<style>a{color:{{c|#fff}};top:{{y|10px}};bottom:{{lift|12}}%}</style>{{on|true}}<script type=\"application/json\" id=\"mc-fields\">{\"k\":{\"options\":[\"a\",\"b\"]},\"lift\":{\"unit\":\"%\"}}</script>");
        assert_eq!(normalize(field(&p, "c"), " rgb(1, 2, 3) ").unwrap(), "rgb(1, 2, 3)");
        assert!(normalize(field(&p, "c"), "red; x").is_err());
        assert_eq!(normalize(field(&p, "y"), "14").unwrap(), "14px");
        assert_eq!(normalize(field(&p, "lift"), "14").unwrap(), "14", "the % is written after the field");
        assert_eq!(field(&p, "lift").unit, "%");
        assert_eq!(normalize(field(&p, "y"), "2vh").unwrap(), "2vh");
        assert!(normalize(field(&p, "y"), "big").is_err());
        assert_eq!(normalize(field(&p, "on"), "0").unwrap(), "false");
        assert!(normalize(field(&p, "k"), "c").is_err());
        assert_eq!(normalize(field(&p, "k"), "b").unwrap(), "b");
    }

    #[test]
    fn make_editable_keeps_the_render_and_names_fields() {
        let pages = [
            "<style>.bar{background:#ff3b6b;height:4px}#fade{color:#fff}.x{background:url(#add)}\n@keyframes k{from{color:rgba(255, 0, 0, .5)}}</style>\
             <div class=\"l3\"><div class=\"name\">Jane Doe</div><div class=bar></div><div class=\"role\">Founder &amp; CEO</div></div>",
            "<!doctype html><html><head><title>Doc</title><style>h1{color:#FFF;text-shadow:0 0 4px #ffffff}</style></head>\
             <body><h1 id=t style=\"border-color:#00ff00\">Make it <b>move</b></h1><p>&#123; {{sub|x}}</p><script>document.title='Not text'</script></body></html>",
            include_str!("../../../../examples/graphics/lower-third.html"),
            include_str!("../../../../examples/graphics/kinetic-title.html"),
        ];
        for src in pages {
            let e = make_editable(src, true, true);
            assert_eq!(fill(&e.src, &BTreeMap::new()).html, fill(src, &BTreeMap::new()).html, "render changed for:\n{src}\n->\n{}", e.src);
            assert!(make_editable(&e.src, true, true).added.is_empty(), "a second pass finds nothing new:\n{}", e.src);
        }
        let e = make_editable(pages[0], true, true);
        let names: Vec<&str> = e.added.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["bar_background", "fade_color", "color", "name", "role"], "{}", e.src);
        assert!(e.src.contains("#fade{"), "an id selector isn't a color: {}", e.src);
        assert!(e.src.contains("url(#add)"), "{}", e.src);
        let p = parse(&e.src);
        assert_eq!(field(&p, "role").plain.as_deref(), Some("Founder & CEO"));
        let e = make_editable(pages[1], true, true);
        let names: Vec<&str> = e.added.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["h1_color", "title_border", "title", "text"], "same color = one field; the <title> and scripts stay: {}", e.src);
        assert!(parse("<script>if(a){{b}}</script>").fields.is_empty(), "bare braces in scripts aren't fields");
        assert!(e.src.contains("text-shadow:0 0 4px {{h1_color|#ffffff}}"), "{}", e.src);
        assert_eq!(hardcoded(pages[0]), ["Jane Doe", "Founder & CEO"]);
        assert!(hardcoded(&make_editable(pages[0], true, false).src).is_empty());
    }
}
