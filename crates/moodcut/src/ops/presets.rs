//! Animation, text-animator and color presets. These turn one short command
//! (`anim c3 popIn`) into a full set of keyframes/expressions, which is the main
//! way MoodCut keeps AI edits token-efficient.

use crate::model::anim::{Anim, Ease, Key};
use crate::model::project::{Animator, Effect, Item, Props};
use crate::model::schema::{self, VCtx};
use crate::model::value::{fmt_num, Color, Val};
use anyhow::{bail, Result};

#[derive(Clone, Debug, Default)]
pub struct AnimOpts {
    pub dur: Option<f64>,
    pub delay: f64,
    pub ease: Option<Ease>,
    pub amount: Option<f64>,
    pub freq: Option<f64>,
    /// Item-relative time for one-shot presets (punch, shakeHit).
    pub at: Option<f64>,
    pub dist: Option<f64>,
}

pub const LAYER_ENTER: &[(&str, &str)] = &[
    ("fadeIn", "fade from transparent"),
    ("popIn", "scale from 0 with overshoot"),
    ("zoomIn", "scale down from 160% + fade"),
    ("growIn", "scale up from 60% + fade"),
    ("slideInLeft|Right|Up|Down", "slide in from that side (dist=220) + fade"),
    ("flyInLeft|Right|Up|Down", "fly in from off-screen"),
    ("riseIn", "rise 60px + fade"),
    ("dropIn", "drop from above with bounce"),
    ("spinIn", "spin + scale from 0"),
    ("flipIn", "3D flip around Y"),
    ("swingIn", "3D swing around X"),
    ("blurIn", "un-blur + fade"),
    ("stretchIn", "stretch horizontally from 0"),
];
pub const LAYER_EXIT: &[(&str, &str)] = &[
    ("fadeOut", "fade to transparent"),
    ("popOut", "scale to 0 (anticipation)"),
    ("zoomOut", "scale up to 160% + fade"),
    ("shrinkOut", "scale to 60% + fade"),
    ("slideOutLeft|Right|Up|Down", "slide toward that side + fade"),
    ("flyOutLeft|Right|Up|Down", "fly off-screen"),
    ("sinkOut", "sink 60px + fade"),
    ("spinOut", "spin + scale to 0"),
    ("flipOut", "3D flip out"),
    ("blurOut", "blur + fade"),
];
pub const LAYER_LOOP: &[(&str, &str)] = &[
    ("pulse", "gentle scale pulse (amount=0.05 freq=1)"),
    ("heartbeat", "double-beat scale"),
    ("float", "bob up/down (amount=12 freq=0.5)"),
    ("bounce", "bouncing (amount=40 freq=1.5)"),
    ("spin", "continuous rotation (freq = turns/sec)"),
    ("swing", "pendulum rotation (amount=8 deg)"),
    ("shake", "continuous jitter (amount=10 freq=12)"),
    ("wiggle", "smooth random drift (amount=20 freq=1.5)"),
    ("breathe", "opacity breathing"),
    ("flicker", "random flicker"),
];
pub const LAYER_ONESHOT: &[(&str, &str)] = &[
    ("kenburns", "slow zoom 100->112% + drift over the whole item"),
    ("punch", "quick scale punch at=T (beat hits)"),
    ("shakeHit", "camera-shake burst at=T"),
    ("zoomPunchIn", "fast zoom-in that holds (talking-head emphasis) at=T"),
];

pub const TEXT_ANIMS: &[(&str, &str)] = &[
    ("typewriter", "characters appear one by one"),
    ("fadeUp", "chars fade in rising 40px"),
    ("pop", "chars pop in with overshoot"),
    ("slideUp", "lines slide up + fade"),
    ("blurIn", "chars un-blur + fade"),
    ("wordPop", "words pop in one by one"),
    ("bounceIn", "chars drop in with bounce"),
    ("dropIn", "chars drop from above"),
    ("spinIn", "chars spin in"),
    ("trackIn", "letter spacing collapses + fade (cinematic)"),
    ("fadeChars", "chars fade in (no motion)"),
    ("wave", "continuous wave (loop)"),
    ("jitter", "continuous random jitter (loop)"),
    ("fadeDown", "exit: chars fade out sinking"),
    ("popOut", "exit: chars shrink away"),
    ("blurOut", "exit: chars blur out"),
    ("typeOut", "exit: chars disappear in reverse"),
    ("slideDown", "exit: lines slide down + fade"),
];

