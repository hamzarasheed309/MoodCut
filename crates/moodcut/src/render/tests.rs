//! Renders real frames (graphics only, no FFmpeg needed) through every effect,
//! transition and blend mode, and checks the camera and 3D layer ordering.

use super::comp::{layer_geom, render_frame, RenderCtx};
use super::decode::FrameServer;
use super::ffmpeg::Ff;
use crate::model::project::*;
use crate::model::schema::{self, Engine};
use crate::ops::{self, Env};
use anyhow::{bail, Result};
use resvg::tiny_skia::Pixmap;
use std::collections::BTreeMap;
use std::sync::Arc;

struct NoMedia;

impl Env for NoMedia {
    fn probe(&self, path: &str) -> Result<Media> {
        bail!("no media in render tests ({path})")
    }
    fn analyze(&self, _: &Project, _: &str, _: &str, _: &BTreeMap<String, String>) -> Result<serde_json::Value> {
        bail!("no analysis in render tests")
    }
}

fn project(script: &str) -> Project {
    let mut p = Project::new("t", 640, 360, 30.0);
    if let Err(e) = ops::run(&mut p, &NoMedia, "s1", script) {
        panic!("{e}\n{script}");
    }
    p
}

fn frame(p: &Project, t: f64) -> Pixmap {
    let frames = FrameServer::new(Arc::new(Ff::missing()), false);
    let rc = RenderCtx { p, frames: &frames, draft: true };
    render_frame(&rc, &p.main, t, 0.5).unwrap()
}

fn px(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * pm.width() + x) * 4) as usize;
    let d = pm.data();
    [d[i], d[i + 1], d[i + 2], d[i + 3]]
}

fn assert_premul(pm: &Pixmap, what: &str) {
    for (i, p) in pm.data().chunks_exact(4).enumerate() {
        assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{what}: invalid premultiplied pixel {p:?} at {i}");
    }
}

fn opaque_px(pm: &Pixmap) -> usize {
    pm.data().chunks_exact(4).filter(|p| p[3] > 8).count()
}

const BASE: &str = "comp set bg=none\nsolid #3366cc w=300 h=180 pos=320,180 at=0 dur=2 as=box\ntext 'MoodCut' size=80 color=#ffcc00 at=0 dur=2 as=word";

#[test]
fn every_effect_renders_valid_pixels() {
    for e in schema::EFFECTS.iter().filter(|e| e.engine == Engine::Native && e.name != "lut") {
        for target in ["box", "word"] {
            let extra = if e.params.iter().any(|p| p.name == "completion") { " completion=50" } else { "" };
            let p = project(&format!("{BASE}\nfx {target} {}{extra}\nkf {target} rot 0:0 2:20", e.name));
            for t in [0.1, 1.0] {
                let pm = frame(&p, t);
                assert_premul(&pm, &format!("{} on {target}", e.name));
            }
        }
    }
}

#[test]
fn effect_behaviour() {
    let visible = |s: &str| opaque_px(&frame(&project(&format!("{BASE}\n{s}")), 1.0));
    let base = visible("");
    assert!(base > 1000);
    // effect-based wipes: 0% shows the layer, 100% hides it
    for w in ["linearwipe", "radialwipe", "iriswipe", "blockdissolve", "venetianblinds", "gradientwipe"] {
        assert_eq!(visible(&format!("fx box,word {w} completion=0")), base, "{w} at 0%");
        assert_eq!(visible(&format!("fx box,word {w} completion=100")), 0, "{w} at 100%");
    }
    // a glow or a choke-spread grows the alpha, a choke shrinks it
    assert!(visible("fx word outerglow size=20") > base);
    assert!(visible("fx box choke amount=10") < base);
    assert!(visible("fx box choke amount=-10") > base);
    // the transform effect moves content without losing it
    let one = |s: &str| opaque_px(&frame(&project(&format!("comp set bg=none\nsolid #3366cc w=300 h=180 pos=320,180 at=0 dur=2 as=box\n{s}")), 1.0));
    assert_eq!(one("fx box transform offset=40,0"), one(""));
    assert!(one("fx box transform scale=50%") < one("") / 3);
    // channel mixer mono gives gray
    let pm = frame(&project(&format!("{BASE}\nfx box channelmixer mono=1")), 1.0);
    let c = px(&pm, 100, 50);
    assert!(c[0] == c[1] && c[1] == c[2], "{c:?}");
    // the video limiter keeps levels inside 16-235
    let pm = frame(&project("solid #ffffff at=0 dur=2\nfx $ legalize"), 1.0);
    assert!(px(&pm, 10, 10)[0] <= 236);
    // posterize time holds frames: an animated layer looks the same within one 4 fps step
    let p = project(&format!("{BASE}\nkf box pos 0:100,180 2:540,180\nfx box posterizetime fps=4"));
    assert_eq!(frame(&p, 0.26).data(), frame(&p, 0.45).data());
    assert_ne!(frame(&p, 0.45).data(), frame(&p, 0.55).data());
    // echo leaves trails of earlier frames
    let p = project("comp set bg=none\nsolid #fff w=40 h=40 at=0 dur=2 as=dot\nkf dot pos 0:50,180 2:590,180@linear\nfx dot echo time=0.1 count=4");
    let q = project("comp set bg=none\nsolid #fff w=40 h=40 at=0 dur=2 as=dot\nkf dot pos 0:50,180 2:590,180@linear");
    assert!(opaque_px(&frame(&p, 1.0)) > opaque_px(&frame(&q, 1.0)) * 3);
}

