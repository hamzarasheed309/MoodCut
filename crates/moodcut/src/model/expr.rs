//! A tiny After-Effects-like expression language for animatable properties.
//!
//! Examples: `time*90`, `value + [0, sin(time*6.28)*20]`, `wiggle(2, 30)`,
//! `linear(time, 0, 2, 0, 100)`, `random(10)`, `clamp(value*2, 0, 1)`.

use super::value::{Color, Val};
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// Context variables available to expressions.
#[derive(Clone, Debug, Default)]
pub struct ExprCtx {
    pub dur: f64,
    pub fps: f64,
    pub index: f64,
    pub w: f64,
    pub h: f64,
    pub cw: f64,
    pub ch: f64,
    pub seed: u64,
}

#[derive(Clone, Debug)]
pub enum EV {
    N(f64),
    V(Vec<f64>),
}

impl EV {
    fn num(&self) -> f64 {
        match self {
            EV::N(n) => *n,
            EV::V(v) => v.first().copied().unwrap_or(0.0),
        }
    }
    fn truthy(&self) -> bool {
        self.num() != 0.0
    }
    fn map(&self, f: impl Fn(f64) -> f64) -> EV {
        match self {
            EV::N(n) => EV::N(f(*n)),
            EV::V(v) => EV::V(v.iter().map(|x| f(*x)).collect()),
        }
    }
    fn zip(&self, o: &EV, f: impl Fn(f64, f64) -> f64) -> EV {
        match (self, o) {
            (EV::N(a), EV::N(b)) => EV::N(f(*a, *b)),
            (EV::V(a), EV::N(b)) => EV::V(a.iter().map(|x| f(*x, *b)).collect()),
            (EV::N(a), EV::V(b)) => EV::V(b.iter().map(|x| f(*a, *x)).collect()),
            (EV::V(a), EV::V(b)) => {
                let n = a.len().max(b.len());
                EV::V((0..n).map(|i| f(*a.get(i).unwrap_or(&0.0), *b.get(i).unwrap_or(&0.0))).collect())
            }
        }
    }
    fn dims(&self) -> usize {
        match self {
            EV::N(_) => 1,
            EV::V(v) => v.len(),
        }
    }
    fn get(&self, i: usize) -> f64 {
        match self {
            EV::N(n) => *n,
            EV::V(v) => *v.get(i).unwrap_or(&0.0),
        }
    }
}

#[derive(Debug, Clone)]
enum Node {
    Num(f64),
    Var(String),
    Vec(Vec<Node>),
    Un(char, Box<Node>),
    Bin(String, Box<Node>, Box<Node>),
    Tern(Box<Node>, Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
    Index(Box<Node>, Box<Node>),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Id(String),
    Op(String),
}

fn lex(src: &str) -> Result<Vec<Tok>> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = vec![];
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() || ch == ';' {
            i += 1;
            continue;
        }
        if ch.is_ascii_digit() || (ch == '.' && i + 1 < c.len() && c[i + 1].is_ascii_digit()) {
            let st = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                i += 1;
            }
            if i < c.len() && (c[i] == 'e' || c[i] == 'E') && i + 1 < c.len() && (c[i + 1].is_ascii_digit() || c[i + 1] == '-') {
                i += 2;
                while i < c.len() && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let s: String = c[st..i].iter().collect();
            out.push(Tok::Num(s.parse().map_err(|_| anyhow!("bad number '{s}'"))?));
            continue;
        }
        if ch.is_alphabetic() || ch == '_' || ch == '$' {
            let st = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '.' || c[i] == '$') {
                i += 1;
            }
            let s: String = c[st..i].iter().collect();
            out.push(Tok::Id(s));
            continue;
        }
        let two: String = c[i..(i + 2).min(c.len())].iter().collect();
        if ["<=", ">=", "==", "!=", "&&", "||", "**"].contains(&two.as_str()) {
            out.push(Tok::Op(two));
            i += 2;
            continue;
        }
        if "+-*/%^()[],?:<>!".contains(ch) {
            out.push(Tok::Op(ch.to_string()));
            i += 1;
            continue;
        }
        bail!("unexpected character '{ch}' in expression");
    }
    Ok(out)
}

