//! Text engine: font discovery (system fonts + bundled Inter), shaping with
//! rustybuzz, bidi reordering (Arabic/Urdu/Hebrew), word wrap, glyph outlines,
//! and After-Effects-style per-unit text animators.

use crate::model::anim::Ease;
use crate::model::project::{Animator, Item};
use crate::model::value::{Color, Val};
use parking_lot::Mutex;
use resvg::tiny_skia::{Path, PathBuilder, Rect, Transform};
use resvg::usvg::fontdb::{self, Database, Family, Query, Stretch, Style, Weight};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

pub static INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.otf");
pub static INTER_BOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-Bold.otf");
pub static INTER_BLACK: &[u8] = include_bytes!("../../assets/fonts/Inter-Black.otf");

pub struct FaceData {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    pub upem: f32,
    pub asc: f32,
    pub desc: f32,
}

struct Fonts {
    db: Arc<Database>,
    faces: Mutex<HashMap<fontdb::ID, Arc<FaceData>>>,
    fallback: Mutex<HashMap<char, Option<fontdb::ID>>>,
    layouts: Mutex<HashMap<String, Arc<Layout>>>,
}

fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(|| {
        let mut db = Database::new();
        db.load_font_data(INTER_REGULAR.to_vec());
        db.load_font_data(INTER_BOLD.to_vec());
        db.load_font_data(INTER_BLACK.to_vec());
        if std::env::var("MOODCUT_NO_SYSTEM_FONTS").is_err() {
            db.load_system_fonts();
        }
        if let Some(d) = crate::config::data_dir() {
            db.load_fonts_dir(d.join("fonts"));
        }
        db.set_sans_serif_family("Inter");
        Fonts { db: Arc::new(db), faces: Mutex::new(HashMap::new()), fallback: Mutex::new(HashMap::new()), layouts: Mutex::new(HashMap::new()) }
    })
}

/// Shared font database (also handed to resvg for SVG text).
pub fn fontdb_arc() -> Arc<Database> {
    fonts().db.clone()
}

/// Lists installed font families (for help/GUI).
pub fn families() -> Vec<String> {
    let mut v: Vec<String> = fonts().db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
    v.sort();
    v.dedup();
    v
}

fn face(id: fontdb::ID) -> Option<Arc<FaceData>> {
    let f = fonts();
    if let Some(fd) = f.faces.lock().get(&id) {
        return Some(fd.clone());
    }
    let (data, index) = f.db.with_face_data(id, |d, i| (d.to_vec(), i))?;
    let ttf = rustybuzz::ttf_parser::Face::parse(&data, index).ok()?;
    let fd = Arc::new(FaceData { upem: ttf.units_per_em() as f32, asc: ttf.ascender() as f32, desc: ttf.descender() as f32, data: Arc::new(data.clone()), index });
    f.faces.lock().insert(id, fd.clone());
    Some(fd)
}

fn query(family: &str, weight: f64, italic: bool) -> Option<fontdb::ID> {
    let fams = [Family::Name(family), Family::SansSerif];
    fonts().db.query(&Query { families: &fams, weight: Weight(weight.round().clamp(100.0, 900.0) as u16), stretch: Stretch::Normal, style: if italic { Style::Italic } else { Style::Normal } })
}

fn has_glyph(fd: &FaceData, c: char) -> bool {
    rustybuzz::ttf_parser::Face::parse(&fd.data, fd.index).ok().and_then(|f| f.glyph_index(c)).is_some()
}

const FALLBACK_FAMILIES: &[&str] = &[
    "Segoe UI", "Arial", "Noto Sans", "Noto Nastaliq Urdu", "Noto Naskh Arabic", "Noto Sans Arabic", "Segoe UI Historic", "Nirmala UI", "Microsoft YaHei", "Noto Sans CJK SC", "Yu Gothic", "Malgun Gothic",
    "Segoe UI Symbol", "Segoe UI Emoji", "Noto Color Emoji", "DejaVu Sans", "FreeSans",
];

fn fallback_for(c: char) -> Option<fontdb::ID> {
    let f = fonts();
    if let Some(r) = f.fallback.lock().get(&c) {
        return *r;
    }
    let mut found = None;
    for fam in FALLBACK_FAMILIES {
        let fams = [Family::Name(fam)];
        if let Some(id) = f.db.query(&Query { families: &fams, ..Default::default() }) {
            if let Some(fd) = face(id) {
                if has_glyph(&fd, c) {
                    found = Some(id);
                    break;
                }
            }
        }
    }
    if found.is_none() {
        for info in f.db.faces().take(2000) {
            if let Some(fd) = face(info.id) {
                if has_glyph(&fd, c) {
                    found = Some(info.id);
                    break;
                }
            }
        }
    }
    f.fallback.lock().insert(c, found);
    found
}

