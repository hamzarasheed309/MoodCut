//! `help` tool content. Static guides plus catalogs generated from the schema,
//! so documentation can never drift from what the engine accepts.

use crate::model::schema::{self, describe, Engine};
use crate::ops::presets;
use crate::ops::templates::TEMPLATES;
use crate::render::encode::PRESETS;

pub const TOPICS: &[(&str, &str)] = &[
    ("workflow", "a complete edit from one request, step by step"),
    ("direction", "creative director: brief -> concept -> treatment -> build -> review; design and editing direction; rules that keep the work grounded in what was seen and done"),
    ("features", "every feature: when to use it, the command, how to verify it, when not to"),
    ("style", "pro editing + motion design rules (hook, pacing, sound, type, easing) as commands"),
    ("recipes", "complete scripts for Reels, YouTube, montage, promo, podcast, documentary, tutorial"),
    ("platforms", "Reels/TikTok/Shorts/YouTube/feed/ads specs and UI safe zones"),
    ("sfx", "sound design: whoosh, riser, hit, pop... placed on cuts, titles and beats"),
    ("brand", "brand kit: font + colors every title, caption and graphic uses"),
    ("edit", "every MoodCut Script command"),
    ("tools", "Premiere Pro / After Effects tools and panels as commands"),
    ("masks", "After Effects masks, Resolve windows and qualifiers, roto vertices, matte finesse, matte view, keying"),
    ("tracking", "mask, point, planar (perspective) and 3D camera tracking, stabilizing"),
    ("grade", "DaVinci Resolve color page: nodes, primaries, wheels, curves, warper, qualifiers, windows, color management, groups, match"),
    ("mix", "buses, sends, automation, processors, repair, audio scopes, stems"),
    ("voice", "AI voice-over in a cloned voice (OmniVoice, on this computer): setup, clone, speak, takes, fixing transcripts"),
    ("time", "time & reference syntax"),
    ("props", "layer/clip/text/solid properties"),
    ("effects", "video effects + params"),
    ("audio", "audio effects, ducking, loudness"),
    ("transitions", "transition types"),
    ("anim", "animation presets, easing, keyframes"),
    ("3d", "cameras, nulls, 3D layers and parallax"),
    ("text", "titles, text animators, captions"),
    ("shapes", "shape layers & masks"),
    ("templates", "motion graphics templates"),
    ("macros", "autocut, captions, montage, reframe..."),
    ("color", "color grading"),
    ("scopes", "Lumetri scopes and grading by measurement"),
    ("expr", "expressions"),
    ("render", "export presets"),
    ("analyze", "analysis kinds"),
    ("graphics", "HTML/CSS/JS motion graphics"),
    ("examples", "complete example scripts"),
    ("fonts", "installed fonts"),
];

const OVERVIEW: &str = "MoodCut: AI-native video editor. Loop: media import -> view -> edit (batch many lines) -> preview (contact sheet) -> render.
Ids: m1 media, c2 clips, t3 text, g4 shapes, o5 solids, j6 adjustment, n7 nested comps, s1 comps; sub-objects c2.f1 (effect) c2.m1 (mask) g4.s1 (shape) t3.x1 (text animator).
Tracks: V1 is the bottom video track (higher = on top), A1.. audio. Times are seconds.
Scripts are atomic: if any line fails nothing changes and the error names the line.";