struct Parser {
    t: Vec<Tok>,
    p: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.p)
    }
    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(o)) if o == op)
    }
    fn eat(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.p += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, op: &str) -> Result<()> {
        if self.eat(op) {
            Ok(())
        } else {
            bail!("expected '{op}' in expression")
        }
    }
    fn expr(&mut self) -> Result<Node> {
        let c = self.bin(0)?;
        if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            return Ok(Node::Tern(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }
    fn prec(op: &str) -> Option<u8> {
        Some(match op {
            "||" => 1,
            "&&" => 2,
            "==" | "!=" => 3,
            "<" | ">" | "<=" | ">=" => 4,
            "+" | "-" => 5,
            "*" | "/" | "%" => 6,
            _ => return None,
        })
    }
    fn bin(&mut self, min: u8) -> Result<Node> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) => o.clone(),
                _ => break,
            };
            let Some(p) = Self::prec(&op) else { break };
            if p < min {
                break;
            }
            self.p += 1;
            let rhs = self.bin(p + 1)?;
            lhs = Node::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn unary(&mut self) -> Result<Node> {
        if self.eat("-") {
            return Ok(Node::Un('-', Box::new(self.unary()?)));
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.eat("!") {
            return Ok(Node::Un('!', Box::new(self.unary()?)));
        }
        let base = self.postfix()?;
        if self.eat("^") || self.eat("**") {
            let e = self.unary()?;
            return Ok(Node::Bin("^".into(), Box::new(base), Box::new(e)));
        }
        Ok(base)
    }
    fn postfix(&mut self) -> Result<Node> {
        let mut n = self.primary()?;
        while self.eat("[") {
            let i = self.expr()?;
            self.expect("]")?;
            n = Node::Index(Box::new(n), Box::new(i));
        }
        Ok(n)
    }
    fn args(&mut self, close: &str) -> Result<Vec<Node>> {
        let mut a = vec![];
        if self.eat(close) {
            return Ok(a);
        }
        loop {
            a.push(self.expr()?);
            if self.eat(",") {
                continue;
            }
            self.expect(close)?;
            break;
        }
        Ok(a)
    }
    fn primary(&mut self) -> Result<Node> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.p += 1;
                Ok(Node::Num(n))
            }
            Some(Tok::Id(id)) => {
                self.p += 1;
                if self.eat("(") {
                    let a = self.args(")")?;
                    Ok(Node::Call(id.to_ascii_lowercase(), a))
                } else {
                    Ok(Node::Var(id))
                }
            }
            Some(Tok::Op(o)) if o == "(" => {
                self.p += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::Op(o)) if o == "[" => {
                self.p += 1;
                Ok(Node::Vec(self.args("]")?))
            }
            other => bail!("unexpected token {:?} in expression", other),
        }
    }
}

fn parse(src: &str) -> Result<Node> {
    let toks = lex(src)?;
    let mut p = Parser { t: toks, p: 0 };
    let n = p.expr()?;
    if p.p != p.t.len() {
        bail!("unexpected trailing tokens in expression '{src}'");
    }
    Ok(n)
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Node>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Node>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_parsed(src: &str) -> Result<Arc<Node>> {
    if let Some(n) = cache().lock().get(src) {
        return Ok(n.clone());
    }
    let n = Arc::new(parse(src)?);
    let mut c = cache().lock();
    if c.len() > 4096 {
        c.clear();
    }
    c.insert(src.to_string(), n.clone());
    Ok(n)
}

/// Validates an expression without evaluating it.
pub fn check(src: &str) -> Result<()> {
    let n = parse(src)?;
    // dry run with a dummy context to surface unknown names/functions
    let env = Env { value: EV::N(0.0), time: 0.0, ctx: &ExprCtx { fps: 30.0, ..Default::default() } };
    env.eval(&n).map(|_| ())
}

fn val_to_ev(v: &Val) -> EV {
    match v {
        Val::N(n) => EV::N(*n),
        Val::V(x, y) => EV::V(vec![*x, *y]),
        Val::C(c) => EV::V(vec![c.r as f64, c.g as f64, c.b as f64, c.a as f64]),
        Val::B(b) => EV::N(if *b { 1.0 } else { 0.0 }),
        Val::S(s) => EV::N(s.parse().unwrap_or(0.0)),
    }
}

fn ev_to_val(e: &EV, like: &Val) -> Val {
    match like {
        Val::N(_) => Val::N(e.num()),
        Val::V(..) => Val::V(e.get(0), if e.dims() > 1 { e.get(1) } else { e.get(0) }),
        Val::C(c) => {
            if e.dims() >= 3 {
                Val::C(Color::rgba(e.get(0) as f32, e.get(1) as f32, e.get(2) as f32, if e.dims() > 3 { e.get(3) as f32 } else { c.a }))
            } else {
                Val::C(*c)
            }
        }
        Val::B(_) => Val::B(e.truthy()),
        Val::S(_) => Val::S(super::value::fmt_num(e.num())),
    }
}

/// Evaluates an expression for a property whose keyed value is `value`.
pub fn eval_cached(src: &str, value: &Val, time: f64, ctx: &ExprCtx) -> Result<Val> {
    let n = get_parsed(src)?;
    let env = Env { value: val_to_ev(value), time, ctx };
    let r = env.eval(&n)?;
    Ok(ev_to_val(&r, value))
}

/// Evaluates an expression to a plain number (used by text `{expr}` placeholders).
pub fn eval_num(src: &str, time: f64, ctx: &ExprCtx) -> Result<f64> {
    let n = get_parsed(src)?;
    let env = Env { value: EV::N(0.0), time, ctx };
    Ok(env.eval(&n)?.num())
}

struct Env<'a> {
    value: EV,
    time: f64,
    ctx: &'a ExprCtx,
}

