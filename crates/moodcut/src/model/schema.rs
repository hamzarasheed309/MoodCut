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
/// A static (not animatable) number with a range: audio effect settings.
macro_rules! pr {
    ($n:expr, $d:expr, $min:expr, $max:expr, $desc:expr) => {
        P { name: $n, t: Num, d: N($d), min: $min, max: $max, u: U::None, anim: false, desc: $desc, alias: &[] }
    };
}

use D::*;
use T::*;

pub const BLEND_MODES: &[&str] = &[
    "normal", "multiply", "screen", "overlay", "darken", "lighten", "colordodge", "colorburn", "hardlight", "softlight", "difference",
    "exclusion", "hue", "saturation", "color", "luminosity", "add", "subtract", "divide", "linearburn", "lineardodge", "linearlight",
    "vividlight", "pinlight", "hardmix", "darkercolor", "lightercolor", "dissolve", "stencilalpha", "stencilluma", "silhouettealpha",
    "silhouetteluma",
];

/// Timeline label colors (Premiere/AE labels); organizing only.
pub const LABELS: &[&str] = &["none", "red", "orange", "yellow", "green", "aqua", "blue", "violet", "pink", "gray"];

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
    ps!("3d", Bool, B(false), "3D layer: seen through the comp camera even without rotation or depth", ["threed", "layer3d"]),
    ps!("label", Enum(LABELS), S("none"), "Label color in the timeline (organizing only)"),
];

