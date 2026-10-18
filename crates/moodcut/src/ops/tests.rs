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
            "loudness" => serde_json::json!({"lufs": -18.0}),
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

// ---------------------------------------------------------------- Premiere / AE tools

/// The mock environment plus an open editor window (selection, playhead).
struct WithUi(super::exec::Ui);

impl Env for WithUi {
    fn probe(&self, path: &str) -> Result<Media> {
        Mock.probe(path)
    }
    fn analyze(&self, p: &Project, m: &str, kind: &str, o: &BTreeMap<String, String>) -> Result<serde_json::Value> {
        Mock.analyze(p, m, kind, o)
    }
    fn ui(&self) -> super::exec::Ui {
        self.0.clone()
    }
}

fn v1(p: &Project) -> Vec<(String, f64, f64, f64)> {
    p.main_comp().unwrap().track("V1").unwrap().items.iter().map(|i| (i.id.clone(), i.start, i.dur, i.src_in)).collect()
}

#[test]
fn rolling_edit_moves_the_cut_between_two_clips() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/b.mp4[10-15]");
    ok(&mut p, "roll c1 +1");
    assert_eq!(v1(&p), vec![("c1".into(), 0.0, 6.0, 0.0), ("c2".into(), 6.0, 4.0, 11.0)]);
    ok(&mut p, "roll c1 to=3");
    assert_eq!(v1(&p), vec![("c1".into(), 0.0, 3.0, 0.0), ("c2".into(), 3.0, 7.0, 8.0)]);
    assert!(err(&mut p, "roll c1 -2.99").contains("less than a frame"));
    assert!(err(&mut p, "roll c2 +1").contains("no clip right after"));
    // the incoming clip needs media before its in-point to roll earlier
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/b.mp4[0-5]");
    assert!(err(&mut p, "roll c1 -1").contains("handle"));
}

#[test]
fn lift_leaves_a_gap_and_extract_closes_it() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-10]\nadd /a/music.mp3 A1 at=0 dur=10");
    ok(&mut p, "lift 2 4 tracks=V1");
    let v = v1(&p);
    assert_eq!(v.len(), 2);
    assert_eq!((v[0].1, v[0].2), (0.0, 2.0));
    assert_eq!((v[1].1, v[1].2, v[1].3), (4.0, 6.0, 4.0));
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-10]\nadd /a/music.mp3 A1 at=0 dur=10\nworkarea 2 4\nextract");
    let v = v1(&p);
    assert_eq!((v[1].1, v[1].2, v[1].3), (2.0, 6.0, 4.0));
    let a1 = &p.main_comp().unwrap().track("A1").unwrap().items;
    assert_eq!(a1.iter().map(|i| i.dur).sum::<f64>(), 8.0);
    assert!(err(&mut proj(), "lift").contains("needs a range"));
}

#[test]
fn swap_rearranges_adjacent_clips_and_trades_others() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-2] /v/b.mp4[0-5] /v/c.mp4[0-3]");
    ok(&mut p, "tr c1 c2 dissolve 0.5");
    ok(&mut p, "swap c1 c2");
    let v = v1(&p);
    assert_eq!(v.iter().map(|x| (x.0.as_str(), x.1)).collect::<Vec<_>>(), vec![("c2", 0.0), ("c1", 5.0), ("c3", 7.0)]);
    assert_eq!(p.item("c2").unwrap().tout.as_ref().unwrap().kind, "dissolve");
    let mut p = proj();
    ok(&mut p, "add /v/a.mp4 dur=2\nadd /v/b.mp4 V2 at=4 dur=2");
    ok(&mut p, "swap c1 c2");
    let (_, t1, _) = p.locate("c1").unwrap();
    assert_eq!((p.comps[0].tracks[t1].id.as_str(), p.item("c1").unwrap().start), ("V2", 4.0));
}

#[test]
fn moving_a_parent_keeps_its_children() {
    let mut p = proj();
    ok(&mut p, "null at=0 dur=5 as=ctrl\ntext 'Hi' at=0 dur=5 as=hi\nparent hi ctrl\nmove ctrl at=1\nset ctrl start=2");
    assert_eq!(p.item("hi").unwrap().parent.as_deref(), Some(p.item("ctrl").unwrap().id.as_str()));
    assert_eq!(p.item("ctrl").unwrap().kind, ItemKind::Null);
}

