//! Fused grades. A run of per-pixel color effects with the same window is
//! baked into one 33-point 3D LUT, so a five-effect grade costs one lookup
//! per pixel instead of five passes. The LUT is made by running the effects
//! themselves over a lattice image (every 33rd RGB value), so any per-pixel
//! color effect fuses without a second copy of its math.

use super::effects::{self, FxCtx};
use super::lut::{self, Lut3};
use super::pixel::{self, LayerBuf, Pm};
use crate::model::project::Effect;
use crate::model::schema;
use std::fmt::Write;
use std::sync::Arc;

const N: usize = 33;

/// Color effects whose result depends only on the pixel's own color, through
/// a smooth function (so a 33-point LUT reproduces it). Posterize, threshold
/// (steps), auto levels (image statistics) and fill (alpha) are left out.
pub fn fusable(e: &Effect) -> bool {
    if !e.on {
        return false;
    }
    let Some(def) = schema::effect(&e.kind) else { return false };
    // a parallel node reads the input of the node before it, not a LUT chain
    if e.props.get("parallel").is_some_and(|a| a.v.bool()) {
        return false;
    }
    // midtone detail is spatial
    if def.name == "primaries" && e.props.get("midDetail").is_some_and(|a| a.is_animated() || a.v.num() != 0.0) {
        return false;
    }
    matches!(
        def.name,
        "lumetri" | "bc" | "exposure" | "levels" | "curves" | "hue" | "saturation" | "vibrance" | "temperature" | "tint" | "duotone" | "grayscale" | "sepia" | "invert" | "lut" | "colorwheels" | "channelmixer" | "leavecolor" | "changecolor" | "secondary" | "huecurves" | "photofilter" | "tritone" | "legalize" | "softclip" | "filmcurve" | "primaries" | "logwheels" | "hdrwheels" | "colorwarper" | "colorslice" | "cst"
    )
}

/// The effect's window expression and mix at the frame time.
pub fn window_of(e: &Effect, fcx: &FxCtx) -> (String, f32) {
    let mask = e.props.get("mask").map(|a| a.eval(fcx.ev.t, fcx.ev.x).str()).unwrap_or_default();
    let mix = e.props.get("mix").map(|a| a.eval(fcx.ev.t, fcx.ev.x).num() as f32).unwrap_or(1.0);
    (mask.trim().to_ascii_lowercase(), mix)
}

/// Length of the fusable run starting at `fx[0]`: same window, full mix.
/// (A partial mix blends each effect separately, which a single LUT can't.)
pub fn run_len(fx: &[&Effect], fcx: &FxCtx) -> usize {
    let Some(first) = fx.first() else { return 0 };
    if !fusable(first) {
        return 0;
    }
    let (w0, m0) = window_of(first, fcx);
    if m0 < 0.99999 {
        return 0;
    }
    fx.iter().take_while(|e| fusable(e) && window_of(e, fcx) == (w0.clone(), m0)).count()
}

/// The fused LUT for a run (cached unless something in it is animated).
pub fn fused(run: &[&Effect], fcx: &FxCtx) -> Arc<Lut3> {
    let mut key = String::from("fused");
    let mut animated = false;
    for e in run {
        let _ = write!(key, "|{}", e.kind);
        for (k, a) in &e.props {
            animated |= a.is_animated();
            let _ = write!(key, ";{k}={:?}", a.eval(fcx.ev.t, fcx.ev.x));
        }
        if e.kind == "lut" {
            // a .cube edited in place is a different grade
            let f = e.props.get("file").map(|a| a.v.str()).unwrap_or_default();
            let m = std::fs::metadata(&f).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            let _ = write!(key, ";mtime={m}");
        }
    }
    lut::get_or_make(&key, !animated, || build(run, fcx))
}

/// Runs the effects over the lattice and reads the result back as a LUT.
fn build(run: &[&Effect], fcx: &FxCtx) -> Lut3 {
    let mut pm = pixel::new_pixmap((N * N) as u32, N as u32);
    let q = |k: usize| ((k * 255 + (N - 1) / 2) / (N - 1)) as u8;
    for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
        px.copy_from_slice(&[q(i % N), q((i / N) % N), q(i / (N * N)), 255]);
    }
    let mut lb = LayerBuf { pm: Pm::Owned(pm), x: 0, y: 0 };
    for e in run {
        effects::apply(e, &mut lb, fcx);
    }
    let data = lb.pm.get().data().chunks_exact(4).map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]).collect();
    Lut3::from_data(N, data)
}
