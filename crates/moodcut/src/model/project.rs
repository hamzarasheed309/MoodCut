//! The MoodCut project document. Serialized as JSON in `.mcut` files.
//!
//! Design: Premiere-style comps (sequences) with tracks and items, where every
//! item is also an After-Effects-style layer with an animatable property bag
//! (`props`), effects, masks, shapes, text animators, transitions, parenting
//! and track mattes. Property names/defaults live in `schema.rs`.

use super::anim::Anim;
use super::value::{Color, Val};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Props = BTreeMap<String, Anim>;

pub const FORMAT_VERSION: u32 = 1;

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub v: u32,
    pub name: String,
    pub main: String,
    #[serde(default)]
    pub media: Vec<Media>,
    #[serde(default)]
    pub comps: Vec<Comp>,
    /// Item id counter (shared by all item kinds).
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub mseq: u64,
    #[serde(default)]
    pub cseq: u64,
    /// Cached analysis results keyed by "<media>:<kind>".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub analysis: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Audio,
    Image,
    Svg,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Media {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: MediaKind,
    #[serde(default)]
    pub dur: f64,
    #[serde(default)]
    pub w: u32,
    #[serde(default)]
    pub h: u32,
    #[serde(default)]
    pub fps: f64,
    #[serde(default)]
    pub has_video: bool,
    #[serde(default)]
    pub has_audio: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub alpha: bool,
    #[serde(default)]
    pub channels: u32,
    #[serde(default)]
    pub rate: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub codec: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mtime: u64,
}

