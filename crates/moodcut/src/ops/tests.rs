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
            gfx: None,
            tags: vec![],
            note: String::new(),
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

#[test]
fn overlay_templates_sit_on_top_of_the_current_comp() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/b.mp4[0-5]\ntext 'Hi' at=0 dur=5");
    let log = ok(&mut p, "title lightleak at=2 dur=3 as=leak\nset $ opacity=50%\ntitle vignette");
    assert!(log.iter().any(|l| l.contains("lightleak V3 2-5")), "{log:?}");
    assert_eq!(p.comps.len(), 1, "overlays must not create precomps");
    let c = p.main_comp().unwrap();
    let leak = &c.track("V3").unwrap().items[0];
    assert_eq!((leak.kind, leak.name.as_deref()), (ItemKind::Solid, Some("leak")));
    assert!(leak.props.contains_key("opacity"), "$ should refer to the overlay layer");
    let vig = &c.track("V4").unwrap().items[0];
    assert_eq!((vig.kind, vig.start, vig.dur), (ItemKind::Adjust, 0.0, 10.0));
}

#[test]
fn timed_anim_on_a_track_hits_the_clip_under_the_time() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/a.mp4[5-10] /v/a.mp4[10-15]");
    let log = ok(&mut p, "anim V1 punch at=7");
    assert_eq!(log.iter().filter(|l| l.contains("anim punch")).count(), 1, "{log:?}");
    assert!(log.iter().any(|l| l.starts_with("~c2 anim")), "{log:?}");
    let e = err(&mut p, "anim V1 punch at=20");
    assert!(e.contains("nothing in V1"), "{e}");
}

#[test]
fn quote_round_trips_through_the_lexer() {
    for s in [r"C:\Users\me\new\talk.mp4", r"\\nas\clips\a.mov", r"clips\take1.mov", "it's \"quoted\"", "two\nlines", r"back\slash text", r"\note"] {
        let script = format!("text {}", super::lex::quote(s));
        let c = super::lex::parse_script(&script).unwrap();
        assert_eq!(c[0].pos(), vec![s], "{script}");
    }
}

#[test]
fn repeated_log_lines_are_compacted_and_ranges_resolve() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-2] /v/a.mp4[2-4] /v/a.mp4[4-6] /v/a.mp4[6-8] /v/a.mp4[8-10]");
    let log = ok(&mut p, "color V1 bw");
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].starts_with("~c1..c5.f1 "), "{log:?}");
    let log = ok(&mut p, "fx c2..c4 blur");
    assert!(log[0].starts_with("+c2..c4.f2 blur"), "{log:?}");
    assert!(err(&mut p, "fx c7..c9 blur").contains("no items in range"));
    // interleaved lines (~cN.f1 / +cN.f2 per clip) still group
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-2] /v/a.mp4[2-4] /v/a.mp4[4-6] /v/a.mp4[6-8] /v/a.mp4[8-10]");
    let log = ok(&mut p, "color V1 cinematic");
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[1].starts_with("+c1..c5.f"), "{log:?}");
}

#[test]
fn abbreviations_get_suggestions() {
    let mut p = proj();
    let e = err(&mut p, "text 'Ship it' at=0 dur=2 enter=type");
    assert!(e.contains("'typewriter'"), "{e}");
    let mut p = proj();
    ok(&mut p, "add /v/a.mp4");
    let e = err(&mut p, "anim c1 punchin at=1");
    assert!(e.contains("'zoomPunchIn'"), "{e}");
}

#[test]
fn media_tags_notes_selectors_and_unimport() {
    let mut p = proj();
    ok(&mut p, "import /v/a.mp4 /v/b.mp4 /v/c.mp4 /v/song.mp3 /v/logo.png tag=day1");
    assert!(p.media.iter().all(|m| m.tags == vec!["day1".to_string()]));
    let log = ok(&mut p, "tag m1..m3 broll\nnote m2 'drone over the beach'\nuntag m3 day1");
    assert!(log[0].contains("tagged broll"), "{log:?}");
    use crate::library::select_media;
    assert_eq!(select_media(&p, "tag:broll").unwrap(), vec!["m1", "m2", "m3"]);
    assert_eq!(select_media(&p, "kind:audio").unwrap(), vec!["m4"]);
    assert_eq!(select_media(&p, "q:beach").unwrap(), vec!["m2"]);
    assert_eq!(select_media(&p, "m2..m4,m1").unwrap(), vec!["m2", "m3", "m4", "m1"]);
    assert!(select_media(&p, "m9").unwrap_err().to_string().contains("unknown media"));
    assert_eq!(p.media(&"m3".to_string()).unwrap().tags, vec!["broll".to_string()]);
    ok(&mut p, "add m1");
    let e = err(&mut p, "unimport m1");
    assert!(e.contains("add `force`"), "{e}");
    ok(&mut p, "unimport m1 force");
    assert!(p.media("m1").is_none());
    assert!(p.main_comp().unwrap().items().next().is_none());
    ok(&mut p, "relink m2 /v/other.mp4");
    let m2 = p.media("m2").unwrap();
    assert_eq!((m2.path.as_str(), m2.note.as_str()), ("/v/other.mp4", "drone over the beach"));
}

#[test]
fn library_formatting() {
    use crate::library::fmt_dur;
    assert_eq!(fmt_dur(12.34), "12.3s");
    assert_eq!(fmt_dur(222.0), "3:42");
    assert_eq!(fmt_dur(3723.0), "1:02:03");
}
