//! Mattes: rasterized mask alpha shared between the mask cut and every effect
//! window that uses it, plus mask expressions (`m1+m2`, `m1*m2`, `-m1`).
//!
//! A static mask is rasterized once per render (the key doesn't change from
//! frame to frame); an animated one once per frame instead of once per effect.
//! Pixel-driven masks (qualifiers, refined edges) depend on the layer's pixels at the
//! point in the effect chain where they are used, so they are never cached.

use super::eval::{hash_str, Ev};
use super::masks::{self, LBounds};
use super::pixel::{IRect, LayerBuf};
use crate::model::project::Mask;
use crate::model::schema;
use anyhow::{bail, Result};
use parking_lot::Mutex;
use resvg::tiny_skia::Transform;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write;
use std::sync::{Arc, OnceLock};

/// A rasterized matte the size of the layer buffer it was made for.
pub struct Matte {
    pub alpha: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// Buffer-local bounding box of the non-zero pixels (None when empty).
    pub bbox: Option<IRect>,
}

impl Matte {
    pub fn new(alpha: Vec<u8>, w: u32, h: u32) -> Matte {
        let bbox = bbox_of(&alpha, w as usize, h as usize);
        Matte { alpha, w, h, bbox }
    }
}

/// Bounding box of non-zero bytes in a w x h buffer.
pub fn bbox_of(a: &[u8], w: usize, h: usize) -> Option<IRect> {
    if w == 0 || h == 0 || a.len() < w * h {
        return None;
    }
    let mut top = None;
    let mut bottom = 0;
    let (mut l, mut r) = (w, 0usize);
    for y in 0..h {
        let row = &a[y * w..(y + 1) * w];
        let Some(first) = row.iter().position(|&v| v != 0) else { continue };
        let last = row.iter().rposition(|&v| v != 0).unwrap_or(first);
        top.get_or_insert(y);
        bottom = y;
        l = l.min(first);
        r = r.max(last);
    }
    let top = top?;
    Some(IRect::from_ltrb(l as i32, top as i32, r as i32 + 1, bottom as i32 + 1))
}

struct Cache {
    map: HashMap<u64, Arc<Matte>>,
    order: VecDeque<u64>,
    bytes: usize,
}

const MAX_BYTES: usize = 160 << 20;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache { map: HashMap::new(), order: VecDeque::new(), bytes: 0 }))
}

/// Cache key: everything that shapes the raster, evaluated at the frame time.
fn key(lb: &LayerBuf, mk: &Mask, mev: &Ev, b: LBounds, m: Transform, scale: f32) -> u64 {
    let mut s = String::with_capacity(320);
    let _ = write!(s, "{}|{}|{},{},{},{}|", mk.kind, mk.on, lb.x, lb.y, lb.w(), lb.h());
    let _ = write!(s, "{:?},{:?},{:?},{:?},{:?},{:?}|", m.sx, m.kx, m.ky, m.sy, m.tx, m.ty);
    let _ = write!(s, "{:?},{:?},{:?},{:?},{:?}|", b.x, b.y, b.w, b.h, scale);
    for p in schema::MASK {
        let _ = write!(s, "{:?};", mev.val(&mk.props, p.name));
    }
    hash_str(&s)
}