impl Media {
    pub fn is_visual(&self) -> bool {
        matches!(self.kind, MediaKind::Video | MediaKind::Image | MediaKind::Svg)
    }
    pub fn is_still(&self) -> bool {
        matches!(self.kind, MediaKind::Image | MediaKind::Svg)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Marker {
    pub t: f64,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Master {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub vol: f64,
    /// Target integrated loudness (LUFS) applied with loudnorm on export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub limiter: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub afx: Vec<Effect>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comp {
    pub id: String,
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    /// Explicit duration. None = end of last item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    #[serde(default = "default_bg")]
    pub bg: Color,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub master: Master,
    /// Motion blur shutter angle in degrees (for layers with mblur on).
    #[serde(default = "default_shutter")]
    pub shutter: f64,
}

fn default_bg() -> Color {
    Color::BLACK
}
fn default_shutter() -> f64 {
    180.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Duck {
    /// Track id whose signal triggers ducking (e.g. the voice-over track).
    pub by: String,
    /// Gain reduction in dB (negative).
    pub amount: f64,
    #[serde(default = "default_attack")]
    pub attack: f64,
    #[serde(default = "default_release")]
    pub release: f64,
}
fn default_attack() -> f64 {
    20.0
}
fn default_release() -> f64 {
    400.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub solo: bool,
    /// Track gain in dB.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub vol: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pan: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duck: Option<Duck>,
    /// Audio effects applied to the whole track bus.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub afx: Vec<Effect>,
}

impl Track {
    pub fn new(id: &str, kind: TrackKind) -> Track {
        Track {
            id: id.to_string(),
            kind,
            name: None,
            items: vec![],
            hidden: false,
            locked: false,
            muted: false,
            solo: false,
            vol: 0.0,
            pan: 0.0,
            duck: None,
            afx: vec![],
        }
    }

    pub fn index(&self) -> usize {
        self.id[1..].parse().unwrap_or(0)
    }

    pub fn end(&self) -> f64 {
        self.items.iter().map(|i| i.end()).fold(0.0, f64::max)
    }

    pub fn sort(&mut self) {
        self.items.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Items overlapping [a, b) excluding `except`.
    pub fn overlapping(&self, a: f64, b: f64, except: Option<&str>) -> Vec<&Item> {
        const EPS: f64 = 1e-4;
        self.items
            .iter()
            .filter(|i| Some(i.id.as_str()) != except)
            .filter(|i| i.start < b - EPS && i.end() > a + EPS)
            .collect()
    }

    pub fn is_free(&self, a: f64, b: f64, except: Option<&str>) -> bool {
        self.overlapping(a, b, except).is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    /// Media clip (video, audio, image or svg).
    Clip,
    Text,
    Shape,
    Solid,
    /// Adjustment layer: its effects apply to everything below it.
    Adjust,
    /// Nested composition (precomp).
    Comp,
}

impl ItemKind {
    pub fn prefix(&self) -> &'static str {
        match self {
            ItemKind::Clip => "c",
            ItemKind::Text => "t",
            ItemKind::Shape => "g",
            ItemKind::Solid => "o",
            ItemKind::Adjust => "j",
            ItemKind::Comp => "n",
        }
    }
    pub fn name(&self) -> &'static str {
        match self {
            ItemKind::Clip => "clip",
            ItemKind::Text => "text",
            ItemKind::Shape => "shape",
            ItemKind::Solid => "solid",
            ItemKind::Adjust => "adjust",
            ItemKind::Comp => "comp",
        }
    }
}

/// A generic sub-object (effect, mask, shape, text animator).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Effect {
    pub id: String,
    pub kind: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub on: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: Props,
}

pub type Mask = Effect;
pub type Shape = Effect;
pub type Animator = Effect;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transition {
    pub kind: String,
    pub dur: f64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, Val>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatteMode {
    Alpha,
    AlphaInv,
    Luma,
    LumaInv,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Matte {
    pub src: String,
    pub mode: MatteMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Media id (Clip) or comp id (Comp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// Timeline start (seconds).
    pub start: f64,
    /// Timeline duration (seconds).
    pub dur: f64,
    /// Source in-point (seconds) for Clip/Comp items.
    #[serde(default, rename = "in", skip_serializing_if = "is_zero")]
    pub src_in: f64,
    /// Text content (Text items). Supports `{expr}` placeholders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub props: Props,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fx: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub afx: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<Mask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<Shape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anims: Vec<Animator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tin: Option<Transition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tout: Option<Transition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matte: Option<Matte>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub off: bool,
    /// Per-item sub-object id counter (fx/masks/shapes/anims).
    #[serde(default, skip_serializing_if = "is_zero_u")]
    pub sub: u32,
}

fn is_zero_u(v: &u32) -> bool {
    *v == 0
}

impl Item {
    pub fn new(id: String, kind: ItemKind, start: f64, dur: f64) -> Item {
        Item {
            id,
            kind,
            name: None,
            src: None,
            start,
            dur,
            src_in: 0.0,
            text: None,
            props: Props::new(),
            fx: vec![],
            afx: vec![],
            masks: vec![],
            shapes: vec![],
            anims: vec![],
            tin: None,
            tout: None,
            parent: None,
            matte: None,
            off: false,
            sub: 0,
        }
    }

    pub fn end(&self) -> f64 {
        self.start + self.dur
    }

    pub fn contains(&self, t: f64) -> bool {
        t >= self.start - 1e-9 && t < self.end() - 1e-9
    }

    pub fn label(&self) -> String {
        match &self.name {
            Some(n) => format!("{}({})", self.id, n),
            None => self.id.clone(),
        }
    }

    pub fn next_sub(&mut self, prefix: &str) -> String {
        loop {
            self.sub += 1;
            let id = format!("{}{}", prefix, self.sub);
            let used = self.fx.iter().chain(&self.afx).chain(&self.masks).chain(&self.shapes).chain(&self.anims).any(|e| e.id == id);
            if !used {
                return id;
            }
        }
    }

    pub fn speed(&self) -> f64 {
        match self.props.get("speed") {
            Some(a) => {
                let s = a.v.num();
                if s.abs() < 1e-6 {
                    1.0
                } else {
                    s.abs()
                }
            }
            None => 1.0,
        }
    }

    pub fn reversed(&self) -> bool {
        self.props.get("reverse").map(|a| a.v.bool()).unwrap_or(false)
    }

    /// All sub-object lists, mutable, with their id prefixes.
    pub fn sub_lists_mut(&mut self) -> [(&'static str, &mut Vec<Effect>); 5] {
        [("f", &mut self.fx), ("a", &mut self.afx), ("m", &mut self.masks), ("s", &mut self.shapes), ("x", &mut self.anims)]
    }
}

impl Comp {
    pub fn new(id: &str, name: &str, w: u32, h: u32, fps: f64) -> Comp {
        Comp {
            id: id.to_string(),
            name: name.to_string(),
            w,
            h,
            fps,
            dur: None,
            bg: Color::BLACK,
            tracks: vec![Track::new("V1", TrackKind::Video), Track::new("A1", TrackKind::Audio)],
            markers: vec![],
            master: Master::default(),
            shutter: 180.0,
        }
    }

    pub fn content_end(&self) -> f64 {
        self.tracks.iter().map(|t| t.end()).fold(0.0, f64::max)
    }

    pub fn duration(&self) -> f64 {
        match self.dur {
            Some(d) => d,
            None => self.content_end(),
        }
    }

    pub fn frame_dur(&self) -> f64 {
        1.0 / self.fps.max(1.0)
    }

    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id.eq_ignore_ascii_case(id))
    }

    pub fn track_mut(&mut self, id: &str) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id.eq_ignore_ascii_case(id))
    }

    pub fn vtracks(&self) -> impl Iterator<Item = &Track> {
        let mut v: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Video).collect();
        v.sort_by_key(|t| t.index());
        v.into_iter()
    }

    pub fn atracks(&self) -> impl Iterator<Item = &Track> {
        let mut v: Vec<&Track> = self.tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect();
        v.sort_by_key(|t| t.index());
        v.into_iter()
    }

    pub fn track_count(&self, kind: TrackKind) -> usize {
        self.tracks.iter().filter(|t| t.kind == kind).map(|t| t.index()).max().unwrap_or(0)
    }

    /// Ensures tracks V1..Vn (or A1..An) exist up to `idx`. Returns the track id.
    pub fn ensure_track(&mut self, kind: TrackKind, idx: usize) -> String {
        let p = if kind == TrackKind::Video { "V" } else { "A" };
        for i in 1..=idx.max(1) {
            let id = format!("{p}{i}");
            if self.track(&id).is_none() {
                self.tracks.push(Track::new(&id, kind));
            }
        }
        self.sort_tracks();
        format!("{p}{}", idx.max(1))
    }

    pub fn sort_tracks(&mut self) {
        self.tracks.sort_by_key(|t| (if t.kind == TrackKind::Video { 0 } else { 1 }, t.index()));
    }

    pub fn find_item(&self, id: &str) -> Option<(&Track, &Item)> {
        for t in &self.tracks {
            for i in &t.items {
                if i.id == id {
                    return Some((t, i));
                }
            }
        }
        None
    }

    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.tracks.iter().flat_map(|t| t.items.iter())
    }
}

impl Project {
    pub fn new(name: &str, w: u32, h: u32, fps: f64) -> Project {
        let mut p = Project {
            v: FORMAT_VERSION,
            name: name.to_string(),
            main: String::new(),
            media: vec![],
            comps: vec![],
            seq: 0,
            mseq: 0,
            cseq: 0,
            analysis: BTreeMap::new(),
        };
        let id = p.next_comp_id();
        p.comps.push(Comp::new(&id, "Main", w, h, fps));
        p.main = id;
        p
    }

