//! Animatable properties: static value, keyframes with easing, optional loop mode
//! and optional expression (see `expr.rs`).

use super::expr::{self, ExprCtx};
use super::value::{fmt_num, Val};
use anyhow::{anyhow, bail, Result};
use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Quad,
    Cubic,
    Quart,
    Quint,
    Sine,
    Expo,
    Circ,
    Back,
    Elastic,
    Bounce,
}

/// Easing applied from a keyframe to the next one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Linear,
    Hold,
    In(Curve),
    Out(Curve),
    InOut(Curve),
    Bezier(f64, f64, f64, f64),
}

impl Default for Ease {
    fn default() -> Self {
        Ease::Linear
    }
}

const CURVES: &[(&str, Curve)] = &[
    ("quad", Curve::Quad),
    ("cubic", Curve::Cubic),
    ("quart", Curve::Quart),
    ("quint", Curve::Quint),
    ("sine", Curve::Sine),
    ("expo", Curve::Expo),
    ("circ", Curve::Circ),
    ("back", Curve::Back),
    ("elastic", Curve::Elastic),
    ("bounce", Curve::Bounce),
];

fn curve_name(c: Curve) -> &'static str {
    CURVES.iter().find(|(_, x)| *x == c).map(|(n, _)| *n).unwrap_or("cubic")
}