#[test]
fn every_transition_renders() {
    for tr in schema::TRANSITIONS {
        let p = project(&format!("solid #d02040 at=0 dur=2\nsolid #20a0d0 V1 at=2 dur=2\ntr o1 o2 {} 1", tr.name));
        for t in [1.6, 2.0, 2.4] {
            assert_premul(&frame(&p, t), tr.name);
        }
        // the cut is fully A just before the transition starts and fully B after it ends
        let a = px(&frame(&p, 1.4), 160, 90);
        let b = px(&frame(&p, 2.6), 160, 90);
        assert!(a[0] > a[2] && b[2] > b[0], "{}: {a:?} {b:?}", tr.name);
    }
}

#[test]
fn every_blend_mode_renders() {
    for mode in schema::BLEND_MODES {
        let p = project(&format!("solid #808080 at=0 dur=1\nsolid #4080ff w=200 h=200 at=0 dur=1 blend={mode}"));
        assert_premul(&frame(&p, 0.5), mode);
    }
    let center = |mode: &str| px(&frame(&project(&format!("solid #c08040 at=0 dur=1\nsolid #404040 w=200 h=200 at=0 dur=1 blend={mode}")), 0.5), 160, 90);
    assert_eq!(&center("subtract")[..3], &[128, 64, 0]);
    assert_eq!(&center("lineardodge")[..3], &[255, 192, 128]);
    // stencil alpha keeps the layers below only inside the top layer
    let pm = frame(&project("comp set bg=none\nsolid #c08040 at=0 dur=1\nsolid #fff w=100 h=100 at=0 dur=1 blend=stencilalpha"), 0.5);
    assert_eq!(px(&pm, 160, 90)[3], 255);
    assert_eq!(px(&pm, 10, 10)[3], 0);
    let pm = frame(&project("comp set bg=none\nsolid #c08040 at=0 dur=1\nsolid #fff w=100 h=100 at=0 dur=1 blend=silhouettealpha"), 0.5);
    assert_eq!((px(&pm, 160, 90)[3], px(&pm, 10, 10)[3]), (0, 255));
}

#[test]
fn camera_views_3d_layers() {
    // a default camera is the same as no camera
    let plain = project("comp set bg=none\nsolid #fff w=200 h=100 z=300 ry=20 at=0 dur=2");
    let cam = project("comp set bg=none\nsolid #fff w=200 h=100 z=300 ry=20 at=0 dur=2\ncamera");
    assert_eq!(frame(&plain, 1.0).data(), frame(&cam, 1.0).data());
    // 3d=1 layers only move with a camera; a push-in makes them bigger
    let p = project("comp set bg=none\nsolid #fff w=200 h=100 at=0 dur=2 as=card 3d=1\ncamera move=pushin amount=300 ease=linear");
    let c = p.main_comp().unwrap();
    let card = p.item("card").unwrap();
    let ws = [0.0, 1.99].map(|t| {
        let b = layer_geom(&p, c, card, t).unwrap().bbox();
        b.2 - b.0
    });
    let (w0, w1) = (ws[0], ws[1]);
    assert!((w0 - 200.0).abs() < 1.0 && w1 > w0 * 1.15, "{w0} {w1}");
    // a layer behind the camera isn't drawn
    let p = project("comp set bg=none\nsolid #fff at=0 dur=2 z=-1000\ncamera z=0");
    assert_eq!(opaque_px(&frame(&p, 1.0)), 0);
}

#[test]
fn layers_in_3d_sort_by_depth() {
    // the near red card is on the lower track but must cover the far blue one
    let p = project("solid #ff0000 w=300 h=200 z=-200 at=0 dur=1\nsolid #0000ff w=300 h=200 z=300 V2 at=0 dur=1");
    let c = px(&frame(&p, 0.5), 160, 90);
    assert!(c[0] > 200 && c[2] < 40, "{c:?}");
    // 2D layers keep the track order
    let p = project("solid #ff0000 w=300 h=200 at=0 dur=1\nsolid #0000ff w=300 h=200 V2 at=0 dur=1");
    assert!(px(&frame(&p, 0.5), 160, 90)[2] > 200);
}

#[test]
fn layer_geometry_matches_the_render() {
    let p = project("solid #fff w=200 h=100 pos=500,250 at=0 dur=1 as=a\nnull pos=100,0 as=n\nparent a n");
    let c = p.main_comp().unwrap();
    let b = layer_geom(&p, c, p.item("a").unwrap(), 0.5).unwrap().bbox();
    assert_eq!(b, (400.0, 200.0, 600.0, 300.0));
    assert!(layer_geom(&p, c, p.item("n").unwrap(), 0.5).is_none(), "nulls have no picture");
}

#[test]
fn cube_spin_faces_fill_the_frame() {
    for (dir, a_first) in [("left", true), ("right", false), ("up", true), ("down", false)] {
        let p = project(&format!("solid #c03050 at=0 dur=2\nsolid #2070c0 V1 at=2 dur=2\ntr o1 o2 cube 1 dir={dir}"));
        let pm = frame(&p, 2.0);
        let vert = matches!(dir, "up" | "down");
        let (w, h) = (pm.width(), pm.height());
        let at = |f: f32| if vert { px(&pm, w / 2, (h as f32 * f) as u32) } else { px(&pm, (w as f32 * f) as u32, h / 2) };
        let (near, far) = (at(0.2), at(0.8));
        let red = |c: [u8; 4]| c[0] > 150 && c[2] < 120;
        let blue = |c: [u8; 4]| c[2] > 150 && c[0] < 100;
        if a_first {
            assert!(red(near) && blue(far), "{dir}: {near:?} {far:?}");
        } else {
            assert!(blue(near) && red(far), "{dir}: {near:?} {far:?}");
        }
    }
}
