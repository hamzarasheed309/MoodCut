//! Schema-driven property catalog. Every animatable property in MoodCut is
//! declared here once; validation, defaults, value parsing, the `help` tool and
//! the GUI inspector are all generated from these tables.

use super::project::ItemKind;
use super::value::{fmt_num, Val};
use anyhow::{anyhow, bail, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum T {
    Num,
    Vec2,
    Color,
    Str,
    Bool,
    Enum(&'static [&'static str]),
}

/// Unit hint used for parsing and docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum U {
    None,
    Px,
    Deg,
    /// 0..1 fraction; accepts `50%`.
    Frac,
    /// Multiplier; accepts `120%`.
    Mult,
    Db,
    Sec,
    /// 0..100 percent.
    Pct,
    Hz,
    Ms,
}

#[derive(Clone, Copy, Debug)]
pub enum D {
    N(f64),
    V(f64, f64),
    C(&'static str),
    S(&'static str),
    B(bool),
    /// Comp center (positions).
    Center,
    CompW,
    CompH,
}

#[derive(Debug)]
pub struct P {
    pub name: &'static str,
    pub t: T,
    pub d: D,
    pub min: f64,
    pub max: f64,
    pub u: U,
    pub anim: bool,
    pub desc: &'static str,
    pub alias: &'static [&'static str],
}

const INF: f64 = f64::INFINITY;

macro_rules! p {
    ($n:expr, $t:expr, $d:expr, $min:expr, $max:expr, $u:expr, $desc:expr) => {
        P { name: $n, t: $t, d: $d, min: $min, max: $max, u: $u, anim: true, desc: $desc, alias: &[] }
    };
    ($n:expr, $t:expr, $d:expr, $min:expr, $max:expr, $u:expr, $desc:expr, [$($a:expr),*]) => {
        P { name: $n, t: $t, d: $d, min: $min, max: $max, u: $u, anim: true, desc: $desc, alias: &[$($a),*] }
    };
}
macro_rules! ps {
    ($n:expr, $t:expr, $d:expr, $desc:expr) => {
        P { name: $n, t: $t, d: $d, min: -INF, max: INF, u: U::None, anim: false, desc: $desc, alias: &[] }
    };
    ($n:expr, $t:expr, $d:expr, $desc:expr, [$($a:expr),*]) => {
        P { name: $n, t: $t, d: $d, min: -INF, max: INF, u: U::None, anim: false, desc: $desc, alias: &[$($a),*] }
    };
}

use D::*;
use T::*;

pub const BLEND_MODES: &[&str] = &[
    "normal", "multiply", "screen", "overlay", "darken", "lighten", "colordodge", "colorburn", "hardlight", "softlight", "difference",
    "exclusion", "hue", "saturation", "color", "luminosity", "add",
];

pub static LAYER: &[P] = &[
    p!("pos", Vec2, Center, -INF, INF, U::Px, "Position in comp pixels (accepts center, top, bottom-left, 50%,80% ...)", ["position", "p", "xy"]),
    p!("scale", Vec2, V(1.0, 1.0), -100.0, 100.0, U::Mult, "Scale factor (1 = 100%, accepts 120% or 1.2 or 1.2,0.8)", ["s", "size%"]),
    p!("rot", Num, N(0.0), -INF, INF, U::Deg, "Rotation in degrees (clockwise)", ["rotation", "r", "angle"]),
    p!("opacity", Num, N(1.0), 0.0, 1.0, U::Frac, "Opacity 0-1 (accepts 50%)", ["o", "alpha"]),
    p!("anchor", Vec2, V(0.5, 0.5), -INF, INF, U::Frac, "Anchor point as fraction of layer bounds (0.5,0.5 = center)", ["ap"]),
    p!("rx", Num, N(0.0), -INF, INF, U::Deg, "3D rotation around X axis (degrees, perspective)", ["rotx", "tilt"]),
    p!("ry", Num, N(0.0), -INF, INF, U::Deg, "3D rotation around Y axis (degrees, perspective)", ["roty", "swing"]),
    p!("z", Num, N(0.0), -INF, INF, U::Px, "3D depth (positive = farther, perspective)"),
    p!("skew", Num, N(0.0), -85.0, 85.0, U::Deg, "Horizontal skew in degrees"),
    ps!("blend", Enum(BLEND_MODES), S("normal"), "Blend mode", ["mode"]),
    ps!("mblur", Bool, B(false), "Motion blur for animated transforms", ["motionblur"]),
];

pub static CLIP: &[P] = &[
    ps!("speed", Num, N(1.0), "Playback speed (2 = 2x faster, 50% = half). Changes duration via the speed command", ["spd"]),
    ps!("reverse", Bool, B(false), "Play backwards", ["rev"]),
    ps!("fit", Enum(&["fit", "fill", "stretch", "none"]), S("fit"), "How media is fitted into the comp"),
    ps!("hold", Bool, B(false), "Freeze frame (shows the in-point frame)", ["freeze"]),
    p!("remap", Num, N(-1.0), -1.0, INF, U::Sec, "Time remap: keyframes map item time -> source seconds (-1 = off)"),
    ps!("interp", Enum(&["none", "blend", "flow"]), S("none"), "Slow-motion frame interpolation"),
    p!("vol", Num, N(0.0), -96.0, 24.0, U::Db, "Audio gain in dB (accepts -6dB)", ["volume", "gain"]),
    ps!("pan", Num, N(0.0), "Stereo pan -1 (left) .. 1 (right)"),
    ps!("mute", Bool, B(false), "Mute the clip's audio"),
    ps!("fadeIn", Num, N(0.0), "Audio fade-in seconds", ["afadein"]),
    ps!("fadeOut", Num, N(0.0), "Audio fade-out seconds", ["afadeout"]),
];

pub static NESTED: &[P] = &[
    ps!("speed", Num, N(1.0), "Playback speed of the nested comp"),
    ps!("fit", Enum(&["none", "fit", "fill", "stretch"]), S("none"), "How the nested comp is fitted"),
    p!("remap", Num, N(-1.0), -1.0, INF, U::Sec, "Time remap keyframes -> nested comp seconds (-1 = off)"),
    p!("vol", Num, N(0.0), -96.0, 24.0, U::Db, "Audio gain of the nested comp in dB", ["volume"]),
    ps!("mute", Bool, B(false), "Mute nested audio"),
];

pub const WEIGHTS: &[(&str, f64)] = &[
    ("thin", 100.0),
    ("extralight", 200.0),
    ("light", 300.0),
    ("regular", 400.0),
    ("normal", 400.0),
    ("medium", 500.0),
    ("semibold", 600.0),
    ("bold", 700.0),
    ("extrabold", 800.0),
    ("black", 900.0),
    ("heavy", 900.0),
];

pub static TEXT: &[P] = &[
    ps!("font", Str, S("Inter"), "Font family (system fonts; Inter is bundled)", ["family"]),
    ps!("weight", Num, N(700.0), "Font weight 100-900 or thin/light/regular/medium/semibold/bold/black", ["w8", "fw"]),
    ps!("italic", Bool, B(false), "Italic style"),
    p!("size", Num, N(96.0), 1.0, 2000.0, U::Px, "Font size in px", ["fontsize", "fs"]),
    p!("color", Color, C("#ffffff"), -INF, INF, U::None, "Text fill color", ["fill", "c"]),
    p!("color2", Color, C("none"), -INF, INF, U::None, "Second fill color for a vertical gradient (none = solid)", ["gradient"]),
    p!("stroke", Color, C("none"), -INF, INF, U::None, "Outline color"),
    p!("strokeW", Num, N(0.0), 0.0, 200.0, U::Px, "Outline width", ["strokewidth", "sw"]),
    ps!("align", Enum(&["center", "left", "right"]), S("center"), "Horizontal alignment"),
    p!("lineH", Num, N(1.15), 0.5, 4.0, U::Mult, "Line height multiplier", ["lineheight", "leading"]),
    p!("tracking", Num, N(0.0), -200.0, 2000.0, U::Px, "Letter spacing in px", ["letterspacing", "spacing"]),
    ps!("boxW", Num, N(0.0), "Wrap width in px (0 = no wrapping)", ["wrap", "maxwidth"]),
    ps!("caps", Bool, B(false), "Render in UPPERCASE", ["upper", "uppercase"]),
    p!("bg", Color, C("none"), -INF, INF, U::None, "Background box color (caption style)", ["box", "background"]),
    p!("bgPad", Num, N(18.0), 0.0, 500.0, U::Px, "Background box padding"),
    p!("bgRadius", Num, N(14.0), 0.0, 500.0, U::Px, "Background box corner radius"),
    ps!("bgMode", Enum(&["block", "line"]), S("line"), "One box for the block or one per line"),
    p!("shadow", Color, C("none"), -INF, INF, U::None, "Drop shadow color (e.g. #000000aa)"),
    p!("shadowX", Num, N(0.0), -500.0, 500.0, U::Px, "Shadow X offset"),
    p!("shadowY", Num, N(6.0), -500.0, 500.0, U::Px, "Shadow Y offset"),
    p!("shadowBlur", Num, N(14.0), 0.0, 200.0, U::Px, "Shadow blur radius"),
    p!("reveal", Num, N(1.0), 0.0, 1.0, U::Frac, "Fraction of characters visible (typewriter style; animate 0->1)"),
];

pub static SOLID: &[P] = &[
    p!("color", Color, C("#ffffff"), -INF, INF, U::None, "Fill color", ["fill", "c"]),
    p!("color2", Color, C("none"), -INF, INF, U::None, "Gradient end color (none = solid)"),
    ps!("gradType", Enum(&["linear", "radial"]), S("linear"), "Gradient type"),
    p!("gradAngle", Num, N(90.0), -INF, INF, U::Deg, "Linear gradient angle (90 = top to bottom)"),
    p!("w", Num, CompW, 0.0, 100000.0, U::Px, "Width (default comp width)", ["width"]),
    p!("h", Num, CompH, 0.0, 100000.0, U::Px, "Height (default comp height)", ["height"]),
    p!("radius", Num, N(0.0), 0.0, 100000.0, U::Px, "Corner radius", ["round", "corner"]),
];

pub static NONE: &[P] = &[];

pub fn item_props(kind: ItemKind) -> &'static [P] {
    match kind {
        ItemKind::Clip => CLIP,
        ItemKind::Text => TEXT,
        ItemKind::Solid => SOLID,
        ItemKind::Comp => NESTED,
        ItemKind::Shape | ItemKind::Adjust => NONE,
    }
}