#[test]
fn paste_attributes_copies_looks_and_motion() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-2] /v/b.mp4[0-2] /v/c.mp4[0-2]");
    ok(&mut p, "color c1 cinematic\nset c1 scale=1.2 opacity=80%\nfx c1 blur radius=4\nfx c2 blur radius=9");
    let log = ok(&mut p, "paste c1 c2,c3");
    assert!(log.iter().any(|l| l.starts_with("~c2 pasted transform,opacity,fx")), "{log:?}");
    let c2 = p.item("c2").unwrap();
    assert_eq!(c2.props["scale"].v.vec(), (1.2, 1.2));
    assert_eq!(c2.fx.iter().filter(|e| e.kind == "blur").count(), 1, "same-kind effects are replaced");
    assert!(c2.fx.iter().any(|e| e.kind == "lumetri"));
    ok(&mut p, "paste c1 c3 color stack");
    assert_eq!(p.item("c3").unwrap().fx.iter().filter(|e| e.kind == "lumetri").count(), 2);
    assert!(err(&mut p, "paste c1 c2 wobble").contains("unknown attribute"));
}

#[test]
fn stagger_and_sequence_layers() {
    let mut p = proj();
    ok(&mut p, "text 'A' at=0 dur=2 V2\ntext 'B' at=0 dur=2 V3\ntext 'C' at=0 dur=2 V4");
    ok(&mut p, "stagger t1,t2,t3 by=0.25");
    let starts: Vec<f64> = ["t1", "t2", "t3"].iter().map(|i| p.item(i).unwrap().start).collect();
    assert_eq!(starts, vec![0.0, 0.25, 0.5]);
    ok(&mut p, "stagger t1..t3 seq overlap=0.5 from=1");
    let starts: Vec<f64> = ["t1", "t2", "t3"].iter().map(|i| p.item(i).unwrap().start).collect();
    assert_eq!(starts, vec![1.0, 2.5, 4.0]);
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-2] /v/b.mp4[0-2]");
    assert!(err(&mut p, "stagger c1,c2 by=1").contains("separate tracks"));
}

#[test]
fn align_and_distribute_use_the_rendered_box() {
    let mut p = proj();
    ok(&mut p, "solid #f00 w=200 h=100 pos=500,500 at=0 dur=3 as=a\nsolid #0f0 w=200 h=100 pos=900,300 at=0 dur=3 as=b\nsolid #00f w=200 h=100 pos=1500,700 at=0 dur=3 as=c");
    ok(&mut p, "align a left top");
    assert_eq!(p.item("a").unwrap().props["pos"].v.vec(), (100.0, 50.0));
    ok(&mut p, "align b right margin=40 to=comp");
    assert_eq!(p.item("b").unwrap().props["pos"].v.vec().0, 1920.0 - 40.0 - 100.0);
    ok(&mut p, "align a,b,c vcenter to=sel");
    ok(&mut p, "distribute a,b,c h");
    // sorted left to right a (100), c (1500), b (1780): c moves halfway
    let xs: Vec<f64> = ["a", "b", "c"].iter().map(|i| p.item(i).unwrap().props["pos"].v.vec().0).collect();
    assert_eq!(xs, vec![100.0, 1780.0, 940.0]);
    let ys: Vec<f64> = ["a", "b", "c"].iter().map(|i| p.item(i).unwrap().props["pos"].v.vec().1).collect();
    assert!(ys.iter().all(|y| (y - ys[0]).abs() < 1e-6), "{ys:?}");
    // keyframed positions move as a whole (measured at the item start)
    ok(&mut p, "kf c pos 0:100,500 1:300,500\nalign c right");
    let k = &p.item("c").unwrap().props["pos"].k;
    assert_eq!((k[0].v.vec().0, k[1].v.vec().0), (1820.0, 2020.0));
}

#[test]
fn keyframe_assistant() {
    let mut p = proj();
    ok(&mut p, "add /v/a.mp4 dur=4\nkf c1 opacity 0:0 1:1@linear 3:0.5\nkf c1 scale 0:1 2:1.5");
    ok(&mut p, "ease c1 opacity easy");
    let o = &p.item("c1").unwrap().props["opacity"].k;
    assert_eq!(o[0].e.name(), "ease");
    assert_eq!(o[1].e.name(), "ease");
    ok(&mut p, "ease c1 all outBack at=1");
    assert_eq!(p.item("c1").unwrap().props["opacity"].k[1].e.name(), "outBack");
    ok(&mut p, "kf c1 opacity reverse");
    let o = &p.item("c1").unwrap().props["opacity"].k;
    assert_eq!(o.iter().map(|k| (k.t, k.v.num())).collect::<Vec<_>>(), vec![(0.0, 0.5), (2.0, 1.0), (3.0, 0.0)]);
    ok(&mut p, "kf c1 all shift=+0.5");
    assert_eq!(p.item("c1").unwrap().props["scale"].k[0].t, 0.5);
    ok(&mut p, "kf c1 scale stretch=2");
    assert_eq!(p.item("c1").unwrap().props["scale"].k[1].t, 4.5);
    ok(&mut p, "expr c1 rot 'time*90'\nkf c1 rot bake step=0.5");
    let r = &p.item("c1").unwrap().props["rot"];
    assert!(r.x.is_none() && r.k.len() == 9 && (r.k[2].v.num() - 90.0).abs() < 1e-6, "{:?}", r.k);
    ok(&mut p, "kf c1 all clear");
    assert!(!p.item("c1").unwrap().props.values().any(|a| !a.k.is_empty()));
}