fn hash64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

/// Deterministic pseudo random in [0,1).
pub fn rand01(seed: u64, n: u64) -> f64 {
    (hash64(seed.wrapping_mul(0x9E3779B97F4A7C15) ^ hash64(n)) >> 11) as f64 / (1u64 << 53) as f64
}

/// Smooth 1D gradient noise in roughly [-1, 1].
pub fn noise1(x: f64, seed: u64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let g = |k: f64| rand01(seed, (k as i64) as u64) * 2.0 - 1.0;
    let a = g(i) * f;
    let b = g(i + 1.0) * (f - 1.0);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    (a + (b - a) * u) * 2.0
}

impl<'a> Env<'a> {
    fn var(&self, name: &str) -> Result<EV> {
        let c = self.ctx;
        Ok(EV::N(match name {
            "time" | "t" => self.time,
            "value" | "v" => return Ok(self.value.clone()),
            "dur" | "duration" => c.dur,
            "fps" => c.fps,
            "frame" => (self.time * c.fps).floor(),
            "index" => c.index,
            "w" | "width" => c.w,
            "h" | "height" => c.h,
            "cw" | "comp.w" | "thisComp.width" | "comp.width" => c.cw,
            "ch" | "comp.h" | "thisComp.height" | "comp.height" => c.ch,
            "pi" | "PI" | "Math.PI" => std::f64::consts::PI,
            "e" | "E" => std::f64::consts::E,
            "seed" => (c.seed % 100000) as f64,
            _ => bail!("unknown variable '{name}' (have: time, value, dur, fps, frame, index, w, h, cw, ch, pi)"),
        }))
    }