/// The matte for one mask (feather, opacity and invert included), cached
/// unless it depends on the layer's pixels.
pub fn mask(lb: &LayerBuf, mk: &Mask, ev: &Ev, b: LBounds, m: Transform, scale: f32) -> Option<Arc<Matte>> {
    let mev = ev.with_lists(&[schema::MASK]);
    let (w, h) = (lb.w(), lb.h());
    // pixel-driven (qualifiers, refine edge) and motion-blurred mattes depend
    // on more than the props at this instant
    if masks::uses_pixels(mk, &mev) || masks::motion_blurred(mk, &mev) {
        return masks::mask_alpha(lb, mk, &mev, b, m, scale).map(|a| Arc::new(Matte::new(a, w, h)));
    }
    let k = key(lb, mk, &mev, b, m, scale);
    if let Some(hit) = cache().lock().map.get(&k) {
        return Some(hit.clone());
    }
    let mt = Arc::new(Matte::new(masks::mask_alpha(lb, mk, &mev, b, m, scale)?, w, h));
    let mut c = cache().lock();
    let size = mt.alpha.len();
    if size <= MAX_BYTES / 4 {
        while c.bytes + size > MAX_BYTES {
            let Some(old) = c.order.pop_front() else { break };
            if let Some(x) = c.map.remove(&old) {
                c.bytes -= x.alpha.len();
            }
        }
        if c.map.insert(k, mt.clone()).is_none() {
            c.order.push_back(k);
            c.bytes += size;
        }
    }
    Some(mt)
}

/// Drops every cached matte (tests and memory pressure).
pub fn clear() {
    let mut c = cache().lock();
    c.map.clear();
    c.order.clear();
    c.bytes = 0;
}

// ---------------------------------------------------------------- expressions

/// A mask expression: `m1`, `m1+m2` (union), `m1*m2` (intersection), `-m1`
/// (outside), with parentheses.
#[derive(Clone, Debug, PartialEq)]
pub enum MExpr {
    Id(String),
    Not(Box<MExpr>),
    Union(Box<MExpr>, Box<MExpr>),
    Inter(Box<MExpr>, Box<MExpr>),
}

impl MExpr {
    /// Mask ids the expression references.
    pub fn ids(&self) -> Vec<&str> {
        match self {
            MExpr::Id(s) => vec![s.as_str()],
            MExpr::Not(a) => a.ids(),
            MExpr::Union(a, b) | MExpr::Inter(a, b) => {
                let mut v = a.ids();
                v.extend(b.ids());
                v
            }
        }
    }
}

/// Parses a `mask=` value. `Ok(None)` for "", none and off.
pub fn parse(src: &str) -> Result<Option<MExpr>> {
    let s = src.trim();
    if matches!(s.to_ascii_lowercase().as_str(), "" | "none" | "off") {
        return Ok(None);
    }
    // two ids with only spaces between them ("m1 m2") are a mistake, not "m1m2"
    let chars: Vec<char> = s.chars().collect();
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (k, c) in chars.iter().enumerate() {
        if c.is_whitespace() && k > 0 && word(chars[k - 1]) {
            if let Some(next) = chars[k..].iter().find(|c| !c.is_whitespace()) {
                if word(*next) {
                    bail!("bad mask expression '{src}': join masks with + (union) or * (intersection), e.g. m1+m2");
                }
            }
        }
    }
    let toks: Vec<char> = chars.into_iter().filter(|c| !c.is_whitespace()).collect();
    let mut i = 0;
    let e = parse_union(&toks, &mut i)?;
    if i != toks.len() {
        bail!("bad mask expression '{src}' (use m1, m1+m2, m1*m2, -m1)");
    }
    Ok(Some(e))
}

fn parse_union(t: &[char], i: &mut usize) -> Result<MExpr> {
    let mut a = parse_inter(t, i)?;
    while *i < t.len() && matches!(t[*i], '+' | '|') {
        *i += 1;
        let b = parse_inter(t, i)?;
        a = MExpr::Union(Box::new(a), Box::new(b));
    }
    Ok(a)
}

fn parse_inter(t: &[char], i: &mut usize) -> Result<MExpr> {
    let mut a = parse_unary(t, i)?;
    while *i < t.len() && matches!(t[*i], '*' | '&') {
        *i += 1;
        let b = parse_unary(t, i)?;
        a = MExpr::Inter(Box::new(a), Box::new(b));
    }
    Ok(a)
}