// ---------------------------------------------------------------- shapes

pub static SHAPE_COMMON: &[P] = &[
    p!("offset", Vec2, V(0.0, 0.0), -INF, INF, U::Px, "Offset of the shape inside its layer (px)", ["at", "xy0"]),
    p!("fill", Color, C("#ffffff"), -INF, INF, U::None, "Fill color (none = no fill)", ["color", "c"]),
    p!("fill2", Color, C("none"), -INF, INF, U::None, "Gradient end color (none = solid fill)", ["gradient"]),
    ps!("gradType", Enum(&["linear", "radial"]), S("linear"), "Gradient type"),
    p!("gradAngle", Num, N(90.0), -INF, INF, U::Deg, "Linear gradient angle"),
    p!("stroke", Color, C("none"), -INF, INF, U::None, "Stroke color"),
    p!("strokeW", Num, N(0.0), 0.0, 1000.0, U::Px, "Stroke width", ["sw", "strokewidth"]),
    ps!("dash", Str, S(""), "Dash pattern e.g. '20,10'"),
    ps!("cap", Enum(&["round", "butt", "square"]), S("round"), "Stroke line cap"),
    ps!("join", Enum(&["round", "miter", "bevel"]), S("round"), "Stroke line join"),
    p!("trimStart", Num, N(0.0), 0.0, 100.0, U::Pct, "Trim paths start % (stroke draw-on)", ["ts"]),
    p!("trimEnd", Num, N(100.0), 0.0, 100.0, U::Pct, "Trim paths end % (animate 0->100 to draw on)", ["te"]),
    p!("trimOffset", Num, N(0.0), -INF, INF, U::Pct, "Trim paths offset %"),
    p!("rot", Num, N(0.0), -INF, INF, U::Deg, "Shape rotation"),
    p!("scale", Vec2, V(1.0, 1.0), -100.0, 100.0, U::Mult, "Shape scale"),
    p!("opacity", Num, N(1.0), 0.0, 1.0, U::Frac, "Shape opacity"),
    p!("repeat", Num, N(0.0), 0.0, 500.0, U::None, "Repeater: number of extra copies", ["copies"]),
    p!("repOffset", Vec2, V(0.0, 0.0), -INF, INF, U::Px, "Repeater offset per copy"),
    p!("repRot", Num, N(0.0), -INF, INF, U::Deg, "Repeater rotation per copy"),
    p!("repScale", Vec2, V(1.0, 1.0), -100.0, 100.0, U::Mult, "Repeater scale per copy"),
    p!("repOpacity", Num, N(1.0), 0.0, 1.0, U::Frac, "Opacity of the last copy (fades across copies)"),
];