    fn eval(&self, n: &Node) -> Result<EV> {
        Ok(match n {
            Node::Num(x) => EV::N(*x),
            Node::Var(v) => self.var(v)?,
            Node::Vec(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval(it)?.num());
                }
                EV::V(out)
            }
            Node::Un(op, a) => {
                let v = self.eval(a)?;
                match op {
                    '-' => v.map(|x| -x),
                    _ => EV::N(if v.truthy() { 0.0 } else { 1.0 }),
                }
            }
            Node::Index(a, i) => {
                let v = self.eval(a)?;
                let i = self.eval(i)?.num() as usize;
                EV::N(v.get(i))
            }
            Node::Tern(c, a, b) => {
                if self.eval(c)?.truthy() {
                    self.eval(a)?
                } else {
                    self.eval(b)?
                }
            }
            Node::Bin(op, a, b) => {
                let x = self.eval(a)?;
                let y = self.eval(b)?;
                let bool_n = |b: bool| if b { 1.0 } else { 0.0 };
                match op.as_str() {
                    "+" => x.zip(&y, |a, b| a + b),
                    "-" => x.zip(&y, |a, b| a - b),
                    "*" => x.zip(&y, |a, b| a * b),
                    "/" => x.zip(&y, |a, b| if b == 0.0 { 0.0 } else { a / b }),
                    "%" => x.zip(&y, |a, b| if b == 0.0 { 0.0 } else { a.rem_euclid(b) }),
                    "^" => x.zip(&y, f64::powf),
                    "<" => EV::N(bool_n(x.num() < y.num())),
                    ">" => EV::N(bool_n(x.num() > y.num())),
                    "<=" => EV::N(bool_n(x.num() <= y.num())),
                    ">=" => EV::N(bool_n(x.num() >= y.num())),
                    "==" => EV::N(bool_n((x.num() - y.num()).abs() < 1e-9)),
                    "!=" => EV::N(bool_n((x.num() - y.num()).abs() >= 1e-9)),
                    "&&" => EV::N(bool_n(x.truthy() && y.truthy())),
                    "||" => EV::N(bool_n(x.truthy() || y.truthy())),
                    _ => bail!("bad operator {op}"),
                }
            }
            Node::Call(f, args) => self.call(f, args)?,
        })
    }

    fn call(&self, f: &str, args: &[Node]) -> Result<EV> {
        let a: Vec<EV> = args.iter().map(|n| self.eval(n)).collect::<Result<_>>()?;
        let n = |i: usize| a.get(i).map(|v| v.num()).unwrap_or(0.0);
        let need = |k: usize| -> Result<()> {
            if a.len() < k {
                bail!("{f}() needs {k} argument(s)")
            }
            Ok(())
        };
        let un = |g: fn(f64) -> f64| -> Result<EV> {
            need(1)?;
            Ok(a[0].map(g))
        };
        Ok(match f {
            "sin" => un(f64::sin)?,
            "cos" => un(f64::cos)?,
            "tan" => un(f64::tan)?,
            "asin" => un(f64::asin)?,
            "acos" => un(f64::acos)?,
            "atan" => un(f64::atan)?,
            "sqrt" => un(|x| x.max(0.0).sqrt())?,
            "abs" => un(f64::abs)?,
            "floor" => un(f64::floor)?,
            "ceil" => un(f64::ceil)?,
            "exp" => un(f64::exp)?,
            "log" => un(|x| if x > 0.0 { x.ln() } else { 0.0 })?,
            "log10" => un(|x| if x > 0.0 { x.log10() } else { 0.0 })?,
            "sign" => un(f64::signum)?,
            "fract" => un(|x| x - x.floor())?,
            "deg" | "radianstodegrees" => un(f64::to_degrees)?,
            "rad" | "degreestoradians" => un(f64::to_radians)?,
            "atan2" => {
                need(2)?;
                EV::N(n(0).atan2(n(1)))
            }
            "round" => {
                need(1)?;
                let p = 10f64.powi(n(1) as i32);
                a[0].map(|x| (x * p).round() / p)
            }
            "min" => {
                need(1)?;
                a.iter().skip(1).fold(a[0].clone(), |acc, v| acc.zip(v, f64::min))
            }
            "max" => {
                need(1)?;
                a.iter().skip(1).fold(a[0].clone(), |acc, v| acc.zip(v, f64::max))
            }
            "clamp" => {
                need(3)?;
                let lo = &a[1];
                let hi = &a[2];
                a[0].zip(lo, f64::max).zip(hi, f64::min)
            }
            "pow" => {
                need(2)?;
                a[0].zip(&a[1], f64::powf)
            }
            "mod" => {
                need(2)?;
                a[0].zip(&a[1], |x, y| if y == 0.0 { 0.0 } else { x.rem_euclid(y) })
            }
            "lerp" | "mix" => {
                need(3)?;
                let t = n(2);
                a[0].zip(&a[1], |x, y| x + (y - x) * t)
            }
            "step" => {
                need(2)?;
                EV::N(if n(1) >= n(0) { 1.0 } else { 0.0 })
            }
            "smoothstep" => {
                need(3)?;
                let t = ((n(2) - n(0)) / (n(1) - n(0))).clamp(0.0, 1.0);
                EV::N(t * t * (3.0 - 2.0 * t))
            }
            "linear" | "ease" | "easein" | "easeout" => {
                // linear(t, tMin, tMax, v1, v2) or linear(t, v1, v2) with t in 0..1
                let (t, t0, t1, v1, v2) = if a.len() >= 5 {
                    (n(0), n(1), n(2), a[3].clone(), a[4].clone())
                } else if a.len() == 3 {
                    (n(0), 0.0, 1.0, a[1].clone(), a[2].clone())
                } else {
                    bail!("{f}(t, tMin, tMax, v1, v2) or {f}(t, v1, v2)")
                };
                let mut p = if (t1 - t0).abs() < 1e-12 { 1.0 } else { ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) };
                p = match f {
                    "ease" => p * p * (3.0 - 2.0 * p),
                    "easein" => p * p,
                    "easeout" => 1.0 - (1.0 - p) * (1.0 - p),
                    _ => p,
                };
                v1.zip(&v2, |x, y| x + (y - x) * p)
            }
            "random" => {
                let frame = (self.time * self.ctx.fps.max(1.0)).floor() as i64 as u64;
                let base = |k: u64| rand01(self.ctx.seed ^ 0xABCD ^ k.wrapping_mul(7919), frame);
                match a.len() {
                    0 => EV::N(base(0)),
                    1 => match &a[0] {
                        EV::N(m) => EV::N(base(0) * m),
                        EV::V(v) => EV::V(v.iter().enumerate().map(|(i, m)| base(i as u64) * m).collect()),
                    },
                    _ => {
                        let lo = &a[0];
                        let hi = &a[1];
                        let dims = lo.dims().max(hi.dims());
                        if dims == 1 {
                            EV::N(n(0) + base(0) * (n(1) - n(0)))
                        } else {
                            EV::V((0..dims).map(|i| lo.get(i) + base(i as u64) * (hi.get(i) - lo.get(i))).collect())
                        }
                    }
                }
            }
            "seedrandom" | "hold" => EV::N(0.0),
            "noise" => {
                need(1)?;
                a[0].map(|x| noise1(x, self.ctx.seed))
            }
            "wiggle" => {
                need(2)?;
                let freq = n(0);
                let amp = &a[1];
                let oct = if a.len() > 2 { n(2).max(1.0) as usize } else { 1 };
                let mult = if a.len() > 3 { n(3) } else { 0.5 };
                let t = if a.len() > 4 { n(4) } else { self.time };
                let off = |d: usize| {
                    let mut s = 0.0;
                    let mut am = 1.0;
                    let mut fr = 1.0;
                    for o in 0..oct {
                        s += am * noise1(t * freq * fr + d as f64 * 31.7 + o as f64 * 7.1, self.ctx.seed.wrapping_add(d as u64 * 1013));
                        am *= mult;
                        fr *= 2.0;
                    }
                    s * amp.get(d.min(amp.dims() - 1))
                };
                match &self.value {
                    EV::N(v) => EV::N(v + off(0)),
                    EV::V(v) => EV::V(v.iter().enumerate().map(|(d, x)| x + off(d)).collect()),
                }
            }
            "pt" | "posterizetime" => {
                need(1)?;
                let f = n(0).max(0.001);
                EV::N((self.time * f).floor() / f)
            }
            "len" | "length" => {
                need(1)?;
                match &a[0] {
                    EV::N(x) => EV::N(x.abs()),
                    EV::V(v) => EV::N(v.iter().map(|x| x * x).sum::<f64>().sqrt()),
                }
            }
            "norm" | "normalize" => {
                need(1)?;
                match &a[0] {
                    EV::N(x) => EV::N(x.signum()),
                    EV::V(v) => {
                        let l = v.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
                        EV::V(v.iter().map(|x| x / l).collect())
                    }
                }
            }
            "vec" => EV::V(a.iter().map(|v| v.num()).collect()),
            "bounce" => {
                // bounce(freq, amp): |sin| based bounce, decaying
                need(2)?;
                let t = self.time;
                EV::N((t * n(0) * std::f64::consts::PI).sin().abs() * n(1) * (-t * 2.0).exp())
            }
            _ => bail!("unknown function '{f}()'"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ExprCtx {
        ExprCtx { fps: 30.0, dur: 5.0, cw: 1920.0, ch: 1080.0, seed: 42, ..Default::default() }
    }

    #[test]
    fn arithmetic() {
        let v = eval_cached("1 + 2*3 - 4/2", &Val::N(0.0), 0.0, &ctx()).unwrap();
        assert_eq!(v, Val::N(5.0));
        let v = eval_cached("time*90", &Val::N(0.0), 2.0, &ctx()).unwrap();
        assert_eq!(v, Val::N(180.0));
        let v = eval_cached("value + [10, 20]", &Val::V(1.0, 2.0), 0.0, &ctx()).unwrap();
        assert_eq!(v, Val::V(11.0, 22.0));
        let v = eval_cached("time > 1 ? 5 : 2^3", &Val::N(0.0), 0.5, &ctx()).unwrap();
        assert_eq!(v, Val::N(8.0));
        let v = eval_cached("linear(time, 0, 2, 0, 100)", &Val::N(0.0), 1.0, &ctx()).unwrap();
        assert_eq!(v, Val::N(50.0));
        let v = eval_cached("value[1]", &Val::V(3.0, 4.0), 0.0, &ctx()).unwrap();
        assert_eq!(v, Val::V(4.0, 4.0));
    }

    #[test]
    fn wiggle_deterministic() {
        let a = eval_cached("wiggle(2, 30)", &Val::V(100.0, 100.0), 1.234, &ctx()).unwrap();
        let b = eval_cached("wiggle(2, 30)", &Val::V(100.0, 100.0), 1.234, &ctx()).unwrap();
        assert_eq!(a, b);
        let (x, y) = a.vec();
        assert!((x - 100.0).abs() <= 60.0 && (y - 100.0).abs() <= 60.0);
        assert!(x != y);
    }

    #[test]
    fn errors() {
        assert!(check("foo(1)").is_err());
        assert!(check("1 +").is_err());
        assert!(check("wiggle(1, 2)").is_ok());
    }
}