#[derive(Clone, Debug)]
pub struct TextStyle {
    pub family: String,
    pub weight: f64,
    pub italic: bool,
    pub size: f32,
    pub align: u8, // 0 left 1 center 2 right
    pub line_h: f32,
    pub tracking: f32,
    pub box_w: f32,
    pub caps: bool,
}

#[derive(Clone)]
pub struct Glyph {
    pub path: Option<Path>,
    pub x: f32,
    pub base: f32,
    pub adv: f32,
    pub ch: usize,
    pub word: usize,
    pub line: usize,
    pub line_pos: usize,
    pub space: bool,
    /// Center of the glyph's advance box (pivot for per-glyph transforms).
    pub cx: f32,
    pub cy: f32,
}

#[derive(Clone, Debug)]
pub struct LineBox {
    pub x: f32,
    pub top: f32,
    pub w: f32,
    pub h: f32,
    pub n: usize,
}

pub struct Layout {
    pub glyphs: Vec<Glyph>,
    pub lines: Vec<LineBox>,
    pub w: f32,
    pub h: f32,
    pub n_chars: usize,
    pub n_words: usize,
    pub combined: Option<Path>,
}

struct PB {
    pb: PathBuilder,
    s: f32,
    x: f32,
    y: f32,
}

impl rustybuzz::ttf_parser::OutlineBuilder for PB {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.x + x * self.s, self.y - y * self.s);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.x + x * self.s, self.y - y * self.s);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb.quad_to(self.x + x1 * self.s, self.y - y1 * self.s, self.x + x * self.s, self.y - y * self.s);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(self.x + x1 * self.s, self.y - y1 * self.s, self.x + x2 * self.s, self.y - y2 * self.s, self.x + x * self.s, self.y - y * self.s);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

struct Shaped {
    gid: u16,
    face: fontdb::ID,
    x_adv: f32,
    x_off: f32,
    y_off: f32,
    /// byte offset into the full text
    byte: usize,
}

