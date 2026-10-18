//! `help` tool content. Static guides plus catalogs generated from the schema,
//! so documentation can never drift from what the engine accepts.

use crate::model::schema::{self, describe, Engine};
use crate::ops::presets;
use crate::ops::templates::TEMPLATES;
use crate::render::encode::PRESETS;

pub const TOPICS: &[(&str, &str)] = &[
    ("edit", "every MoodCut Script command"),
    ("time", "time & reference syntax"),
    ("props", "layer/clip/text/solid properties"),
    ("effects", "video effects + params"),
    ("audio", "audio effects, ducking, loudness"),
    ("transitions", "transition types"),
    ("anim", "animation presets, easing, keyframes"),
    ("text", "titles, text animators, captions"),
    ("shapes", "shape layers & masks"),
    ("templates", "motion graphics templates"),
    ("macros", "autocut, captions, montage, reframe..."),
    ("color", "color grading"),
    ("expr", "expressions"),
    ("render", "export presets"),
    ("analyze", "analysis kinds"),
    ("examples", "complete example scripts"),
    ("fonts", "installed fonts"),
];

const OVERVIEW: &str = "MoodCut: AI-native video editor. Loop: media import -> view -> edit (batch many lines) -> preview (contact sheet) -> render.
Ids: m1 media, c2 clips, t3 text, g4 shapes, o5 solids, j6 adjustment, n7 nested comps, s1 comps; sub-objects c2.f1 (effect) c2.m1 (mask) g4.s1 (shape) t3.x1 (text animator).
Tracks: V1 is the bottom video track (higher = on top), A1.. audio. Times are seconds.
Scripts are atomic: if any line fails nothing changes and the error names the line.";

const EDIT: &str = "MoodCut Script — `verb args key=value`, one per line (or `;`). Quote text with '...'. Flags are bare words (ripple).
PLACE
 add <media|path|comp>[in-out] [V2|A1] [at=T|after=id] [in=] [out=] [dur=] [speed=2] [as=name] [mode=overwrite|insert] [props..]
   no at= -> appends to end of V1/A1. at= -> lowest free track. mode=insert ripples.
 append m1[0-5] m2[10-15] m3     sequential clips (magnetic)
 insert m4 at=10                 ripple insert
 text 'Title' [at=0 dur=3] [size= color= font= weight=] [style=title|caption|box|neon|outline|gradient] [enter=fadeUp exit=fadeDown]
 shape <rect|ellipse|circle|polygon|star|path|line|arrow> [w= h= fill= stroke= strokeW= radius=] [pos=..] [at dur]
 shape add <shapeLayer> ellipse ...  (more shapes in one layer)
 solid <color> [at dur] [w h radius color2 gradAngle]   adjust [at dur] [fx=blur,vignette]   (adjustment layer)
 import <path..> [as=name]       comp new 'Name' 1080x1920 30 | comp use s2 | comp main s2 | comp set s1 bg=#111 dur=20
 track add V|A [n] | track set V2 hidden=1 locked=1 mute=1 solo=1 vol=-3 | track del V3 [force]
EDIT
 cut <id|V1|all> at=T[,T2]       trim <id> start=T|+d end=T|-d dur= in= [ripple]
 del <ids> [ripple|ripple=all]   move <ids> at=T|by=+2 [track=V2] [mode=overwrite]
 slip <id> +0.5   slide <id> -0.3   speed <ids> 2x|50% [ripple] [interp=blend|flow]   reverse <id>   fit <id> dur=5
 freeze <id> at=T dur=2   dup <id> [at= track=]   replace <id> <media>   detach <id> (split audio)   close V1 (remove gaps)   gap V1 at= dur=
 name <id> <name>   marker add T 'drop'   marker del T   precomp <ids> name='Group'
