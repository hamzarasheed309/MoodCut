//! Layer masks (rect / ellipse / polygon / star / path in normalized layer
//! coordinates) with feather, expansion, opacity, invert and boolean modes.

use super::eval::Ev;
use super::pixel::{apply_alpha_mask, blur_mask, LayerBuf};
use super::shapes;
use crate::model::project::{Effect, Mask};
use crate::model::schema;
use resvg::tiny_skia::{FillRule, Mask as SkMask, Path, Rect, Transform};

/// Layer bounds in layer px (x, y, w, h).
#[derive(Clone, Copy, Debug)]
pub struct LBounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

fn mask_path(m: &Mask, ev: &Ev, b: LBounds) -> Option<Path> {
    let p = &m.props;
    let cx = b.x + ev.f(p, "x") * b.w;
    let cy = b.y + ev.f(p, "y") * b.h;
    let ex = ev.f(p, "expand") * 2.0;
    let w = (ev.f(p, "w") * b.w + ex).max(0.01);
    let h = (ev.f(p, "h") * b.h + ex).max(0.01);
    let rot = ev.f(p, "rot");
    let path = match m.kind.as_str() {
        "rect" => shapes::rounded_rect_path(cx, cy, w, h, ev.f(p, "radius")),
        "ellipse" => shapes::ellipse_path(cx, cy, w, h),
        "polygon" => shapes::polygon_path(cx, cy, ev.num(p, "points").round() as usize, w.min(h) / 2.0),
        "star" => shapes::star_path(cx, cy, ev.num(p, "points").round() as usize, w.min(h) / 2.0, w.min(h) / 4.0),
        "path" => shapes::parse_svg_path(&ev.str(p, "d"), b.w, b.h, b.x, b.y),
        _ => None,
    }?;
    if rot.abs() > 1e-6 {
        path.transform(Transform::from_rotate_at(rot, cx, cy))
    } else {
        Some(path)
    }
}

/// Crop effect -> rect mask in normalized coordinates.
fn crop_mask(e: &Effect, ev: &Ev, b: LBounds) -> Option<(Path, f32)> {
    let p = &e.props;
    let (l, t, r, bt) = (ev.f(p, "l"), ev.f(p, "t"), ev.f(p, "r"), ev.f(p, "b"));
    if l <= 0.0 && t <= 0.0 && r <= 0.0 && bt <= 0.0 {
        return None;
    }
    let rect = Rect::from_ltrb(b.x + l * b.w, b.y + t * b.h, b.x + (1.0 - r) * b.w, b.y + (1.0 - bt) * b.h)?;
    Some((resvg::tiny_skia::PathBuilder::from_rect(rect), ev.f(p, "soft")))
}

/// Applies masks (and crop effects) to `lb`. `m` maps layer px -> output px.
pub fn apply_masks(lb: &mut LayerBuf, masks: &[Mask], fx: &[Effect], ev: &Ev, b: LBounds, m: Transform, scale: f32) {
    let crops: Vec<&Effect> = fx.iter().filter(|e| e.on && e.kind == "crop").collect();
    let active: Vec<&Mask> = masks.iter().filter(|x| x.on).collect();
    if active.is_empty() && crops.is_empty() {
        return;
    }
    let (w, h) = (lb.w(), lb.h());
    let to_buf = Transform::from_translate(-lb.x as f32, -lb.y as f32).pre_concat(m);
    let n = (w * h) as usize;
    let mev = ev.with_lists(&[schema::MASK]);
    let mut acc: Option<Vec<f32>> = None;
    for (i, mk) in active.iter().enumerate() {
        let mode = mev.str(&mk.props, "mode");
        let Some(path) = mask_path(mk, &mev, b) else { continue };
        let Some(mut sm) = SkMask::new(w, h) else { continue };
        sm.fill_path(&path, FillRule::Winding, true, to_buf);
        let mut data = sm.take();
        let feather = mev.f(&mk.props, "feather") * scale;
        if feather > 0.3 {
            blur_mask(&mut data, w as usize, h as usize, feather / 2.0);
        }
        let op = mev.f(&mk.props, "opacity").clamp(0.0, 1.0);
        let inv = mev.bool(&mk.props, "invert");
        let mv: Vec<f32> = data.iter().map(|&v| {
            let x = v as f32 / 255.0 * op;
            if inv {
                1.0 - x
            } else {
                x
            }
        }).collect();
        let a = acc.get_or_insert_with(|| if i == 0 && (mode == "subtract") { vec![1.0; n] } else { vec![0.0; n] });
        for (dst, s) in a.iter_mut().zip(mv.iter()) {
            *dst = match mode.as_str() {
                "subtract" => *dst * (1.0 - s),
                "intersect" => {
                    if i == 0 {
                        *s
                    } else {
                        dst.min(*s)
                    }
                }
                "difference" => (*dst - s).abs(),
                _ => dst.max(*s),
            };
        }
    }
    let cev = ev.with_lists(&[schema::effect("crop").map(|e| e.params).unwrap_or(&[])]);
    for c in crops {
        let Some((path, soft)) = crop_mask(c, &cev, b) else { continue };
        let Some(mut sm) = SkMask::new(w, h) else { continue };
        sm.fill_path(&path, FillRule::Winding, true, to_buf);
        let mut data = sm.take();
        if soft * scale > 0.3 {
            blur_mask(&mut data, w as usize, h as usize, soft * scale / 2.0);
        }
        let a = acc.get_or_insert_with(|| vec![1.0; n]);
        for (dst, &s) in a.iter_mut().zip(data.iter()) {
            *dst *= s as f32 / 255.0;
        }
    }
    if let Some(a) = acc {
        let bytes: Vec<u8> = a.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        apply_alpha_mask(lb.pm.make_mut(), &bytes);
    }
}