/// Shapes `s` (a slice of the full text starting at `base_byte`) into glyphs.
fn shape_run(s: &str, base_byte: usize, primary: fontdb::ID, rtl: bool, size: f32) -> Vec<Shaped> {
    // segment by font coverage
    let pf = match face(primary) {
        Some(f) => f,
        None => return vec![],
    };
    let mut segs: Vec<(usize, usize, fontdb::ID)> = vec![];
    let mut cur_start = 0;
    let mut cur_face = None;
    for (bi, c) in s.char_indices() {
        let fid = if c.is_whitespace() || c.is_control() || has_glyph(&pf, c) { primary } else { fallback_for(c).unwrap_or(primary) };
        match cur_face {
            None => {
                cur_face = Some(fid);
                cur_start = bi;
            }
            Some(f) if f != fid => {
                segs.push((cur_start, bi, f));
                cur_start = bi;
                cur_face = Some(fid);
            }
            _ => {}
        }
    }
    if let Some(f) = cur_face {
        segs.push((cur_start, s.len(), f));
    }
    if rtl {
        segs.reverse();
    }
    let mut out = vec![];
    for (a, b, fid) in segs {
        let Some(fd) = face(fid) else { continue };
        let Some(rf) = rustybuzz::Face::from_slice(&fd.data, fd.index) else { continue };
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(&s[a..b]);
        buf.set_direction(if rtl { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
        buf.guess_segment_properties();
        let gb = rustybuzz::shape(&rf, &[], buf);
        let scale = size / fd.upem;
        for (info, pos) in gb.glyph_infos().iter().zip(gb.glyph_positions()) {
            out.push(Shaped {
                gid: info.glyph_id as u16,
                face: fid,
                x_adv: pos.x_advance as f32 * scale,
                x_off: pos.x_offset as f32 * scale,
                y_off: pos.y_offset as f32 * scale,
                byte: base_byte + a + info.cluster as usize,
            });
        }
    }
    out
}

fn measure(s: &str, primary: fontdb::ID, size: f32, tracking: f32) -> f32 {
    let rtl = s.chars().any(is_rtl_char);
    let g = shape_run(s, 0, primary, rtl, size);
    g.iter().map(|x| x.x_adv + tracking).sum()
}

fn is_rtl_char(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// Lays out text (cached).
pub fn layout(text: &str, st: &TextStyle) -> Arc<Layout> {
    let key = format!("{}\u{1}{}\u{1}{}\u{1}{}\u{1}{:.2}\u{1}{}\u{1}{:.3}\u{1}{:.2}\u{1}{:.1}\u{1}{}", text, st.family, st.weight, st.italic, st.size, st.align, st.line_h, st.tracking, st.box_w, st.caps);
    let f = fonts();
    if let Some(l) = f.layouts.lock().get(&key) {
        return l.clone();
    }
    let l = Arc::new(do_layout(text, st));
    let mut g = f.layouts.lock();
    if g.len() > 600 {
        g.clear();
    }
    g.insert(key, l.clone());
    l
}

fn do_layout(text_in: &str, st: &TextStyle) -> Layout {
    let text: String = if st.caps { text_in.to_uppercase() } else { text_in.to_string() };
    let primary = query(&st.family, st.weight, st.italic).or_else(|| query("Inter", 400.0, false));
    let empty = Layout { glyphs: vec![], lines: vec![], w: 0.0, h: st.size * st.line_h, n_chars: 0, n_words: 0, combined: None };
    let Some(primary) = primary else { return empty };
    let Some(pf) = face(primary) else { return empty };
    let size = st.size.max(0.1);
    let scale = size / pf.upem;
    let asc = pf.asc * scale;
    let desc = -pf.desc * scale;
    let line_h = size * st.line_h;
    // unit indices per byte
    let mut char_unit: HashMap<usize, (usize, usize)> = HashMap::new();
    let mut nc = 0usize;
    let mut nw = 0usize;
    let mut in_word = false;
    for (bi, c) in text.char_indices() {
        if c.is_whitespace() {
            in_word = false;
            char_unit.insert(bi, (usize::MAX, nw.saturating_sub(1)));
        } else {
            if !in_word {
                in_word = true;
                nw += 1;
            }
            char_unit.insert(bi, (nc, nw - 1));
            nc += 1;
        }
    }
    // wrap into lines (byte ranges)
    let mut lines: Vec<(usize, usize)> = vec![];
    let mut off = 0usize;
    for para in text.split('\n') {
        let pstart = off;
        off += para.len() + 1;
        if st.box_w <= 0.0 || para.is_empty() {
            lines.push((pstart, pstart + para.len()));
            continue;
        }
        let space_w = measure(" ", primary, size, st.tracking);
        let mut line_start = pstart;
        let mut line_w = 0.0f32;
        let mut cursor = pstart;
        for word in para.split(' ') {
            let ww = if word.is_empty() { 0.0 } else { measure(word, primary, size, st.tracking) };
            let add = if cursor == line_start { ww } else { space_w + ww };
            if line_w + add > st.box_w && cursor > line_start {
                lines.push((line_start, cursor - 1));
                line_start = cursor;
                line_w = ww;
            } else {
                line_w += add;
            }
            cursor += word.len() + 1;
        }
        lines.push((line_start, (pstart + para.len()).max(line_start)));
    }
    let mut glyphs: Vec<Glyph> = vec![];
    let mut boxes: Vec<LineBox> = vec![];
    for (li, (a, b)) in lines.iter().enumerate() {
        let s = &text[*a..*b];
        let top = li as f32 * line_h;
        let base = top + (line_h - (asc + desc)) / 2.0 + asc;
        let mut shaped: Vec<Shaped> = vec![];
        if s.chars().any(is_rtl_char) {
            let bidi = unicode_bidi::BidiInfo::new(s, None);
            if let Some(para) = bidi.paragraphs.first() {
                let (levels, runs) = bidi.visual_runs(para, para.range.clone());
                for r in runs {
                    let rtl = levels[r.start].is_rtl();
                    shaped.extend(shape_run(&s[r.clone()], a + r.start, primary, rtl, size));
                }
            }
        } else {
            shaped = shape_run(s, *a, primary, false, size);
        }
        let mut pen = 0.0f32;
        let start_idx = glyphs.len();
        let mut pos_in_line = 0;
        for g in &shaped {
            let c = text[g.byte..].chars().next().unwrap_or(' ');
            let space = c.is_whitespace();
            let (cu, wu) = char_unit.get(&g.byte).copied().unwrap_or((usize::MAX, 0));
            let fd = face(g.face);
            let path = if space {
                None
            } else {
                fd.and_then(|fd| {
                    let tf = rustybuzz::ttf_parser::Face::parse(&fd.data, fd.index).ok()?;
                    let s = size / fd.upem;
                    let mut pb = PB { pb: PathBuilder::new(), s, x: pen + g.x_off, y: base - g.y_off };
                    tf.outline_glyph(rustybuzz::ttf_parser::GlyphId(g.gid), &mut pb)?;
                    pb.pb.finish()
                })
            };
            glyphs.push(Glyph { path, x: pen + g.x_off, base, adv: g.x_adv, ch: cu, word: wu, line: li, line_pos: pos_in_line, space, cx: pen + g.x_adv / 2.0, cy: base - asc * 0.35 });
            pos_in_line += 1;
            pen += g.x_adv + st.tracking;
        }
        let w = (pen - if shaped.is_empty() { 0.0 } else { st.tracking }).max(0.0);
        boxes.push(LineBox { x: 0.0, top, w, h: line_h, n: glyphs.len() - start_idx });
    }
    let max_w = if st.box_w > 0.0 { st.box_w.max(boxes.iter().map(|b| b.w).fold(0.0, f32::max)) } else { boxes.iter().map(|b| b.w).fold(0.0, f32::max) };
    // alignment
    let mut gi = 0;
    for lb in boxes.iter_mut() {
        let dx = match st.align {
            0 => 0.0,
            2 => max_w - lb.w,
            _ => (max_w - lb.w) / 2.0,
        };
        lb.x = dx;
        for g in &mut glyphs[gi..gi + lb.n] {
            g.x += dx;
            g.cx += dx;
            if let Some(p) = g.path.take() {
                g.path = p.transform(Transform::from_translate(dx, 0.0));
            }
        }
        gi += lb.n;
    }
    let mut pb = PathBuilder::new();
    let mut any = false;
    for g in &glyphs {
        if let Some(p) = &g.path {
            pb.push_path(p);
            any = true;
        }
    }
    let combined = if any { pb.finish() } else { None };
    Layout { h: boxes.len().max(1) as f32 * line_h, w: max_w, glyphs, lines: boxes, n_chars: nc, n_words: nw, combined }
}

/// Per-glyph animation state.
#[derive(Clone, Copy, Debug)]
pub struct GState {
    pub opacity: f32,
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
    pub rot: f32,
    pub blur: f32,
    pub track: f32,
    pub color: Option<(Color, f32)>,
}

impl Default for GState {
    fn default() -> Self {
        GState { opacity: 1.0, dx: 0.0, dy: 0.0, scale: 1.0, rot: 0.0, blur: 0.0, track: 0.0, color: None }
    }
}

impl GState {
    pub fn is_identity(&self) -> bool {
        self.opacity >= 0.999 && self.dx == 0.0 && self.dy == 0.0 && (self.scale - 1.0).abs() < 1e-4 && self.rot == 0.0 && self.blur < 0.3 && self.track == 0.0 && self.color.is_none()
    }
}

fn ap(a: &Animator, k: &str) -> Option<Val> {
    a.props.get(k).map(|x| x.v.clone())
}
fn apn(a: &Animator, k: &str, d: f64) -> f64 {
    ap(a, k).map(|v| v.num()).unwrap_or(d)
}

fn order_rank(order: &str, i: usize, n: usize, seed: u64) -> f64 {
    let nf = n as f64;
    let c = (nf - 1.0) / 2.0;
    match order {
        "reverse" => nf - 1.0 - i as f64,
        "center" => (i as f64 - c).abs() * 2.0,
        "edges" => (c - (i as f64 - c).abs()) * 2.0,
        "random" => {
            // deterministic permutation rank
            let mut v: Vec<(u64, usize)> = (0..n).map(|k| (crate::model::expr::rand01(seed, k as u64).to_bits(), k)).collect();
            v.sort();
            v.iter().position(|(_, k)| *k == i).unwrap_or(i) as f64
        }
        _ => i as f64,
    }
}

/// Evaluates all animators for every glyph at item-local time `tl`.
pub fn glyph_states(lay: &Layout, item: &Item, tl: f64, seed: u64) -> Vec<GState> {
    let mut st = vec![GState::default(); lay.glyphs.len()];
    let anims: Vec<&Animator> = item.anims.iter().filter(|a| a.on).collect();
    if anims.is_empty() {
        return st;
    }
    for a in anims {
        let unit = ap(a, "unit").map(|v| v.str()).unwrap_or_else(|| "char".into());
        let n_units = match unit.as_str() {
            "word" => lay.n_words,
            "line" => lay.lines.len(),
            _ => lay.n_chars,
        }
        .max(1);
        let order = ap(a, "order").map(|v| v.str()).unwrap_or_else(|| "forward".into());
        let stagger = apn(a, "stagger", 0.04);
        let dur = apn(a, "dur", 0.5).max(0.0);
        let delay = apn(a, "delay", 0.0);
        let ease = ap(a, "ease").and_then(|v| Ease::parse(&v.str()).ok()).unwrap_or(Ease::Out(crate::model::anim::Curve::Cubic));
        let times: Vec<f64> = ap(a, "times").map(|v| v.str()).unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
        let o_to = apn(a, "opacity", 1.0) as f32;
        let x = apn(a, "x", 0.0) as f32;
        let y = apn(a, "y", 0.0) as f32;
        let sc = apn(a, "scale", 1.0) as f32;
        let rot = apn(a, "rot", 0.0) as f32;
        let blur = apn(a, "blur", 0.0) as f32;
        let trk = apn(a, "tracking", 0.0) as f32;
        let color = ap(a, "color").map(|v| v.color()).filter(|c| c.a > 0.0);
        let freq = apn(a, "freq", 1.0);
        let phase = apn(a, "phase", 30.0).to_radians();
        let total = (n_units as f64 - 1.0).max(0.0) * stagger + dur;
        for (gi, g) in lay.glyphs.iter().enumerate() {
            let ui = match unit.as_str() {
                "word" => g.word,
                "line" => g.line,
                _ => {
                    if g.ch == usize::MAX {
                        // spaces follow the previous char
                        continue;
                    }
                    g.ch
                }
            };
            let rank = order_rank(&order, ui.min(n_units - 1), n_units, seed);
            let s = &mut st[gi];
            match a.kind.as_str() {
                "loop" => {
                    let v = (2.0 * std::f64::consts::PI * freq * tl - phase * rank).sin() as f32;
                    s.dx += x * v;
                    s.dy += y * v;
                    s.rot += rot * v;
                    s.scale *= 1.0 + (sc - 1.0) * v;
                    if o_to < 1.0 {
                        s.opacity *= 1.0 - (1.0 - o_to) * (v + 1.0) / 2.0;
                    }
                }
                "hl" => {
                    let t0 = if !times.is_empty() { times.get(ui).copied().unwrap_or(f64::INFINITY) } else { delay + rank * stagger };
                    let t1 = if !times.is_empty() { times.get(ui + 1).copied().unwrap_or(item.dur + 1.0) } else { t0 + stagger.max(dur) };
                    let fade = dur.max(1e-3);
                    let amt = if tl < t0 { 0.0 } else if tl < t1 { ((tl - t0) / fade).min(1.0) } else { (1.0 - (tl - t1) / fade).max(0.0) };
                    let amt = amt as f32;
                    if amt > 0.0 {
                        s.dx += x * amt;
                        s.dy += y * amt;
                        s.scale *= 1.0 + (sc - 1.0) * amt;
                        s.rot += rot * amt;
                        if let Some(c) = color {
                            s.color = Some((c, amt));
                        }
                    }
                }
                kind => {
                    let start = if !times.is_empty() {
                        times.get(ui).copied().unwrap_or(0.0)
                    } else if kind == "out" {
                        item.dur - delay - total + rank * stagger
                    } else {
                        delay + rank * stagger
                    };
                    let p = if dur <= 1e-6 {
                        if tl >= start {
                            1.0
                        } else {
                            0.0
                        }
                    } else {
                        ((tl - start) / dur).clamp(0.0, 1.0)
                    };
                    let e = ease.apply(p) as f32;
                    let amt = if kind == "out" { e } else { 1.0 - e };
                    if amt <= 0.0 {
                        continue;
                    }
                    s.opacity *= 1.0 + (o_to - 1.0) * amt.clamp(0.0, 1.0);
                    s.dx += x * amt;
                    s.dy += y * amt;
                    s.scale *= 1.0 + (sc - 1.0) * amt;
                    s.rot += rot * amt;
                    s.blur = s.blur.max(blur * amt);
                    s.track += trk * amt;
                    if let Some(c) = color {
                        s.color = Some((c, amt.min(1.0)));
                    }
                }
            }
        }
        // spaces inherit the state of the previous glyph (for char units)
        if unit == "char" {
            for gi in 1..lay.glyphs.len() {
                if lay.glyphs[gi].ch == usize::MAX {
                    st[gi] = st[gi - 1];
                }
            }
        }
    }
    st
}

/// Glyph transform (layer space) for a state; tracking spreads glyphs from the line center.
pub fn glyph_transform(lay: &Layout, gi: usize, s: &GState, align: u8) -> Transform {
    let g = &lay.glyphs[gi];
    let mut tx = s.dx;
    if s.track != 0.0 {
        let n = lay.lines[g.line].n.max(1) as f32;
        let af = match align {
            0 => 0.0,
            2 => 1.0,
            _ => 0.5,
        };
        tx += s.track * (g.line_pos as f32 - af * (n - 1.0));
    }
    Transform::from_translate(g.cx + tx, g.cy + s.dy).pre_rotate(s.rot).pre_scale(s.scale, s.scale).pre_translate(-g.cx, -g.cy)
}

/// Background boxes (per line or block) in layer space.
pub fn bg_rects(lay: &Layout, pad: f32, block: bool) -> Vec<Rect> {
    if lay.lines.is_empty() {
        return vec![];
    }
    if block {
        let l = lay.lines.iter().filter(|l| l.n > 0).map(|l| l.x).fold(f32::INFINITY, f32::min);
        let r = lay.lines.iter().filter(|l| l.n > 0).map(|l| l.x + l.w).fold(f32::NEG_INFINITY, f32::max);
        if !l.is_finite() {
            return vec![];
        }
        return Rect::from_ltrb(l - pad, -pad * 0.5, r + pad, lay.h + pad * 0.5).into_iter().collect();
    }
    lay.lines.iter().filter(|l| l.n > 0 && l.w > 0.0).filter_map(|l| Rect::from_ltrb(l.x - pad, l.top - pad * 0.35, l.x + l.w + pad, l.top + l.h + pad * 0.35)).collect()
}

/// Rounded rectangle path.
pub fn rounded_rect(r: Rect, rad: f32) -> Option<Path> {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    if rad < 0.01 {
        return Some(PathBuilder::from_rect(r));
    }
    let k = 0.5523 * rad;
    let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rr - rad, t);
    pb.cubic_to(rr - rad + k, t, rr, t + rad - k, rr, t + rad);
    pb.line_to(rr, b - rad);
    pb.cubic_to(rr, b - rad + k, rr - rad + k, b, rr - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    pb.finish()
}

/// Substitutes `{expr}` placeholders with evaluated numbers.
pub fn expand_placeholders(s: &str, tl: f64, ctx: &crate::model::expr::ExprCtx) -> String {
    if !s.contains('{') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find('}') {
            let code = &after[..j];
            match crate::model::expr::eval_num(code, tl, ctx) {
                Ok(v) => out.push_str(&format_num(v)),
                Err(_) => {
                    out.push('{');
                    out.push_str(code);
                    out.push('}');
                }
            }
            rest = &after[j + 1..];
        } else {
            out.push_str(&rest[i..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn format_num(v: f64) -> String {
    let s = crate::model::value::fmt_num(v);
    // thousands separators for big integers
    if v.abs() >= 10000.0 && !s.contains('.') {
        let neg = s.starts_with('-');
        let digits: Vec<char> = s.trim_start_matches('-').chars().collect();
        let mut o = String::new();
        for (i, c) in digits.iter().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                o.push(',');
            }
            o.push(*c);
        }
        return if neg { format!("-{o}") } else { o };
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> TextStyle {
        TextStyle { family: "Inter".into(), weight: 700.0, italic: false, size: 100.0, align: 1, line_h: 1.2, tracking: 0.0, box_w: 0.0, caps: false }
    }

    #[test]
    fn lays_out_and_wraps() {
        std::env::set_var("MOODCUT_NO_SYSTEM_FONTS", "1");
        let l = layout("Hello world", &style());
        assert_eq!(l.n_chars, 10);
        assert_eq!(l.n_words, 2);
        assert!(l.w > 300.0 && l.w < 800.0, "{}", l.w);
        assert!(l.combined.is_some());
        let mut st = style();
        st.box_w = 350.0;
        let l2 = layout("Hello world again", &st);
        assert!(l2.lines.len() >= 2);
        let l3 = layout("one\ntwo", &style());
        assert_eq!(l3.lines.len(), 2);
    }

    #[test]
    fn placeholders() {
        let ctx = crate::model::expr::ExprCtx { fps: 30.0, ..Default::default() };
        assert_eq!(expand_placeholders("${round(time*100)}k", 1.5, &ctx), "$150k");
        assert_eq!(expand_placeholders("{12345.0}", 0.0, &ctx), "12,345");
    }
}