pub static CLIP: &[P] = &[
    ps!("speed", Num, N(1.0), "Playback speed (2 = 2x faster, 50% = half). Changes duration via the speed command", ["spd"]),
    ps!("reverse", Bool, B(false), "Play backwards", ["rev"]),
    ps!("fit", Enum(&["fit", "fill", "blur", "stretch", "none"]), S("fit"), "How media is fitted into the comp (blur: fitted over a blurred fill of itself)"),
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

/// Camera layer (uses the layer's pos, z, rx, ry, rot as its position and orientation).
pub static CAMERA: &[P] = &[
    p!("zoom", Num, N(0.0), 0.0, 100000.0, U::Px, "Focal length in px (0 = default: 0.926 x comp width, like a 50mm lens)", ["focal"]),
    p!("poi", Vec2, Center, -INF, INF, U::Px, "Point of interest the camera aims at (two-node camera)", ["target", "lookat"]),
    p!("poiZ", Num, N(0.0), -INF, INF, U::Px, "Depth of the point of interest"),
    ps!("aim", Bool, B(true), "Aim at the point of interest (off: one-node camera aimed with rx/ry/rot)"),
];

pub static NONE: &[P] = &[];

pub fn item_props(kind: ItemKind) -> &'static [P] {
    match kind {
        ItemKind::Clip => CLIP,
        ItemKind::Text => TEXT,
        ItemKind::Solid => SOLID,
        ItemKind::Comp => NESTED,
        ItemKind::Camera => CAMERA,
        ItemKind::Shape | ItemKind::Adjust | ItemKind::Null => NONE,
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
        ps!("look", Str, S(""), "Creative look last applied with `color <id> <preset>` (a new look replaces it; your own changes are kept)"),
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
    fx!("colorwheels", ["wheels", "liftgammagain", "colorbalance", "threeway"], Native, "color", "Lumetri color wheels: tint shadows / midtones / highlights (color alpha = strength) and shift their brightness", [
        p!("shadows", Color, C("none"), -INF, INF, U::None, "Shadow (lift) tint, e.g. #2a6cff40", ["lift"]),
        p!("midtones", Color, C("none"), -INF, INF, U::None, "Midtone (gamma) tint", ["mids", "gamma"]),
        p!("highlights", Color, C("none"), -INF, INF, U::None, "Highlight (gain) tint", ["gain"]),
        p!("shadowsL", Num, N(0.0), -100.0, 100.0, U::None, "Shadow brightness (lift)", ["liftl"]),
        p!("midtonesL", Num, N(0.0), -100.0, 100.0, U::None, "Midtone brightness (gamma)", ["gammal", "midsl"]),
        p!("highlightsL", Num, N(0.0), -100.0, 100.0, U::None, "Highlight brightness (gain)", ["gainl"]),
    ]),
    fx!("channelmixer", ["mixer", "channels"], Native, "color", "Channel mixer: each output channel as % of the input red, green and blue", [
        p!("rr", Num, N(100.0), -200.0, 200.0, U::None, "Red from red %"),
        p!("rg", Num, N(0.0), -200.0, 200.0, U::None, "Red from green %"),
        p!("rb", Num, N(0.0), -200.0, 200.0, U::None, "Red from blue %"),
        p!("gr", Num, N(0.0), -200.0, 200.0, U::None, "Green from red %"),
        p!("gg", Num, N(100.0), -200.0, 200.0, U::None, "Green from green %"),
        p!("gb", Num, N(0.0), -200.0, 200.0, U::None, "Green from blue %"),
        p!("br", Num, N(0.0), -200.0, 200.0, U::None, "Blue from red %"),
        p!("bg", Num, N(0.0), -200.0, 200.0, U::None, "Blue from green %"),
        p!("bb", Num, N(100.0), -200.0, 200.0, U::None, "Blue from blue %"),
        ps!("mono", Bool, B(false), "Monochrome: the red row drives all channels (custom black & white)"),
    ]),
    fx!("leavecolor", ["colorpass", "keepcolor", "colorsplash"], Native, "color", "Leave color: keep one color, desaturate everything else", [
        p!("color", Color, C("#e02020"), -INF, INF, U::None, "Color to keep"),
        p!("tolerance", Num, N(0.1), 0.0, 1.0, U::Frac, "Hue tolerance"),
        p!("soft", Num, N(0.08), 0.0, 1.0, U::Frac, "Edge softness"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Desaturation of the other colors"),
    ]),
    fx!("changecolor", ["changetocolor", "colorreplace", "replacecolor"], Native, "color", "Change one color to another", [
        p!("from", Color, C("#e02020"), -INF, INF, U::None, "Color to change"),
        p!("to", Color, C("#2060ff"), -INF, INF, U::None, "New color"),
        p!("tolerance", Num, N(0.1), 0.0, 1.0, U::Frac, "Hue tolerance"),
        p!("soft", Num, N(0.08), 0.0, 1.0, U::Frac, "Edge softness"),
        ps!("mode", Enum(&["hue", "color"]), S("hue"), "hue: new hue and saturation, shading kept; color: replace the color"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
    ]),
    fx!("secondary", ["hslsecondary", "hsl2", "qualifier", "selectivecolor"], Native, "color", "Lumetri HSL secondary: key a hue / saturation / luma range and correct only that range", [
        p!("hue", Num, N(25.0), -INF, INF, U::Deg, "Key hue center (0 red, 25 skin, 60 yellow, 120 green, 210 sky, 270 purple)"),
        p!("hueRange", Num, N(30.0), 0.0, 180.0, U::Deg, "Hue range (+/- degrees)"),
        p!("satMin", Num, N(0.1), 0.0, 1.0, U::Frac, "Minimum saturation keyed"),
        p!("lumMin", Num, N(0.05), 0.0, 1.0, U::Frac, "Minimum luma keyed"),
        p!("lumMax", Num, N(0.95), 0.0, 1.0, U::Frac, "Maximum luma keyed"),
        p!("soft", Num, N(0.15), 0.0, 1.0, U::Frac, "Key softness"),
        p!("shift", Num, N(0.0), -180.0, 180.0, U::Deg, "Hue shift of the keyed range"),
        p!("sat", Num, N(1.0), 0.0, 4.0, U::Mult, "Saturation of the keyed range"),
        p!("light", Num, N(0.0), -1.0, 1.0, U::None, "Lightness of the keyed range"),
        p!("temp", Num, N(0.0), -100.0, 100.0, U::None, "Temperature of the keyed range"),
        p!("tint", Num, N(0.0), -100.0, 100.0, U::None, "Tint of the keyed range"),
        ps!("show", Bool, B(false), "Show the key as a gray matte (to check the selection)", ["showmask"]),
        ps!("invert", Bool, B(false), "Correct everything except the keyed range"),
    ]),
    fx!("huecurves", ["huesatcurves", "huevssat", "hslcurves"], Native, "color", "Lumetri hue/saturation curves. Each point 'x,y[,width]' adds a smooth bump: x = position 0..1 (hue or luma), y = change -1..1", [
        ps!("hueSat", Str, S(""), "Hue vs saturation, e.g. '0.33,-0.6' mutes greens, '0.58,0.4' boosts sky blue"),
        ps!("hueHue", Str, S(""), "Hue vs hue (y=0.1 rotates by 36 degrees)"),
        ps!("hueLuma", Str, S(""), "Hue vs luma, e.g. '0.6,-0.3' darkens blue sky"),
        ps!("lumaSat", Str, S(""), "Luma vs saturation, e.g. '0.05,-1' removes color from the blacks"),
        ps!("satSat", Str, S(""), "Saturation vs saturation"),
    ]),
    fx!("photofilter", ["filter", "warmingfilter", "coolingfilter"], Native, "color", "Photo filter: tint as with a colored lens filter", [
        p!("color", Color, C("#ec8a00"), -INF, INF, U::None, "Filter color (warming #ec8a00, cooling #006dff)"),
        p!("density", Num, N(0.25), 0.0, 1.0, U::Frac, "Density"),
        ps!("preserve", Bool, B(true), "Preserve luminosity"),
    ]),
    fx!("tritone", [], Native, "color", "Map shadows, midtones and highlights to three colors", [
        p!("shadows", Color, C("#0b0b1a"), -INF, INF, U::None, "Shadow color"),
        p!("midtones", Color, C("#7f6a4f"), -INF, INF, U::None, "Midtone color"),
        p!("highlights", Color, C("#fff6e5"), -INF, INF, U::None, "Highlight color"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
    ]),
    fx!("autolevels", ["autocontrast", "autocolor", "autotone", "autocorrect"], Native, "color", "Auto levels: stretch every frame to the full range (perchannel=1 also removes color casts)", [
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
        p!("clip", Num, N(0.005), 0.0, 0.2, U::Frac, "Fraction clipped at black and at white"),
        ps!("perchannel", Bool, B(false), "Per channel (auto color)"),
    ]),
    fx!("legalize", ["videolimiter", "broadcastsafe", "legal"], Native, "color", "Video limiter: keep levels inside the broadcast-legal range", [
        p!("min", Num, N(0.0627), 0.0, 0.5, U::Frac, "Lowest level (16/255)"),
        p!("max", Num, N(0.9216), 0.5, 1.0, U::Frac, "Highest level (235/255)"),
        ps!("mode", Enum(&["compress", "clip"]), S("compress"), "compress: rescale the full range; clip: cut off what's outside"),
    ]),
    // ---- blur & sharpen
    fx!("blur", ["gaussian", "gaussianblur", "boxblur", "fastblur"], Native, "blur", "Gaussian blur", [
        p!("radius", Num, N(10.0), 0.0, 500.0, U::Px, "Blur radius", ["r", "amount"]),
        ps!("dir", Enum(&["both", "h", "v"]), S("both"), "Blur direction"),
    ]),
    fx!("dirblur", ["motionblurfx", "directionalblur"], Native, "blur", "Directional (motion) blur", [
        p!("angle", Num, N(0.0), -INF, INF, U::Deg, "Direction"),
        p!("length", Num, N(30.0), 0.0, 1000.0, U::Px, "Blur length"),
    ]),
    fx!("zoomblur", ["radialblur", "spinblur"], Native, "blur", "Radial blur: zoom (streaks from a center) or spin (rotational)", [
        p!("amount", Num, N(0.15), 0.0, 1.0, U::None, "Strength (spin: 1 = 60 degrees)"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X (fraction of frame)"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y (fraction of frame)"),
        ps!("type", Enum(&["zoom", "spin"]), S("zoom"), "Blur type"),
    ]),
    fx!("tiltshift", ["miniature"], Native, "blur", "Tilt-shift: a sharp band with blur above and below (miniature look)", [
        p!("center", Num, N(0.55), -INF, INF, U::Frac, "Band center (fraction of frame height)"),
        p!("width", Num, N(0.12), 0.0, 2.0, U::Frac, "Sharp band half-height (fraction of frame height)"),
        p!("soft", Num, N(0.25), 0.001, 2.0, U::Frac, "Distance to full blur"),
        p!("radius", Num, N(18.0), 0.0, 500.0, U::Px, "Blur radius"),
        p!("angle", Num, N(0.0), -INF, INF, U::Deg, "Band angle"),
    ]),
    fx!("smooth", ["surfaceblur", "skinsmooth", "beauty", "bilateral", "smartblur"], Native, "blur", "Edge-preserving blur (skin smoothing, clean surfaces)", [
        p!("radius", Num, N(6.0), 0.0, 100.0, U::Px, "Radius"),
        p!("threshold", Num, N(0.08), 0.001, 1.0, U::Frac, "Edge threshold (lower keeps more detail)"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
    ]),
    fx!("lensblur", ["bokeh", "defocus", "camerablur", "cameralensblur"], Native, "blur", "Camera lens blur with blooming bokeh highlights", [
        p!("radius", Num, N(12.0), 0.0, 300.0, U::Px, "Blur radius"),
        p!("bright", Num, N(1.5), 0.0, 10.0, U::None, "Highlight bloom"),
        p!("threshold", Num, N(0.75), 0.0, 1.0, U::Frac, "Highlight threshold"),
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
    fx!("findedges", ["edges", "sobel", "edgedetect"], Native, "stylize", "Find edges (line-drawing look)", [
        ps!("invert", Bool, B(true), "Dark lines on white (off: bright lines on black)"),
        p!("strength", Num, N(1.0), 0.0, 10.0, U::None, "Edge strength"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Blend with the original"),
    ]),
    fx!("emboss", ["relief"], Native, "stylize", "Emboss", [
        p!("angle", Num, N(135.0), -INF, INF, U::Deg, "Light direction"),
        p!("relief", Num, N(2.0), 0.0, 50.0, U::Px, "Relief depth"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Blend with the original"),
        ps!("color", Bool, B(false), "Keep the original colors (color emboss)"),
    ]),
    fx!("halftone", ["colorhalftone", "dots", "comic"], Native, "stylize", "Halftone dots (print / comic look)", [
        p!("size", Num, N(10.0), 2.0, 200.0, U::Px, "Dot spacing"),
        p!("angle", Num, N(45.0), -INF, INF, U::Deg, "Screen angle"),
        ps!("mono", Bool, B(true), "Black dots on white (off: colored dots per channel)"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Amount"),
    ]),
    fx!("strobe", ["strobelight", "flashing"], Native, "stylize", "Strobe light", [
        p!("color", Color, C("#ffffff"), -INF, INF, U::None, "Flash color"),
        p!("freq", Num, N(4.0), 0.01, 60.0, U::Hz, "Flashes per second"),
        p!("duty", Num, N(0.25), 0.01, 1.0, U::Frac, "Flash length (fraction of a period)"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Strength"),
        ps!("mode", Enum(&["flash", "hide"]), S("flash"), "flash: color over the layer; hide: the layer blinks off"),
    ]),
    fx!("bevel", ["bevelalpha", "bevelemboss"], Native, "stylize", "Bevel the layer's edges (text, logos)", [
        p!("depth", Num, N(6.0), 0.0, 200.0, U::Px, "Edge thickness"),
        p!("angle", Num, N(135.0), -INF, INF, U::Deg, "Light angle"),
        p!("intensity", Num, N(0.6), 0.0, 1.0, U::Frac, "Light intensity"),
        p!("light", Color, C("#ffffff"), -INF, INF, U::None, "Highlight color"),
        p!("dark", Color, C("#000000"), -INF, INF, U::None, "Shadow color"),
    ]),
    fx!("innershadow", [], Native, "stylize", "Inner shadow inside the layer's edges", [
        p!("color", Color, C("#000000"), -INF, INF, U::None, "Shadow color"),
        p!("opacity", Num, N(0.6), 0.0, 1.0, U::Frac, "Opacity"),
        p!("angle", Num, N(135.0), -INF, INF, U::Deg, "Light angle"),
        p!("distance", Num, N(8.0), 0.0, 500.0, U::Px, "Distance"),
        p!("blur", Num, N(10.0), 0.0, 500.0, U::Px, "Softness"),
    ]),
    fx!("outerglow", ["alphaglow", "halo", "neonglow"], Native, "stylize", "Colored glow around the layer's alpha (neon text, logo halos)", [
        p!("color", Color, C("#ffdd55"), -INF, INF, U::None, "Glow color"),
        p!("opacity", Num, N(0.8), 0.0, 1.0, U::Frac, "Opacity"),
        p!("size", Num, N(20.0), 0.0, 500.0, U::Px, "Glow size"),
        p!("spread", Num, N(0.1), 0.0, 1.0, U::Frac, "Spread (harder glow)"),
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
    fx!("fourcolor", ["4color", "fourcolorgradient"], Native, "generate", "Four-color corner gradient over the layer (apply to a solid for a background)", [
        p!("tl", Color, C("#ff3b6b"), -INF, INF, U::None, "Top-left color"),
        p!("tr", Color, C("#ffb020"), -INF, INF, U::None, "Top-right color"),
        p!("bl", Color, C("#3b5bff"), -INF, INF, U::None, "Bottom-left color"),
        p!("br", Color, C("#20d0a0"), -INF, INF, U::None, "Bottom-right color"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Opacity"),
    ]),
    fx!("checker", ["checkerboard"], Native, "generate", "Checkerboard pattern over the layer", [
        p!("size", Num, N(64.0), 1.0, 5000.0, U::Px, "Square size"),
        p!("c1", Color, C("#ffffff"), -INF, INF, U::None, "First color"),
        p!("c2", Color, C("#000000"), -INF, INF, U::None, "Second color"),
        p!("offset", Vec2, V(0.0, 0.0), -INF, INF, U::None, "Pattern offset (px)"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Opacity"),
    ]),
    fx!("grid", ["gridlines"], Native, "generate", "Grid lines over the layer", [
        p!("spacing", Num, N(80.0), 2.0, 5000.0, U::Px, "Cell size"),
        p!("width", Num, N(2.0), 0.0, 200.0, U::Px, "Line width"),
        p!("color", Color, C("#ffffff80"), -INF, INF, U::None, "Line color"),
        p!("offset", Vec2, V(0.0, 0.0), -INF, INF, U::None, "Grid offset (px)"),
    ]),
    fx!("fractal", ["fractalnoise", "clouds", "noisetexture"], Native, "generate", "Fractal noise texture over the layer (smoke, clouds, animated backgrounds; apply to a solid)", [
        p!("scale", Num, N(240.0), 1.0, 10000.0, U::Px, "Noise size"),
        p!("complexity", Num, N(5.0), 1.0, 10.0, U::None, "Detail octaves"),
        p!("contrast", Num, N(1.0), 0.0, 10.0, U::None, "Contrast"),
        p!("brightness", Num, N(0.0), -1.0, 1.0, U::None, "Brightness"),
        p!("speed", Num, N(0.3), -50.0, 50.0, U::None, "Evolution per second"),
        ps!("type", Enum(&["smooth", "turbulent", "ridged"]), S("smooth"), "Noise type"),
        p!("c1", Color, C("#000000"), -INF, INF, U::None, "Dark color"),
        p!("c2", Color, C("#ffffff"), -INF, INF, U::None, "Bright color"),
        p!("offset", Vec2, V(0.0, 0.0), -INF, INF, U::None, "Scroll (px, animate to drift)"),
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Opacity"),
    ]),
    fx!("lensflare", ["flare"], Native, "generate", "Lens flare, screen-blended over the layer (use on footage or an adjustment layer)", [
        p!("pos", Vec2, V(0.0, 0.0), -INF, INF, U::Px, "Flare center (comp px; default upper right)", ["center"]),
        p!("brightness", Num, N(1.0), 0.0, 5.0, U::None, "Brightness"),
        p!("color", Color, C("#ffd7a0"), -INF, INF, U::None, "Tint"),
        p!("rays", Num, N(10.0), 0.0, 64.0, U::None, "Number of rays"),
        p!("streak", Num, N(0.6), 0.0, 2.0, U::None, "Anamorphic streak"),
        p!("ghosts", Num, N(0.5), 0.0, 2.0, U::None, "Ghost circles toward the frame center"),
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
    fx!("transform", ["xform"], Native, "distort", "Transform inside the effect stack (applies after the effects above it: shakes and pulses on top of motion)", [
        p!("offset", Vec2, V(0.0, 0.0), -INF, INF, U::None, "Position offset (px)", ["pos", "position"]),
        p!("scale", Vec2, V(1.0, 1.0), -100.0, 100.0, U::Mult, "Scale"),
        p!("rot", Num, N(0.0), -INF, INF, U::Deg, "Rotation", ["rotation"]),
        p!("skew", Num, N(0.0), -85.0, 85.0, U::Deg, "Skew"),
        p!("anchor", Vec2, V(0.5, 0.5), -INF, INF, U::Frac, "Pivot as a fraction of the layer's box"),
        p!("opacity", Num, N(1.0), 0.0, 1.0, U::Frac, "Opacity"),
    ]),
    fx!("offset", ["wrap", "scroll"], Native, "distort", "Offset: shift the image with wrap-around (seamless scrolling)", [
        p!("x", Num, N(0.0), -INF, INF, U::Px, "Horizontal shift"),
        p!("y", Num, N(0.0), -INF, INF, U::Px, "Vertical shift"),
        p!("blend", Num, N(0.0), 0.0, 1.0, U::Frac, "Blend with the original"),
    ]),
    fx!("polar", ["polarcoordinates", "littleplanet"], Native, "distort", "Polar coordinates (rect to polar = 'little planet', or back)", [
        p!("amount", Num, N(1.0), 0.0, 1.0, U::Frac, "Interpolation"),
        ps!("mode", Enum(&["topolar", "torect"]), S("topolar"), "Conversion"),
    ]),
    fx!("ripple", ["waterripple", "rings"], Native, "distort", "Concentric ripples", [
        p!("amp", Num, N(8.0), 0.0, 500.0, U::Px, "Amplitude"),
        p!("length", Num, N(60.0), 1.0, 5000.0, U::Px, "Wavelength"),
        p!("speed", Num, N(1.0), -50.0, 50.0, U::Hz, "Speed"),
        p!("radius", Num, N(0.8), 0.0, 4.0, U::Frac, "Fade-out radius (fraction of frame height)"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y"),
    ]),
    fx!("lens", ["lensdistortion", "barrel", "optics", "opticscompensation", "lenscorrection"], Native, "distort", "Lens distortion: barrel (+) or pincushion (-); negative removes action-cam curvature", [
        p!("amount", Num, N(0.25), -1.0, 1.0, U::None, "Curvature"),
        p!("zoom", Num, N(1.0), 0.1, 10.0, U::Mult, "Zoom (hide the edges)"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y"),
    ]),
    fx!("turbulence", ["turbulentdisplace", "displace", "warpnoise"], Native, "distort", "Turbulent displace: organic wobble, heat haze, liquid", [
        p!("amount", Num, N(20.0), 0.0, 1000.0, U::Px, "Displacement"),
        p!("size", Num, N(120.0), 1.0, 5000.0, U::Px, "Noise size"),
        p!("complexity", Num, N(3.0), 1.0, 8.0, U::None, "Detail octaves"),
        p!("speed", Num, N(0.5), -50.0, 50.0, U::None, "Evolution per second"),
    ]),
    fx!("tile", ["motiontile", "repeat"], Native, "distort", "Motion tile: repeat the layer to fill a larger area (mirror for seamless edges)", [
        p!("outW", Num, N(3.0), 1.0, 20.0, U::Mult, "Output width (x the layer's width)"),
        p!("outH", Num, N(3.0), 1.0, 20.0, U::Mult, "Output height (x the layer's height)"),
        p!("tileW", Num, N(1.0), 0.05, 10.0, U::Frac, "Tile width (fraction of the layer)"),
        p!("tileH", Num, N(1.0), 0.05, 10.0, U::Frac, "Tile height (fraction of the layer)"),
        ps!("mirror", Bool, B(false), "Mirror edges"),
        p!("phase", Vec2, V(0.0, 0.0), -INF, INF, U::None, "Scroll the tiles (px)"),
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
    fx!("choke", ["matterchoke", "simplechoker", "erode", "mattechoker"], Native, "keying", "Matte choker: shrink (+) or spread (-) the alpha edge after a key", [
        p!("amount", Num, N(2.0), -100.0, 100.0, U::Px, "Choke amount"),
        p!("soft", Num, N(1.0), 0.0, 100.0, U::Px, "Edge softness"),
    ]),
    fx!("spill", ["spillsuppress", "despill", "spillsuppressor"], Native, "keying", "Spill suppressor: remove green/blue spill from edges after a key", [
        p!("color", Color, C("#00ff00"), -INF, INF, U::None, "Key color"),
        p!("amount", Num, N(0.7), 0.0, 1.0, U::Frac, "Suppression"),
    ]),
    // ---- transition effects (animate completion 0 -> 100, any layer)
    fx!("linearwipe", [], Native, "transition", "Linear wipe: hides the layer along a line (animate completion 0->100)", [
        p!("completion", Num, N(0.0), 0.0, 100.0, U::Pct, "Transition completion %"),
        p!("angle", Num, N(90.0), -INF, INF, U::Deg, "Wipe angle (90 = from the left)"),
        p!("feather", Num, N(0.0), 0.0, 5000.0, U::Px, "Edge feather"),
    ]),
    fx!("radialwipe", ["clockwipefx"], Native, "transition", "Radial wipe: clock-hand reveal around a center", [
        p!("completion", Num, N(0.0), 0.0, 100.0, U::Pct, "Transition completion %"),
        p!("start", Num, N(0.0), -INF, INF, U::Deg, "Start angle (0 = 12 o'clock)"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y"),
        ps!("dir", Enum(&["cw", "ccw", "both"]), S("cw"), "Direction"),
        p!("feather", Num, N(0.0), 0.0, 180.0, U::Deg, "Edge feather (degrees)"),
    ]),
    fx!("iriswipe", ["circlewipe"], Native, "transition", "Iris wipe: a circle closes onto the center (invert opens from the center)", [
        p!("completion", Num, N(0.0), 0.0, 100.0, U::Pct, "Transition completion %"),
        p!("cx", Num, N(0.5), -INF, INF, U::Frac, "Center X"),
        p!("cy", Num, N(0.5), -INF, INF, U::Frac, "Center Y"),
        p!("feather", Num, N(0.0), 0.0, 5000.0, U::Px, "Edge feather"),
        ps!("invert", Bool, B(false), "Hide from the center outward"),
    ]),
    fx!("blockdissolve", ["dissolveblocks"], Native, "transition", "Block dissolve: the layer disappears in random blocks", [
        p!("completion", Num, N(0.0), 0.0, 100.0, U::Pct, "Transition completion %"),
        p!("size", Num, N(16.0), 1.0, 2000.0, U::Px, "Block size"),
        p!("soft", Num, N(0.0), 0.0, 1.0, U::Frac, "Softness"),
    ]),
    fx!("venetianblinds", ["blindsfx", "stripes"], Native, "transition", "Venetian blinds wipe", [
        p!("completion", Num, N(0.0), 0.0, 100.0, U::Pct, "Transition completion %"),
        p!("angle", Num, N(0.0), -INF, INF, U::Deg, "Stripe angle"),
        p!("width", Num, N(40.0), 1.0, 5000.0, U::Px, "Stripe width"),
        p!("feather", Num, N(0.0), 0.0, 500.0, U::Px, "Edge feather"),
    ]),
    fx!("gradientwipe", ["lumawipefx"], Native, "transition", "Gradient wipe: dark areas disappear first (by the layer's own luminance)", [
        p!("completion", Num, N(0.0), 0.0, 100.0, U::Pct, "Transition completion %"),
        p!("soft", Num, N(0.1), 0.0, 1.0, U::Frac, "Softness"),
        ps!("invert", Bool, B(false), "Bright areas first"),
    ]),
    // ---- time (evaluated by the compositor)
    fx!("posterizetime", ["framerate", "choppy", "stutter", "lowfps"], Native, "time", "Posterize time: the layer updates at a lower frame rate (stop-motion look)", [ps!("fps", Num, N(12.0), "Frame rate")]),
    fx!("echo", ["trails", "timeecho", "ghosting"], Native, "time", "Echo: trails of the layer's earlier frames", [
        ps!("time", Num, N(0.05), "Seconds between echoes (negative = future frames)"),
        ps!("count", Num, N(4.0), "Number of echoes (1-16)"),
        ps!("decay", Num, N(0.6), "Opacity multiplier per echo"),
        ps!("mode", Enum(&["behind", "front", "add", "screen"]), S("behind"), "How the echoes combine"),
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
        pr!("bass", 0.0, -24.0, 24.0, "Low shelf gain dB (100Hz)"),
        pr!("mid", 0.0, -24.0, 24.0, "Mid gain dB (1kHz)"),
        pr!("treble", 0.0, -24.0, 24.0, "High shelf gain dB (8kHz)"),
    ]),
    fx!("highpass", ["hpf", "lowcut"], Audio, "audio", "High-pass filter", [pr!("freq", 80.0, 10.0, 20000.0, "Cutoff Hz")]),
    fx!("lowpass", ["lpf", "highcut", "muffle"], Audio, "audio", "Low-pass filter", [pr!("freq", 12000.0, 20.0, 20000.0, "Cutoff Hz")]),
    fx!("compress", ["compressor", "comp"], Audio, "audio", "Dynamic range compressor", [
        pr!("threshold", -18.0, -60.0, 0.0, "Threshold dB"),
        pr!("ratio", 4.0, 1.0, 20.0, "Ratio"),
        pr!("attack", 20.0, 0.01, 2000.0, "Attack ms"),
        pr!("release", 250.0, 0.01, 9000.0, "Release ms"),
        pr!("makeup", 0.0, 0.0, 36.0, "Makeup gain dB"),
    ]),
    fx!("limiter", ["limit"], Audio, "audio", "Peak limiter", [pr!("limit", -1.0, -24.0, 0.0, "Ceiling dBFS")]),
    fx!("gate", ["noisegate"], Audio, "audio", "Noise gate", [pr!("threshold", -45.0, -80.0, 0.0, "Threshold dB")]),
    fx!("denoise", ["noisereduction", "nr", "afftdn"], Audio, "audio", "FFT noise reduction", [pr!("amount", 12.0, 0.0, 97.0, "Reduction dB")]),
    fx!("normalize", ["loudnorm", "loudness"], Audio, "audio", "EBU R128 loudness normalization", [pr!("lufs", -14.0, -70.0, -5.0, "Target LUFS"), pr!("tp", -1.0, -9.0, 0.0, "True peak dBTP")]),
    fx!("reverb", ["room"], Audio, "audio", "Simple reverb", [pr!("amount", 0.3, 0.0, 1.0, "Wet amount 0-1"), pr!("size", 0.5, 0.0, 1.0, "Room size 0-1")]),
    fx!("echo", ["delay"], Audio, "audio", "Echo", [pr!("delay", 300.0, 1.0, 5000.0, "Delay ms"), pr!("decay", 0.4, 0.0, 1.0, "Decay 0-1")]),
    fx!("pitch", ["pitchshift"], Audio, "audio", "Pitch shift (keeps tempo)", [pr!("semitones", 0.0, -24.0, 24.0, "Semitones")]),
    fx!("deess", ["deesser"], Audio, "audio", "De-esser", [pr!("amount", 0.5, 0.0, 1.0, "Intensity 0-1")]),
    fx!("voice", ["enhance", "voiceenhance", "podcast"], Audio, "audio", "Voice enhancement chain (HPF + denoise + compress + presence)", [pr!("amount", 1.0, 0.0, 1.0, "Intensity 0-1")]),
    fx!("telephone", ["phone", "radio"], Audio, "audio", "Telephone / radio band-pass", []),
    fx!("peq", ["parametric", "parametriceq", "eq3"], Audio, "audio", "Parametric EQ: three bell bands (freq Hz, gain dB, q)", [
        pr!("f1", 200.0, 20.0, 20000.0, "Band 1 frequency Hz"),
        pr!("g1", 0.0, -30.0, 30.0, "Band 1 gain dB"),
        pr!("q1", 1.0, 0.1, 20.0, "Band 1 width (Q)"),
        pr!("f2", 1000.0, 20.0, 20000.0, "Band 2 frequency Hz"),
        pr!("g2", 0.0, -30.0, 30.0, "Band 2 gain dB"),
        pr!("q2", 1.0, 0.1, 20.0, "Band 2 width (Q)"),
        pr!("f3", 5000.0, 20.0, 20000.0, "Band 3 frequency Hz"),
        pr!("g3", 0.0, -30.0, 30.0, "Band 3 gain dB"),
        pr!("q3", 1.0, 0.1, 20.0, "Band 3 width (Q)"),
    ]),
    fx!("bassboost", ["bass", "lowshelf"], Audio, "audio", "Bass boost (low shelf)", [pr!("gain", 6.0, -30.0, 30.0, "Gain dB"), pr!("freq", 120.0, 20.0, 1000.0, "Shelf frequency Hz")]),
    fx!("gain", ["amplify", "volume"], Audio, "audio", "Static gain stage", [pr!("db", 0.0, -60.0, 30.0, "Gain dB")]),
    fx!("chorus", [], Audio, "audio", "Chorus (thicker, doubled sound)", [pr!("depth", 0.5, 0.0, 1.0, "Depth 0-1"), pr!("mix", 0.5, 0.0, 1.0, "Wet amount 0-1")]),
    fx!("flanger", [], Audio, "audio", "Flanger (jet sweep)", [pr!("depth", 2.0, 0.0, 10.0, "Sweep depth ms"), pr!("speed", 0.5, 0.1, 10.0, "Sweep speed Hz")]),
    fx!("phaser", [], Audio, "audio", "Phaser", [pr!("speed", 0.5, 0.1, 2.0, "Speed Hz"), pr!("decay", 0.4, 0.0, 0.99, "Feedback 0-0.99")]),
    fx!("tremolo", [], Audio, "audio", "Tremolo (volume wobble)", [pr!("freq", 5.0, 0.1, 20000.0, "Rate Hz"), pr!("depth", 0.5, 0.0, 1.0, "Depth 0-1")]),
    fx!("vibrato", [], Audio, "audio", "Vibrato (pitch wobble)", [pr!("freq", 5.0, 0.1, 20000.0, "Rate Hz"), pr!("depth", 0.5, 0.0, 1.0, "Depth 0-1")]),
    fx!("distortion", ["overdrive", "saturate", "softclip"], Audio, "audio", "Distortion / saturation", [pr!("amount", 0.5, 0.0, 1.0, "Drive 0-1")]),
    fx!("bitcrush", ["crusher", "lofi"], Audio, "audio", "Bit crusher (lo-fi)", [pr!("bits", 8.0, 1.0, 16.0, "Bit depth 1-16"), pr!("downsample", 1.0, 1.0, 250.0, "Sample-rate reduction factor 1-250")]),
    fx!("widen", ["stereowiden", "extrastereo", "wide"], Audio, "audio", "Stereo widener (1 = unchanged, 0 = mono)", [pr!("amount", 1.6, 0.0, 4.0, "Width 0-4")]),
    fx!("mono", ["tomono", "downmix"], Audio, "audio", "Sum to mono (both channels)", []),
    fx!("swap", ["swapchannels", "channelswap"], Audio, "audio", "Swap left and right channels", []),
    fx!("dehum", ["humremove", "hum"], Audio, "audio", "Remove mains hum (notches at the hum frequency and its harmonics)", [pr!("freq", 50.0, 50.0, 60.0, "Hum frequency: 50 (Europe/Asia) or 60 (Americas)"), pr!("harmonics", 4.0, 1.0, 8.0, "Harmonics to notch 1-8")]),
    fx!("declick", ["declicker", "crackle"], Audio, "audio", "Remove clicks and crackle", []),
    fx!("exciter", ["presence", "brilliance"], Audio, "audio", "Harmonic exciter (adds brightness and presence)", [pr!("amount", 1.0, 0.0, 10.0, "Amount 0-10"), pr!("freq", 7500.0, 2000.0, 12000.0, "Above this frequency Hz")]),
    fx!("autogain", ["dynaudnorm", "leveler", "levelling", "speechnorm"], Audio, "audio", "Dynamic leveler: evens out loud and quiet passages", [pr!("strength", 0.5, 0.0, 1.0, "Strength 0-1")]),
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
    tr!("cube", ["cubespin", "cube3d"], "3D cube spin: both clips are faces of a turning cube", [ps!("dir", Enum(DIRS), S("left"), "Direction the cube turns toward")]),
    tr!("blocks", ["blockdissolve", "randomblocks"], "Random blocks dissolve", [ps!("size", Num, N(48.0), "Block size px")]),
    tr!("checker", ["checkerwipe", "checkerboard"], "Checkerboard wipe", [ps!("count", Num, N(8.0), "Squares across")]),
    tr!("inset", ["boxwipe", "corner"], "Inset: incoming clip grows from a corner", [ps!("dir", Enum(&["tl", "tr", "bl", "br", "center"]), S("tl"), "Corner it grows from"), ps!("soft", Num, N(0.0), "Edge softness 0-1")]),
    tr!("irisbox", ["boxiris"], "Box iris from the center", [ps!("soft", Num, N(0.03), "Edge softness")]),
    tr!("iriscross", ["crossiris", "plus"], "Cross-shaped iris from the center", [ps!("soft", Num, N(0.03), "Edge softness")]),
    tr!("band", ["bandwipe", "bands"], "Band wipe: alternating bands slide in from both sides", [ps!("count", Num, N(8.0), "Number of bands"), ps!("dir", Enum(&["h", "v"]), S("h"), "Band orientation")]),
    tr!("filmdissolve", ["film", "gammadissolve"], "Film dissolve (linear-light, smoother highlights)", []),
    tr!("additive", ["additivedissolve", "burnin"], "Additive dissolve: both images add up at the middle", []),
    tr!("leak", ["lightleak", "leakwipe"], "Dissolve through a burst of light leaks", [ps!("color", Color, C("#ff7a18"), "Leak color")]),
    tr!("burn", ["filmburn", "fire"], "Film burn: the image burns away in noisy, glowing patches", [ps!("color", Color, C("#ff6a00"), "Burn glow color"), ps!("soft", Num, N(0.08), "Edge softness")]),
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
    // fallback for abbreviations ('type' -> typeOut/typewriter, 'punchin' -> zoomPunchIn)
    let mut part: Vec<&str> = vec![];
    for c in cands {
        let cl = c.to_ascii_lowercase();
        let d = levenshtein(&l, &cl);
        if d <= 2.max(l.len() / 3) && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, c));
        }
        if l.len() >= 3 && (cl.starts_with(&l) || (l.len() >= 5 && cl.contains(&l))) && !part.contains(&c) {
            part.push(c);
        }
    }
    part.sort_by_key(|c| c.len());
    let mut out: Vec<&str> = best.map(|(_, c)| c).into_iter().collect();
    out.extend(part.into_iter().filter(|c| Some(*c) != best.map(|(_, b)| b)));
    // callers wrap the result as "Did you mean '{h}'?"
    (!out.is_empty()).then(|| out.into_iter().take(3).collect::<Vec<_>>().join("' or '"))
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
    fn audio_effect_settings_have_ranges_around_their_defaults() {
        // the mixer draws its sliders from these ranges
        for e in AUDIO_EFFECTS {
            for p in e.params.iter().filter(|p| p.t == Num) {
                assert!(p.min.is_finite() && p.max.is_finite() && p.min < p.max, "{}.{} has no range", e.name, p.name);
                let d = default_val(p, CX).num();
                assert!(d >= p.min && d <= p.max, "{}.{} default {d} is outside {}..{}", e.name, p.name, p.min, p.max);
                assert!(!p.anim, "{}.{} must stay static", e.name, p.name);
            }
        }
    }

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
    fn names_and_aliases_are_unique() {
        for (what, names) in [
            ("effect", EFFECTS.iter().flat_map(|e| std::iter::once(e.name).chain(e.alias.iter().copied())).collect::<Vec<_>>()),
            ("audio effect", AUDIO_EFFECTS.iter().flat_map(|e| std::iter::once(e.name).chain(e.alias.iter().copied())).collect()),
            ("transition", TRANSITIONS.iter().flat_map(|t| std::iter::once(t.name).chain(t.alias.iter().copied())).collect()),
        ] {
            let mut seen = std::collections::HashSet::new();
            for n in names {
                assert_eq!(n, n.to_ascii_lowercase().replace(['-', '_'], ""), "{what} name '{n}' must be normalized");
                assert!(seen.insert(n), "{what} name or alias '{n}' is used twice");
            }
        }
        for e in EFFECTS.iter().chain(AUDIO_EFFECTS) {
            let mut seen = std::collections::HashSet::new();
            for p in e.params {
                for n in std::iter::once(p.name).chain(p.alias.iter().copied()) {
                    assert!(seen.insert(n.to_ascii_lowercase()), "{}: param name or alias '{n}' is used twice", e.name);
                }
            }
        }
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