impl Ease {
    pub fn parse(s: &str) -> Result<Ease> {
        let raw = s.trim();
        let l = raw.to_ascii_lowercase().replace(['-', '_', ' '], "");
        if l.is_empty() || l == "linear" || l == "lin" {
            return Ok(Ease::Linear);
        }
        if l == "hold" || l == "step" || l == "constant" {
            return Ok(Ease::Hold);
        }
        if let Some(args) = l.strip_prefix("bezier(").or_else(|| l.strip_prefix("cubicbezier(")).or_else(|| l.strip_prefix("cb(")) {
            let nums: Vec<f64> = args
                .trim_end_matches(')')
                .split(',')
                .map(|p| p.trim().parse::<f64>())
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| anyhow!("bad bezier '{raw}'"))?;
            if nums.len() != 4 {
                bail!("bezier needs 4 numbers: bezier(x1,y1,x2,y2)");
            }
            return Ok(Ease::Bezier(nums[0], nums[1], nums[2], nums[3]));
        }
        // AE / CSS style names
        match l.as_str() {
            "ease" | "easyease" | "smooth" => return Ok(Ease::Bezier(0.33, 0.0, 0.67, 1.0)),
            "easein" | "easyeasein" => return Ok(Ease::Bezier(0.42, 0.0, 1.0, 1.0)),
            "easeout" | "easyeaseout" => return Ok(Ease::Bezier(0.0, 0.0, 0.58, 1.0)),
            "easeinout" => return Ok(Ease::Bezier(0.42, 0.0, 0.58, 1.0)),
            "in" => return Ok(Ease::In(Curve::Cubic)),
            "out" => return Ok(Ease::Out(Curve::Cubic)),
            "inout" => return Ok(Ease::InOut(Curve::Cubic)),
            "snap" | "fast" => return Ok(Ease::Bezier(0.2, 0.9, 0.1, 1.0)),
            "anticipate" => return Ok(Ease::Bezier(0.36, 0.0, 0.66, -0.56)),
            "overshoot" => return Ok(Ease::Bezier(0.34, 1.56, 0.64, 1.0)),
            _ => {}
        }
        // [ease](in|out|inout)<curve>  or bare curve name (= out)
        let body = l.strip_prefix("ease").unwrap_or(&l);
        let (dir, rest) = if let Some(r) = body.strip_prefix("inout") {
            (2, r)
        } else if let Some(r) = body.strip_prefix("in") {
            (0, r)
        } else if let Some(r) = body.strip_prefix("out") {
            (1, r)
        } else {
            (1, body)
        };
        if let Some((_, c)) = CURVES.iter().find(|(n, _)| *n == rest) {
            return Ok(match dir {
                0 => Ease::In(*c),
                1 => Ease::Out(*c),
                _ => Ease::InOut(*c),
            });
        }
        bail!("unknown ease '{raw}' (try linear, hold, in, out, inout, ease, outBack, inOutSine, outElastic, outBounce, bezier(x1,y1,x2,y2))")
    }

    pub fn name(&self) -> String {
        match self {
            Ease::Linear => "linear".into(),
            Ease::Hold => "hold".into(),
            Ease::In(c) => format!("in{}", cap(curve_name(*c))),
            Ease::Out(c) => format!("out{}", cap(curve_name(*c))),
            Ease::InOut(c) => format!("inOut{}", cap(curve_name(*c))),
            Ease::Bezier(a, b, c, d) => {
                if (*a, *b, *c, *d) == (0.33, 0.0, 0.67, 1.0) {
                    "ease".into()
                } else {
                    format!("bezier({},{},{},{})", fmt_num(*a), fmt_num(*b), fmt_num(*c), fmt_num(*d))
                }
            }
        }
    }

    /// Maps linear progress p in [0,1] to eased progress.
    pub fn apply(&self, p: f64) -> f64 {
        let p = p.clamp(0.0, 1.0);
        match self {
            Ease::Linear => p,
            Ease::Hold => {
                if p >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Ease::In(c) => ease_in(*c, p),
            Ease::Out(c) => 1.0 - ease_in(*c, 1.0 - p),
            Ease::InOut(c) => {
                if p < 0.5 {
                    ease_in(*c, p * 2.0) / 2.0
                } else {
                    1.0 - ease_in(*c, (1.0 - p) * 2.0) / 2.0
                }
            }
            Ease::Bezier(x1, y1, x2, y2) => cubic_bezier(*x1, *y1, *x2, *y2, p),
        }
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn ease_in(c: Curve, p: f64) -> f64 {
    match c {
        Curve::Quad => p * p,
        Curve::Cubic => p * p * p,
        Curve::Quart => p.powi(4),
        Curve::Quint => p.powi(5),
        Curve::Sine => 1.0 - (p * PI / 2.0).cos(),
        Curve::Expo => {
            if p <= 0.0 {
                0.0
            } else {
                (2f64).powf(10.0 * p - 10.0)
            }
        }
        Curve::Circ => 1.0 - (1.0 - p * p).max(0.0).sqrt(),
        Curve::Back => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            c3 * p * p * p - c1 * p * p
        }
        Curve::Elastic => {
            if p <= 0.0 || p >= 1.0 {
                p
            } else {
                let c4 = (2.0 * PI) / 3.0;
                -(2f64.powf(10.0 * p - 10.0)) * ((p * 10.0 - 10.75) * c4).sin()
            }
        }
        Curve::Bounce => 1.0 - bounce_out(1.0 - p),
    }
}

fn bounce_out(x: f64) -> f64 {
    let n1 = 7.5625;
    let d1 = 2.75;
    if x < 1.0 / d1 {
        n1 * x * x
    } else if x < 2.0 / d1 {
        let x = x - 1.5 / d1;
        n1 * x * x + 0.75
    } else if x < 2.5 / d1 {
        let x = x - 2.25 / d1;
        n1 * x * x + 0.9375
    } else {
        let x = x - 2.625 / d1;
        n1 * x * x + 0.984375
    }
}

/// CSS-style cubic bezier timing function (same algorithm as WebKit's UnitBezier).
pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;
    let sample_x = |t: f64| ((ax * t + bx) * t + cx) * t;
    let sample_y = |t: f64| ((ay * t + by) * t + cy) * t;
    let deriv_x = |t: f64| (3.0 * ax * t + 2.0 * bx) * t + cx;
    // Newton-Raphson
    let mut t = x;
    for _ in 0..8 {
        let err = sample_x(t) - x;
        if err.abs() < 1e-7 {
            return sample_y(t);
        }
        let d = deriv_x(t);
        if d.abs() < 1e-6 {
            break;
        }
        t -= err / d;
    }
    // Bisection fallback
    let (mut lo, mut hi) = (0.0, 1.0);
    t = x;
    for _ in 0..40 {
        let v = sample_x(t);
        if (v - x).abs() < 1e-7 {
            break;
        }
        if x > v {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    sample_y(t)
}

impl Serialize for Ease {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.name())
    }
}

impl<'de> Deserialize<'de> for Ease {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ease::parse(&s).map_err(de::Error::custom)
    }
}

/// A keyframe. `t` is relative to the owning object's start (seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub t: f64,
    pub v: Val,
    pub e: Ease,
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let n = if self.e == Ease::Linear { 2 } else { 3 };
        let mut seq = s.serialize_seq(Some(n))?;
        seq.serialize_element(&self.t)?;
        seq.serialize_element(&self.v)?;
        if n == 3 {
            seq.serialize_element(&self.e)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw: Vec<serde_json::Value> = Vec::deserialize(d)?;
        if raw.len() < 2 {
            return Err(de::Error::custom("keyframe must be [t, v, ease?]"));
        }
        let t = raw[0].as_f64().ok_or_else(|| de::Error::custom("keyframe time must be a number"))?;
        let v: Val = serde_json::from_value(raw[1].clone()).map_err(de::Error::custom)?;
        let e = match raw.get(2) {
            Some(serde_json::Value::String(s)) => Ease::parse(s).map_err(de::Error::custom)?,
            _ => Ease::Linear,
        };
        Ok(Key { t, v, e })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoopMode {
    Cycle,
    Pingpong,
    Offset,
}

/// Animatable value.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Anim {
    /// Static value (used when there are no keys).
    pub v: Val,
    /// Keyframes sorted by time.
    pub k: Vec<Key>,
    /// Optional expression evaluated on top of the keyed value (`value` variable).
    pub x: Option<String>,
    /// Loop keyframes after the last key.
    pub lp: Option<LoopMode>,
}