pub const COLOR_PRESETS: &[(&str, &str)] = &[
    ("cinematic", "contrasty teal shadows / warm highlights"),
    ("tealorange", "strong teal & orange blockbuster look"),
    ("vintage", "faded warm film with grain"),
    ("film", "subtle film emulation"),
    ("bw", "black & white"),
    ("noir", "high-contrast B&W with vignette"),
    ("warm", "warmer white balance"),
    ("cool", "cooler white balance"),
    ("punchy", "contrast + vibrance"),
    ("faded", "matte, low contrast"),
    ("moody", "dark, desaturated, cool"),
    ("airy", "bright & soft"),
    ("sunset", "golden warm"),
    ("cyberpunk", "magenta/cyan neon"),
    ("natural", "neutral clean correction"),
];

fn base(item: &Item, prop: &str, cx: VCtx) -> Val {
    if let Some(a) = item.props.get(prop) {
        return a.v.clone();
    }
    match schema::item_prop(item.kind, prop) {
        Some(p) => schema::default_val(p, cx),
        None => Val::N(0.0),
    }
}

fn merge(item: &mut Item, prop: &str, keys: Vec<Key>, cx: VCtx) {
    if keys.is_empty() {
        return;
    }
    let b = base(item, prop, cx);
    let a = item.props.entry(prop.to_string()).or_insert_with(|| Anim::new(b));
    let lo = keys.iter().map(|k| k.t).fold(f64::INFINITY, f64::min) - 1e-6;
    let hi = keys.iter().map(|k| k.t).fold(f64::NEG_INFINITY, f64::max) + 1e-6;
    let mut ks: Vec<Key> = a.k.iter().filter(|k| k.t < lo || k.t > hi).cloned().collect();
    ks.extend(keys);
    let v = a.v.clone();
    a.set_keys(ks);
    // keep the static value meaningful for outlines
    if a.k.is_empty() {
        a.v = v;
    }
}

fn set_expr(item: &mut Item, prop: &str, expr: String, cx: VCtx) {
    let b = base(item, prop, cx);
    let a = item.props.entry(prop.to_string()).or_insert_with(|| Anim::new(b));
    a.x = Some(expr);
}

fn k(t: f64, v: Val, e: Ease) -> Key {
    Key { t, v, e }
}

fn scale_v(v: &Val, f: f64) -> Val {
    let (x, y) = v.vec();
    Val::V(x * f, y * f)
}

fn add_v(v: &Val, dx: f64, dy: f64) -> Val {
    let (x, y) = v.vec();
    Val::V(x + dx, y + dy)
}

fn dir_vec(dir: &str) -> (f64, f64) {
    match dir {
        "left" => (-1.0, 0.0),
        "right" => (1.0, 0.0),
        "up" => (0.0, -1.0),
        _ => (0.0, 1.0),
    }
}

/// Splits names like slideInLeft -> ("slideIn", "left").
fn split_dir(name: &str) -> (String, Option<&'static str>) {
    let l = name.to_ascii_lowercase();
    for d in ["left", "right", "up", "down"] {
        if let Some(b) = l.strip_suffix(d) {
            if !b.is_empty() {
                return (b.to_string(), Some(match d {
                    "left" => "left",
                    "right" => "right",
                    "up" => "up",
                    _ => "down",
                }));
            }
        }
    }
    (l, None)
}

pub fn is_layer_anim(name: &str) -> bool {
    let (b, _) = split_dir(name);
    matches!(
        b.as_str(),
        "fadein" | "popin" | "zoomin" | "growin" | "slidein" | "flyin" | "risein" | "dropin" | "spinin" | "flipin" | "swingin" | "blurin" | "stretchin"
            | "fadeout" | "popout" | "zoomout" | "shrinkout" | "slideout" | "flyout" | "sinkout" | "spinout" | "flipout" | "blurout"
            | "pulse" | "heartbeat" | "float" | "bounce" | "spin" | "swing" | "shake" | "wiggle" | "breathe" | "flicker"
            | "kenburns" | "punch" | "shakehit" | "zoompunchin" | "none" | "clear"
    )
}

