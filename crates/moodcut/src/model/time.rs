//! Time parsing/formatting for the edit DSL.
//!
//! Accepted forms: `12`, `12.5s`, `500ms`, `90f` (frames), `1:02.5` (m:s),
//! `1:02:03.5` (h:m:s), `00:00:12:15` (timecode h:m:s:frames), `end`,
//! `c3.end` / `c3.start` / `c3.mid`, `@marker`, `50%` (of item duration where
//! meaningful), and sums like `c3.end+0.5` or `end-2`.

use super::value::fmt_num;
use anyhow::{anyhow, bail, Result};

pub struct TimeCtx<'a> {
    pub fps: f64,
    pub comp_end: f64,
    /// Resolves an item reference to (start, end).
    pub item: &'a dyn Fn(&str) -> Option<(f64, f64)>,
    /// Resolves a marker name to its time.
    pub marker: &'a dyn Fn(&str) -> Option<f64>,
    /// Duration that `%` refers to.
    pub pct_of: Option<f64>,
}

pub fn parse_time(s: &str, cx: &TimeCtx) -> Result<f64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty time");
    }
    // split into signed terms
    let chars: Vec<char> = s.chars().collect();
    let mut terms: Vec<(f64, String)> = vec![];
    let mut cur = String::new();
    let mut sign = 1.0;
    for (i, &c) in chars.iter().enumerate() {
        if (c == '+' || c == '-') && i > 0 && !matches!(chars[i - 1], 'e' | 'E' | '+' | '-') {
            terms.push((sign, std::mem::take(&mut cur)));
            sign = if c == '-' { -1.0 } else { 1.0 };
        } else if (c == '+' || c == '-') && i == 0 {
            sign = if c == '-' { -1.0 } else { 1.0 };
        } else {
            cur.push(c);
        }
    }
    terms.push((sign, cur));
    let mut total = 0.0;
    for (sg, t) in terms {
        total += sg * term(&t, cx)?;
    }
    Ok(total)
}

fn term(t: &str, cx: &TimeCtx) -> Result<f64> {
    let t = t.trim();
    let l = t.to_ascii_lowercase();
    if l.is_empty() {
        bail!("bad time expression");
    }
    if l == "end" || l == "out" {
        return Ok(cx.comp_end);
    }
    if l == "start" || l == "begin" || l == "0" {
        return Ok(0.0);
    }
    if let Some(name) = t.strip_prefix('@') {
        return (cx.marker)(name).ok_or_else(|| anyhow!("unknown marker '@{name}'"));
    }
    if let Some(p) = l.strip_suffix('%') {
        let v: f64 = p.parse().map_err(|_| anyhow!("bad percent '{t}'"))?;
        let base = cx.pct_of.ok_or_else(|| anyhow!("'%' times are only valid for keyframes/item-relative times"))?;
        return Ok(v / 100.0 * base);
    }
    if l.contains(':') {
        let parts: Vec<&str> = l.split(':').collect();
        let nums: Vec<f64> = parts.iter().map(|p| p.parse::<f64>()).collect::<std::result::Result<_, _>>().map_err(|_| anyhow!("bad time '{t}'"))?;
        return Ok(match nums.len() {
            2 => nums[0] * 60.0 + nums[1],
            3 => nums[0] * 3600.0 + nums[1] * 60.0 + nums[2],
            4 => nums[0] * 3600.0 + nums[1] * 60.0 + nums[2] + nums[3] / cx.fps.max(1.0),
            _ => bail!("bad time '{t}'"),
        });
    }
    // item reference: c3, c3.end, title.start
    let (base, field) = match l.rsplit_once('.') {
        Some((b, f)) if matches!(f, "end" | "start" | "mid" | "out" | "in") && !b.is_empty() && b.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) => {
            (&t[..b.len()], f)
        }
        _ => (t, ""),
    };
    if base.chars().next().map(|c| c.is_alphabetic() || c == '$').unwrap_or(false) {
        let (st, en) = (cx.item)(base).ok_or_else(|| anyhow!("unknown item '{base}' in time '{t}'"))?;
        return Ok(match field {
            "end" | "out" => en,
            "mid" => (st + en) / 2.0,
            _ => st,
        });
    }
    if let Some(n) = l.strip_suffix("ms") {
        return Ok(n.parse::<f64>().map_err(|_| anyhow!("bad time '{t}'"))? / 1000.0);
    }
    if let Some(n) = l.strip_suffix('f') {
        return Ok(n.parse::<f64>().map_err(|_| anyhow!("bad time '{t}'"))? / cx.fps.max(1.0));
    }
    if let Some(n) = l.strip_suffix("min") {
        return Ok(n.parse::<f64>().map_err(|_| anyhow!("bad time '{t}'"))? * 60.0);
    }
    let n = l.strip_suffix('s').unwrap_or(&l);
    n.parse::<f64>().map_err(|_| anyhow!("bad time '{t}' (use seconds like 12.5, 1:02.5, 90f, end, c3.end, @marker)"))
}

/// Compact seconds formatting used in AI-facing output.
pub fn fmt_t(t: f64) -> String {
    fmt_num((t * 1000.0).round() / 1000.0)
}

/// SMPTE-like timecode HH:MM:SS:FF.
pub fn fmt_tc(t: f64, fps: f64) -> String {
    let fps_i = fps.round().max(1.0) as i64;
    let total_frames = (t * fps).round() as i64;
    let f = total_frames % fps_i;
    let s = total_frames / fps_i;
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> TimeCtx<'static> {
        TimeCtx {
            fps: 30.0,
            comp_end: 60.0,
            item: &|k| if k == "c3" { Some((10.0, 20.0)) } else { None },
            marker: &|m| if m == "drop" { Some(32.0) } else { None },
            pct_of: Some(4.0),
        }
    }

    #[test]
    fn forms() {
        let c = cx();
        assert_eq!(parse_time("12.5", &c).unwrap(), 12.5);
        assert_eq!(parse_time("500ms", &c).unwrap(), 0.5);
        assert_eq!(parse_time("90f", &c).unwrap(), 3.0);
        assert_eq!(parse_time("1:02.5", &c).unwrap(), 62.5);
        assert_eq!(parse_time("00:00:01:15", &c).unwrap(), 1.5);
        assert_eq!(parse_time("end", &c).unwrap(), 60.0);
        assert_eq!(parse_time("end-2", &c).unwrap(), 58.0);
        assert_eq!(parse_time("c3.end+0.5", &c).unwrap(), 20.5);
        assert_eq!(parse_time("c3.mid", &c).unwrap(), 15.0);
        assert_eq!(parse_time("@drop", &c).unwrap(), 32.0);
        assert_eq!(parse_time("50%", &c).unwrap(), 2.0);
        assert_eq!(parse_time("-1", &c).unwrap(), -1.0);
        assert!(parse_time("c9.end", &c).is_err());
    }

    #[test]
    fn tc() {
        assert_eq!(fmt_tc(61.5, 30.0), "00:01:01:15");
        assert_eq!(fmt_t(12.50000001), "12.5");
    }
}
