//! Primitive value types shared by the whole engine: colors, 2D vectors and the
//! dynamically-typed property value `Val` used by schema-driven property bags.

use anyhow::{anyhow, bail, Result};
use serde::de::{self, Deserializer, Visitor};
use serde::ser::{SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Straight-alpha RGBA color, channels in 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Color::BLACK
    }
}

impl Color {
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    pub fn from_u8(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 }
    }

    pub fn to_u8(&self) -> [u8; 4] {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [c(self.r), c(self.g), c(self.b), c(self.a)]
    }

    /// Premultiplied 8-bit representation (what tiny-skia stores).
    pub fn to_premul_u8(&self) -> [u8; 4] {
        let a = self.a.clamp(0.0, 1.0);
        let c = |v: f32| (v.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
        [c(self.r), c(self.g), c(self.b), (a * 255.0 + 0.5) as u8]
    }

    pub fn to_skia(&self) -> resvg::tiny_skia::Color {
        resvg::tiny_skia::Color::from_rgba(
            self.r.clamp(0.0, 1.0),
            self.g.clamp(0.0, 1.0),
            self.b.clamp(0.0, 1.0),
            self.a.clamp(0.0, 1.0),
        )
        .unwrap_or(resvg::tiny_skia::Color::BLACK)
    }

    pub fn with_alpha(&self, a: f32) -> Color {
        Color { a, ..*self }
    }

    pub fn lerp(&self, o: &Color, t: f32) -> Color {
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }

    pub fn luma(&self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }

    pub fn to_hex(&self) -> String {
        let [r, g, b, a] = self.to_u8();
        if a == 255 {
            format!("#{:02x}{:02x}{:02x}", r, g, b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a)
        }
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r,g,b)`, `rgba(r,g,b,a)`,
    /// CSS-ish names, and `none`/`transparent`.
    pub fn parse(s: &str) -> Result<Color> {
        let s = s.trim();
        let lower = s.to_ascii_lowercase();
        if let Some(hex) = lower.strip_prefix('#') {
            return Self::parse_hex(hex).ok_or_else(|| anyhow!("bad color '{s}'"));
        }
        if let Some(inner) = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb(")) {
            let inner = inner.trim_end_matches(')');
            let parts: Vec<f32> = inner
                .split(',')
                .map(|p| p.trim().parse::<f32>())
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| anyhow!("bad color '{s}'"))?;
            if parts.len() < 3 {
                bail!("bad color '{s}'");
            }
            let a = parts.get(3).copied().unwrap_or(1.0);
            let a = if a > 1.0 { a / 255.0 } else { a };
            return Ok(Color::rgba(parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0, a));
        }
        let named = match lower.as_str() {
            "none" | "transparent" | "clear" => "00000000",
            "black" => "000000",
            "white" => "ffffff",
            "red" => "ff3b30",
            "green" => "34c759",
            "blue" => "0a84ff",
            "yellow" => "ffd60a",
            "orange" => "ff9f0a",
            "purple" => "bf5af2",
            "pink" => "ff2d55",
            "cyan" => "32d3ff",
            "magenta" => "ff00ff",
            "teal" => "30b0c7",
            "gray" | "grey" => "8e8e93",
            "darkgray" | "darkgrey" => "3a3a3c",
            "lightgray" | "lightgrey" => "d1d1d6",
            "gold" => "ffc83d",
            "navy" => "0b1f4b",
            "lime" => "a6ff00",
            "brown" => "8b5a2b",
            _ => {
                // bare hex without '#'
                if (lower.len() == 6 || lower.len() == 8) && lower.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Self::parse_hex(&lower).ok_or_else(|| anyhow!("bad color '{s}'"));
                }
                bail!("unknown color '{s}' (use #rrggbb, #rrggbbaa, rgb(), or a name like white/black/red)")
            }
        };
        Ok(Self::parse_hex(named).unwrap())
    }

    fn parse_hex(hex: &str) -> Option<Color> {
        let h = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
        match hex.len() {
            3 => Some(Color::from_u8(h(0, 1)? * 17, h(1, 1)? * 17, h(2, 1)? * 17, 255)),
            4 => Some(Color::from_u8(h(0, 1)? * 17, h(1, 1)? * 17, h(2, 1)? * 17, h(3, 1)? * 17)),
            6 => Some(Color::from_u8(h(0, 2)?, h(2, 2)?, h(4, 2)?, 255)),
            8 => Some(Color::from_u8(h(0, 2)?, h(2, 2)?, h(4, 2)?, h(6, 2)?)),
            _ => None,
        }
    }

    /// True when the string is unambiguously a hex color literal.
    pub fn looks_like_hex(s: &str) -> bool {
        if let Some(h) = s.strip_prefix('#') {
            matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit())
        } else {
            false
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).map_err(de::Error::custom)
    }
}

/// Dynamically typed property value.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    N(f64),
    V(f64, f64),
    C(Color),
    S(String),
    B(bool),
}

impl Default for Val {
    fn default() -> Self {
        Val::N(0.0)
    }
}

impl Val {
    pub fn num(&self) -> f64 {
        match self {
            Val::N(n) => *n,
            Val::V(x, _) => *x,
            Val::B(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            Val::C(c) => c.a as f64,
            Val::S(s) => s.parse().unwrap_or(0.0),
        }
    }

    pub fn vec(&self) -> (f64, f64) {
        match self {
            Val::V(x, y) => (*x, *y),
            Val::N(n) => (*n, *n),
            _ => (0.0, 0.0),
        }
    }

    pub fn color(&self) -> Color {
        match self {
            Val::C(c) => *c,
            Val::S(s) => Color::parse(s).unwrap_or(Color::TRANSPARENT),
            _ => Color::TRANSPARENT,
        }
    }

    pub fn str(&self) -> String {
        match self {
            Val::S(s) => s.clone(),
            Val::N(n) => fmt_num(*n),
            Val::V(x, y) => format!("{},{}", fmt_num(*x), fmt_num(*y)),
            Val::C(c) => c.to_hex(),
            Val::B(b) => b.to_string(),
        }
    }

    pub fn bool(&self) -> bool {
        match self {
            Val::B(b) => *b,
            Val::N(n) => *n != 0.0,
            Val::S(s) => matches!(s.as_str(), "1" | "true" | "yes" | "on"),
            _ => false,
        }
    }

    pub fn is_interpolable(&self) -> bool {
        matches!(self, Val::N(_) | Val::V(..) | Val::C(_))
    }

    /// Interpolates between two values. Non-interpolable values hold `self` until t>=1.
    pub fn lerp(&self, o: &Val, t: f64) -> Val {
        match (self, o) {
            (Val::N(a), Val::N(b)) => Val::N(a + (b - a) * t),
            (Val::V(ax, ay), Val::V(bx, by)) => Val::V(ax + (bx - ax) * t, ay + (by - ay) * t),
            (Val::N(a), Val::V(bx, by)) => Val::V(a + (bx - a) * t, a + (by - a) * t),
            (Val::V(ax, ay), Val::N(b)) => Val::V(ax + (b - ax) * t, ay + (b - ay) * t),
            (Val::C(a), Val::C(b)) => Val::C(a.lerp(b, t as f32)),
            _ => {
                if t >= 1.0 {
                    o.clone()
                } else {
                    self.clone()
                }
            }
        }
    }

    pub fn display(&self) -> String {
        match self {
            Val::S(s) => {
                if s.contains(' ') || s.is_empty() {
                    format!("'{}'", s.replace('\'', "\\'"))
                } else {
                    s.clone()
                }
            }
            other => other.str(),
        }
    }
}

/// Formats a number compactly: up to 3 decimals, trailing zeros removed.
pub fn fmt_num(n: f64) -> String {
    if !n.is_finite() {
        return "0".into();
    }
    if (n - n.round()).abs() < 1e-9 {
        return format!("{}", n.round() as i64);
    }
    let s = format!("{:.3}", n);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

impl Serialize for Val {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Val::N(n) => {
                if (n - n.round()).abs() < 1e-12 && n.abs() < 1e15 {
                    s.serialize_i64(n.round() as i64)
                } else {
                    s.serialize_f64(*n)
                }
            }
            Val::V(x, y) => {
                let mut seq = s.serialize_seq(Some(2))?;
                seq.serialize_element(x)?;
                seq.serialize_element(y)?;
                seq.end()
            }
            Val::C(c) => s.serialize_str(&c.to_hex()),
            Val::S(st) => s.serialize_str(st),
            Val::B(b) => s.serialize_bool(*b),
        }
    }
}

impl<'de> Deserialize<'de> for Val {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Val;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("number, [x,y], color string, string or bool")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Val, E> {
                Ok(Val::B(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Val, E> {
                Ok(Val::N(v as f64))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Val, E> {
                Ok(Val::N(v as f64))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Val, E> {
                Ok(Val::N(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Val, E> {
                if Color::looks_like_hex(v) {
                    if let Ok(c) = Color::parse(v) {
                        return Ok(Val::C(c));
                    }
                }
                Ok(Val::S(v.to_string()))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Val, A::Error> {
                let x: f64 = seq.next_element()?.unwrap_or(0.0);
                let y: f64 = seq.next_element()?.unwrap_or(x);
                while seq.next_element::<serde_json::Value>()?.is_some() {}
                Ok(Val::V(x, y))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parse() {
        assert_eq!(Color::parse("#fff").unwrap().to_hex(), "#ffffff");
        assert_eq!(Color::parse("#ff000080").unwrap().to_u8()[3], 128);
        assert_eq!(Color::parse("rgb(255,0,0)").unwrap().to_hex(), "#ff0000");
        assert!(Color::parse("nope").is_err());
        assert_eq!(Color::parse("none").unwrap().a, 0.0);
    }

    #[test]
    fn val_roundtrip() {
        let v: Vec<Val> = serde_json::from_str(r##"[1, 2.5, [3,4], "#ff0000", "hi", true]"##).unwrap();
        assert_eq!(v[0], Val::N(1.0));
        assert_eq!(v[2], Val::V(3.0, 4.0));
        assert!(matches!(v[3], Val::C(_)));
        assert_eq!(v[4], Val::S("hi".into()));
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, r##"[1,2.5,[3.0,4.0],"#ff0000","hi",true]"##);
    }

    #[test]
    fn fmt() {
        assert_eq!(fmt_num(12.5), "12.5");
        assert_eq!(fmt_num(3.0), "3");
        assert_eq!(fmt_num(0.33333), "0.333");
        assert_eq!(fmt_num(-0.0001), "0");
    }
}
