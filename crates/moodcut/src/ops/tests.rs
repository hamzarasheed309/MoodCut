use super::exec::{run, Env};
use super::outline;
use crate::model::project::*;
use anyhow::{bail, Result};
use std::collections::BTreeMap;

struct Mock;

impl Env for Mock {
    fn probe(&self, path: &str) -> Result<Media> {
        let name = path.rsplit(['/', '\\']).next().unwrap().to_string();
        let (kind, dur, hv, ha) = if name.ends_with(".mp3") || name.ends_with(".wav") {
            (MediaKind::Audio, 60.0, false, true)
        } else if name.ends_with(".png") || name.ends_with(".jpg") {
            (MediaKind::Image, 0.0, true, false)
        } else if name.starts_with("missing") {
            bail!("file not found: {path}")
        } else {
            (MediaKind::Video, 30.0, true, true)
        };
        Ok(Media {
            id: String::new(),
            path: path.into(),
            name,
            kind,
            dur,
            w: 1920,
            h: 1080,
            fps: 30.0,
            has_video: hv,
            has_audio: ha,
            alpha: false,
            channels: 2,
            rate: 48000,
            codec: "h264".into(),
            size: 1,
            mtime: 1,
        })
    }
    fn analyze(&self, _p: &Project, _m: &str, kind: &str, _o: &BTreeMap<String, String>) -> Result<serde_json::Value> {
        Ok(match kind {
            "silence" => serde_json::json!({"silences": [[2.0, 4.0], [10.0, 10.3], [20.0, 25.0]]}),
            "scenes" => serde_json::json!({"cuts": [5.0, 12.0]}),
            "beats" => serde_json::json!({"bpm": 120.0, "beats": (0..120).map(|i| i as f64 * 0.5).collect::<Vec<_>>()}),
            "transcript" => serde_json::json!({"segments": [
                {"s": 0.5, "e": 3.0, "text": "Hello world this is MoodCut.", "words": [
                    {"w": "Hello", "s": 0.5, "e": 0.9}, {"w": "world", "s": 0.9, "e": 1.3}, {"w": "this", "s": 1.4, "e": 1.6},
                    {"w": "is", "s": 1.6, "e": 1.8}, {"w": "MoodCut.", "s": 1.8, "e": 2.6}]}
            ]}),
            _ => bail!("no analysis {kind}"),
        })
    }
}

fn proj() -> Project {
    Project::new("Test", 1920, 1080, 30.0)
}

fn ok(p: &mut Project, s: &str) -> Vec<String> {
    match run(p, &Mock, "s1", s) {
        Ok(o) => o.log,
        Err(e) => panic!("script failed: {e}\nscript:\n{s}"),
    }
}

fn err(p: &mut Project, s: &str) -> String {
    match run(p, &Mock, "s1", s) {
        Ok(o) => panic!("expected error, got {:?}", o.log),
        Err(e) => e.to_string(),
    }
}

#[test]
fn append_and_cut() {
    let mut p = proj();
    let log = ok(&mut p, "import /v/a.mp4 /v/b.mp4\nappend m1[0-10] m2[5-15]\ncut c1 at=4");
    assert!(log.iter().any(|l| l.starts_with("+c1")), "{log:?}");
    let c = p.main_comp().unwrap();
    let v1 = c.track("V1").unwrap();
    assert_eq!(v1.items.len(), 3);
    let right = &v1.items[1];
    assert_eq!(right.start, 4.0);
    assert_eq!(right.src_in, 4.0);
    assert_eq!(v1.items[2].start, 10.0);
    assert_eq!(v1.items[2].src_in, 5.0);
}

#[test]
fn overlap_errors_and_auto_track() {
    let mut p = proj();
    ok(&mut p, "add /v/a.mp4");
    let e = err(&mut p, "add m1 V1 at=5");
    assert!(e.contains("overlaps"), "{e}");
    ok(&mut p, "add m1 at=5 dur=2");
    let c = p.main_comp().unwrap();
    assert_eq!(c.track("V2").unwrap().items.len(), 1);
}

#[test]
fn ripple_delete_and_trim() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/a.mp4[10-15] /v/a.mp4[20-25]");
    ok(&mut p, "del c2 ripple");
    let v1 = &p.main_comp().unwrap().track("V1").unwrap().items;
    assert_eq!(v1.len(), 2);
    assert_eq!(v1[1].start, 5.0);
    ok(&mut p, "trim c1 start=+1 end=-1");
    let v1 = &p.main_comp().unwrap().track("V1").unwrap().items;
    assert_eq!(v1[0].start, 1.0);
    assert_eq!(v1[0].src_in, 1.0);
    assert_eq!(v1[0].dur, 3.0);
}

#[test]
fn text_shape_anim_fx() {
    let mut p = proj();
    let log = ok(
        &mut p,
        "add /v/a.mp4\ntext 'Hello World' at=1 dur=3 size=120 color=#ff0 enter=fadeUp exit=fadeDown as=title\nshape rect w=400 h=100 fill=red at=0 dur=2 radius=20 enter=popIn\nanim title pulse\nfx c1 blur radius=8\nkf c1.f1 radius 0:0 1:20@outCubic\ncolor c1 cinematic\nmask c1 ellipse w=0.6 h=0.6 feather=40 invert",
    );
    assert!(log.iter().any(|l| l.contains("text 'Hello World'")), "{log:?}");
    let t = p.item("title").unwrap();
    assert_eq!(t.kind, ItemKind::Text);
    assert_eq!(t.anims.len(), 2);
    assert!(t.props["scale"].x.is_some());
    let c1 = p.item("c1").unwrap();
    assert_eq!(c1.fx[0].props["radius"].k.len(), 2);
    assert!(c1.fx.iter().any(|f| f.kind == "lumetri"));
    assert!(c1.fx.iter().any(|f| f.kind == "vignette"));
    assert_eq!(c1.masks.len(), 1);
    // text is on a track above the video
    let (_, ti, _) = p.locate("title").unwrap();
    assert_eq!(p.comps[0].tracks[ti].id, "V2");
}