impl Anim {
    pub fn new(v: Val) -> Anim {
        Anim { v, k: vec![], x: None, lp: None }
    }

    pub fn is_animated(&self) -> bool {
        !self.k.is_empty() || self.x.is_some()
    }

    pub fn set_keys(&mut self, mut keys: Vec<Key>) {
        keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        keys.dedup_by(|b, a| (a.t - b.t).abs() < 1e-6 && {
            // keep the later one's value
            a.v = b.v.clone();
            a.e = b.e;
            true
        });
        if let Some(first) = keys.first() {
            self.v = first.v.clone();
        }
        self.k = keys;
    }

    pub fn add_key(&mut self, key: Key) {
        let mut keys = std::mem::take(&mut self.k);
        keys.retain(|k| (k.t - key.t).abs() > 1e-6);
        keys.push(key);
        self.set_keys(keys);
    }

    /// Keyframed value (without expression) at local time t.
    pub fn keyed(&self, t: f64) -> Val {
        let k = &self.k;
        if k.is_empty() {
            return self.v.clone();
        }
        if k.len() == 1 {
            return k[0].v.clone();
        }
        let first = k[0].t;
        let last = k[k.len() - 1].t;
        let mut tt = t;
        let mut offset_cycles = 0.0;
        if t > last {
            if let Some(lp) = self.lp {
                let span = last - first;
                if span > 1e-9 {
                    let rel = t - first;
                    let n = (rel / span).floor();
                    let frac = rel - n * span;
                    tt = match lp {
                        LoopMode::Cycle | LoopMode::Offset => first + frac,
                        LoopMode::Pingpong => {
                            if (n as i64) % 2 == 1 {
                                last - frac
                            } else {
                                first + frac
                            }
                        }
                    };
                    if lp == LoopMode::Offset {
                        offset_cycles = n;
                    }
                }
            } else {
                return k[k.len() - 1].v.clone();
            }
        }
        if tt <= first {
            return k[0].v.clone();
        }
        // binary search segment
        let idx = match k.binary_search_by(|key| key.t.partial_cmp(&tt).unwrap_or(std::cmp::Ordering::Less)) {
            Ok(i) => return add_offset(&k[i].v, &k[0].v, &k[k.len() - 1].v, offset_cycles),
            Err(i) => i, // k[i-1].t < tt < k[i].t
        };
        if idx >= k.len() {
            return add_offset(&k[k.len() - 1].v, &k[0].v, &k[k.len() - 1].v, offset_cycles);
        }
        let a = &k[idx - 1];
        let b = &k[idx];
        let span = b.t - a.t;
        let p = if span <= 1e-12 { 1.0 } else { (tt - a.t) / span };
        let e = a.e.apply(p);
        add_offset(&a.v.lerp(&b.v, e), &k[0].v, &k[k.len() - 1].v, offset_cycles)
    }

    /// Full evaluation (keys + expression).
    pub fn eval(&self, t: f64, ctx: &ExprCtx) -> Val {
        let base = self.keyed(t);
        match &self.x {
            Some(src) => match expr::eval_cached(src, &base, t, ctx) {
                Ok(v) => v,
                Err(_) => base,
            },
            None => base,
        }
    }

    /// Time of the last keyframe, if any.
    pub fn last_key_t(&self) -> Option<f64> {
        self.k.last().map(|k| k.t)
    }

    /// Shifts all keyframes by dt (used when splitting/trimming objects).
    pub fn shift(&mut self, dt: f64) {
        for k in &mut self.k {
            k.t += dt;
        }
    }

    /// Compact text form for outlines: `v`, or `t:v@ease t:v ...`, plus `~expr`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.k.is_empty() {
            s.push_str(&self.v.display());
        } else {
            let parts: Vec<String> = self
                .k
                .iter()
                .map(|k| {
                    if k.e == Ease::Linear {
                        format!("{}:{}", fmt_num(k.t), k.v.display())
                    } else {
                        format!("{}:{}@{}", fmt_num(k.t), k.v.display(), k.e.name())
                    }
                })
                .collect();
            s.push_str(&parts.join(" "));
            if let Some(lp) = self.lp {
                s.push_str(&format!(" loop={:?}", lp).to_lowercase());
            }
        }
        if let Some(x) = &self.x {
            s.push_str(&format!(" ~'{}'", x));
        }
        s
    }
}