/// Applies a layer animation preset by writing keyframes/expressions.
pub fn apply_layer_anim(item: &mut Item, name: &str, o: &AnimOpts, cx: VCtx) -> Result<()> {
    let (b, dir) = split_dir(name);
    let d = o.dur.unwrap_or(match b.as_str() {
        "kenburns" => item.dur,
        "dropin" => 0.9,
        "popin" | "popout" => 0.45,
        _ => 0.5,
    })
    .max(0.01);
    let t0 = o.delay; // enter start
    let te = item.dur - o.delay; // exit end
    let ts = (te - d).max(0.0); // exit start
    let pos = base(item, "pos", cx);
    let sc = base(item, "scale", cx);
    let op = base(item, "opacity", cx);
    let rot = base(item, "rot", cx).num();
    let e_out = o.ease.unwrap_or(Ease::Out(crate::model::anim::Curve::Cubic));
    let e_in = o.ease.unwrap_or(Ease::In(crate::model::anim::Curve::Cubic));
    use crate::model::anim::Curve::*;
    let fade_in = |item: &mut Item, dd: f64| merge(item, "opacity", vec![k(t0, Val::N(0.0), Ease::Out(Quad)), k(t0 + dd, op.clone(), Ease::Linear)], cx);
    let fade_out = |item: &mut Item, dd: f64| merge(item, "opacity", vec![k(te - dd, op.clone(), Ease::In(Quad)), k(te, Val::N(0.0), Ease::Linear)], cx);
    match b.as_str() {
        "none" | "clear" => {
            for p in ["pos", "scale", "opacity", "rot", "rx", "ry"] {
                if let Some(a) = item.props.get_mut(p) {
                    a.k.clear();
                    a.x = None;
                }
            }
        }
        "fadein" => fade_in(item, d),
        "fadeout" => fade_out(item, d),
        "popin" => {
            merge(item, "scale", vec![k(t0, Val::V(0.0, 0.0), Ease::Out(Back)), k(t0 + d, sc.clone(), Ease::Linear)], cx);
            fade_in(item, d * 0.35);
        }
        "popout" => {
            merge(item, "scale", vec![k(ts, sc.clone(), Ease::In(Back)), k(te, Val::V(0.0, 0.0), Ease::Linear)], cx);
            fade_out(item, d * 0.35);
        }
        "zoomin" => {
            merge(item, "scale", vec![k(t0, scale_v(&sc, 1.6), Ease::Out(Expo)), k(t0 + d, sc.clone(), Ease::Linear)], cx);
            fade_in(item, d * 0.6);
        }
        "zoomout" => {
            merge(item, "scale", vec![k(ts, sc.clone(), Ease::In(Expo)), k(te, scale_v(&sc, 1.6), Ease::Linear)], cx);
            fade_out(item, d * 0.6);
        }
        "growin" => {
            merge(item, "scale", vec![k(t0, scale_v(&sc, 0.6), e_out), k(t0 + d, sc.clone(), Ease::Linear)], cx);
            fade_in(item, d * 0.7);
        }
        "shrinkout" => {
            merge(item, "scale", vec![k(ts, sc.clone(), e_in), k(te, scale_v(&sc, 0.6), Ease::Linear)], cx);
            fade_out(item, d * 0.7);
        }
        "slidein" | "flyin" => {
            let dir = dir.unwrap_or("left");
            let (dx, dy) = dir_vec(dir);
            let fly = b == "flyin";
            let dist = o.dist.unwrap_or(if fly { if dx != 0.0 { cx.cw } else { cx.ch } } else { 220.0 });
            // "from the left" means starting left of the target
            merge(item, "pos", vec![k(t0, add_v(&pos, dx * dist, dy * dist), Ease::Out(Expo)), k(t0 + d, pos.clone(), Ease::Linear)], cx);
            if !fly {
                fade_in(item, d * 0.6);
            }
        }
        "slideout" | "flyout" => {
            let dir = dir.unwrap_or("left");
            let (dx, dy) = dir_vec(dir);
            let fly = b == "flyout";
            let dist = o.dist.unwrap_or(if fly { if dx != 0.0 { cx.cw } else { cx.ch } } else { 220.0 });
            merge(item, "pos", vec![k(ts, pos.clone(), Ease::In(Expo)), k(te, add_v(&pos, dx * dist, dy * dist), Ease::Linear)], cx);
            if !fly {
                fade_out(item, d * 0.6);
            }
        }
        "risein" => {
            merge(item, "pos", vec![k(t0, add_v(&pos, 0.0, o.dist.unwrap_or(60.0)), e_out), k(t0 + d, pos.clone(), Ease::Linear)], cx);
            fade_in(item, d);
        }
        "sinkout" => {
            merge(item, "pos", vec![k(ts, pos.clone(), e_in), k(te, add_v(&pos, 0.0, o.dist.unwrap_or(60.0)), Ease::Linear)], cx);
            fade_out(item, d);
        }
        "dropin" => {
            merge(item, "pos", vec![k(t0, add_v(&pos, 0.0, -o.dist.unwrap_or(300.0)), Ease::Out(Bounce)), k(t0 + d, pos.clone(), Ease::Linear)], cx);
            fade_in(item, d * 0.2);
        }
        "spinin" => {
            merge(item, "rot", vec![k(t0, Val::N(rot - 180.0), Ease::Out(Back)), k(t0 + d, Val::N(rot), Ease::Linear)], cx);
            merge(item, "scale", vec![k(t0, Val::V(0.0, 0.0), Ease::Out(Back)), k(t0 + d, sc.clone(), Ease::Linear)], cx);
            fade_in(item, d * 0.3);
        }
        "spinout" => {
            merge(item, "rot", vec![k(ts, Val::N(rot), Ease::In(Back)), k(te, Val::N(rot + 180.0), Ease::Linear)], cx);
            merge(item, "scale", vec![k(ts, sc.clone(), Ease::In(Back)), k(te, Val::V(0.0, 0.0), Ease::Linear)], cx);
            fade_out(item, d * 0.3);
        }
        "flipin" => {
            merge(item, "ry", vec![k(t0, Val::N(90.0), e_out), k(t0 + d, Val::N(0.0), Ease::Linear)], cx);
            fade_in(item, d * 0.3);
        }
        "flipout" => {
            merge(item, "ry", vec![k(ts, Val::N(0.0), e_in), k(te, Val::N(-90.0), Ease::Linear)], cx);
            fade_out(item, d * 0.3);
        }
        "swingin" => {
            merge(item, "rx", vec![k(t0, Val::N(-90.0), Ease::Out(Back)), k(t0 + d, Val::N(0.0), Ease::Linear)], cx);
            fade_in(item, d * 0.3);
        }
        "stretchin" => {
            let (sx, sy) = sc.vec();
            merge(item, "scale", vec![k(t0, Val::V(0.0, sy), Ease::Out(Expo)), k(t0 + d, Val::V(sx, sy), Ease::Linear)], cx);
        }
        "blurin" | "blurout" => {
            let enter = b == "blurin";
            let r = o.amount.unwrap_or(40.0);
            let fid = item.next_sub("f");
            let mut props = Props::new();
            let mut a = Anim::new(Val::N(0.0));
            a.set_keys(if enter {
                vec![k(t0, Val::N(r), e_out), k(t0 + d, Val::N(0.0), Ease::Linear)]
            } else {
                vec![k(ts, Val::N(0.0), e_in), k(te, Val::N(r), Ease::Linear)]
            });
            props.insert("radius".into(), a);
            item.fx.push(Effect { id: fid, kind: "blur".into(), on: true, props });
            if enter {
                fade_in(item, d);
            } else {
                fade_out(item, d);
            }
        }
        // ---- loops (expressions keep them composable with keyframes)
        "pulse" => set_expr(item, "scale", format!("value*(1+{}*sin(time*2*pi*{}))", fmt_num(o.amount.unwrap_or(0.05)), fmt_num(o.freq.unwrap_or(1.0))), cx),
        "heartbeat" => set_expr(item, "scale", format!("value*(1+{}*max(0,sin(time*2*pi*{}))^12)", fmt_num(o.amount.unwrap_or(0.1)), fmt_num(o.freq.unwrap_or(1.2))), cx),
        "float" => set_expr(item, "pos", format!("value+[0,sin(time*2*pi*{})*{}]", fmt_num(o.freq.unwrap_or(0.5)), fmt_num(o.amount.unwrap_or(12.0))), cx),
        "bounce" => set_expr(item, "pos", format!("value-[0,abs(sin(time*pi*{}))*{}]", fmt_num(o.freq.unwrap_or(1.5)), fmt_num(o.amount.unwrap_or(40.0))), cx),
        "spin" => set_expr(item, "rot", format!("value+time*360*{}", fmt_num(o.freq.unwrap_or(0.5))), cx),
        "swing" => set_expr(item, "rot", format!("value+sin(time*2*pi*{})*{}", fmt_num(o.freq.unwrap_or(0.8)), fmt_num(o.amount.unwrap_or(8.0))), cx),
        "shake" => set_expr(item, "pos", format!("wiggle({},{})", fmt_num(o.freq.unwrap_or(12.0)), fmt_num(o.amount.unwrap_or(10.0))), cx),
        "wiggle" => set_expr(item, "pos", format!("wiggle({},{})", fmt_num(o.freq.unwrap_or(1.5)), fmt_num(o.amount.unwrap_or(20.0))), cx),
        "breathe" => set_expr(item, "opacity", format!("value*(0.75+0.25*sin(time*2*pi*{}))", fmt_num(o.freq.unwrap_or(0.5))), cx),
        "flicker" => set_expr(item, "opacity", format!("value*(random()>{}?1:0.25)", fmt_num(o.amount.unwrap_or(0.15))), cx),
        // ---- one-shots
        "kenburns" => {
            let z = o.amount.unwrap_or(0.12);
            merge(item, "scale", vec![k(0.0, sc.clone(), Ease::InOut(Sine)), k(item.dur, scale_v(&sc, 1.0 + z), Ease::Linear)], cx);
            let drift = cx.cw * 0.015;
            merge(item, "pos", vec![k(0.0, pos.clone(), Ease::InOut(Sine)), k(item.dur, add_v(&pos, -drift, -drift * 0.4), Ease::Linear)], cx);
        }
        "punch" => {
            let at = o.at.unwrap_or(0.0);
            let z = o.amount.unwrap_or(0.12);
            let dd = o.dur.unwrap_or(0.35);
            merge(item, "scale", vec![k(at, sc.clone(), Ease::Out(Quad)), k(at + 0.07, scale_v(&sc, 1.0 + z), Ease::InOut(Sine)), k(at + dd, sc.clone(), Ease::Linear)], cx);
        }
        "zoompunchin" => {
            let at = o.at.unwrap_or(0.0);
            let z = o.amount.unwrap_or(0.18);
            merge(item, "scale", vec![k(at, sc.clone(), Ease::Out(Expo)), k(at + o.dur.unwrap_or(0.25), scale_v(&sc, 1.0 + z), Ease::Linear)], cx);
        }
        "shakehit" => {
            let at = o.at.unwrap_or(0.0);
            let dd = o.dur.unwrap_or(0.4);
            let amp = o.amount.unwrap_or(25.0);
            set_expr(
                item,
                "pos",
                format!("time>={a} && time<{e} ? value+(wiggle(18,{amp})-value)*(1-(time-{a})/{d}) : value", a = fmt_num(at), e = fmt_num(at + dd), d = fmt_num(dd), amp = fmt_num(amp)),
                cx,
            );
        }
        _ => bail!("unknown animation preset '{name}'"),
    }
    Ok(())
}