pub static SHAPE_RECT: &[P] = &[
    p!("w", Num, N(400.0), 0.0, 100000.0, U::Px, "Width", ["width"]),
    p!("h", Num, N(200.0), 0.0, 100000.0, U::Px, "Height", ["height"]),
    p!("radius", Num, N(0.0), 0.0, 100000.0, U::Px, "Corner radius", ["round", "r"]),
];
pub static SHAPE_ELLIPSE: &[P] = &[
    p!("w", Num, N(200.0), 0.0, 100000.0, U::Px, "Width", ["width", "d"]),
    p!("h", Num, N(200.0), 0.0, 100000.0, U::Px, "Height", ["height"]),
];
pub static SHAPE_POLYGON: &[P] = &[
    p!("points", Num, N(6.0), 3.0, 100.0, U::None, "Number of sides", ["sides", "n"]),
    p!("radius", Num, N(120.0), 0.0, 100000.0, U::Px, "Outer radius", ["r"]),
];
pub static SHAPE_STAR: &[P] = &[
    p!("points", Num, N(5.0), 2.0, 100.0, U::None, "Number of points", ["n"]),
    p!("r1", Num, N(120.0), 0.0, 100000.0, U::Px, "Outer radius", ["radius", "outer"]),
    p!("r2", Num, N(55.0), 0.0, 100000.0, U::Px, "Inner radius", ["inner"]),
];
pub static SHAPE_PATH: &[P] = &[ps!("d", Str, S("M-100,0 L100,0"), "SVG path data, e.g. 'M0,0 C50,-80 150,-80 200,0'", ["path"])];
pub static SHAPE_LINE: &[P] = &[
    p!("from", Vec2, V(-200.0, 0.0), -INF, INF, U::Px, "Start point (layer px)"),
    p!("to", Vec2, V(200.0, 0.0), -INF, INF, U::Px, "End point (layer px)"),
];
pub static SHAPE_ARROW: &[P] = &[
    p!("from", Vec2, V(-200.0, 0.0), -INF, INF, U::Px, "Tail point"),
    p!("to", Vec2, V(200.0, 0.0), -INF, INF, U::Px, "Head point"),
    p!("head", Num, N(40.0), 0.0, 1000.0, U::Px, "Arrow head size"),
];

pub const SHAPE_KINDS: &[&str] = &["rect", "ellipse", "circle", "polygon", "star", "path", "line", "arrow"];

pub fn shape_props(kind: &str) -> Option<&'static [P]> {
    Some(match kind {
        "rect" | "square" | "box" => SHAPE_RECT,
        "ellipse" | "circle" | "oval" => SHAPE_ELLIPSE,
        "polygon" | "poly" | "triangle" | "hexagon" => SHAPE_POLYGON,
        "star" => SHAPE_STAR,
        "path" => SHAPE_PATH,
        "line" => SHAPE_LINE,
        "arrow" => SHAPE_ARROW,
        _ => return None,
    })
}

pub fn normalize_shape_kind(kind: &str) -> Option<&'static str> {
    Some(match kind.to_ascii_lowercase().as_str() {
        "rect" | "square" | "box" | "rectangle" => "rect",
        "ellipse" | "circle" | "oval" => "ellipse",
        "polygon" | "poly" | "triangle" | "hexagon" => "polygon",
        "star" => "star",
        "path" => "path",
        "line" => "line",
        "arrow" => "arrow",
        _ => return None,
    })
}

// ---------------------------------------------------------------- masks

pub static MASK: &[P] = &[
    p!("x", Num, N(0.5), -INF, INF, U::Frac, "Mask center X as fraction of layer width"),
    p!("y", Num, N(0.5), -INF, INF, U::Frac, "Mask center Y as fraction of layer height"),
    p!("w", Num, N(0.8), 0.0, INF, U::Frac, "Mask width as fraction of layer width", ["width"]),
    p!("h", Num, N(0.8), 0.0, INF, U::Frac, "Mask height as fraction of layer height", ["height"]),
    p!("radius", Num, N(0.0), 0.0, INF, U::Px, "Corner radius (rect masks)", ["round"]),
    ps!("d", Str, S(""), "Path mask: SVG path in normalized 0..1 layer coordinates"),
    p!("points", Num, N(5.0), 2.0, 100.0, U::None, "Polygon/star points"),
    p!("feather", Num, N(0.0), 0.0, 1000.0, U::Px, "Edge feather (px)", ["soft", "blur"]),
    p!("expand", Num, N(0.0), -1000.0, 1000.0, U::Px, "Grow/shrink the mask (px)", ["expansion"]),
    p!("opacity", Num, N(1.0), 0.0, 1.0, U::Frac, "Mask opacity"),
    p!("rot", Num, N(0.0), -INF, INF, U::Deg, "Mask rotation"),
    ps!("invert", Bool, B(false), "Invert the mask", ["inv"]),
    ps!("mode", Enum(&["add", "subtract", "intersect", "difference"]), S("add"), "How masks combine"),
];
pub const MASK_KINDS: &[&str] = &["rect", "ellipse", "path", "polygon", "star"];

// ---------------------------------------------------------------- text animators

pub static ANIMATOR: &[P] = &[
    ps!("unit", Enum(&["char", "word", "line"]), S("char"), "Animate per character, word or line", ["by"]),
    ps!("order", Enum(&["forward", "reverse", "center", "edges", "random"]), S("forward"), "Stagger order"),
    ps!("stagger", Num, N(0.04), "Delay between units (s)"),
    ps!("dur", Num, N(0.5), "Per-unit animation duration (s)"),
    ps!("delay", Num, N(0.0), "Start delay relative to item start (in) or end (out)"),
    ps!("ease", Str, S("outCubic"), "Easing (see help anim)"),
    ps!("times", Str, S(""), "Explicit unit start times 't0,t1,..' (karaoke); overrides stagger"),
    ps!("opacity", Num, N(1.0), "Opacity of units at the animated extreme", ["o"]),
    ps!("x", Num, N(0.0), "X offset (px)"),
    ps!("y", Num, N(0.0), "Y offset (px)"),
    ps!("scale", Num, N(1.0), "Scale"),
    ps!("rot", Num, N(0.0), "Rotation (deg)"),
    ps!("blur", Num, N(0.0), "Blur (px)"),
    ps!("tracking", Num, N(0.0), "Extra spacing (px)"),
    ps!("color", Color, C("none"), "Color override (none = unchanged)"),
    ps!("freq", Num, N(1.0), "Loop frequency (Hz) for kind=loop"),
    ps!("phase", Num, N(30.0), "Loop phase offset per unit (deg) for kind=loop"),
];
pub const ANIMATOR_KINDS: &[&str] = &["in", "out", "loop", "hl"];

// ---------------------------------------------------------------- effects

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Engine {
    /// Rendered by MoodCut's compositor (animatable, works on any layer).
    Native,
    /// FFmpeg filter applied while decoding media clips (static params).
    Ff,
    /// FFmpeg audio filter.
    Audio,
}

pub struct EffectDef {
    pub name: &'static str,
    pub alias: &'static [&'static str],
    pub engine: Engine,
    pub cat: &'static str,
    pub desc: &'static str,
    pub params: &'static [P],
}

macro_rules! fx {
    ($n:expr, [$($a:expr),*], $e:expr, $cat:expr, $desc:expr, [$($p:expr),* $(,)?]) => {
        EffectDef { name: $n, alias: &[$($a),*], engine: $e, cat: $cat, desc: $desc, params: &[$($p),*] }
    };
}

use Engine::*;