fn add_offset(v: &Val, first: &Val, last: &Val, cycles: f64) -> Val {
    if cycles == 0.0 {
        return v.clone();
    }
    match (v, first, last) {
        (Val::N(x), Val::N(f), Val::N(l)) => Val::N(x + (l - f) * cycles),
        (Val::V(x, y), Val::V(fx, fy), Val::V(lx, ly)) => Val::V(x + (lx - fx) * cycles, y + (ly - fy) * cycles),
        _ => v.clone(),
    }
}

impl From<Val> for Anim {
    fn from(v: Val) -> Self {
        Anim::new(v)
    }
}

impl Serialize for Anim {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        if self.k.is_empty() && self.x.is_none() {
            return self.v.serialize(s);
        }
        let mut m = s.serialize_map(None)?;
        if self.k.is_empty() {
            m.serialize_entry("v", &self.v)?;
        } else {
            m.serialize_entry("k", &self.k)?;
        }
        if let Some(x) = &self.x {
            m.serialize_entry("x", x)?;
        }
        if let Some(lp) = &self.lp {
            m.serialize_entry("loop", lp)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for Anim {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(d)?;
        if let serde_json::Value::Object(map) = &raw {
            if map.contains_key("k") || map.contains_key("x") || map.contains_key("v") {
                let mut a = Anim::default();
                if let Some(v) = map.get("v") {
                    a.v = serde_json::from_value(v.clone()).map_err(de::Error::custom)?;
                }
                if let Some(k) = map.get("k") {
                    let keys: Vec<Key> = serde_json::from_value(k.clone()).map_err(de::Error::custom)?;
                    a.set_keys(keys);
                }
                if let Some(serde_json::Value::String(x)) = map.get("x") {
                    a.x = Some(x.clone());
                }
                if let Some(l) = map.get("loop") {
                    a.lp = serde_json::from_value(l.clone()).ok();
                }
                return Ok(a);
            }
        }
        let v: Val = serde_json::from_value(raw).map_err(de::Error::custom)?;
        Ok(Anim::new(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_parse_names() {
        assert_eq!(Ease::parse("outBack").unwrap(), Ease::Out(Curve::Back));
        assert_eq!(Ease::parse("ease-in-out-sine").unwrap(), Ease::InOut(Curve::Sine));
        assert_eq!(Ease::parse("in").unwrap(), Ease::In(Curve::Cubic));
        assert_eq!(Ease::parse("bounce").unwrap(), Ease::Out(Curve::Bounce));
        assert!(matches!(Ease::parse("bezier(.2,.8,.2,1)").unwrap(), Ease::Bezier(..)));
        assert!(Ease::parse("wobble").is_err());
    }

    #[test]
    fn ease_endpoints() {
        for e in [Ease::Out(Curve::Back), Ease::InOut(Curve::Expo), Ease::Bezier(0.33, 0.0, 0.67, 1.0), Ease::Out(Curve::Bounce)] {
            assert!((e.apply(0.0)).abs() < 1e-6, "{:?}", e);
            assert!((e.apply(1.0) - 1.0).abs() < 1e-6, "{:?}", e);
        }
        let mid = Ease::Bezier(0.42, 0.0, 0.58, 1.0).apply(0.5);
        assert!((mid - 0.5).abs() < 1e-3);
    }

    #[test]
    fn keyed_interp() {
        let mut a = Anim::new(Val::N(0.0));
        a.set_keys(vec![
            Key { t: 0.0, v: Val::N(0.0), e: Ease::Linear },
            Key { t: 2.0, v: Val::N(100.0), e: Ease::Linear },
        ]);
        assert_eq!(a.keyed(1.0), Val::N(50.0));
        assert_eq!(a.keyed(-1.0), Val::N(0.0));
        assert_eq!(a.keyed(5.0), Val::N(100.0));
        a.lp = Some(LoopMode::Cycle);
        assert_eq!(a.keyed(3.0), Val::N(50.0));
        a.lp = Some(LoopMode::Pingpong);
        assert_eq!(a.keyed(3.0), Val::N(50.0));
        assert_eq!(a.keyed(2.5), Val::N(75.0));
    }

    #[test]
    fn anim_serde() {
        let a: Anim = serde_json::from_str("5").unwrap();
        assert_eq!(a.v, Val::N(5.0));
        let b: Anim = serde_json::from_str(r#"{"k":[[0,0],[1,10,"outBack"]]}"#).unwrap();
        assert_eq!(b.k.len(), 2);
        assert_eq!(b.k[1].e, Ease::Out(Curve::Back));
        let s = serde_json::to_string(&b).unwrap();
        assert_eq!(s, r#"{"k":[[0.0,0],[1.0,10,"outBack"]]}"#);
    }
}