fn anim_props(pairs: &[(&str, Val)]) -> Props {
    pairs.iter().map(|(k, v)| (k.to_string(), Anim::new(v.clone()))).collect()
}

pub fn is_text_anim(name: &str) -> bool {
    TEXT_ANIMS.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) || matches!(name.to_ascii_lowercase().as_str(), "karaoke" | "highlight")
}

/// Builds a text animator for a preset. `n_chars` helps fit typewriter timing into the item.
pub fn text_animator(name: &str, item_dur: f64, n_chars: usize, o: &AnimOpts) -> Result<(String, Props)> {
    let l = name.to_ascii_lowercase();
    let n = n_chars.max(1) as f64;
    let mut pairs: Vec<(&str, Val)> = vec![];
    let kind;
    match l.as_str() {
        "typewriter" => {
            kind = "in";
            let total = o.dur.unwrap_or((n * 0.05).min(item_dur * 0.5).max(0.2));
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(0.0)), ("stagger", Val::N(total / n)), ("opacity", Val::N(0.0)), ("ease", Val::S("hold".into()))]);
        }
        "fadeup" => {
            kind = "in";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.55))), ("stagger", Val::N(0.03)), ("opacity", Val::N(0.0)), ("y", Val::N(o.dist.unwrap_or(40.0)))]);
        }
        "fadechars" => {
            kind = "in";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.4))), ("stagger", Val::N(0.03)), ("opacity", Val::N(0.0)), ("ease", Val::S("linear".into()))]);
        }
        "pop" => {
            kind = "in";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.45))), ("stagger", Val::N(0.035)), ("opacity", Val::N(0.0)), ("scale", Val::N(0.0)), ("ease", Val::S("outBack".into()))]);
        }
        "slideup" => {
            kind = "in";
            pairs.extend([("unit", Val::S("line".into())), ("dur", Val::N(o.dur.unwrap_or(0.7))), ("stagger", Val::N(0.12)), ("opacity", Val::N(0.0)), ("y", Val::N(o.dist.unwrap_or(80.0))), ("ease", Val::S("outExpo".into()))]);
        }
        "blurin" => {
            kind = "in";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.6))), ("stagger", Val::N(0.03)), ("opacity", Val::N(0.0)), ("blur", Val::N(o.amount.unwrap_or(18.0)))]);
        }
        "wordpop" => {
            kind = "in";
            pairs.extend([("unit", Val::S("word".into())), ("dur", Val::N(o.dur.unwrap_or(0.4))), ("stagger", Val::N(0.14)), ("opacity", Val::N(0.0)), ("scale", Val::N(0.5)), ("ease", Val::S("outBack".into()))]);
        }
        "bouncein" => {
            kind = "in";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.8))), ("stagger", Val::N(0.04)), ("opacity", Val::N(0.0)), ("y", Val::N(-o.dist.unwrap_or(120.0))), ("ease", Val::S("outBounce".into()))]);
        }
        "dropin" => {
            kind = "in";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.5))), ("stagger", Val::N(0.035)), ("opacity", Val::N(0.0)), ("y", Val::N(-o.dist.unwrap_or(60.0))), ("ease", Val::S("outBack".into()))]);
        }
        "spinin" => {
            kind = "in";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.6))), ("stagger", Val::N(0.04)), ("opacity", Val::N(0.0)), ("rot", Val::N(-120.0)), ("scale", Val::N(0.3)), ("ease", Val::S("outBack".into()))]);
        }
        "trackin" => {
            kind = "in";
            pairs.extend([("unit", Val::S("line".into())), ("dur", Val::N(o.dur.unwrap_or(1.4))), ("stagger", Val::N(0.1)), ("opacity", Val::N(0.0)), ("tracking", Val::N(o.amount.unwrap_or(40.0))), ("ease", Val::S("outExpo".into()))]);
        }
        "wave" => {
            kind = "loop";
            pairs.extend([("unit", Val::S("char".into())), ("y", Val::N(o.amount.unwrap_or(14.0))), ("freq", Val::N(o.freq.unwrap_or(1.0))), ("phase", Val::N(25.0))]);
        }
        "jitter" => {
            kind = "loop";
            pairs.extend([("unit", Val::S("char".into())), ("x", Val::N(o.amount.unwrap_or(3.0))), ("y", Val::N(o.amount.unwrap_or(3.0))), ("rot", Val::N(4.0)), ("freq", Val::N(o.freq.unwrap_or(8.0))), ("phase", Val::N(77.0))]);
        }
        "fadedown" => {
            kind = "out";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.4))), ("stagger", Val::N(0.02)), ("opacity", Val::N(0.0)), ("y", Val::N(30.0)), ("ease", Val::S("inCubic".into()))]);
        }
        "popout" => {
            kind = "out";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.35))), ("stagger", Val::N(0.02)), ("opacity", Val::N(0.0)), ("scale", Val::N(0.0)), ("ease", Val::S("inBack".into()))]);
        }
        "blurout" => {
            kind = "out";
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(o.dur.unwrap_or(0.5))), ("stagger", Val::N(0.02)), ("opacity", Val::N(0.0)), ("blur", Val::N(18.0)), ("ease", Val::S("inCubic".into()))]);
        }
        "typeout" => {
            kind = "out";
            let total = o.dur.unwrap_or((n * 0.03).min(item_dur * 0.4).max(0.2));
            pairs.extend([("unit", Val::S("char".into())), ("dur", Val::N(0.0)), ("stagger", Val::N(total / n)), ("opacity", Val::N(0.0)), ("order", Val::S("reverse".into())), ("ease", Val::S("hold".into()))]);
        }
        "slidedown" => {
            kind = "out";
            pairs.extend([("unit", Val::S("line".into())), ("dur", Val::N(o.dur.unwrap_or(0.5))), ("stagger", Val::N(0.08)), ("opacity", Val::N(0.0)), ("y", Val::N(60.0)), ("ease", Val::S("inExpo".into()))]);
        }
        "karaoke" | "highlight" => {
            kind = "hl";
            pairs.extend([("unit", Val::S("word".into())), ("dur", Val::N(0.08)), ("color", Val::C(Color::parse("#ffd60a").unwrap())), ("scale", Val::N(1.08))]);
        }
        _ => bail!("unknown text animation '{name}' (see help text)"),
    }
    if o.delay != 0.0 {
        pairs.push(("delay", Val::N(o.delay)));
    }
    if let Some(e) = o.ease {
        pairs.retain(|(k, _)| *k != "ease");
        pairs.push(("ease", Val::S(e.name())));
    }
    Ok((kind.to_string(), anim_props(&pairs)))
}