fn parse_unary(t: &[char], i: &mut usize) -> Result<MExpr> {
    match t.get(*i) {
        Some('-') | Some('!') | Some('~') => {
            *i += 1;
            Ok(MExpr::Not(Box::new(parse_unary(t, i)?)))
        }
        Some('(') => {
            *i += 1;
            let e = parse_union(t, i)?;
            if t.get(*i) != Some(&')') {
                bail!("mask expression: missing ')'");
            }
            *i += 1;
            Ok(e)
        }
        Some(c) if c.is_ascii_alphanumeric() || *c == '_' => {
            let st = *i;
            while *i < t.len() && (t[*i].is_ascii_alphanumeric() || t[*i] == '_') {
                *i += 1;
            }
            Ok(MExpr::Id(t[st..*i].iter().collect()))
        }
        _ => bail!("mask expression: expected a mask id (m1) at position {}", *i + 1),
    }
}

/// Evaluates a mask expression to a matte for `lb`. Returns None when a
/// referenced mask is missing or can't be rasterized; an off mask is empty.
pub fn eval(e: &MExpr, lb: &LayerBuf, masks_list: &[Mask], ev: &Ev, b: LBounds, m: Transform, scale: f32) -> Option<Arc<Matte>> {
    let (w, h) = (lb.w(), lb.h());
    match e {
        MExpr::Id(id) => {
            let mk = masks_list.iter().find(|mk| mk.id.eq_ignore_ascii_case(id))?;
            if !mk.on {
                return Some(Arc::new(Matte { alpha: vec![0; (w * h) as usize], w, h, bbox: None }));
            }
            let mt = mask(lb, mk, ev, b, m, scale)?;
            // a subtract-mode mask used as a window means "outside it"
            let mev = ev.with_lists(&[schema::MASK]);
            if mev.str(&mk.props, "mode") == "subtract" {
                return Some(Arc::new(Matte::new(mt.alpha.iter().map(|v| 255 - v).collect(), w, h)));
            }
            Some(mt)
        }
        MExpr::Not(a) => {
            let x = eval(a, lb, masks_list, ev, b, m, scale)?;
            Some(Arc::new(Matte::new(x.alpha.iter().map(|v| 255 - v).collect(), w, h)))
        }
        MExpr::Union(a, bb) => {
            let (x, y) = (eval(a, lb, masks_list, ev, b, m, scale)?, eval(bb, lb, masks_list, ev, b, m, scale)?);
            Some(Arc::new(Matte::new(x.alpha.iter().zip(&y.alpha).map(|(p, q)| (*p).max(*q)).collect(), w, h)))
        }
        MExpr::Inter(a, bb) => {
            let (x, y) = (eval(a, lb, masks_list, ev, b, m, scale)?, eval(bb, lb, masks_list, ev, b, m, scale)?);
            Some(Arc::new(Matte::new(x.alpha.iter().zip(&y.alpha).map(|(p, q)| masks::mul255(*p, *q)).collect(), w, h)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expressions() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("none").unwrap(), None);
        assert_eq!(parse("m1").unwrap(), Some(MExpr::Id("m1".into())));
        let e = parse("m1+m2*-m3").unwrap().unwrap();
        assert_eq!(e.ids(), vec!["m1", "m2", "m3"]);
        assert!(matches!(e, MExpr::Union(..)));
        let e = parse("(m1 + m2) * m3").unwrap().unwrap();
        assert!(matches!(e, MExpr::Inter(..)));
        assert!(parse("m1+").is_err());
        assert!(parse("(m1").is_err());
        assert!(parse("m1 m2").is_err());
    }

    #[test]
    fn bbox_finds_nonzero_region() {
        let mut a = vec![0u8; 10 * 8];
        a[3 * 10 + 2] = 9;
        a[5 * 10 + 6] = 1;
        assert_eq!(bbox_of(&a, 10, 8), Some(IRect::from_ltrb(2, 3, 7, 6)));
        assert_eq!(bbox_of(&vec![0u8; 80], 10, 8), None);
    }
}