PROPERTIES & MOTION
 set <ids|id.f1|V1> key=value ...   (set clears keyframes on that prop)
 kf <id|id.f1> <prop> t:v[@ease] ... [ease=out] [loop=cycle|pingpong]   kf+ adds/merges   kf- <id> <prop> [t] removes
   keyframe times are item-relative: 0, 1.5, end, end-0.5, 50%. Values like props (pos=x,y scale=1.2 or 1.2,0.8)
 expr <id> <prop> 'wiggle(2,30)'   (- clears)      anim <ids> popIn,fadeOut [dur= delay= ease= amount= freq= at=T]
 parent <ids> <parent|none>   matte <id> <srcLayer> [alpha|alphainv|luma|lumainv]
LOOK
 fx <ids> <effect> [params]   fx <id.f1> param=v | del | on | off | up | down   fx <id> clear
 afx <id|A1|master> <audioEffect> [params]
 color <ids> <preset> | color <ids> exposure=.3 contrast=15 temp=-10 sat=110   lut <ids> file.cube [amount]
 mask <id> <rect|ellipse|path|polygon|star> [x y w h feather expand invert mode]   mask <id.m1> feather=60 | del
 tr <a> <b> <kind> [dur]   tr <id> in|out <kind> [dur]   tr V1 dissolve 0.4 (all cuts)   tr <a> <b> none
 fade <ids> [in=0.5] [out=1] [video|audio]
AUDIO
 set <id> vol=-6dB pan=-0.3 mute=1 fadeIn=1 fadeOut=2   duck A1 by=A2 amount=-12   master vol=-1 lufs=-14 limiter=1
MACROS (see help macros)
 autocut silence|scenes <clip>   captions <clip> [style=]   montage m1,m2 music=<m|clip> [beats=2]   beats <music>
 reframe 9:16 [mode=blur|fill|fit]   keep <media> 0-12.5,15-30   title <template> [params]
REFERENCES  $ last created, $2 second created in this script; names given with as= work anywhere an id does.";

const TIME: &str = "Times: 12 | 12.5s | 500ms | 90f (frames) | 1:02.5 (m:s) | 1:02:03.5 | 00:00:12:15 (timecode with frames)
 end (comp end) | c3.start c3.end c3.mid | @drop (marker) | sums: c3.end+0.5, end-2. Relative edits: +0.5 / -1.
Keyframe times are relative to the item start; end = item duration; 50% = half of the item.
Positions: pos=960,540 | pos=center | top | bottom | left | right | top-left ... | lower-third | pos=50%,80%.
Units: opacity=50% scale=120% vol=-6dB rot=45 (degrees). Colors: #fff #ff0055 #00000080 rgb(255,0,0) red none.";

const ANIM_HEAD: &str = "Easing (kf @ease, anim ease=, animator ease=): linear hold ease(easy ease) in out inout easeIn easeOut easeInOut snap overshoot anticipate,
 in/out/inOut + Quad Cubic Quart Quint Sine Expo Circ Back Elastic Bounce (e.g. outBack, inOutSine), bezier(x1,y1,x2,y2).
Default keyframe ease is `ease` (smooth); add @linear for constant speed.
anim presets write keyframes/expressions on any layer (combine with commas):";

const TEXT: &str = "text '<content>' at dur [props]. \\n = new line. boxW=900 wraps. caps=1. {expr} placeholders are live: '{round(linear(time,0,2,0,1000))} users'.
Styles: style=title|subtitle|caption|box|neon|minimal|outline|gradient|shadow|lowerthird.
Per-character animators: enter=<preset> exit=<preset> or `anim <text> <preset>`; tune with set <text>.x1 stagger=0.05 dur=0.6 ease=outBack unit=word order=random.
Animator kinds: in (from props -> normal), out (normal -> props), loop (wave: x/y/rot amplitude, freq, phase), hl (highlight units at times= list, karaoke).
Animator props: unit char|word|line, order forward|reverse|center|edges|random, stagger, dur, delay, ease, times, opacity x y scale rot blur tracking color freq phase.
Captions: `captions <clip> style=bold|box|minimal|neon words=3 hl=#ffd60a` (needs analyze transcript).";

const SHAPES: &str = "Shapes: shape <kind> [props] creates a shape layer; shape add <layer> <kind> adds more. Kinds & params:";