pub fn new_animator(id: String, kind: String, props: Props) -> Animator {
    Effect { id, kind, on: true, props }
}

/// Color grading presets -> lumetri parameters plus optional extra effects.
pub fn color_preset(name: &str) -> Option<(Vec<(&'static str, Val)>, Vec<(&'static str, Vec<(&'static str, Val)>)>)> {
    let c = |s: &str| Val::C(Color::parse(s).unwrap());
    let n = Val::N;
    Some(match name.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "cinematic" => (vec![("contrast", n(20.0)), ("highlights", n(-15.0)), ("shadows", n(8.0)), ("sat", n(92.0)), ("stint", c("#16707f55")), ("htint", c("#ff9d5a44"))], vec![("vignette", vec![("amount", n(0.3))])]),
        "tealorange" => (vec![("contrast", n(25.0)), ("sat", n(110.0)), ("vibrance", n(15.0)), ("stint", c("#0f8a9a88")), ("htint", c("#ff8a3d77"))], vec![]),
        "vintage" => (vec![("fade", n(28.0)), ("temp", n(18.0)), ("sat", n(78.0)), ("contrast", n(-12.0)), ("htint", c("#ffd9a044"))], vec![("noise", vec![("amount", n(0.08))]), ("vignette", vec![("amount", n(0.35))])]),
        "film" => (vec![("fade", n(10.0)), ("contrast", n(10.0)), ("temp", n(6.0)), ("sat", n(95.0))], vec![("noise", vec![("amount", n(0.05))])]),
        "bw" | "blackwhite" | "mono" => (vec![("sat", n(0.0)), ("contrast", n(18.0))], vec![]),
        "noir" => (vec![("sat", n(0.0)), ("contrast", n(45.0)), ("blacks", n(-20.0)), ("highlights", n(-10.0))], vec![("vignette", vec![("amount", n(0.6))]), ("noise", vec![("amount", n(0.06))])]),
        "warm" => (vec![("temp", n(25.0)), ("tint", n(4.0))], vec![]),
        "cool" => (vec![("temp", n(-25.0))], vec![]),
        "punchy" | "vivid" => (vec![("contrast", n(25.0)), ("vibrance", n(30.0)), ("sat", n(110.0))], vec![]),
        "faded" | "matte" => (vec![("fade", n(30.0)), ("contrast", n(-15.0)), ("sat", n(85.0))], vec![]),
        "moody" | "dark" => (vec![("exposure", n(-0.3)), ("contrast", n(20.0)), ("sat", n(72.0)), ("temp", n(-10.0)), ("highlights", n(-20.0))], vec![("vignette", vec![("amount", n(0.45))])]),
        "airy" | "bright" => (vec![("exposure", n(0.3)), ("highlights", n(-20.0)), ("shadows", n(25.0)), ("sat", n(90.0)), ("temp", n(5.0))], vec![]),
        "sunset" | "golden" => (vec![("temp", n(35.0)), ("tint", n(10.0)), ("sat", n(115.0)), ("htint", c("#ffb34766"))], vec![]),
        "cyberpunk" | "neon" => (vec![("contrast", n(25.0)), ("sat", n(120.0)), ("stint", c("#00e0ff66")), ("htint", c("#ff2bd666"))], vec![]),
        "natural" | "clean" => (vec![("contrast", n(8.0)), ("vibrance", n(10.0))], vec![]),
        _ => return None,
    })
}