#[test]
fn transitions() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/a.mp4[5-10] /v/a.mp4[10-15]");
    ok(&mut p, "tr c1 c2 wipeleft 0.8; tr c3 out fadeblack 1");
    let c1 = p.item("c1").unwrap();
    let t = c1.tout.as_ref().unwrap();
    assert_eq!(t.kind, "wipe");
    assert_eq!(t.props["dir"].str(), "left");
    ok(&mut p, "tr V1 dissolve 0.4");
    assert_eq!(p.item("c2").unwrap().tout.as_ref().unwrap().kind, "dissolve");
    let e = err(&mut p, "tr c1 c3 dissolve");
    assert!(e.contains("adjacent"), "{e}");
}

#[test]
fn typo_hints() {
    let mut p = proj();
    ok(&mut p, "add /v/a.mp4");
    assert!(err(&mut p, "fx c1 gausian").contains("blur") || err(&mut p, "fx c1 gausian").contains("gaussian"));
    assert!(err(&mut p, "set c1 opactiy=0.5").contains("opacity"));
    assert!(err(&mut p, "trm c1 end=3").contains("trim"));
    assert!(err(&mut p, "set c1 blend=scren").contains("screen"));
    assert!(err(&mut p, "set c9 opacity=1").contains("unknown item"));
}

#[test]
fn transactional_failure_reports_line() {
    let mut p = proj();
    let e = err(&mut p, "add /v/a.mp4\nset c1 opacity=0.5\nbogus thing");
    assert!(e.starts_with("line 3"), "{e}");
}

#[test]
fn macros_work() {
    let mut p = proj();
    ok(&mut p, "add /v/talk.mp4");
    let log = ok(&mut p, "autocut silence c1");
    assert!(log.iter().any(|l| l.contains("removed")), "{log:?}");
    let v1 = &p.main_comp().unwrap().track("V1").unwrap().items;
    assert!(v1.len() >= 3);
    // contiguous after ripple
    for w in v1.windows(2) {
        assert!((w[0].end() - w[1].start).abs() < 1e-6);
    }
    ok(&mut p, "captions c1 words=2");
    assert!(p.main_comp().unwrap().items().filter(|i| i.kind == ItemKind::Text).count() >= 2);
    ok(&mut p, "title lowerthird text='Jane Doe' sub='CEO' at=1");
    assert!(p.comps.len() >= 2);
    ok(&mut p, "beats /a/music.mp3");
    assert!(p.main_comp().unwrap().markers.len() > 10);
    ok(&mut p, "reframe 9:16 mode=blur");
    let last = p.comps.last().unwrap();
    assert_eq!((last.w, last.h), (1080, 1920));
}

#[test]
fn montage_and_keep() {
    let mut p = proj();
    ok(&mut p, "import /v/a.mp4 /v/b.mp4 /i/c.png /a/music.mp3");
    let log = ok(&mut p, "montage m1,m2,m3 music=m4 beats=4 dur=8");
    assert!(log.iter().any(|l| l.contains("montage")), "{log:?}");
    let v1 = &p.main_comp().unwrap().track("V1").unwrap().items;
    assert_eq!(v1.len(), 4);
    let mut p2 = proj();
    ok(&mut p2, "keep /v/a.mp4 1-3,5-8");
    assert_eq!(p2.main_comp().unwrap().track("V1").unwrap().items.len(), 2);
}

#[test]
fn outline_is_compact() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/b.mp4[0-5]\ntext 'Hi' at=0 dur=2\ntr c1 c2 dissolve 0.5\nadd /a/music.mp3 vol=-12");
    let o = outline::timeline(&p, "s1", "s1", 1);
    assert!(o.contains("V1: c1 m1 0-5 >dissolve:0.5 | c2 m2 5-10"), "{o}");
    assert!(o.contains("A1: c4 m3 0-60 vol=-12"), "{o}");
    assert!(o.len() < 400, "{}", o.len());
}

#[test]
fn speed_and_freeze() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-10] /v/b.mp4[0-5]");
    ok(&mut p, "speed c1 2x ripple");
    let v1 = &p.main_comp().unwrap().track("V1").unwrap().items;
    assert_eq!(v1[0].dur, 5.0);
    assert_eq!(v1[1].start, 5.0);
    ok(&mut p, "freeze c1 at=2 dur=1");
    let v1 = &p.main_comp().unwrap().track("V1").unwrap().items;
    assert_eq!(v1.len(), 4);
    assert_eq!(v1[1].start, 2.0);
    assert!(v1[1].props["hold"].v.bool());
    assert_eq!(v1[1].src_in, 4.0);
    assert_eq!(v1[3].start, 6.0);
}

#[test]
fn parent_and_precomp() {
    let mut p = proj();
    ok(&mut p, "shape rect pos=500,500 at=0 dur=4 as=box\ntext 'Label' pos=600,500 at=0 dur=4 as=lab\nparent lab box");
    let lab = p.item("lab").unwrap();
    assert_eq!(lab.props["pos"].v.vec(), (100.0, 0.0));
    ok(&mut p, "precomp box,lab name=Group");
    assert_eq!(p.comps.len(), 2);
    assert!(p.main_comp().unwrap().items().any(|i| i.kind == ItemKind::Comp));
}