pub static EFFECTS: &[EffectDef] = &[
    // ---- color
    fx!("lumetri", ["grade", "color", "colorgrade"], Native, "color", "Premiere-style basic correction", [
        p!("exposure", Num, N(0.0), -5.0, 5.0, U::None, "Exposure (stops)"),
        p!("contrast", Num, N(0.0), -100.0, 100.0, U::None, "Contrast"),
        p!("highlights", Num, N(0.0), -100.0, 100.0, U::None, "Highlights"),
        p!("shadows", Num, N(0.0), -100.0, 100.0, U::None, "Shadows"),
        p!("whites", Num, N(0.0), -100.0, 100.0, U::None, "Whites"),
        p!("blacks", Num, N(0.0), -100.0, 100.0, U::None, "Blacks"),
        p!("temp", Num, N(0.0), -100.0, 100.0, U::None, "Temperature (- cool, + warm)", ["temperature"]),
        p!("tint", Num, N(0.0), -100.0, 100.0, U::None, "Tint (- green, + magenta)"),
        p!("sat", Num, N(100.0), 0.0, 300.0, U::None, "Saturation %", ["saturation"]),
        p!("vibrance", Num, N(0.0), -100.0, 100.0, U::None, "Vibrance"),
        p!("fade", Num, N(0.0), 0.0, 100.0, U::None, "Faded film (lifted blacks)"),
        p!("stint", Color, C("none"), -INF, INF, U::None, "Shadow tint for split toning (alpha = strength), e.g. #1b6e8a66", ["shadowtint"]),
        p!("htint", Color, C("none"), -INF, INF, U::None, "Highlight tint for split toning (alpha = strength), e.g. #ff9a4466", ["highlighttint"]),
    ]),
    fx!("bc", ["brightness", "contrast", "brightnesscontrast"], Native, "color", "Brightness & contrast", [
        p!("brightness", Num, N(0.0), -100.0, 100.0, U::None, "Brightness", ["b"]),
        p!("contrast", Num, N(0.0), -100.0, 100.0, U::None, "Contrast", ["c"]),
    ]),
    fx!("exposure", [], Native, "color", "Exposure, offset and gamma", [
        p!("stops", Num, N(0.0), -5.0, 5.0, U::None, "Exposure in stops", ["exposure"]),
        p!("offset", Num, N(0.0), -1.0, 1.0, U::None, "Offset"),
        p!("gamma", Num, N(1.0), 0.1, 10.0, U::None, "Gamma correction"),
    ]),
    fx!("levels", [], Native, "color", "Input/output levels", [
        p!("inBlack", Num, N(0.0), 0.0, 1.0, U::Frac, "Input black"),
        p!("inWhite", Num, N(1.0), 0.0, 1.0, U::Frac, "Input white"),
        p!("gamma", Num, N(1.0), 0.1, 10.0, U::None, "Gamma"),
        p!("outBlack", Num, N(0.0), 0.0, 1.0, U::Frac, "Output black"),
        p!("outWhite", Num, N(1.0), 0.0, 1.0, U::Frac, "Output white"),
    ]),
    fx!("curves", [], Native, "color", "Tone curves from control points 'x,y x,y ...' (0..1)", [
        ps!("master", Str, S(""), "RGB curve points e.g. '0,0 .25,.2 .75,.85 1,1'", ["rgb"]),
        ps!("r", Str, S(""), "Red curve points"),
        ps!("g", Str, S(""), "Green curve points"),
        ps!("b", Str, S(""), "Blue curve points"),
    ]),
    fx!("hue", ["huesat", "hsl"], Native, "color", "Hue / saturation / lightness", [
        p!("shift", Num, N(0.0), -360.0, 360.0, U::Deg, "Hue rotation", ["hue"]),
        p!("sat", Num, N(1.0), 0.0, 4.0, U::Mult, "Saturation multiplier", ["saturation"]),
        p!("light", Num, N(0.0), -1.0, 1.0, U::None, "Lightness"),
    ]),
    fx!("saturation", ["sat"], Native, "color", "Saturation", [p!("amount", Num, N(1.0), 0.0, 4.0, U::Mult, "Saturation multiplier (0 = gray)")]),
    fx!("vibrance", [], Native, "color", "Vibrance (protects saturated colors)", [p!("amount", Num, N(0.5), -1.0, 1.0, U::None, "Vibrance")]),
    fx!("temperature", ["wb", "whitebalance"], Native, "color", "White balance shift", [
        p!("temp", Num, N(0.0), -100.0, 100.0, U::None, "Temperature (- cool, + warm)"),
        p!("tint", Num, N(0.0), -100.0, 100.0, U::None, "Tint (- green, + magenta)"),
    ]),
    fx!("tint", [], Native, "color", "Map blacks/whites to colors", [
        p!("black", Color, C("#000000"), -INF, INF, U::None, "Map black to"),
        p!("white", Color, C("#ffffff"), -INF, INF, U::None, "Map white to"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
    ]),
    fx!("duotone", [], Native, "color", "Two-color gradient map", [
        p!("dark", Color, C("#1b1464"), -INF, INF, U::None, "Shadow color"),
        p!("light", Color, C("#ff5f6d"), -INF, INF, U::None, "Highlight color"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
    ]),
    fx!("fill", ["colorize", "colorfill"], Native, "color", "Fill the layer with a color (keeps alpha)", [
        p!("color", Color, C("#ffffff"), -INF, INF, U::None, "Fill color"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
    ]),
    fx!("grayscale", ["bw", "blackwhite", "mono", "desaturate"], Native, "color", "Black & white", [p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount")]),
    fx!("sepia", [], Native, "color", "Sepia tone", [p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount")]),
    fx!("invert", ["negative"], Native, "color", "Invert colors", [p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount")]),
    fx!("lut", ["lut3d", "cube"], Native, "color", "Apply a .cube 3D LUT", [
        ps!("file", Str, S(""), "Path to .cube file", ["path"]),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Intensity"),
    ]),
    fx!("posterize", [], Native, "color", "Reduce color levels", [p!("levels", Num, N(6.0), 2.0, 64.0, U::None, "Levels per channel")]),
    fx!("threshold", [], Native, "color", "Black/white threshold", [p!("level", Num, N(0.5), 0.0, 1.0, U::Frac, "Threshold")]),
    // ---- blur & sharpen
    fx!("blur", ["gaussian", "gaussianblur"], Native, "blur", "Gaussian blur", [
        p!("radius", Num, N(10.0), 0.0, 500.0, U::Px, "Blur radius", ["r", "amount"]),
        ps!("dir", Enum(&["both", "h", "v"]), S("both"), "Blur direction"),
    ]),
    fx!("dirblur", ["motionblurfx", "directionalblur"], Native, "blur", "Directional (motion) blur", [
        p!("angle", Num, N(0.0), -INF, INF, U::Deg, "Direction"),
        p!("length", Num, N(30.0), 0.0, 1000.0, U::Px, "Blur length"),
    ]),
    fx!("zoomblur", ["radialblur"], Native, "blur", "Radial zoom blur", [
        p!("amount", Num, N(0.15), 0.0, 1.0, U::None, "Strength"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X (fraction of frame)"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y (fraction of frame)"),
    ]),
    fx!("sharpen", ["unsharp"], Native, "blur", "Unsharp mask sharpening", [
        p!("amount", Num, N(1.0), 0.0, 5.0, U::None, "Amount"),
        p!("radius", Num, N(1.5), 0.5, 20.0, U::Px, "Radius"),
    ]),
    fx!("glow", ["bloom"], Native, "stylize", "Glow / bloom", [
        p!("radius", Num, N(24.0), 0.0, 500.0, U::Px, "Glow radius"),
        p!("intensity", Num, N(1.0), 0.0, 10.0, U::None, "Intensity"),
        p!("threshold", Num, N(0.6), 0.0, 1.0, U::Frac, "Brightness threshold"),
        p!("color", Color, C("none"), -INF, INF, U::None, "Glow tint (none = original colors)"),
    ]),
    // ---- stylize
    fx!("shadow", ["dropshadow"], Native, "stylize", "Drop shadow", [
        p!("color", Color, C("#000000"), -INF, INF, U::None, "Shadow color"),
        p!("opacity", Num, N(0.6), 0.0, 1.0, U::Frac, "Opacity"),
        p!("angle", Num, N(135.0), -INF, INF, U::Deg, "Light angle (135 = shadow down-right)"),
        p!("distance", Num, N(12.0), 0.0, 1000.0, U::Px, "Distance"),
        p!("blur", Num, N(16.0), 0.0, 500.0, U::Px, "Softness"),
    ]),
    fx!("stroke", ["outline", "border"], Native, "stylize", "Outline around the layer's alpha", [
        p!("color", Color, C("#ffffff"), -INF, INF, U::None, "Stroke color"),
        p!("width", Num, N(6.0), 0.0, 200.0, U::Px, "Stroke width"),
    ]),
    fx!("vignette", [], Native, "stylize", "Darken the frame edges", [
        p!("amount", Num, N(0.5), 0.0, 1.0, U::Frac, "Strength"),
        p!("size", Num, N(0.6), 0.0, 1.5, U::None, "Clear area size"),
        p!("soft", Num, N(0.5), 0.01, 1.0, U::None, "Softness"),
        p!("color", Color, C("#000000"), -INF, INF, U::None, "Color"),
    ]),
    fx!("noise", ["grain", "filmgrain"], Native, "stylize", "Film grain / noise", [
        p!("amount", Num, N(0.12), 0.0, 1.0, U::Frac, "Amount"),
        ps!("mono", Bool, B(true), "Monochrome grain"),
        ps!("animated", Bool, B(true), "Re-seed every frame"),
    ]),
    fx!("pixelate", ["mosaic"], Native, "stylize", "Pixelate", [p!("size", Num, N(16.0), 1.0, 500.0, U::Px, "Block size")]),
    fx!("rgbsplit", ["chromatic", "chromaticaberration", "rgbshift"], Native, "stylize", "RGB channel split", [
        p!("amount", Num, N(6.0), 0.0, 200.0, U::Px, "Offset"),
        p!("angle", Num, N(0.0), -INF, INF, U::Deg, "Direction"),
    ]),
    fx!("glitch", ["datamosh"], Native, "stylize", "Digital glitch (slice displacement + RGB split)", [
        p!("amount", Num, N(0.5), 0.0, 1.0, U::Frac, "Intensity"),
        p!("slices", Num, N(14.0), 1.0, 200.0, U::None, "Number of slices"),
        p!("speed", Num, N(12.0), 0.0, 60.0, U::Hz, "Changes per second"),
    ]),
    fx!("scanlines", ["crt"], Native, "stylize", "CRT scanlines", [
        p!("spacing", Num, N(4.0), 2.0, 50.0, U::Px, "Line spacing"),
        p!("opacity", Num, N(0.3), 0.0, 1.0, U::Frac, "Darkness"),
    ]),
    fx!("letterbox", ["cinemabars", "bars"], Native, "stylize", "Cinematic bars", [
        p!("ratio", Num, N(2.39), 1.0, 4.0, U::None, "Target aspect ratio"),
        p!("color", Color, C("#000000"), -INF, INF, U::None, "Bar color"),
    ]),
    fx!("gradient", ["ramp", "gradientramp"], Native, "generate", "Gradient overlay (blends over layer content)", [
        p!("c1", Color, C("#000000"), -INF, INF, U::None, "Start color"),
        p!("c2", Color, C("#ffffff"), -INF, INF, U::None, "End color"),
        ps!("type", Enum(&["linear", "radial"]), S("linear"), "Gradient type"),
        p!("angle", Num, N(90.0), -INF, INF, U::Deg, "Angle (linear)"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Opacity"),
    ]),
    fx!("lightleak", ["leak"], Native, "generate", "Animated warm light leaks (screen blend)", [
        p!("color", Color, C("#ff7a18"), -INF, INF, U::None, "Leak color"),
        p!("amount", Num, N(0.6), 0.0, 1.0, U::Frac, "Strength"),
        p!("speed", Num, N(0.25), 0.0, 5.0, U::None, "Drift speed"),
    ]),
    // ---- distort
    fx!("wave", ["wavewarp"], Native, "distort", "Wave warp", [
        p!("amp", Num, N(12.0), 0.0, 500.0, U::Px, "Amplitude"),
        p!("length", Num, N(120.0), 1.0, 5000.0, U::Px, "Wavelength"),
        p!("speed", Num, N(1.0), -50.0, 50.0, U::Hz, "Speed"),
        ps!("dir", Enum(&["h", "v"]), S("h"), "Displacement direction"),
    ]),
    fx!("twirl", ["swirl"], Native, "distort", "Twirl around a center", [
        p!("angle", Num, N(90.0), -INF, INF, U::Deg, "Twist amount"),
        p!("radius", Num, N(0.4), 0.0, 2.0, U::Frac, "Radius (fraction of frame height)"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y"),
    ]),
    fx!("bulge", ["pinch", "fisheye"], Native, "distort", "Bulge (+) / pinch (-)", [
        p!("amount", Num, N(0.5), -1.0, 1.0, U::None, "Strength"),
        p!("radius", Num, N(0.4), 0.0, 2.0, U::Frac, "Radius (fraction of frame height)"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y"),
    ]),
    fx!("mirror", [], Native, "distort", "Mirror half of the frame", [ps!("axis", Enum(&["h", "v", "both"]), S("h"), "Mirror axis")]),
    fx!("kaleido", ["kaleidoscope"], Native, "distort", "Kaleidoscope", [
        p!("segments", Num, N(6.0), 2.0, 64.0, U::None, "Segments"),
        p!("angle", Num, N(0.0), -INF, INF, U::Deg, "Rotation"),
    ]),
    fx!("flip", [], Native, "distort", "Flip horizontally / vertically", [ps!("h", Bool, B(true), "Flip horizontally"), ps!("v", Bool, B(false), "Flip vertically")]),
    fx!("crop", [], Native, "distort", "Crop layer edges (fractions of the layer)", [
        p!("l", Num, N(0.0), 0.0, 1.0, U::Frac, "Left", ["left"]),
        p!("t", Num, N(0.0), 0.0, 1.0, U::Frac, "Top", ["top"]),
        p!("r", Num, N(0.0), 0.0, 1.0, U::Frac, "Right", ["right"]),
        p!("b", Num, N(0.0), 0.0, 1.0, U::Frac, "Bottom", ["bottom"]),
        p!("soft", Num, N(0.0), 0.0, 500.0, U::Px, "Edge feather"),
    ]),
    fx!("cornerpin", ["pin", "perspective"], Native, "distort", "Pin the layer corners to comp points (screen replacement)", [
        p!("tl", Vec2, V(0.0, 0.0), -INF, INF, U::Px, "Top-left corner (comp px)"),
        p!("tr", Vec2, V(1920.0, 0.0), -INF, INF, U::Px, "Top-right corner"),
        p!("br", Vec2, V(1920.0, 1080.0), -INF, INF, U::Px, "Bottom-right corner"),
        p!("bl", Vec2, V(0.0, 1080.0), -INF, INF, U::Px, "Bottom-left corner"),
    ]),
    // ---- keying
    fx!("chromakey", ["greenscreen", "keylight", "key"], Native, "keying", "Remove a key color (green screen)", [
        p!("color", Color, C("#00ff00"), -INF, INF, U::None, "Key color"),
        p!("similarity", Num, N(0.35), 0.0, 1.0, U::Frac, "Tolerance"),
        p!("smooth", Num, N(0.12), 0.0, 1.0, U::Frac, "Edge softness"),
        p!("spill", Num, N(0.6), 0.0, 1.0, U::Frac, "Spill suppression"),
    ]),
    fx!("lumakey", [], Native, "keying", "Key out dark (or bright) areas", [
        p!("threshold", Num, N(0.1), 0.0, 1.0, U::Frac, "Luma threshold"),
        p!("smooth", Num, N(0.1), 0.0, 1.0, U::Frac, "Softness"),
        ps!("invert", Bool, B(false), "Key out bright areas instead"),
    ]),
    // ---- ffmpeg-engine (decode-time, media clips only)
    fx!("denoise", ["hqdn3d"], Ff, "ffmpeg", "Video denoise (FFmpeg hqdn3d)", [ps!("strength", Num, N(4.0), "Strength 0-20")]),
    fx!("stabilize", ["deshake", "warpstabilizer"], Ff, "ffmpeg", "Stabilize shaky footage (FFmpeg deshake)", [ps!("strength", Num, N(16.0), "Search range px")]),
    fx!("deinterlace", ["bwdif", "yadif"], Ff, "ffmpeg", "Deinterlace (FFmpeg bwdif)", []),
    fx!("deflicker", [], Ff, "ffmpeg", "Remove brightness flicker", [ps!("size", Num, N(5.0), "Frames averaged")]),
    fx!("ff", ["ffmpeg", "vf"], Ff, "ffmpeg", "Raw FFmpeg video filter chain (escape hatch)", [ps!("filter", Str, S(""), "Filter chain e.g. 'eq=gamma=1.2,unsharp'", ["f", "chain"])]),
];

pub static AUDIO_EFFECTS: &[EffectDef] = &[
    fx!("eq", ["equalizer", "tone"], Audio, "audio", "3-band EQ", [
        ps!("bass", Num, N(0.0), "Low shelf gain dB (100Hz)"),
        ps!("mid", Num, N(0.0), "Mid gain dB (1kHz)"),
        ps!("treble", Num, N(0.0), "High shelf gain dB (8kHz)"),
    ]),
    fx!("highpass", ["hpf", "lowcut"], Audio, "audio", "High-pass filter", [ps!("freq", Num, N(80.0), "Cutoff Hz")]),
    fx!("lowpass", ["lpf", "highcut", "muffle"], Audio, "audio", "Low-pass filter", [ps!("freq", Num, N(12000.0), "Cutoff Hz")]),
    fx!("compress", ["compressor", "comp"], Audio, "audio", "Dynamic range compressor", [
        ps!("threshold", Num, N(-18.0), "Threshold dB"),
        ps!("ratio", Num, N(4.0), "Ratio"),
        ps!("attack", Num, N(20.0), "Attack ms"),
        ps!("release", Num, N(250.0), "Release ms"),
        ps!("makeup", Num, N(0.0), "Makeup gain dB"),
    ]),
    fx!("limiter", ["limit"], Audio, "audio", "Peak limiter", [ps!("limit", Num, N(-1.0), "Ceiling dBFS")]),
    fx!("gate", ["noisegate"], Audio, "audio", "Noise gate", [ps!("threshold", Num, N(-45.0), "Threshold dB")]),
    fx!("denoise", ["noisereduction", "nr", "afftdn"], Audio, "audio", "FFT noise reduction", [ps!("amount", Num, N(12.0), "Reduction dB")]),
    fx!("normalize", ["loudnorm", "loudness"], Audio, "audio", "EBU R128 loudness normalization", [ps!("lufs", Num, N(-14.0), "Target LUFS"), ps!("tp", Num, N(-1.0), "True peak dBTP")]),
    fx!("reverb", ["room"], Audio, "audio", "Simple reverb", [ps!("amount", Num, N(0.3), "Wet amount 0-1"), ps!("size", Num, N(0.5), "Room size 0-1")]),
    fx!("echo", ["delay"], Audio, "audio", "Echo", [ps!("delay", Num, N(300.0), "Delay ms"), ps!("decay", Num, N(0.4), "Decay 0-1")]),
    fx!("pitch", ["pitchshift"], Audio, "audio", "Pitch shift (keeps tempo)", [ps!("semitones", Num, N(0.0), "Semitones")]),
    fx!("deess", ["deesser"], Audio, "audio", "De-esser", [ps!("amount", Num, N(0.5), "Intensity 0-1")]),
    fx!("voice", ["enhance", "voiceenhance", "podcast"], Audio, "audio", "Voice enhancement chain (HPF + denoise + compress + presence)", [ps!("amount", Num, N(1.0), "Intensity 0-1")]),
    fx!("telephone", ["phone", "radio"], Audio, "audio", "Telephone / radio band-pass", []),
    fx!("ff", ["ffmpeg", "af"], Audio, "audio", "Raw FFmpeg audio filter chain", [ps!("filter", Str, S(""), "Filter chain e.g. 'aecho=0.8:0.9:500:0.3'", ["f", "chain"])]),
];

pub fn effect(name: &str) -> Option<&'static EffectDef> {
    let l = name.to_ascii_lowercase().replace(['-', '_'], "");
    EFFECTS.iter().find(|e| e.name == l || e.alias.contains(&l.as_str()))
}

pub fn audio_effect(name: &str) -> Option<&'static EffectDef> {
    let l = name.to_ascii_lowercase().replace(['-', '_'], "");
    AUDIO_EFFECTS.iter().find(|e| e.name == l || e.alias.contains(&l.as_str()))
}

// ---------------------------------------------------------------- transitions

pub struct TransDef {
    pub name: &'static str,
    pub alias: &'static [&'static str],
    pub desc: &'static str,
    pub params: &'static [P],
}

macro_rules! tr {
    ($n:expr, [$($a:expr),*], $desc:expr, [$($p:expr),* $(,)?]) => {
        TransDef { name: $n, alias: &[$($a),*], desc: $desc, params: &[$($p),*] }
    };
}

const DIRS: &[&str] = &["left", "right", "up", "down"];

pub static TRANSITIONS: &[TransDef] = &[
    tr!("dissolve", ["crossfade", "cross", "fade", "xfade", "mix", "crossdissolve"], "Cross dissolve", []),
    tr!("dip", ["diptocolor"], "Dip through a color", [ps!("color", Color, C("#000000"), "Dip color")]),
    tr!("fadeblack", ["diptoblack", "black"], "Dip to black", []),
    tr!("fadewhite", ["diptowhite", "white"], "Dip to white", []),
    tr!("wipe", ["linearwipe"], "Linear wipe", [ps!("dir", Enum(DIRS), S("left"), "Direction the edge travels toward"), ps!("soft", Num, N(0.08), "Edge softness 0-1")]),
    tr!("slide", ["cover"], "Incoming clip slides in over outgoing", [ps!("dir", Enum(DIRS), S("left"), "Direction of motion")]),
    tr!("push", [], "Incoming clip pushes outgoing out", [ps!("dir", Enum(DIRS), S("left"), "Direction of motion")]),
    tr!("reveal", ["uncover"], "Outgoing clip slides away revealing incoming", [ps!("dir", Enum(DIRS), S("left"), "Direction of motion")]),
    tr!("zoom", ["zoomin", "crosszoom"], "Zoom through with blur", [ps!("amount", Num, N(1.5), "Zoom factor")]),
    tr!("zoomout", [], "Outgoing shrinks away revealing incoming", []),
    tr!("iris", ["circle", "circleopen", "irisround"], "Circle reveal", [ps!("soft", Num, N(0.05), "Edge softness")]),
    tr!("irisclose", ["circleclose"], "Circle closes onto incoming", [ps!("soft", Num, N(0.05), "Edge softness")]),
    tr!("diamond", ["irisdiamond"], "Diamond reveal", [ps!("soft", Num, N(0.05), "Edge softness")]),
    tr!("clock", ["radial", "clockwipe"], "Clock wipe", [ps!("soft", Num, N(0.02), "Edge softness")]),
    tr!("barn", ["barndoor", "split"], "Barn doors open from the center", [ps!("dir", Enum(&["h", "v"]), S("h"), "Orientation"), ps!("soft", Num, N(0.04), "Edge softness")]),
    tr!("blinds", ["venetian"], "Venetian blinds", [ps!("count", Num, N(10.0), "Number of bands"), ps!("dir", Enum(&["h", "v"]), S("v"), "Band orientation")]),
    tr!("blur", ["blurdissolve", "hblur"], "Blur out / blur in", [ps!("amount", Num, N(40.0), "Peak blur px")]),
    tr!("whip", ["whippan", "swish"], "Fast whip pan with motion blur", [ps!("dir", Enum(DIRS), S("left"), "Direction of motion")]),
    tr!("spin", ["rotate"], "Spin & scale through", [ps!("turns", Num, N(0.5), "Rotation turns")]),
    tr!("flash", ["flashwhite", "lightning"], "Bright flash at the cut", [ps!("color", Color, C("#ffffff"), "Flash color")]),
    tr!("glitch", ["digital"], "Glitchy RGB/slice transition", []),
    tr!("pixelate", ["pixelize", "mosaic"], "Pixelate out and in", [ps!("size", Num, N(60.0), "Max block size px")]),
    tr!("luma", ["lumawipe"], "Luminance-based wipe", [ps!("soft", Num, N(0.1), "Softness"), ps!("invert", Bool, B(false), "Bright areas first")]),
    tr!("flip", ["cardflip", "flip3d"], "3D card flip", [ps!("dir", Enum(&["h", "v"]), S("h"), "Flip axis")]),
];

pub fn transition(name: &str) -> Option<(&'static TransDef, Vec<(&'static str, Val)>)> {
    let l = name.to_ascii_lowercase().replace(['-', '_'], "");
    if let Some(t) = TRANSITIONS.iter().find(|t| t.name == l || t.alias.contains(&l.as_str())) {
        return Some((t, vec![]));
    }
    // xfade-style compound names: wipeleft, slideup, smoothright, pushdown...
    for (prefix, base, extra_soft) in [("wipe", "wipe", false), ("smooth", "wipe", true), ("slide", "slide", false), ("push", "push", false), ("reveal", "reveal", false), ("whip", "whip", false), ("cover", "slide", false)] {
        if let Some(dir) = l.strip_prefix(prefix) {
            if DIRS.contains(&dir) {
                let def = TRANSITIONS.iter().find(|t| t.name == base).unwrap();
                let mut extras = vec![("dir", Val::S(dir.to_string()))];
                if extra_soft {
                    extras.push(("soft", Val::N(0.35)));
                }
                return Some((def, extras));
            }
        }
    }
    None
}

// ---------------------------------------------------------------- lookup & parsing

pub fn find<'a>(list: &'a [P], name: &str) -> Option<&'a P> {
    let l = name.to_ascii_lowercase();
    list.iter()
        .find(|p| p.name.eq_ignore_ascii_case(&l))
        .or_else(|| list.iter().find(|p| p.alias.iter().any(|a| a.eq_ignore_ascii_case(&l))))
}

/// Looks up a property for an item kind: kind-specific props first, then layer props.
pub fn item_prop(kind: ItemKind, name: &str) -> Option<&'static P> {
    find(item_props(kind), name).or_else(|| find(LAYER, name))
}

/// Closest names for "did you mean" hints.
pub fn suggest<'a>(cands: impl Iterator<Item = &'a str>, name: &str) -> Option<String> {
    let l = name.to_ascii_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for c in cands {
        let d = levenshtein(&l, &c.to_ascii_lowercase());
        if d <= 2.max(l.len() / 3) && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c.to_string())
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Context for parsing values (positions relative to the comp).
#[derive(Clone, Copy, Debug)]
pub struct VCtx {
    pub cw: f64,
    pub ch: f64,
}

pub fn default_val(p: &P, cx: VCtx) -> Val {
    match p.d {
        N(n) => Val::N(n),
        V(x, y) => Val::V(x, y),
        C(c) => Val::C(super::value::Color::parse(c).unwrap_or(super::value::Color::TRANSPARENT)),
        S(s) => Val::S(s.to_string()),
        B(b) => Val::B(b),
        Center => Val::V(cx.cw / 2.0, cx.ch / 2.0),
        CompW => Val::N(cx.cw),
        CompH => Val::N(cx.ch),
    }
}

fn parse_scalar(p: &P, s: &str) -> Result<f64> {
    let s = s.trim();
    let l = s.to_ascii_lowercase();
    let (body, pct) = match l.strip_suffix('%') {
        Some(b) => (b, true),
        None => (l.as_str(), false),
    };
    let body = body.trim_end_matches("db").trim_end_matches("deg").trim_end_matches("px").trim_end_matches("ms").trim_end_matches("hz");
    let body = if p.u == U::Sec { body.trim_end_matches('s') } else { body };
    let body = if p.u == U::Mult { body.trim_end_matches('x') } else { body };
    let mut v: f64 = body.parse().map_err(|_| anyhow!("'{}' expects a number, got '{s}'", p.name))?;
    if pct && matches!(p.u, U::Frac | U::Mult) {
        v /= 100.0;
    }
    if !v.is_finite() {
        bail!("'{}' must be finite", p.name);
    }
    Ok(v)
}

fn pos_keyword(k: &str, cx: VCtx) -> Option<(f64, f64)> {
    let (w, h) = (cx.cw, cx.ch);
    let m = 0.08; // safe margin
    Some(match k {
        "center" | "middle" | "c" => (w / 2.0, h / 2.0),
        "top" | "top-center" | "topcenter" => (w / 2.0, h * (m + 0.04)),
        "bottom" | "bottom-center" | "bottomcenter" => (w / 2.0, h * (1.0 - m - 0.04)),
        "left" | "left-center" => (w * (m + 0.12), h / 2.0),
        "right" | "right-center" => (w * (1.0 - m - 0.12), h / 2.0),
        "top-left" | "topleft" | "tl" => (w * (m + 0.12), h * (m + 0.04)),
        "top-right" | "topright" | "tr" => (w * (1.0 - m - 0.12), h * (m + 0.04)),
        "bottom-left" | "bottomleft" | "bl" => (w * (m + 0.12), h * (1.0 - m - 0.04)),
        "bottom-right" | "bottomright" | "br" => (w * (1.0 - m - 0.12), h * (1.0 - m - 0.04)),
        "lower-third" | "lowerthird" | "lower" => (w * 0.3, h * 0.82),
        "upper-third" | "upperthird" | "upper" => (w / 2.0, h * 0.22),
        _ => return None,
    })
}

/// Parses a raw DSL value for property `p`.
pub fn parse_val(p: &P, raw: &str, cx: VCtx) -> Result<Val> {
    let s = raw.trim();
    match p.t {
        Num => {
            if p.name == "weight" {
                if let Some((_, w)) = WEIGHTS.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)) {
                    return Ok(Val::N(*w));
                }
            }
            let v = parse_scalar(p, s)?;
            Ok(Val::N(v))
        }
        Vec2 => {
            let l = s.to_ascii_lowercase();
            if p.u == U::Px {
                if let Some((x, y)) = pos_keyword(&l, cx) {
                    return Ok(Val::V(x, y));
                }
            }
            let parts: Vec<&str> = s.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()).collect();
            let conv = |part: &str, axis: f64| -> Result<f64> {
                if p.u == U::Px && part.ends_with('%') {
                    let v: f64 = part.trim_end_matches('%').parse().map_err(|_| anyhow!("bad value '{part}' for {}", p.name))?;
                    Ok(v / 100.0 * axis)
                } else {
                    parse_scalar(p, part)
                }
            };
            match parts.len() {
                1 => {
                    let v = conv(parts[0], cx.cw)?;
                    if p.u == U::Px {
                        bail!("'{}' needs x,y (or a keyword like center/top/bottom-left)", p.name)
                    }
                    Ok(Val::V(v, v))
                }
                2 => Ok(Val::V(conv(parts[0], cx.cw)?, conv(parts[1], cx.ch)?)),
                _ => bail!("'{}' expects x,y", p.name),
            }
        }
        Color => Ok(Val::C(super::value::Color::parse(s)?)),
        Str => Ok(Val::S(s.to_string())),
        Bool => Ok(Val::B(match s.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "" => true,
            "0" | "false" | "no" | "off" => false,
            _ => bail!("'{}' expects true/false", p.name),
        })),
        Enum(opts) => {
            let l = s.to_ascii_lowercase().replace(['-', '_'], "");
            if let Some(o) = opts.iter().find(|o| o.replace('-', "") == l) {
                Ok(Val::S(o.to_string()))
            } else {
                let hint = suggest(opts.iter().copied(), s).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
                bail!("'{}' must be one of {}.{}", p.name, opts.join("|"), hint)
            }
        }
    }
}

/// Clamps numeric values to the declared range.
pub fn clamp_val(p: &P, v: Val) -> Val {
    match v {
        Val::N(n) if p.t == Num => Val::N(n.clamp(p.min, p.max)),
        other => other,
    }
}

/// Short type/range description used in help output.
pub fn describe(p: &P) -> String {
    let ty = match p.t {
        Num => "num".to_string(),
        Vec2 => "x,y".to_string(),
        Color => "color".to_string(),
        Str => "text".to_string(),
        Bool => "bool".to_string(),
        Enum(o) => o.join("|"),
    };
    let def = match p.d {
        N(n) => fmt_num(n),
        V(x, y) => format!("{},{}", fmt_num(x), fmt_num(y)),
        C(c) => c.to_string(),
        S(s) => {
            if s.is_empty() {
                "''".into()
            } else {
                s.to_string()
            }
        }
        B(b) => b.to_string(),
        Center => "center".into(),
        CompW => "compW".into(),
        CompH => "compH".into(),
    };
    let range = if p.t == Num && p.min.is_finite() && p.max.is_finite() { format!(" {}..{}", fmt_num(p.min), fmt_num(p.max)) } else { String::new() };
    let anim = if p.anim { "" } else { " (static)" };
    format!("{}={} [{}{}]{} {}", p.name, def, ty, range, anim, p.desc)
}

#[cfg(test)]
mod tests {
    use super::*;
    const CX: VCtx = VCtx { cw: 1920.0, ch: 1080.0 };

    #[test]
    fn parse_values() {
        let op = find(LAYER, "opacity").unwrap();
        assert_eq!(parse_val(op, "50%", CX).unwrap(), Val::N(0.5));
        let sc = find(LAYER, "scale").unwrap();
        assert_eq!(parse_val(sc, "120%", CX).unwrap(), Val::V(1.2, 1.2));
        assert_eq!(parse_val(sc, "1.5,2", CX).unwrap(), Val::V(1.5, 2.0));
        let pos = find(LAYER, "pos").unwrap();
        assert_eq!(parse_val(pos, "center", CX).unwrap(), Val::V(960.0, 540.0));
        assert_eq!(parse_val(pos, "50%,25%", CX).unwrap(), Val::V(960.0, 270.0));
        let vol = find(CLIP, "vol").unwrap();
        assert_eq!(parse_val(vol, "-6dB", CX).unwrap(), Val::N(-6.0));
        let w = find(TEXT, "weight").unwrap();
        assert_eq!(parse_val(w, "black", CX).unwrap(), Val::N(900.0));
        let blend = find(LAYER, "blend").unwrap();
        assert!(parse_val(blend, "scren", CX).unwrap_err().to_string().contains("screen"));
    }

    #[test]
    fn lookups() {
        assert!(effect("Gaussian-Blur").is_some());
        assert!(effect("greenscreen").is_some());
        assert!(audio_effect("compressor").is_some());
        let (t, extra) = transition("wipeleft").unwrap();
        assert_eq!(t.name, "wipe");
        assert_eq!(extra[0].1, Val::S("left".into()));
        assert_eq!(transition("crossfade").unwrap().0.name, "dissolve");
        assert!(item_prop(ItemKind::Text, "fontsize").is_some());
        assert!(item_prop(ItemKind::Clip, "position").is_some());
    }
}