    pub fn next_item_id(&mut self, kind: ItemKind) -> String {
        self.seq += 1;
        format!("{}{}", kind.prefix(), self.seq)
    }

    pub fn next_media_id(&mut self) -> String {
        self.mseq += 1;
        format!("m{}", self.mseq)
    }

    pub fn next_comp_id(&mut self) -> String {
        self.cseq += 1;
        format!("s{}", self.cseq)
    }

    pub fn comp(&self, id: &str) -> Option<&Comp> {
        self.comps.iter().find(|c| c.id == id || c.name.eq_ignore_ascii_case(id))
    }

    pub fn comp_mut(&mut self, id: &str) -> Option<&mut Comp> {
        self.comps.iter_mut().find(|c| c.id == id || c.name.eq_ignore_ascii_case(id))
    }

    pub fn main_comp(&self) -> Option<&Comp> {
        self.comp(&self.main)
    }

    pub fn media(&self, id: &str) -> Option<&Media> {
        self.media.iter().find(|m| m.id == id).or_else(|| self.media.iter().find(|m| m.name.eq_ignore_ascii_case(id)))
    }

    /// Finds an item by id or name anywhere in the project. Returns (comp index, track index, item index).
    pub fn locate(&self, key: &str) -> Option<(usize, usize, usize)> {
        let mut by_name = None;
        for (ci, c) in self.comps.iter().enumerate() {
            for (ti, t) in c.tracks.iter().enumerate() {
                for (ii, it) in t.items.iter().enumerate() {
                    if it.id == key {
                        return Some((ci, ti, ii));
                    }
                    if by_name.is_none() && it.name.as_deref().map(|n| n.eq_ignore_ascii_case(key)).unwrap_or(false) {
                        by_name = Some((ci, ti, ii));
                    }
                }
            }
        }
        by_name
    }

    pub fn item(&self, key: &str) -> Option<&Item> {
        self.locate(key).map(|(c, t, i)| &self.comps[c].tracks[t].items[i])
    }

    pub fn item_mut(&mut self, key: &str) -> Option<&mut Item> {
        self.locate(key).map(move |(c, t, i)| &mut self.comps[c].tracks[t].items[i])
    }

    /// True if `comp` (directly or transitively) nests `target`.
    pub fn comp_contains(&self, comp: &str, target: &str, depth: usize) -> bool {
        if depth > 32 {
            return true;
        }
        let Some(c) = self.comp(comp) else { return false };
        for it in c.items() {
            if it.kind == ItemKind::Comp {
                if let Some(src) = &it.src {
                    if src == target || self.comp_contains(src, target, depth + 1) {
                        return true;
                    }
                }
            }
        }
        false
    }
}
