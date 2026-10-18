//! What separates a produced video from an assembled one, written as rules an AI can act on:
//! every rule names the command that carries it out. Served on demand through `help`.

use crate::ops::pro::BRAND_PRESETS;
use crate::sfx;

pub const STYLE: &str = "Edit and animate like a senior editor and motion designer. Generic output comes from defaults; every rule below is a command.
DECIDE FIRST (one line each, then build): platform + length (help platforms) | structure: hook -> setup -> payoff -> CTA or loop | brand (brand <preset> or the client's font/colors) | ONE look for the whole piece | a motion vocabulary of 2-3 moves reused everywhere (e.g. wipeIn lines + slamIn headlines + whip transitions) | a sound palette (music + whoosh/hit/riser) | where the music's drop or chorus lands. Start from the closest help('recipes').
HOOK (the first 1-2 s decide retention): open on the strongest image or claim, never a logo or a slow intro; a text hook within the first 6 frames (text 'X' at=0.1 enter=slamIn + sfx hit at=0.1, or enter=wordPop); motion from frame 1 (punch, whip, speedramp, camera pushin); music starts on a strong beat or a phrase; no fade from black on social.
PACING: average shot length Reels/TikTok/Shorts 0.8-2 s, YouTube talking head 3-6 s with b-roll every 4-8 s, cinematic montage 1.5-4 s on the beat (every 2 or 4 beats, every beat after the drop). Something new every 3-5 s on social (punch-in, b-roll, text pop, sound cue). Vary shot size (wide / medium / close); never the same framing twice in a row. Trim hard: start a shot on its action, leave before it settles.
CUTS: on action, on the beat (beats <music>, then at=@b12), on a transient or a breath. Jump cuts: autocut silence <clip> then punchin V1 (alternating 100/112%). Interviews and dialogue: J/L cuts (jcut <next clip> by=0.4) so sound leads picture. B-roll over speech starts on the word it illustrates: add m5[2-5] V2 at=<word time>.
TRANSITIONS: the cut is the default. At most one or two motivated types per video: whip (+ sfx whoosh) between places or topics, fadeblack for time jumps, dissolve for mood or memory, zoom or glitch only for energetic brands. No cube, spin, blinds or checker in professional work unless asked for playful. 0.3-0.5 s, covered by sound (sfx whoosh on=transitions).
SPEED: ramp into action and into drops (speedramp V1 at=@drop-0.5 to=4x dur=0.5), slow motion after (to=40%); slow motion only on 50/60 fps footage or with interp=blend.
SOUND (the biggest amateur/pro difference, help sfx): music ducked under speech (sound <talk> dialogue + sound <music> music, or duck A2 by=A1 amount=-12); whoosh on whips, transitions and fast moves; swish on text and graphics sliding in; hit or boom on title slams and the drop; riser ending on a reveal; pop or click for UI, stickers and counters; ding for checkmarks; typing under typewriter text; shutter on freeze frames. SFX sit under dialogue (-10..-18 dB). End on the music's ending or fade at a phrase (fadeOut 1.5-3), never a hard stop. master lufs=-14 limiter=1.
COLOR: correct before you style (exposure, white balance, contrast per shot; help scopes), then ONE look for everything (color V1 kodak), match shots (paste c1 c2..c9 color), protect skin (fx c1 secondary hue=25 sat=1.05). Texture on purpose: grain 0.04-0.08 + vignette 0.2-0.35 for cinematic, none for commercial or tech.
TYPOGRAPHY: one family (the brand font), at most two weights, one accent color. Headline 7-11% of frame height, heavy, tracking -1..-3; support line 40-55% of the headline, lighter; labels in caps with wide tracking (+8..+20). Short lines (<= 28 characters vertical, <= 42 landscape), at most 2 lines; left-aligned for information, centered only for single statements. On screen >= 0.3 s per word + 1 s.
MOTION: ease everything that starts or stops. Entrances outExpo or outQuint, 0.3-0.6 s; exits inCubic or inExpo and faster (0.2-0.4 s); overshoot (outBack, popIn) only for playful pops; linear only for constant drift, rotation, progress bars and tickers. Easy ease on manual keyframes (ease <id> all easy). Stagger 0.03-0.06 s per character, 0.08-0.12 s per word or line, 0.1-0.15 s per element (stagger <ids> by=0.1). Move along the direction of the story; enter and exit on the same axis. Motion blur on fast moves (mblur=1, whipIn). Depth with 3D layers + camera move=pushin; kenburns 6-10% on stills, never 20%. Hold a finished reveal 1-2 s before the next move.
LAYOUT: margins 5-7% (social: inside the platform safe zone), align titles to a third or the subject's eye line (align <ids> left to=safe), keep faces clear, contrast >= 4.5:1 behind text (bg=, a dark box, a vignette, or a shadow).
CONSISTENT = DESIGNED: the same transition, title entrance, lower third, caption style and sound for the same kind of moment across the whole video. Different every time reads as random.
GENERIC TELLS TO AVOID: default colors with no brand; a different transition on every cut; text that pops in and out without easing; music that stops dead; a logo intro over 1 s; kenburns on every shot; zooms that land on nothing; captions over faces or platform buttons; stacked effects (glow + shadow + stroke + gradient); the same sound on 20 cuts of a talking head (sfx on=cuts is for montages).
REVIEW AS A VIEWER: preview n=12 and ask: would I stop scrolling at frame 1? Does something change every 3-5 s? Is any text unreadable or too short? Does the music land on the cuts and the end? Then check target=<platform> and fix every craft note.";

pub const PLATFORMS: &str = "Platform specs. Set the comp first (comp new 'Reel' 1080x1920 30, or reframe 9:16 an existing edit), build, then check target=<platform> (it flags text in the platform's UI zones, slow hooks and pacing).
INSTAGRAM REELS / TIKTOK / YOUTUBE SHORTS (target=reels | tiktok | shorts): 1080x1920, 30 fps (60 for sport and gaming), best 9-35 s (Reels up to 90 s, Shorts 60 s). Hook in 1-2 s; payoff before 70% of the length; end on a loop (the last frame flows into the first) or a 1-2 s CTA.
 The app's UI covers the top ~12% (tabs, handle), the bottom ~25% (caption, audio, comments) and the right ~15% below mid-height (like/share). Keep text and faces in x 6-85%, y 14-72%: captions at y 62-70% (captions defaults there on vertical comps), headlines at y 20-35%.
 Burned-in captions always (people start muted): captions V1 style=punch words=2 (creator energy) or style=bold words=3.
 -14 LUFS, true peak -1 dB (master lufs=-14 limiter=1). render preset=social.
YOUTUBE (target=youtube): 1920x1080 (3840x2160 for 4K), 24/25/30 fps (60 for gaming and screen recordings). A 5-15 s cold open that promises the payoff; title card <= 3 s or none; b-roll or a graphic every 5-10 s over a talking head; a lower third for every new speaker; chapters from markers (marker add T 'Chapter', render preset=chapters); last 15-20 s keep the lower half and right side free for end-screen elements. Captions burned in for shorts-style content, else render preset=srt. -14 LUFS. render preset=youtube.
FEED VIDEO (Instagram / LinkedIn / X / Facebook, target=social): 1080x1350 (4:5) or 1080x1080, 15-90 s, plays muted: captions plus text-led storytelling, the brand visible early.
ADS: product or claim within 3 s, brand by 5 s, one message, a 2-3 s CTA end card; cut 6 s / 15 s / 30 s versions (9-16 shots in 15 s).
BROADCAST / CLIENT MASTER (target=broadcast): 1920x1080 at 25 or 29.97, -23 LUFS (EBU R128; ATSC -24), legal levels (fx V1 legalize), 10% title safe (check safe=0.1), render preset=prores.
COVERS / THUMBNAILS: preview t=<moment> width=1280 to choose a cover frame; on social the first frame is the cover, so make it read as one.";

pub fn sound_help() -> String {
    let mut s = String::from(
        "sfx <kind> at=T[,T2..] | on=cuts|transitions|titles|markers[:prefix]|beats|<ids> [dur=] [vol=-10dB] [pitch=1.2] [track=V1] [same]
Synthesized sound design (no sample library needed). Each instance is a slightly different variation (same=1 for identical copies), and the clip is placed so its moment lands exactly on T: a whoosh's peak on the cut, a riser's end on the reveal, a hit's attack on the slam. Sounds go on audio tracks above dialogue and music, at a level that sits under speech; adjust with vol= or set <clip> vol=.
on=: cuts (every cut on track=, default V1: montages), transitions (every transition), titles (every text / graphic / title template above V1 starting), markers or markers:b (markers whose name starts with b), beats (markers from `beats`), or ids (their start).
Pairings: whip / whipIn / fast push -> whoosh | text or graphic sliding in -> swish | slamIn title, beat drop, hard cut to black -> hit (+ boom for big moments) | before a reveal or the drop -> riser at=<reveal> (it ends there) or reverse | after a big hit -> downlifter | icons, stickers, captions emphasis -> pop | cursor, toggles, counters -> click | checkmarks, success -> ding | typewriter text -> typing dur=<typing time> | freeze frame / photo -> shutter | glitch transitions -> glitch.
Kinds:\n",
    );
    for k in sfx::KINDS {
        let when = if k.sync <= 0.0 { "starts on T".to_string() } else if k.sync >= 1.0 { "ends on T".to_string() } else { format!("peaks on T ({}% in)", (k.sync * 100.0).round()) };
        s.push_str(&format!(" {} ({:.2} s, {} dB, {when}{}): {}\n", k.name, k.dur, k.vol, if k.aliases.is_empty() { String::new() } else { format!("; aka {}", k.aliases.join(" ")) }, k.desc));
    }
    s
}

pub fn brand_help() -> String {
    let mut s = String::from(
        "brand [preset] [font=] [weight=] [primary=] [accent=] [text=] [dark=] | brand clear | brand (shows it; view shows it too).
Set it first. New text (font, headline weight, color), captions (font; the spoken word in the accent), title templates (color = accent, font), shapes (fill = accent) and HTML graphics default to it; explicit params still win. HTML graphics get CSS variables: font: var(--brand-weight) 7vh var(--brand-font); color: var(--brand-text); background: var(--brand-primary); accents var(--brand-accent); dark boxes var(--brand-dark).
Use the client's font and colors when the brief gives them (the font must be installed: help fonts; Inter 100-900 always is). Otherwise pick the preset that fits the brief:\n",
    );
    for (n, use_, v) in BRAND_PRESETS {
        s.push_str(&format!(" {n} ({use_}): font={} weight={} primary={} accent={} text={} dark={}\n", v[0], v[1], v[2], v[3], v[4], v[5]));
    }
    s.push_str("One accent color used sparingly (the key word, a line, the active caption word) looks designed; three accents look generic.");
    s
}

/// Complete recipes for the formats people ask for. Media ids: m1 the main talking head or interview,
/// m2-m6 b-roll, m7 a photo, m8 music, m9 a logo (png). Every line is checked by a test.
pub const RECIPES: &str = "Recipes for professional formats. Each is a starting point: use your real media ids, trim points from filmstrips and transcripts, and the times from replies. Assumed media: m1 talking head / interview, m2-m6 b-roll, m7 photo, m8 music, m9 logo (png). Read help('style') for the why.

## reel-talk: Instagram Reels / TikTok / Shorts from a talking head (creator style)
# build bottom-up: picture, b-roll, titles, captions last (they go on top)
comp new 'Reel' 1080x1920 30
brand bold
add m1 V1 at=0 fit=fill
autocut silence V1
# alternate wide / close on every jump cut
punchin V1 scale=1.15
# a b-roll insert every few seconds, each with a whoosh
add m2[1-3] V2 at=4 fit=fill
anim $ zoomIn dur=0.35
sfx whoosh at=4
add m3[5-7] V2 at=9 fit=fill
anim $ whipInRight
sfx whoosh at=9
color V1,V2 commercial
# hook on frame 3, with an impact
text 'Nobody tells you this' at=0.1 dur=1.8 size=96 pos=540,500 enter=slamIn exit=popOut
sfx hit at=0.1
captions V1 style=punch
add m8 A2 at=0 vol=-8dB
sound V1 dialogue
sound A2 music
master lufs=-14 limiter=1
# then: check target=reels and render preset=social

## youtube-talk: YouTube talking head with b-roll, lower third, chapters
brand clean
add m1 V1 at=0
autocut silence V1
punchin V1 scale=1.12 every=3
color V1 natural
title lowerthird text='Alex Rivera' sub='Product designer' style=minimal at=3
add m2[3-7] V2 at=8
anim $ kenburns amount=0.06
sfx whoosh at=8 vol=-16dB
add m3[0-4] V2 at=17
anim $ kenburns amount=0.06
marker add 0 'Intro'
marker add 13 'The problem'
title chapter num=1 text='The problem' at=13 dur=2.5
sfx swish on=titles vol=-18dB
add m8 A2 at=0 vol=-10dB
sound V1 dialogue
sound A2 music
master lufs=-14 limiter=1
# then: check target=youtube, render preset=youtube and preset=chapters

## montage: cinematic music montage (travel, events, brand film)
brand mono
add m8 A1 at=0 as=song
beats song
montage m2,m3,m4,m5,m6 music=song beats=2 kenburns
color V1 kodak
fx V1 noise amount=0.05
title letterbox ratio=2.39
text 'ICELAND' at=0.6 dur=3 size=150 weight=200 tracking=40 enter=trackIn exit=fadeOut
# a ramp into the drop and a riser that ends on it
speedramp V1 at=3.4 to=4x dur=0.5
sfx riser at=4
sfx boom at=4
tr V1 whip 0.3
sfx whoosh on=transitions vol=-14dB
master lufs=-14 limiter=1

## promo: product / brand promo with kinetic typography (launch film, ad)
comp new 'Promo' 1920x1080 30
comp set bg=#05060a
brand tech
add m8 A1 at=0
text 'Meet' at=0.4 dur=1.4 size=150 enter=fadeUp exit=blurOut
text 'the new standard.' at=1.9 dur=2 size=150 enter=wordPop exit=fadeOut
sfx hit at=1.9
add m2[0-3] V1 at=4
camera at=4 dur=3 move=pushin amount=250 layers=V1
shape rect w=520 h=6 pos=960,700 at=4.4 dur=2.4 enter=wipeInLeft exit=wipeOutRight
text 'Faster. Lighter. Smarter.' at=4.5 dur=2.3 size=64 pos=960,640 enter=slideUp exit=fadeDown
sfx swish at=4.4
add m3[2-5] V1 at=7
title counter from=0 to=48 suffix=' h battery' at=10 dur=3
sfx riser at=13
add m9 V2 at=13 dur=3 pos=960,500 scale=0.5
anim $ slamIn
sfx boom at=13
text 'Available now' at=13.6 dur=2.4 size=56 pos=960,760 enter=fadeUp
color V1 commercial
master lufs=-14 limiter=1

## podcast-clip: vertical podcast or interview clip with a hook and progress bar
comp new 'Clip' 1080x1920 30
brand playful
add m1[30-62] V1 at=0 fit=fill
autocut silence V1
punchin V1 scale=1.18
color V1 natural
text 'The one habit that changed everything' at=0 dur=3 size=74 pos=540,420 boxW=900 enter=wordPop exit=fadeOut
sfx pop on=titles
title progress pos=top
captions V1 style=punch words=2
master lufs=-14 limiter=1

## documentary: interview-led documentary or brand story (restrained, emotional)
brand warm
append m1[2-10] m2[0-4] m1[14-22] m3[1-5]
# sound leads picture into the second answer
jcut c3 by=0.5
title lowerthird text='Mara Okafor' sub='Marine biologist' style=minimal at=1
tr c2 c3 dissolve 0.6
color V1 documentary
fx V1 noise amount=0.04
add m8 A2 at=0 vol=-14dB
sound c1,c3 dialogue
sound A2 music
master lufs=-16 limiter=1

## tutorial: screen recording or how-to (zoom on actions, callouts, clicks)
brand clean
add m1 V1 at=0
anim c1 zoomPunchIn at=4 amount=0.35
sfx click at=4
title callout text='Click Export' x=1500 y=300 at=4.2 dur=2.5
sfx pop at=4.2
marker add 0 'Setup'
marker add 12 'Export'
title chapter num=2 text='Export' at=12 dur=2
sfx swish at=12
add m8 A2 at=0 vol=-22dB
master lufs=-16 limiter=1";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::project::*;
    use crate::ops::{self, Env};
    use anyhow::Result;
    use std::collections::BTreeMap;

    struct Mock;

    impl Env for Mock {
        fn probe(&self, path: &str) -> Result<Media> {
            let audio = path.ends_with(".mp3") || path.ends_with(".wav");
            let image = path.ends_with(".png");
            Ok(Media {
                id: String::new(),
                path: path.into(),
                name: path.rsplit('/').next().unwrap().into(),
                kind: if audio { MediaKind::Audio } else if image { MediaKind::Image } else { MediaKind::Video },
                dur: if image { 0.0 } else { 90.0 },
                w: 1920,
                h: 1080,
                fps: 30.0,
                has_video: !audio,
                has_audio: !image,
                alpha: image,
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
        fn analyze(&self, _: &Project, _: &str, kind: &str, _: &BTreeMap<String, String>) -> Result<serde_json::Value> {
            Ok(match kind {
                "silence" => serde_json::json!({"silences": [[2.0, 3.0], [9.0, 9.5], [16.0, 17.0], [40.0, 41.0]]}),
                "beats" => serde_json::json!({"bpm": 120.0, "beats": (0..180).map(|i| i as f64 * 0.5).collect::<Vec<_>>()}),
                "loudness" => serde_json::json!({"lufs": -20.0}),
                _ => serde_json::json!({"segments": [{"s": 0.5, "e": 60.0, "text": "This is how we did it, step by step, for real."}]}),
            })
        }
        fn sfx(&self, kind: &str, dur: f64, pitch: f64, seed: u64) -> Result<Media> {
            let k = sfx::kind(kind).unwrap();
            let mut m = self.probe(&format!("/sfx/{}", sfx::file_name(k, dur, pitch, seed)))?;
            m.dur = dur;
            Ok(m)
        }
    }

    #[test]
    fn every_recipe_runs() {
        let mut n = 0;
        for block in RECIPES.split("\n## ").skip(1) {
            let (title, body) = block.split_once('\n').unwrap();
            let mut p = Project::new("t", 1920, 1080, 30.0);
            ops::run(&mut p, &Mock, "s1", "import /f/talk.mp4 /f/b1.mp4 /f/b2.mp4 /f/b3.mp4 /f/b4.mp4 /f/b5.mp4 /f/photo.png /f/music.mp3 /f/logo.png").unwrap();
            // one script, as an AI would send it (errors name the line)
            if let Err(e) = ops::run(&mut p, &Mock, "s1", body) {
                panic!("recipe '{title}': {e}");
            }
            n += 1;
        }
        assert_eq!(n, 7);
    }

    #[test]
    fn sound_and_brand_help_list_everything() {
        let s = sound_help();
        assert!(sfx::KINDS.iter().all(|k| s.contains(&format!(" {} (", k.name))));
        let b = brand_help();
        assert!(BRAND_PRESETS.iter().all(|p| b.contains(p.0)));
    }
}