pub const WORKFLOW: &str = "Turning one request into a finished, professional edit. Do every step yourself; send many script lines per edit call; ask the person only if something essential is missing.
1 GATHER  media find path=<folder> (what is there) -> media import paths=[<folder>] tag=<shoot>. Large library: media list (filters/paging) + preview media=m1..m48 (poster frames, 48 per sheet). Write down what you see so you never look twice: note m3 'wide drone, sunset' | tag m10..m18 interview.
2 UNDERSTAND  speech: analyze target=tag:interview kind=transcript, then view what=search q=<words> to find quotes (times are source seconds). Shots: analyze target=all kind=scenes / stats (exposure). Music: analyze <m> kind=beats. Filmstrips (preview media=m7 n=8) to pick in/out points.
3 PLAN  like a creative director (help direction: brief, one-line concept, five-line treatment; help features: which tool serves each choice; help style: the craft numbers): platform and length (help platforms; comp new 'Main' 1080x1920 30 or reframe 9:16 later), structure (hook in the first 1-2 s -> body -> payoff / call to action or loop), the closest help('recipes') to start from, brand (brand <preset> or the client's font and colors), ONE look, 2-3 reusable motion moves, music and where its drop lands, a sound palette. Build only what was asked, but finish it to a professional standard.
4 ASSEMBLE  bottom-up: story on V1: append m4[12.3-18.9] m4[41-47.5] ... (quotes from transcripts) or add + autocut silence V1 + punchin V1 for talking heads; keep m4 12-30,45-61 for rough cuts; jcut/lcut between interview answers; b-roll on V2 over speech starting on the word it shows (add m7[2-5] V2 at=c3.start+1); montage m1,m2,... music=m9 beats=2 for beat-cut sequences; speedramp into action and drops. Shot lengths per platform (help style); vary framing; cut on action and on the beat; no repeated shots. Portrait footage in a landscape edit (or the reverse): set <ids> fit=blur.
5 POLISH  color <ids|V1> <one look> then fine-tune by scopes; one or two motivated transition types (whip between places, cuts elsewhere); titles and lower thirds in the brand (graphic tool for bespoke designs, else title templates) with the same entrance everywhere; captions V1 last (they go on top) for speech-led videos; anim kenburns amount=0.06 on stills; then sound design: sfx whoosh on=transitions, hit on title slams, riser into reveals (help sfx).
6 SOUND  music on A2 under speech: duck A2 by=A1 amount=-12 (by=V1 when speech is camera audio); fade music in/out; master lufs=-14 limiter=1 (web) | -16 (podcast) | -23 (broadcast).
7 VERIFY  check target=<platform> expect={dur, size, captions, music} (fix=true applies the safe fixes) and look at its problem frames; fix its craft notes too (hook, pacing, safe zones, easing, music ending, sound design); preview n=12 and judge it as a viewer (would you stop scrolling at frame 1? does something change every 3-5 s?); fix and check again until clean.
8 DELIVER  render preset=youtube | social | prores ... (wait=true); captions as a file: render preset=srt; YouTube chapters from markers: render preset=chapters; to finish in Premiere/Resolve: render preset=xml; vertical version: reframe 9:16 main, check, render preset=social. Report what you made: length, structure, music, files, the concept and key choices, what check and the previews confirmed, and anything you could not do or verify (help direction).";

const GRAPHICS: &str = "graphic tool: html -> rendered video with alpha (or a still, dur=0) in the library. The page is the frame: 100% width/height = comp size (or size=). Transparent unless you paint a background (alpha=false keeps an opaque page).
Timing runs on a virtual clock, so animate any way you like and every frame is exact:
 CSS @keyframes/transitions (animation-delay to sequence; fill-mode both to hold), el.animate() (Web Animations), requestAnimationFrame + performance.now(), setTimeout/setInterval,
 GSAP / anime.js / Lottie / three.js / canvas / SVG <animate> (load from a CDN or files next to the html).
 Finish (or loop) within dur. Time the outro from the end so changing the length keeps it: animation: out .5s calc(var(--mc-dur) - .6s) forwards (--mc-dur = the graphic's seconds; --mc-w/--mc-h = frame size; MC.duration in JS).
Hooks: MC.onFrame = t => {...} runs each frame (t seconds); MC.time = now; MC.ready = a promise awaited before frame 0; MC.duration/fps/width/height/vars.
FIELDS (required): people edit graphics in the editor without touching code, through fields. Every visible text, color, size or choice someone may want to change is a {{name|default}} placeholder; the default is what renders until it's changed:
 <h1>{{title|Big launch}}</h1>  background: {{accent|var(--brand-accent)}};  bottom: {{lift|12}}%;  <img src='{{logo|mc://m3}}'>
 The type follows the default: text (a newline or <br> = multi-line), color (#hex, rgb(), var(--brand-*) follows the brand kit), number with its unit, true/false toggle, mc://m3 media.
 Labels, ranges and choices (optional): <script type='application/json' id='mc-fields'>{\"lift\":{\"label\":\"Height\",\"min\":4,\"max\":40},\"style\":{\"type\":\"select\",\"options\":[\"bar\",\"box\"]}}</script> (a field only scripts read needs a \"default\" here).
 Values are escaped for where they stand (HTML, attributes, CSS, JS strings); in scripts read MC.vars.name (strings, defaults included). Name fields for what they are (title, speaker, accent), not text1.
 The reply lists the fields. A page without any gets its visible texts and colors made into fields automatically (editable=false keeps the html as written).
Change values later without rewriting: graphic id=m7 vars={title:'New title'} (vars={title:null} resets to the default; every clip using m7 updates). Variant: graphic from=m7 vars={...}; for one clip only add replace=c12. Fix the design in place: graphic id=m7 html=... Read back what people changed: graphic id=m7 source=true. Brand kit changed: graphic refresh=true. Stills: dur=0 at=2 captures the design at 2s.
Media: <img src='mc://m3'>, <video src='mc://m4' muted> (seeked frame-accurately) use library media.
Fonts: Inter 100-900 built in; Google Fonts <link> works when online; system fonts.
Layout: size with %, vw/vh so it scales; keep text inside the 5% title-safe margin. The reply reports the content box and warns on overflow, empty frames and JS errors; check the contact sheet.
Use: place='V3 at=2' or later `add m7 V3 at=2`; it is a normal clip (trim, move, fade, effects, blend).
Example lower third (fields name, role, accent):
<style>.l3{position:absolute;left:6%;bottom:12%;font:800 4.4vh Inter;color:#fff;animation:out .4s calc(var(--mc-dur) - .5s) ease-in forwards}
.bar{height:.6vh;margin:.8vh 0;background:{{accent|var(--brand-accent)}};transform-origin:left;animation:grow .6s cubic-bezier(.2,.8,.2,1) both}
.name{animation:up .6s .1s both}.role{font:500 2.6vh Inter;opacity:.85;animation:up .6s .25s both}
@keyframes grow{from{transform:scaleX(0)}}@keyframes up{from{opacity:0;transform:translateY(40%)}}@keyframes out{to{opacity:0;transform:translateY(-20%)}}</style>
<div class=l3><div class=name>{{name|Jane Doe}}</div><div class=bar></div><div class=role>{{role|Founder}}</div></div>";

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
 import <path..> [as=name] [tag=a,b]   comp new 'Name' 1080x1920 30 | comp use s2 | comp main s2 | comp set s1 bg=#111 dur=20
 track add V|A [n] | track set V2 hidden=1 locked=1 mute=1 solo=1 vol=-3 | track del V3 [force]
LIBRARY (media: m3, m1..m40, all, tag:x, kind:video|audio|image|graphic, q:words)
 tag <media> broll,drone | untag <media> [tag] | note <media> 'what is in the shot' | relink m3 <new path> | unimport <media> [force]
EDIT
 cut <id|V1|all> at=T[,T2]       trim <id> start=T|+d end=T|-d dur= in= [ripple]
 del <ids> [ripple|ripple=all]   move <ids> at=T|by=+2 [track=V2] [mode=overwrite]
 slip <id> +0.5   slide <id> -0.3   speed <ids> 2x|50% [ripple] [interp=blend|flow]   reverse <id>   fit <id> dur=5
 speedramp <clip|V1> at=T [to=4x|40%] [dur=0.6] [ramp=0.2] (needs spare source after the out point to speed up; stacks)   time remap by hand: kf <clip> remap 0:0 2:2 3:5.25@easeIn
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
 afx <id|A1|master> <audioEffect> [params]   afx <id.a1|A1.a1|master.a1> param=v | del | on | off | up | down   afx <id|A1|master> clear
 color <ids> <preset> (a new look replaces the last one; hand-set values stay) | color <ids> exposure=.3 contrast=15 temp=-10 sat=110 | color <ids> none   lut <ids> file.cube [amount]
 mask <id> <rect|ellipse|path|polygon|star> [x y w h feather expand invert mode]   mask <id.m1> feather=60 | del
 path masks use normalized layer coordinates: d='M0.1,0.1 C0.2,0.05 0.4,0.05 0.5,0.1 L0.8,0.7 Z'; keyframe matching paths with `kf c1.m1 d 0:M0,0C0.2,0.1,0.3,0.1,0.5,0 1:M0,0C0.2,0.2,0.3,0.2,0.5,0` (same segment structure for smooth morphing)
 mask <id.m1> pt 3 x,y [at=T] | pt 3 smooth|corner|first | topath | track mode=perspective   (help masks)
 node <clip> add [serial|parallel|layer|outside] | node <clip> N primaries gainR=1.03 | node <clip> N window add circle | node <clip> N qualify hsl hue=25 | node <clip> N mix=.7   (Resolve nodes; help grade)
 track <clip.m1> mode=pos|rotation|similarity|affine|perspective | track <clip> corners='..' to=cornerpin on=<layer> | track <clip> region=.. to=null|stabilize   (help tracking)
 camtrack <clip> [fov=] | camtrack <clip> ground=p1,p2,p3 | camtrack <clip> attach text 'Hi' at=p1,p2,p3 | camtrack <clip> points   (3D camera tracker)
 tr <a> <b> <kind> [dur]   tr <id> in|out <kind> [dur]   tr V1 dissolve 0.4 (all cuts)   tr <a> <b> none
 fade <ids> [in=0.5] [out=1] [video|audio]
AUDIO
 set <id> vol=-6dB pan=-0.3 mute=1 fadeIn=1 fadeOut=2   duck A1 by=A2 amount=-12   master vol=-1 lufs=-14 limiter=1
TOOLS (see help tools: Premiere/AE tools and panels)
 roll <id> +0.3   lift|extract [in out]   swap <a> <b>   paste <from> <to> [fx|color|transform|...]   stagger <ids> by=0.1|seq   workarea 2 8
 align <ids> left|center|... [to=comp|safe|sel]   distribute <ids> h|v   nudge <ids> pos=10,0 scale=120% rot=5 [key]   ease <ids> [props|all] easy   kf <id> all reverse|shift=+1|stretch=2|bake|clear
 null [children=<ids>]   camera [move=pushin] [layers=<ids>]   sound <clips> dialogue|music|sfx|ambience   marker set <name|T> name= color= dur= note=
 selectors: sel after:T before:T at:T kind:text name:x V2:after:T; time keyword playhead
MACROS (see help macros)
 autocut silence|scenes <clip>   captions <clip> [style=]   montage m1,m2 music=<m|clip> [beats=2]   beats <music>
 reframe 9:16 [mode=blur|fill|fit]   keep <media> 0-12.5,15-30   title <template> [params]
REFERENCES  $ last created, $2 second created in this script; names given with as= work anywhere an id does.";

const MASKS: &str = "Masks (After Effects masks, Resolve power windows and qualifiers). Ids count per layer: a layer's first mask is m1, the next m2 (effects f1, f2 on their own).
 mask <id> rect|ellipse|polygon|star|path|gradient [x= y= w= h= rot= radius= points= feather= ...] [window] [invert]
   Resolve names work too: circle (ellipse), linear (rect), curve (path), polygon, gradient. x,y = center, w,h = size, all fractions of the layer.
   path: d='M0.1,0.1 C0.2,0.05 0.4,0.05 0.5,0.1 L0.8,0.7 Z' (normalized layer coords, cubic Beziers); smooth=1 is RotoBezier (tension=-1..1).
 Cut or window: a plain mask cuts the layer (After Effects); with the word window (cut=0) it only limits effects (a Resolve window). mode=add|subtract|intersect|lighten|darken|difference|none combines cutting masks as in AE.
 Edge: feather= (px), featherY= (AE's two-value feather), falloff=smooth|linear, featherBias=-1..1 (inside..outside), vfeather='3:40 5:-10' (per-vertex feather points, px; + outward, - inward),
   expand= (grow/shrink px), opacity=, invert, mblur=1 shutter=180 (mask motion blur), refine=8 refineSoft=.2 (Refine Edge: snaps soft edges to hair and fur).
 Vertices (AE pen tools; numbers as preview view=overlay shows them): mask c1.m1 pt 3 0.42,0.31 [in=x,y out=x,y] [at=T] moves vertex 3 (with at= it keys the path there) |
   pt 3 smooth | pt 3 corner (Convert Vertex) | pt 3 first (Set First Vertex, decides how paths morph) | pt add after=2 | pt 4 del. mask c1.m1 topath turns a shape mask into an editable path.
 Animate: kf c1.m1 d 0:'M..' 2:'M..' (paths with the same vertex count morph), kf c1.m1 feather|x|w|opacity|expand ... Track: track c1.m1 mode=pos|rotation|similarity|affine|perspective (help tracking).
 On effects: every in-place effect takes mask= and mix=: fx c1 blur radius=40 mask=m1 (blur a face), color c1 sat=80 mask=m2. Expressions: m1+m2 union, m1*m2 intersection, -m1 outside, parentheses: (m1+m2)*-m3.
 Qualifiers (Resolve keys from the picture, always windows): mask c1 hsl hue=25 hueRange=40 satLo=.15 satHi=.8 lumLo=.2 lumHi=.9 qsoft=.1 window | mask c1 luma lumLo=.7 window |
   mask c1 rgb rLo=.6 gHi=.4 window | mask c1 3d color=#d8a080 tolerance=.12 window (read the color with preview pick=x,y). This color inside this window: mask=m1*m2.
   Matte finesse: cleanBlack= cleanWhite= (levels), clean= (despeckle px), morph=shrink|grow|open|close morphR=2, matteBlur=4, expand=.
 Auto-trace: mask c1 trace [channel=luma|alpha|red|green|blue] [threshold=.5] [pick=x,y] [invert] [range=a-b every=0.5] (AE Auto-trace; with range= the path is keyed).
 Track mattes: matte <layer> <matteLayer> alpha|alphainv|luma|lumainv.
 Look at it: preview view=matte id=c1.m1 t=2 (gray matte, coverage, box, soft-edge share) | view=overlay (outline + numbered vertices over the picture); expressions too: id=c1.m1*m2. Colors: preview pick=x,y.
 Keying: fx c1 chromakey auto=1 similarity=.35 clipBlack=.1 clipWhite=.9 edge=2; fx c1 matteclean size=2; fx c1 lightwrap width=12; fx c1 spill. Garbage/holdout mattes are masks.";
const TRACKING: &str = "Tracking (After Effects' tracker and mask tracker, a Mocha-style planar tracker and the 3D Camera Tracker). Results are ordinary keyframes; at most 60 s per call (split long shots with range=a-b).
 Masks (AE mask tracking methods): track c1.m1 [mode=pos|rotation|similarity|affine|perspective] [range=a-b] [at=T] [planar]   (mask c1.m1 track ... is the same)
   pos, rotation and similarity key the mask's placement (offset, trot, scale), so its own shape keys stay; affine and perspective key the path's vertices (a shape mask becomes a path).
   at= is the time the mask is drawn right (default: playhead or clip start). planar uses the feature tracker for pos/similarity too (better on detailed surfaces).
 A point to a null: track c1 region=cx,cy,w,h to=null [mode=pos|rotation|similarity|perspective] -> a null with position (and rotation, scale) keys; parent titles to it: parent t1 <null>.
 Screen replacement / corner pin: track c1 corners='x,y x,y x,y x,y' to=cornerpin on=c5 -> the plane inside the four corners (clockwise from top-left, fractions) followed in perspective;
   mode=points tracks the four corners themselves (only when they are crisp marks).
 Stabilize: track c1 region=cx,cy,w,h to=stabilize [mode=similarity] [smooth=1] [zoom].
 3D camera (AE's 3D Camera Tracker): camtrack c1 [range=a-b] [at=T] [fov=54] [nodal] [points=500]   (track c1 to=camera is the same)
   -> a one-node camera layer keyed from the shot (pos, z, rx, ry, rot, zoom) plus solved 3D points; the reply gives the error (px), field of view and unsolved stretches.
   The clip must fill the comp centered and level (otherwise precomp it). The focal length is solved unless fov= gives the horizontal angle of view. Tripod pans and tilts solve as a rotating camera (nodal).
   preview view=points id=c1 t=2 shows the numbered points (green = accurate) and the floor grid; camtrack c1 points [t=] [near=x,y] [n=20] lists them.
   camtrack c1 ground=p3,p7,p9 [origin=p3]: those points' plane becomes the floor (level, y down) and the origin the comp center at z=0; the camera and attached layers follow.
   camtrack c1 attach text 'SALE' at=p3,p7,p9 [size=1.5] [face=plane|camera] [as=name] | attach solid #fff | attach null | attach shape rect | attach m4 | attach <layer id>
     -> a 3D layer on those points: lying on their plane (3+ points) or facing the camera, at its natural size at the reference frame. Re-solving replaces the camera's keys.
 Replies name lost stretches: `~c1.m1 tracked 0-8.4s, 31 keys; !lost 3.2-3.6 (conf 0.41)` -> check with preview view=overlay t=3.4 and fix: kf c1.m1 offset 3.4:0.12,0.03 or mask c1.m1 pt ... at=3.4
 Reversed, remapped or frozen clips: track before changing timing.";
const GRADE: &str = "Grading the DaVinci Resolve way. Every color effect is a node; consecutive effects with the same node label are ONE corrector node: its tools run together, keyed once by the node's
window/qualifier and blended once by its mix (key output gain). Per-pixel tools in a node fuse into one LUT.
NODES (node verb; nodes are numbered 1, 2, ... in order; view what=grade or node c1 list shows them)
 node c1 add [serial|parallel|layer|outside] [after=N] [name=Skin] [composite=softlight]   a new node starts as neutral primaries (Resolve's corrector)
 node c1 2 primaries liftY=-.02 gammaR=.01 gainB=1.03 contrast=1.1 pivot=.42 sat=55 | node c1 2 curves luma='0,0 .3,.26 .75,.8 1,1' | node c1 2 huecurves hueSat='.33,-.4'
   (a tool the node lacks is added to it; any color tool below, plus blur, sharpen, glow, smooth)
 node c1 2 window add circle x=.5 y=.4 w=.3 h=.4 feather=60 | window add linear | polygon | curve d='M..' | gradient rot=0 h=.3 | node c1 2 qualify hsl hue=25 hueRange=30 satLo=.15
   node key = (window + window ...) * qualifier: windows join, qualifiers narrow. node c1 2 window=m1+m2 sets it whole; window none clears it. Track it: node c1 2 track mode=similarity.
 node c1 add outside after=2 (the inverse of node 2's key) | node c1 2 mix=.7 | off | on | reset | del | name=Skin | parallel | layer composite=screen | serial | up | down
 parallel nodes take the input of the node before them and their changes add up (parallel mixer); layer nodes take that input too and composite on top by their key
 (layer mixer: normal add subtract multiply screen overlay softlight hardlight darken lighten difference color luminosity). Same with fx: fx c1 primaries node=Skin mask=m2 mix=.8 parallel=1.
TOOLS (fx names; help effects lists every param)
 primaries: lift/gamma/gain/offset as master (Y) and R G B: liftY -1..1, gammaY -1..1 (+ brighter mids), gainY 1 = unchanged, offsetY -1..1; contrast 1 + pivot .435; sat 50 = unchanged (0 gray, 100 double);
   hue 50 = unchanged; lumMix 100; temp (Resolve units, + warmer) tint; boost (color boost); shadows, highlights; midDetail (MD, local contrast).
 logwheels: shadow/midtone/highlight/offset tints (#rrggbbaa, alpha = strength) and levels, low=.333 high=.55 ranges, contrast, pivot. hdrwheels: black/dark/shadow/light/highlight/specular zones (Exp, Sat, Tint each; zone edges in stops).
 colorwheels: lift/gamma/gain/offset tints and levels with shadowPivot/highlightPivot. curves: master r g b luma (custom curves, YRGB). huecurves: hueHue hueSat hueLuma lumaSat satSat satLuma.
 colorwarper pins='35,.55>24,.45' luma='215,.5>-.12' (hue-sat warper). colorslice: Hue/Sat/Den for red skin yellow green cyan blue magenta. secondary (HSL key + correction). channelmixer (RGB mixer, mono=1).
 lut file='look.cube' amount=. cst from=slog3 to=rec709 tonemap=simple|luma|none gamut=compress|clip (color space transform). softclip, filmcurve, halation, legalize (gamut limiter).
COLOR MANAGEMENT color c1 input=slog3|logc3|logc4|vlog|clog3|dlog|hlg|pq|rec709|auto (decoder input transform at source precision; auto reads HLG/PQ tags) or a cst node.
BALANCE AND MATCH color c1 wb=auto | wb=pick:.4,.6 (a neutral point) | wb=off. match c2..c9 to=c1|still:hero [what=exposure,balance,sat] -> a colorwheels node tag=match (redone on re-match).
GROUPS AND VERSIONS grade save day-ext from=c1 (the clip's color effects become a linked post-clip group) | grade apply day-ext c2..c9 [pre] (pre-clip) | grade detach c4 | grade copy c1 c2..c9 [append] |
 grade version c1 save|load|del <name> | grade version c1 list | grade reset c1 | grade del | grade list. Timeline grade: an adjustment layer over the clips.
GALLERY still save hero from=c1 [at=T] | still list | still del. preview ref=hero split=0.5 t=.. (wipe, with the measured difference).
MEASURE preview scope=all|waveform|parade|vectorscope|histogram|falsecolor [region=cx,cy,w,h] t=..; preview view=matte id=c1.m2 (a node's key); view what=grade [id=c1].
Typical: color c1 input=slog3 -> node c1 add (balance: primaries gainR=..) -> node c1 add (contrast: primaries contrast=1.15 pivot=.4) -> node c1 add parallel, node c1 3 qualify hsl hue=25 (skin),
 node c1 3 huecurves -> node c1 add, node c1 4 window add circle, node c1 4 track -> node c1 add layer composite=softlight, node c1 5 lut file=.. -> grade save look from=c1 -> match c2..c9 to=c1.";
const VOICE: &str = "AI voice-over in a cloned voice: OmniVoice (k2-fsa/OmniVoice) runs on this computer, on the GPU when there is one. The AI Voice panel shows the same setup, voices and tasks.
PERMISSION: clone a voice only when the person says it is their own voice or the speaker agreed (consent=true states that). Never clone a public figure or someone who isn't part of the project.
STATUS: voice action=status -> setup state, voices (id, name, seconds, transcript) and recent tasks with progress. Not set up: ask the person, then voice action=setup drive=D (model 3.3 GB + Python with PyTorch 6.5 GB on that drive; status shows the progress), or they Locate a model they have in the panel. voice action=repair fixes a setup whose folder was moved or whose packages broke (no model download).
CLONE: voice action=clone name='Host' media=m1 consent=true -> without end=, the best 3-12 s of one person speaking in the first 30 s is picked (cut at pauses), Whisper writes the transcript (a one-time 1.6 GB download) and the reply shows it. Better clones: 5-10 s of clear speech without music (start= end= pick it: read the transcript or a preview first); denoise=true for noisy phone recordings; if the transcript has a wrong word: voice action=retranscribe voice=<id> transcript='the exact words' (the words must match the audio). A typed transcript= on clone skips Whisper: then set start= and end= to exactly those words.
SPEAK: voice action=speak voice=<id or name> text='Welcome back. Today we test three cameras.' [language=English] [speed=1.1] [quality=fast|standard|best] [takes=2] [at=4 | at=playhead] -> one clip per paragraph (a blank line in text starts the next; split=false keeps one clip), imported with tag voice and the script as the note; at= places the first take of each part on an audio track one after another. Numbers are read as words (numbers=false keeps them); loudness is evened out to about -16 LUFS (loudness=false keeps the model's level). Each take has a seed: seed=<n> repeats a reading, another seed gives a new one.
PROSODY: punctuation shapes the reading (commas pause, full stops fall, question marks rise); write numbers, dates and abbreviations the way they are said when it matters ('twenty twenty-six', 'A I'); inline tags like [laughter] and [breath] are kept. Speed 0.9-1.1 sounds natural; for an exact length trim or speed the clip after.
CHECK: listen before using it (the editor's Source monitor; preview media=<id> shows its waveform); analyze target=<id> kind=transcript confirms the words. Mix it like dialogue: sound <clip> dialogue, duck music under it.
ERRORS come with what to do (out of GPU memory: close other GPU apps or use shorter paragraphs; setup broken: repair; no speech in the reference: pick another part).";

const MIX: &str = "Mixing (Audition-style). Signal flow: clip (gain, pan, fades, afx) -> track (vol, pan, automation, afx) -> bus(es) -> master (vol, afx, loudness, limiter).
  bus add B1 name=Dialogue | bus set B1 vol=-3 pan=0 out=B3|master mute=0 | bus del B1 | bus list
  track set A1 out=B1          route a track to a bus
  send A1 B2 -12dB [pre]       a send (shared reverb, parallel compression); send A1 B2 off. Loops are refused.
  kf A1 vol 0:-6 4:0 / kf B2 pan 0:-1 3:1   track and bus automation (comp seconds); kf c1 pan ... for clips. set c1 fadeCurve=linear|log|exp|scurve|power; audio transitions are equal-power.
  afx <clip|A1|B1|master> <effect> [mix=0.5]: every audio effect takes mix (wet/dry). Racks: afx A1 rack=podcast-voice|voiceover|phone|music-bed|glue|room-verb|hall-verb|master.
  Processors: peq bands='120,-3,1,lowshelf 3000,2,1,bell 80,0,0.7,highpass' (8 bands), multiband, limiter truepeak=1, reverb type=room|hall|plate|booth|echo (convolution), denoise amount=12 learn=0.2-1.0 (noise print), declip, dehum, deess, stereo width=1.2 balance=0 invertl=1, compress, gate, ...
  duck B2 by=B1 (buses or tracks). master lufs=-14 limiter=1 truepeak=1.
  Measured repair: repair <clips|A1> (adds dehum / declip / highpass / denoise only when measured), roomtone A1 (fills gaps with the quietest second), match c2 to=c1 eq|loudness.
  Measure: preview scope=spectrum|spectrogram|loudness|phase|mix [target=c3|A1|B1|master] [range=a-b] (image + I/S/M LUFS, LRA, true peak, noise floor, clipping, DC, six bands, correlation, hum); scope=mix gives one line per track and bus. analyze noise|spectrum|dynamics on media. view what=mix lists routing, sends, chains, automation and levels. render preset=stems writes a WAV per bus plus the mix.";
const TIME: &str = "Times: 12 | 12.5s | 500ms | 90f (frames) | 1:02.5 (m:s) | 1:02:03.5 | 00:00:12:15 (timecode with frames)
 end (comp end) | c3.start c3.end c3.mid | V1.end (where the last clip on V1 ends) | @drop (marker) | sums: c3.end+0.5, end-2. Relative edits: +0.5 / -1.
Keyframe times are relative to the item start; end = item duration; 50% = half of the item.
Positions: pos=960,540 | pos=center | top | bottom | left | right | top-left ... | lower-third | pos=50%,80%.
Units: opacity=50% scale=120% vol=-6dB rot=45 (degrees). Colors: #fff #ff0055 #00000080 rgb(255,0,0) red none.";

const TOOLS: &str = "Premiere Pro and After Effects tools and panels as MoodCut Script. The editor's panels send these same lines, so everything is undoable and visible to both of you.
TIMELINE TOOLS
 selection (V): move <ids> at=T|by=+1f|track=V2   track select forward (A): any command with after:T or V2:after:T
 ripple edit (B): trim <id> start=+0.5 ripple | trim <id> end=-1 ripple   rolling edit (N): roll <id> +0.3 | roll <id> to=T (cut between <id> and the next clip)
 rate stretch (R): fit <id> dur=4 | speed <id> 150% [ripple]   razor (C): cut <id|V1|all> at=T   slip (Y): slip <id> +0.5   slide (U): slide <id> -0.3
 rearrange: swap <a> <b>   lift (;): lift 2 5 [tracks=V1,A1]   extract ('): extract 2 5   both use the work area when no range is given
 work area / sequence in-out: workarea 2 8 | workarea c3 | workarea all | workarea clear (render range=work)
 paste attributes: paste <from> <to ids> [fx|color|transform|opacity|masks|audio|speed|anims|all] [stack] (default fx+transform+opacity+masks; same-kind effects are replaced)
 enable/disable: set <id> off=1   label color: set <id> label=green   nudge: move <ids> by=+1f
SELECTORS (anywhere ids go) sel = the editor's selection | after:T | before:T | at:T | kind:text|clip|video|audio|image|graphic|shape|solid|adjust|comp|null|camera | name:intro | V2:after:10
 time keyword playhead (or now) = the editor's playhead, e.g. text 'Hi' at=playhead
KEYFRAME ASSISTANT (Effect Controls, Graph Editor)
 ease <ids|id.f1> [prop,prop|all] easy|easeIn|easeOut|linear|hold|outBack|bezier(x1,y1,x2,y2) [at=T]   (F9 Easy Ease = ease <id> all easy)
 kf <id|id.f1> <prop|all> reverse | shift=+0.5 | stretch=2 | bake [step=1f] (expression/loop -> keyframes) | clear | smooth
 stagger <ids> by=0.1 [from=T] [order=forward|reverse|random|time] | stagger <ids> seq [overlap=0.5] (Sequence Layers)
ALIGN (After Effects Align panel)
 align <ids> left|hcenter|right|top|vcenter|bottom|center [to=comp|safe|sel] [margin=40] [at=T]   distribute <ids> h|v [gap=24]
 nudge <ids> [pos=dx,dy] [scale=120%] [rot=15] [key] [at=T]: relative move/scale/rotate (all keyframes move; with key: a keyframe at the playhead)
 boxes are measured from the rendered layer (text, shapes, media, parenting, 3D); position keyframes move together.
LAYERS null [at dur] [children=<ids>] | camera [move=pushin|pullout|truck|pedestal|orbit|drift|shake] [layers=<ids>] (help 3d) | set <id> blend=<mode> | matte | precomp
COLOR (Lumetri Color panel; help color)
 basic correction: color <ids> exposure= contrast= highlights= shadows= whites= blacks= temp= tint= sat= vibrance=   creative: color <ids> <look> + fade= stint= htint=
 curves: fx <id> curves master='0,0 .25,.2 .75,.85 1,1' | fx <id> huecurves hueSat='0.33,-0.6' lumaSat='0.05,-1'
 color wheels: fx <id> colorwheels shadows=#2a6cff40 highlights=#ff9a4440 midtonesL=5   HSL secondary: fx <id> secondary hue=120 hueRange=25 sat=0.6
 vignette: fx <id> vignette   match a grade: paste <graded> <others> color   scopes: preview scope=all (help scopes)
AUDIO (Essential Sound, Audio Track Mixer)
 sound <clips> dialogue|music|sfx|ambience|clear [amount=1] [match=0] [under=A1] [duck=-12]: loudness match, speech chain (voice+de-ess), music ducks under dialogue
 mixer: track set A2 vol=-3 pan=-0.2 mute=1 solo=1 | master vol=-1 lufs=-14 limiter=1 | afx <clip|A1|master> <effect> (help audio)
MARKERS marker add T 'name' [color=#f00 dur=2 note='...'] | marker set <name|T> name= t= color= dur= note= | marker del <name|T> | marker clear
HISTORY project action=history (undo labels) | undo [n] / redo [n]";

const THREE_D: &str = "3D layers and cameras (After Effects style). x right, y down, z into the screen (bigger z = farther away).
A layer is 3D when it has rx, ry or z, or 3d=1 (flat layers need 3d=1 to move with a camera). 3D layers are drawn back to front as seen from the camera; 2D layers keep track order and ignore the camera.
camera [at= dur=] [zoom=px] [pos=x,y] [z=offset] [poi=x,y] [poiZ=] [aim=0] [move=<preset> amount= ease=] [layers=<ids>]
 zoom 0 = default (0.926 x comp width, like a 50mm lens). z moves the camera along its view: + dollies in. poi is the point it aims at (two-node);
 aim=0 makes a one-node camera turned with rx/ry/rot. The topmost camera layer active at a time is used. Keyframe pos, z, poi and zoom like any property; parent it to a null for rigs.
 move presets: pushin, pullout (z, amount=px), truck, truckleft (x), pedestal, pedestaldown (y), orbit, orbitleft (around the poi, amount=degrees), drift, shake. layers=V2,V3 switches those layers to 3D.
null [at dur] [pos=] [children=<ids>]: an invisible layer to parent others to (parent <ids> <null>) and animate them together.
Parallax recipe: background at z=800 scale=150%, b-roll at z=0, title at z=-300, all 3d=1, then camera move=pushin amount=400.
Layers behind the camera aren't drawn. Card flips and tilts: kf <id> ry 0:-90 0.6:0@outBack (no camera needed).
Match a real camera move (3D Camera Tracker): camtrack c1 solves the shot into a camera layer and 3D points; camtrack c1 attach text 'Hi' at=p3,p7,p9 puts a 3D layer on them (help tracking).";

const SCOPES: &str = "preview scope=waveform|parade|vectorscope|histogram|all [t=seconds] [comp=] [width=] returns the Lumetri scope and a measured line:
 luma 1st-99th percentile, median and mean | clipped (any channel >= 99.5%) | crushed (luma <= 1%) | outside broadcast 16-235 | saturation | color cast (warm/cool, green/magenta) | hints.
 all = the frame + luma waveform + vectorscope (75% color targets, skin-tone line) + RGB parade. The time defaults to the editor's playhead, else the comp middle.
Grade by numbers: correct first (exposure/whites/blacks until luma spans roughly 2-98% with under 1% clipped; temp/tint until the cast reads neutral), then apply a look.
Normal scenes have a median around 35-55%; skin tones sit on the vectorscope's skin line. fx autolevels (perchannel=1) fixes levels and casts per frame; fx legalize keeps broadcast levels.";

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

const COLOR: &str = "color <ids> <preset> [then fine-tune] e.g. `color c1 cinematic` then `color c1 exposure=0.2 contrast=18 pivot=.42 sat=105 mask=m1`. Color effects accept mask=m1 to grade only that layer mask and mix=0.75 to blend the effect. Add a grade window with `mask c1 ellipse feather=24` or a Bézier path, adjust it with `set c1.m1 feather=30`, and select it per effect with mask=m1. Import custom LUTs with `lut c1 file='C:/LUTs/look.cube' amount=.8 mask=m1`; 1D, 3D and combined .cube files are supported. The Color panel also exposes Lift/Gamma/Gain/Offset primaries, curves, HSL secondary and scopes. For a Resolve-style grade (nodes, primaries, log/HDR wheels, warper, qualifiers, windows, groups, match) see help grade.
lumetri params: exposure(stops) contrast highlights shadows whites blacks temp tint (-100..100) sat(%) vibrance fade stint htint (split-tone colors, alpha=strength).
Other color fx: primaries (Resolve lift/gamma/gain/offset YRGB, contrast/pivot, sat, hue, lum mix, color boost, MD), logwheels, hdrwheels, colorwarper, colorslice, cst, lut (file.cube), curves (master='0,0 .25,.2 .75,.85 1,1'), colorwheels (lift/gamma/gain), huecurves, secondary (HSL key), channelmixer, leavecolor, changecolor, photofilter, tritone, autolevels, legalize, levels, hue, saturation, vibrance, temperature, tint, duotone, bc, exposure, grayscale, sepia, invert, posterize.
Measure with preview scope=all (help scopes); copy a grade with paste <from> <to> color.
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
        "masks" | "mask" | "roto" | "matte" | "keying" | "key" | "qualifier" | "qualifiers" | "windows" | "window" => {
            let mut s = format!("{MASKS}\n");
            s.push_str(&props_section("Mask props (static ones can't be keyframed)", schema::MASK));
            s
        }
        "tracking" | "track" | "tracker" | "camtrack" | "camera-tracking" | "3dtrack" | "planar" => TRACKING.to_string(),
        "grade" | "grades" | "grading-nodes" | "node" | "nodes" | "resolve" | "matching" | "match" | "stills" | "colormanagement" => GRADE.to_string(),
        "mix" | "mixing" | "mixer" | "buses" | "bus" => MIX.to_string(),
        "voice" | "voices" | "tts" | "voiceover" | "voice-over" | "clone" | "cloning" | "omnivoice" | "narration" => VOICE.to_string(),
        "tools" | "panels" | "premiere" | "aftereffects" | "ae" | "pro" => TOOLS.to_string(),
        "3d" | "camera" | "cameras" | "null" | "nulls" | "parallax" => THREE_D.to_string(),
        "scopes" | "scope" | "lumetri" | "waveform" | "vectorscope" => SCOPES.to_string(),
        "time" | "times" | "units" | "refs" => TIME.to_string(),
        "props" | "properties" => {
            let mut s = String::from("Static props (static) can't be keyframed. Use with add/set/kf.\n");
            s.push_str(&props_section("Every layer", schema::LAYER));
            s.push_str(&props_section("Media clip", schema::CLIP));
            s.push_str(&props_section("Text", schema::TEXT));
            s.push_str(&props_section("Solid", schema::SOLID));
            s.push_str(&props_section("Nested comp", schema::NESTED));
            s.push_str(&props_section("Camera (camera layers; also pos z rx ry rot)", schema::CAMERA));
            s.push_str(&format!("Blend modes: {}\nNull layers have only the layer props.", schema::BLEND_MODES.join(" ")));
            s
        }
        "effects" | "fx" | "video" => {
            let mut s = String::from("fx <id> <effect> [param=value...]  (params are animatable via kf <id>.fN <param> ...). Spatial params in comp px. Effects run top to bottom; reorder with fx <id.fN> up|down.\nAfter Effects / Premiere names work as aliases (gaussian blur, keylight, fractal noise, turbulent displace, venetian blinds, motion tile...). [transition] effects hide the layer as completion goes 0->100; [time] effects change when frames are taken.\n");
            let mut cat = "";
            for e in schema::EFFECTS {
                if e.cat != cat {
                    cat = e.cat;
                    s.push_str(&format!("[{}]{}\n", cat, if e.engine == Engine::Ff { " decode-time, video clips only, static params" } else { "" }));
                }
                let ps: Vec<String> = schema::effect_params(e).iter().map(|p| format!("{}={}", p.name, short_def(p))).collect();
                let al = if e.alias.is_empty() { String::new() } else { format!(" (aka {})", e.alias.join(" ")) };
                s.push_str(&format!(" {}{al}: {}. {}\n", e.name, e.desc, ps.join(" ")));
            }
            s
        }
        "audio" | "afx" | "sound" => {
            let mut s = String::from("afx <clip|A1|master> <effect> [params]. Edit one: afx <c1.a1|A1.a1|master.a1> param=v | del | on | off | up | down (audio effect params are static; automate level with kf c1 vol ...); afx <target> clear removes all. Clip props: vol(dB, keyframable) pan mute fadeIn fadeOut. Track: track set A1 vol=-3 pan=.2 mute=1 solo=1.\nDucking: duck A2 by=A1 amount=-12 attack=20 release=400. Master: master vol=-1 lufs=-14 limiter=1.\n");
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
        "shapes" | "shape" => {
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
        "color" | "grading" => {
            let mut s = format!("{COLOR}\n");
            s.push_str(&presets::COLOR_PRESETS.iter().map(|(n, d)| format!("{n} ({d})")).collect::<Vec<_>>().join(", "));
            s
        }
        "expr" | "expressions" => EXPR.to_string(),
        "render" | "export" | "presets" => {
            let mut s = String::from("render tool: preset=<name> [out=path] [range=a-b] [scale=0.5] [hw=auto (default: the best GPU)|off (CPU)|nvenc|qsv|amf] [wait=true]. Output defaults to <Videos>/MoodCut Renders. Resolution = comp size × scale.\n");
            for (n, d) in PRESETS {
                s.push_str(&format!(" {n}: {d}\n"));
            }
            s.push_str("srt/vtt/chapters/xml/edl are written instantly (no encode). xml keeps cuts, tracks, source in/out and markers with links to the original media; HTML graphics come along as media files; text, shapes, solids and precomps don't translate, so also render those as a prores4444 overlay (hide V1 first). A finished delivery for a client: the video + srt + chapters (+ xml if they finish in Premiere/Resolve).\n");
            s
        }
        "analyze" | "analysis" => {
            let mut s = String::from("analyze tool: target=<media|clip> kind=<kind> [params]. Results are cached and power the macros.\n");
            for (n, d) in crate::analyze::KINDS {
                s.push_str(&format!(" {n}: {d}\n"));
            }
            s
        }
        "examples" | "example" => EXAMPLES.to_string(),
        "style" | "craft" | "professional" | "rules" => super::craft::STYLE.to_string(),
        "direction" | "director" | "creative" | "creative-direction" | "art-direction" | "brief" | "concept" | "treatment" | "review" | "grounding" => super::direction::DIRECTION.to_string(),
        "features" | "feature" | "feature-map" | "when" | "which" | "usage" | "capabilities" => super::direction::FEATURES.to_string(),
        "recipes" | "formats" | "reels" | "youtube" | "templates-edit" => super::craft::RECIPES.to_string(),
        "platforms" | "platform" | "specs" | "social" | "safezones" => super::craft::PLATFORMS.to_string(),
        "sfx" | "sounddesign" | "sound-design" | "whoosh" => super::craft::sound_help(),
        "brand" | "brandkit" | "branding" => super::craft::brand_help(),
        "graphics" | "graphic" | "html" | "gfx" | "design" => GRAPHICS.to_string(),
        "workflow" | "pipeline" | "recipe" | "start" => WORKFLOW.to_string(),
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

/// `view what=mix`: routing, sends, chains, automation and measured levels.
pub fn mix_view(p: &crate::model::project::Project, comp: &str, levels: Option<Vec<(String, crate::render::audio_scope::Stats)>>) -> String {
    let Some(c) = p.comp(comp) else { return format!("no comp {comp}") };
    let fx = |v: &[crate::model::project::Effect]| v.iter().map(|e| if e.on { format!("{}.{}", e.id, e.kind) } else { format!("({}.{})", e.id, e.kind) }).collect::<Vec<_>>().join(" > ");
    let sends = |v: &[crate::model::project::AuxSend]| v.iter().map(|s| format!(" send {} {}dB{}", s.to, crate::model::value::fmt_num(s.db), if s.pre { " pre" } else { "" })).collect::<String>();
    let auto = |a: &std::collections::BTreeMap<String, crate::model::anim::Anim>| a.iter().filter(|(_, x)| x.is_animated()).map(|(k, x)| format!(" {k}:{}keys", x.k.len())).collect::<String>();
    let mut out = vec![];
    for t in c.tracks.iter().filter(|t| t.kind == crate::model::project::TrackKind::Audio || !t.afx.is_empty()) {
        out.push(format!(
            "{} vol {}dB pan {} -> {}{}{}{}{}",
            t.id,
            crate::model::value::fmt_num(t.vol),
            crate::model::value::fmt_num(t.pan),
            t.out.as_deref().unwrap_or("master"),
            sends(&t.sends),
            auto(&t.auto),
            t.duck.as_ref().map(|d| format!(" ducks under {}", d.by)).unwrap_or_default(),
            if t.afx.is_empty() { String::new() } else { format!(" fx {}", fx(&t.afx)) }
        ));
    }
    for b in &c.buses {
        out.push(format!(
            "{}{} vol {}dB pan {} -> {}{}{}{}{}",
            b.id,
            b.name.as_ref().map(|n| format!(" ({n})")).unwrap_or_default(),
            crate::model::value::fmt_num(b.vol),
            crate::model::value::fmt_num(b.pan),
            b.out.as_deref().unwrap_or("master"),
            sends(&b.sends),
            auto(&b.auto),
            if b.muted { " muted" } else { "" },
            if b.afx.is_empty() { String::new() } else { format!(" fx {}", fx(&b.afx)) }
        ));
    }
    let m = &c.master;
    out.push(format!("master vol {}dB lufs {} limiter {}{}{}", crate::model::value::fmt_num(m.vol), m.lufs.map(crate::model::value::fmt_num).unwrap_or_else(|| "off".into()), m.limiter, if m.truepeak { " truepeak" } else { "" }, if m.afx.is_empty() { String::new() } else { format!(" fx {}", fx(&m.afx)) }));
    if let Some(l) = levels {
        out.push("measured:".into());
        out.extend(l.iter().map(|(n, s)| crate::render::audio_scope::line(n, s)));
    }
    out.join("\n")
}

/// `view what=grade [id=c1]`: each clip's color node list with windows and mix, and shared grades.
pub fn grade_view(p: &crate::model::project::Project, id: Option<&str>) -> String {
    use crate::model::project::ItemKind;
    let mut out = vec![];
    let node = |e: &crate::model::project::Effect| {
        let g = |k: &str| e.props.get(k).map(|a| a.v.str()).filter(|s| !s.is_empty() && s != "1");
        let mut s = format!("{}.{}", e.id, e.kind);
        if let Some(m) = g("mask") {
            s.push_str(&format!("[{m}]"));
        }
        if let Some(m) = e.props.get("mix").map(|a| a.v.num()).filter(|m| *m < 0.999) {
            s.push_str(&format!(" mix {}", crate::model::value::fmt_num(m)));
        }
        if let Some(t) = g("tag") {
            s.push_str(&format!(" ({t})"));
        }
        if let Some(n) = g("node") {
            s = format!("{n}:{s}");
        }
        if e.props.get("parallel").is_some_and(|a| a.v.bool()) {
            s = format!("|| {s}");
        }
        if !e.on {
            s = format!("({s} off)");
        }
        s
    };
    for c in &p.comps {
        for t in &c.tracks {
            for it in t.items.iter().filter(|i| matches!(i.kind, ItemKind::Clip | ItemKind::Adjust) && id.is_none_or(|x| x == i.id)) {
                // the node graph, numbered as `node <clip> N` addresses it
                let nodes = crate::ops::nodes::describe(it);
                let input = it.props.get("input").map(|a| a.v.str()).filter(|s| s != "auto");
                if nodes.is_empty() && it.grade.is_none() && it.pregrade.is_none() && input.is_none() {
                    continue;
                }
                let masks: Vec<String> = it.masks.iter().map(|m| format!("{} {}{}", m.id, m.kind, if m.props.get("cut").is_some_and(|c| !c.v.bool()) { " window" } else { "" })).collect();
                let mut head = format!("{}{}", it.id, input.map(|i| format!(" input={i}")).unwrap_or_default());
                if let Some(g) = &it.pregrade {
                    head.push_str(&format!(" pre-clip group {g}"));
                }
                if let Some(g) = &it.grade {
                    head.push_str(&format!(" post-clip group {g}"));
                }
                if !it.versions.is_empty() {
                    head.push_str(&format!(" versions {}", it.versions.keys().cloned().collect::<Vec<_>>().join(" ")));
                }
                let mut block = vec![format!("{head}:")];
                if nodes.is_empty() {
                    block.push("  (no nodes)".into());
                }
                block.extend(nodes.iter().map(|n| format!("  {n}")));
                if !masks.is_empty() {
                    block.push(format!("  masks {}", masks.join(", ")));
                }
                out.push(block.join("\n"));
            }
        }
    }
    for (n, fx) in &p.grades {
        out.push(format!("grade {n}: {}", fx.iter().map(node).collect::<Vec<_>>().join(" > ")));
    }
    for (n, s) in &p.stills {
        out.push(format!("still {n}: {} at {}s", s.item, crate::model::time::fmt_t(s.t)));
    }
    if out.is_empty() { "no color work yet".into() } else { out.join("\n") }
}