const MACROS: &str = "autocut silence <clip> [thresh=-35 min=0.4 pad=0.12 sync]  remove pauses (jump cuts), ripples the track (sync = all tracks)
autocut scenes <clip> [thresh=0.3]      split a clip at scene changes
captions <clip> [style=bold|box|minimal|neon] [words=3] [pos=] [size=] [hl=#ffd60a] [enter=pop] [lang=] [src=file.srt]  animated word-highlight captions
montage m1,m2,m3 music=<media|clip> [beats=2] [at=0] [dur=] [track=V1] [kenburns] [tr=dissolve]  beat-synced cut
beats <music clip|media> [every=1]      markers b1,b2... on beats (use at=@b8)
reframe 9:16|1:1|4:5|WxH [mode=blur|fill|fit] [x=offset] [main]  new comp nesting the current one (blurred background fill)
keep <media> 0-12.5,15-30,41-55        rough cut from source ranges (transcript-driven editing)
precomp <ids> name='Group'              nest items into a new comp
duck <musicTrack> by=<voiceTrack> amount=-12   sidechain ducking
title <template> ...                    motion graphics (help templates)";

const COLOR: &str = "color <ids> <preset> [then fine-tune] e.g. `color c1 cinematic` then `color c1 exposure=0.2 sat=105`.
lumetri params: exposure(stops) contrast highlights shadows whites blacks temp tint (-100..100) sat(%) vibrance fade stint htint (split-tone colors, alpha=strength).
Other color fx: lut (file.cube), curves (master='0,0 .25,.2 .75,.85 1,1'), levels, hue, saturation, vibrance, temperature, tint, duotone, bc, exposure, grayscale, sepia, invert, posterize.
Use analyze stats to check exposure first. Presets:";

const EXPR: &str = "Expressions (expr <id> <prop> '...') run every frame on top of keyframes.
Vars: time value dur fps frame index w h cw ch pi. value is the keyed value (number or [x,y]).
Ops: + - * / % ^ < > <= >= == != && || ?: ; vectors [a,b] with elementwise math, value[0].
Funcs: sin cos tan asin acos atan atan2 sqrt abs floor ceil round(x,n) min max clamp pow exp log mod lerp step smoothstep
 linear(t,t0,t1,v0,v1) ease(...) easeIn easeOut random([max]|[min,max]) noise(x) wiggle(freq,amp[,oct,mult,t]) pt(fps) len norm vec deg rad bounce(freq,amp).
Examples: rot 'time*90' | pos 'wiggle(3,20)' | scale 'value*(1+0.05*sin(time*6))' | opacity 'linear(time,0,1,0,1)'.";

const EXAMPLES: &str = "# Talking-head cleanup + captions + music
import 'C:/Videos/talk.mp4' 'C:/Music/bed.mp3'
add m1
autocut silence $
color c1 natural
captions c1 style=bold words=3
add m2 A2 at=0 vol=-18dB
duck A2 by=A1 amount=-12
master lufs=-14

# Promo with motion graphics (after importing m1 m2 m3)
append m1[2-6] m2[0-4] m3[5-9]
tr V1 dissolve 0.4
anim c1 kenburns
text 'SUMMER SALE' at=0.3 dur=3 style=title enter=pop exit=blurOut
title lowerthird text='Jane Doe' sub='Founder' at=4
shape rect w=1920 h=8 fill=#ff3b30 pos=960,1076 at=0 dur=end
kf $ scale 0:0,1@linear end:1,1

# Vertical version
reframe 9:16 mode=blur main";

fn props_section(title: &str, list: &[schema::P]) -> String {
    let mut s = format!("{title}:\n");
    for p in list {
        s.push_str(&format!(" {}\n", describe(p)));
    }
    s
}

pub fn help(topic: &str) -> String {
    let t = topic.trim().to_ascii_lowercase();
    match t.as_str() {
        "" | "overview" | "index" | "help" => {
            let mut s = format!("{OVERVIEW}\nTopics: ");
            s.push_str(&TOPICS.iter().map(|(n, d)| format!("{n} ({d})")).collect::<Vec<_>>().join(", "));
            s
        }
        "edit" | "dsl" | "script" | "commands" => EDIT.to_string(),
        "time" | "times" | "units" | "refs" => TIME.to_string(),
        "props" | "properties" => {
            let mut s = String::from("Static props (static) can't be keyframed. Use with add/set/kf.\n");
            s.push_str(&props_section("Every layer", schema::LAYER));
            s.push_str(&props_section("Media clip", schema::CLIP));
            s.push_str(&props_section("Text", schema::TEXT));
            s.push_str(&props_section("Solid", schema::SOLID));
            s.push_str(&props_section("Nested comp", schema::NESTED));
            s
        }
        "effects" | "fx" | "video" => {
            let mut s = String::from("fx <id> <effect> [param=value...]  (params are animatable via kf <id>.fN <param> ...). Spatial params in comp px.\n");
            let mut cat = "";
            for e in schema::EFFECTS {
                if e.cat != cat {
                    cat = e.cat;
                    s.push_str(&format!("[{}]{}\n", cat, if e.engine == Engine::Ff { " decode-time, video clips only, static params" } else { "" }));
                }
                let ps: Vec<String> = e.params.iter().map(|p| format!("{}={}", p.name, short_def(p))).collect();
                let al = if e.alias.is_empty() { String::new() } else { format!(" (aka {})", e.alias.join(" ")) };
                s.push_str(&format!(" {}{al}: {}. {}\n", e.name, e.desc, ps.join(" ")));
            }
            s
        }
        "audio" | "afx" | "sound" => {
            let mut s = String::from("afx <clip|A1|master> <effect> [params]. Clip props: vol(dB, keyframable) pan mute fadeIn fadeOut. Track: track set A1 vol=-3 pan=.2 mute=1 solo=1.\nDucking: duck A2 by=A1 amount=-12 attack=20 release=400. Master: master vol=-1 lufs=-14 limiter=1.\n");
            for e in schema::AUDIO_EFFECTS {
                let ps: Vec<String> = e.params.iter().map(|p| format!("{}={}", p.name, short_def(p))).collect();
                s.push_str(&format!(" {}: {}. {}\n", e.name, e.desc, ps.join(" ")));
            }
            s
        }
        "transitions" | "tr" => {
            let mut s = String::from("tr <a> <b> <kind> [dur] [params] (adjacent clips, centered on the cut; uses source handles) | tr <id> in|out <kind> [dur] (from/to transparent) | tr V1 <kind> [dur] (every cut on a track). Default dur 0.5. Any kind accepts ease=. xfade names work too (wipeleft, slideup, circleopen, fadeblack, smoothleft...).\n");
            for t in schema::TRANSITIONS {
                let ps: Vec<String> = t.params.iter().map(|p| format!("{}={}", p.name, short_def(p))).collect();
                s.push_str(&format!(" {}: {}{}\n", t.name, t.desc, if ps.is_empty() { String::new() } else { format!(". {}", ps.join(" ")) }));
            }
            s
        }
        "anim" | "animation" | "keyframes" | "easing" | "motion" => {
            let mut s = format!("{ANIM_HEAD}\n");
            for (title, list) in [("enter", presets::LAYER_ENTER), ("exit", presets::LAYER_EXIT), ("loop", presets::LAYER_LOOP), ("one-shot", presets::LAYER_ONESHOT)] {
                s.push_str(&format!("[{title}] {}\n", list.iter().map(|(n, d)| format!("{n} ({d})")).collect::<Vec<_>>().join(", ")));
            }
            s.push_str(&format!("[text, per character] {}\n", presets::TEXT_ANIMS.iter().map(|(n, d)| format!("{n} ({d})")).collect::<Vec<_>>().join(", ")));
            s.push_str("Options: dur= delay= ease= amount= freq= dist= at=T (one-shots, timeline time). Exit presets end at the item end.\nKeyframes: kf c1 pos 0:-200,540@outExpo 0.6:960,540 | kf c1.f1 radius 0:30 0.5:0 | kf t3 opacity 0:0 0.3:1 end-0.3:1 end:0");
            s
        }
        "text" | "titles" | "captions" | "typography" => {
            let mut s = format!("{TEXT}\n");
            s.push_str(&props_section("Text props", schema::TEXT));
            s.push_str(&props_section("Animator props", schema::ANIMATOR));
            s
        }
        "shapes" | "shape" | "masks" | "mask" => {
            let mut s = format!("{SHAPES}\n");
            for k in ["rect", "ellipse", "polygon", "star", "path", "line", "arrow"] {
                let ps: Vec<String> = schema::shape_props(k).unwrap().iter().map(|p| format!("{}={}", p.name, short_def(p))).collect();
                s.push_str(&format!(" {k}: {}\n", ps.join(" ")));
            }
            s.push_str(&props_section("Common shape props (trim paths = draw-on, repeater = copies)", schema::SHAPE_COMMON));
            s.push_str("Draw-on line: shape path d='M0,0 C200,-150 400,150 600,0' stroke=#fff strokeW=6 fill=none; kf $.s1 trimEnd 0:0 1:100\n");
            s.push_str(&props_section("Masks (x,y,w,h are fractions of the layer; mask <id> <kind> ...)", schema::MASK));
            s.push_str("Track matte: matte <layer> <matteLayer> alpha|alphainv|luma|lumainv (the matte layer is hidden automatically).");
            s
        }
        "templates" | "template" | "title" | "mogrt" => {
            let mut s = String::from("title <template> [at=T] [dur=] [params] — builds an editable precomp (inner layer ids are listed in the result).\n");
            for (n, p, d) in TEMPLATES {
                s.push_str(&format!(" {n}: {d}. {p}\n"));
            }
            s
        }
        "macros" | "auto" => MACROS.to_string(),
        "color" | "grade" | "grading" => {
            let mut s = format!("{COLOR}\n");
            s.push_str(&presets::COLOR_PRESETS.iter().map(|(n, d)| format!("{n} ({d})")).collect::<Vec<_>>().join(", "));
            s
        }
        "expr" | "expressions" => EXPR.to_string(),
        "render" | "export" | "presets" => {
            let mut s = String::from("render tool: preset=<name> [out=path] [range=a-b] [scale=0.5] [hw=auto|nvenc|qsv|amf] [wait=true]. Output defaults to <Videos>/MoodCut Renders. Resolution = comp size × scale.\n");
            for (n, d) in PRESETS {
                s.push_str(&format!(" {n}: {d}\n"));
            }
            s
        }
        "analyze" | "analysis" => {
            let mut s = String::from("analyze tool: target=<media|clip> kind=<kind> [params]. Results are cached and power the macros.\n");
            for (n, d) in crate::analyze::KINDS {
                s.push_str(&format!(" {n}: {d}\n"));
            }
            s
        }
        "examples" | "example" | "recipes" => EXAMPLES.to_string(),
        "fonts" => {
            let f = crate::render::text::families();
            format!("{} families: {}", f.len(), f.join(", "))
        }
        other => format!("unknown topic '{other}'. Topics: {}", TOPICS.iter().map(|t| t.0).collect::<Vec<_>>().join(" ")),
    }
}

fn short_def(p: &schema::P) -> String {
    let d = describe(p);
    // "name=def [..]" -> def
    d.split_once('=').map(|(_, r)| r.split(' ').next().unwrap_or("").to_string()).unwrap_or_default()
}

/// The whole reference as Markdown (`moodcut docs > docs/REFERENCE.md`).
/// Machine-specific topics (installed fonts) are left out.
pub fn reference_markdown() -> String {
    let mut s = String::from(
        "# MoodCut Script reference\n\n\
         <!-- Generated by `moodcut docs`; do not edit by hand. -->\n\n\
         This is exactly what AI assistants get from the MCP `help` tool, generated from the same schema the engine validates against.\n\n",
    );
    s.push_str(&format!("```text\n{}\n```\n", help("")));
    for (topic, desc) in TOPICS.iter().filter(|(t, _)| *t != "fonts") {
        s.push_str(&format!("\n## {topic}: {desc}\n\n```text\n{}\n```\n", help(topic).trim_end()));
    }
    s
}