#[test]
fn null_and_camera_layers() {
    let mut p = proj();
    let log = ok(&mut p, "text 'A' at=0 dur=5 as=a\nnull children=a as=rig\ncamera move=pushin layers=a zoom=1800");
    assert!(log.iter().any(|l| l.contains(" null V")), "{log:?}");
    let rig = p.item("rig").unwrap().id.clone();
    assert!(rig.starts_with('l'));
    assert_eq!(p.item("a").unwrap().parent.as_deref(), Some(rig.as_str()));
    assert!(p.item("a").unwrap().props["3d"].v.bool());
    let cam = p.main_comp().unwrap().items().find(|i| i.kind == ItemKind::Camera).unwrap();
    assert!(cam.id.starts_with('k'));
    assert_eq!(cam.props["zoom"].v.num(), 1800.0);
    assert_eq!(cam.props["z"].k.len(), 2);
    ok(&mut p, "camera move=orbit amount=40");
    assert!(err(&mut p, "camera move=wobble").contains("unknown camera move"));
    assert!(err(&mut p, "set rig zoom=3").contains("unknown property"));
}

#[test]
fn work_area_markers_and_ripple_trim() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/b.mp4[0-5]\nworkarea 1 3\nmarker add 2 'Hook' color=#f00 dur=1 note='big moment'");
    assert_eq!(p.main_comp().unwrap().work, Some((1.0, 3.0)));
    ok(&mut p, "marker set Hook name=Drop t=2.5\ncomp set work=none");
    let m = &p.main_comp().unwrap().markers[0];
    assert_eq!((m.name.as_str(), m.t, m.dur, m.note.as_str()), ("Drop", 2.5, 1.0, "big moment"));
    assert!(p.main_comp().unwrap().work.is_none());
    // Premiere ripple trim: the head goes, the clip stays put, the rest follows
    ok(&mut p, "trim c1 start=+1 ripple");
    assert_eq!(v1(&p), vec![("c1".into(), 0.0, 4.0, 1.0), ("c2".into(), 4.0, 5.0, 0.0)]);
}

#[test]
fn selectors_selection_and_playhead() {
    let mut p = proj();
    ok(&mut p, "append /v/a.mp4[0-5] /v/b.mp4[0-5] /v/c.mp4[0-5]\ntext 'Hi' at=6 dur=2");
    let log = ok(&mut p, "fx after:5 blur");
    assert!(log.iter().any(|l| l.starts_with("+c2.f1")) && log.iter().any(|l| l.starts_with("+t4.f1")), "{log:?}");
    let log = ok(&mut p, "set V1:at:7 opacity=50%");
    assert_eq!(log, vec!["~c2 opacity=50%"]);
    let log = ok(&mut p, "fx kind:text glow");
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(err(&mut p, "fx kind:blob blur").contains("unknown kind"));
    let ui = super::exec::Ui { selection: vec!["c3".into(), "c1".into()], playhead: Some(6.5), comp: None };
    let o = run(&mut p, &WithUi(ui), "s1", "color sel bw\ntext 'Here' at=playhead dur=1 as=here").unwrap();
    assert!(o.log.iter().any(|l| l.contains("c3")), "{:?}", o.log);
    assert_eq!(p.item("here").unwrap().start, 6.5);
    assert!(err(&mut p, "fx sel blur").contains("nothing is selected"));
    assert!(err(&mut p, "text 'x' at=playhead").contains("editor"));
}

#[test]
fn essential_sound_types() {
    let mut p = proj();
    ok(&mut p, "add /v/talk.mp4\nadd /a/music.mp3 A2 at=0");
    let log = ok(&mut p, "sound c1 dialogue\nsound c2 music");
    assert!(log.iter().any(|l| l.contains("vol=-2dB")), "{log:?}");
    let c1 = p.item("c1").unwrap();
    assert_eq!(c1.afx.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["voice", "deess"]);
    let a2 = p.main_comp().unwrap().track("A2").unwrap();
    assert_eq!(a2.duck.as_ref().unwrap().by, "V1");
    assert_eq!(p.item("c2").unwrap().props["vol"].v.num(), -10.0);
    ok(&mut p, "sound c1 dialogue amount=0.5");
    assert_eq!(p.item("c1").unwrap().afx.len(), 2, "re-running replaces the chain");
    ok(&mut p, "sound c1 clear");
    assert!(p.item("c1").unwrap().afx.is_empty() && !p.item("c1").unwrap().props.contains_key("vol"));
}
