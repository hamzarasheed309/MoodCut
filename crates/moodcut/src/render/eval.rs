//! Property evaluation helpers: read an animatable prop (with schema default)
//! at a given local time.

use crate::model::expr::ExprCtx;
use crate::model::project::Props;
use crate::model::schema::{self, VCtx, P};
use crate::model::value::{Color, Val};

const EMPTY: &[P] = &[];

#[derive(Clone, Copy)]
pub struct Ev<'a> {
    pub t: f64,
    pub x: &'a ExprCtx,
    pub vc: VCtx,
    pub lists: [&'static [P]; 3],
}

impl<'a> Ev<'a> {
    pub fn new(t: f64, x: &'a ExprCtx, vc: VCtx, lists: &[&'static [P]]) -> Ev<'a> {
        let mut l = [EMPTY; 3];
        for (i, s) in lists.iter().take(3).enumerate() {
            l[i] = s;
        }
        Ev { t, x, vc, lists: l }
    }
    pub fn val(&self, props: &Props, name: &str) -> Val {
        if let Some(a) = props.get(name) {
            return a.eval(self.t, self.x);
        }
        for l in self.lists {
            if let Some(p) = schema::find(l, name) {
                return schema::default_val(p, self.vc);
            }
        }
        Val::N(0.0)
    }
    pub fn num(&self, props: &Props, name: &str) -> f64 {
        self.val(props, name).num()
    }
    pub fn f(&self, props: &Props, name: &str) -> f32 {
        self.num(props, name) as f32
    }
    pub fn vec(&self, props: &Props, name: &str) -> (f64, f64) {
        self.val(props, name).vec()
    }
    pub fn color(&self, props: &Props, name: &str) -> Color {
        self.val(props, name).color()
    }
    pub fn str(&self, props: &Props, name: &str) -> String {
        self.val(props, name).str()
    }
    pub fn bool(&self, props: &Props, name: &str) -> bool {
        self.val(props, name).bool()
    }
    pub fn with_t(&self, t: f64) -> Ev<'a> {
        Ev { t, ..*self }
    }
    pub fn with_lists(&self, lists: &[&'static [P]]) -> Ev<'a> {
        Ev::new(self.t, self.x, self.vc, lists)
    }
}

/// Stable 64-bit hash of a string (FNV-1a) for seeds and cache keys.
pub fn hash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
