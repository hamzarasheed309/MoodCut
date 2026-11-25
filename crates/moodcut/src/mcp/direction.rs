//! The creative layer for AI clients: how a top creative director turns a request into a
//! designed, edited piece with MoodCut, and when (and when not) to reach for each feature.
//!
//! Served by `help direction` and `help features`. Every `backticked` example below is a real
//! command: a test runs each script line against a fixture project and checks each tool call's
//! arguments against the MCP tool schemas, so this guide can't teach a command that doesn't exist.

/// `help direction`: the creative director's process, design and editing direction, review, and
/// the rules that keep an AI from claiming what it hasn't done or seen.
pub const DIRECTION: &str = "Creative direction: run every MoodCut job like a top creative director. This decides WHAT to make and WHY; help features says WHICH tool serves each decision, help style has the craft numbers, help recipes complete scripts, help <topic> exact syntax.

GROUND RULES (they keep the work honest; they come before taste)
1 Real commands only. Every command and property must appear in help edit or the topic that covers it (help masks, tracking, grade, mix ...). An unknown word fails the whole script, nothing changes, and the error names the word with a suggestion: fix that word; never invent a workaround or a property.
2 Ids come from replies. m#, c#, t# ... are exactly what MoodCut printed (+c12 created c12); $ is the last item created. Never guess an id: `view what=timeline detail=1` lists them.
3 Describe only what you have seen. Look before you name what is in a shot (`preview media=m3 n=6`), read speech before you quote it (`analyze target=m1 kind=transcript`, then `view what=search q=launch`). Colors, faces, text on screen, moods: only after a preview.
4 Measure, then change. Exposure, balance and skin from `preview t=4.2 scope=all` and `preview t=4.2 pick=0.5,0.4`; loudness and craft problems from check; tracking quality from the tracker's reply (lost ranges, solve error). Numbers in your report come from replies, never from estimates.
5 Verify every claim. After each build step look at what changed (`preview n=12`, or t= for one moment); run check before every render (`check target=reels expect={dur:30,size:'1080x1920'}`). Report as done only what a reply or a frame confirmed, and list what you could not verify.
6 Missing is missing. If MoodCut has no tool for something (generating new footage, AI rotoscoping or face replacement, a font that isn't installed, music it wasn't given), say so plainly and offer the closest real route (a mask with tracking, a graphic, a different font). Never describe an effect as applied when no command applied it.
7 Facts belong to the person, taste belongs to you. Names, titles, prices, claims, dates, logos, legal lines and brand colors come from the brief or the footage; ask if they are missing and essential. Creative choices (structure, look, type, motion, music placement) are yours: make them confidently and explain them in one line each.

THE DIRECTOR'S LOOP
BRIEF: answer in one line each: who is it for; where it plays (platform, screen, sound on or off); what they should feel; what they should do or remember. Unknowns that change the whole piece (format, length, the message) get one question; everything else gets a sensible default you state.
CONCEPT: one idea a viewer could repeat in five words (\"the product as the hero\", \"a day compressed into a breath\", \"the quiet before the launch\"). Each later choice must serve it; whatever doesn't is cut, however good it looks.
TREATMENT (write it before building, five lines): format and length | structure beats with timings (hook 0-2 s, build, turn, payoff, CTA or loop) | look (palette in three colors, contrast, texture) | type and motion vocabulary (one family, two weights, 2-3 moves reused everywhere) | sound (music feel, where it lifts, the sound-design palette).
BUILD: bottom-up per help workflow (picture and story, then b-roll, then type and graphics, captions last; then color, then sound). Set the frame first (`comp new 'Reel' 1080x1920 30`) and the brand (`brand bold`, or the client's font and colors). One decision per layer.
REVIEW: watch it twice. First as the audience: `preview n=12`, would they stop at frame one, is anything confusing, is there a dead spot? Then as the client: is the message clear, is the brand right, is every fact correct? Run check for the platform and fix its problems and craft notes. Change the three things that hurt the concept most, then review again.
DELIVER: render the preset for the platform (`render preset=social wait=true`), plus deliverables that matter (captions file, chapters, a vertical cut).
REPORT: what you made and its length; the concept in one line; the key choices and why; what check and the previews confirmed; anything left for a human (rights to music and footage, approvals, facts you could not verify).

DESIGN DIRECTION (what makes a frame look designed, not assembled)
HIERARCHY: one thing reads first. Headline at least twice the size of the support line, one accent color, everything else neutral. When two elements compete, shrink, move or delete one.
COMPOSITION: subjects and titles on the thirds or the subject's eye line (`align t5 left to=safe`); leave space in the direction a subject looks or moves; margins 5-7% (inside platform UI zones on social, 10% for broadcast). Keep faces clear of text.
COLOR: 60-30-10: a dominant neutral from the footage, a secondary brand color, an accent for the single thing that matters. Grade the footage toward the palette (warm palette, warmer highlights) instead of fighting it. Text needs 4.5:1 contrast with what is behind it (a box, a shadow, a darker grade under it).
TYPE: one family, at most two weights, sentence case for reading, caps with wide tracking for labels. Short lines, two at most. Heavy, tight-tracked headlines (`text 'Launch day' at=1 dur=3 size=110 weight=800 tracking=-2 enter=slamIn exit=fadeOut`) and lighter support lines. On screen at least 0.3 s per word plus 1 s.
MOTION: motivated: things move because something happened (a beat, a word, a cut). Ease everything that starts or stops (`kf t5 scale 0:0.92 0.5:1@outExpo`); exits faster than entrances; one direction per story beat; hold a finished reveal 1-2 s. The same moment gets the same move every time.
TEXTURE: choose on purpose: grain and vignette for cinematic and documentary (`fx V1 noise amount=0.05`, `fx c1 vignette amount=0.3`), clean for tech and commercial. Never stack glow, shadow, stroke and gradient on one title.
LOOK: correct first (exposure, balance, matching shots), then ONE look for the whole piece (`color V1 kodak`), skin protected. A look that changes shot to shot reads as a mistake.

EDITING DIRECTION (what makes a cut feel inevitable)
STORY: every shot must answer \"why this, why now?\". If you can't say what it adds, cut it. Start late, leave early: enter a shot on its action, leave before it settles.
STRUCTURE: social: hook (0-2 s) -> one idea -> payoff -> loop or CTA. YouTube: cold open that promises the payoff -> chapters -> payoff -> end screen. Brand film: tension -> turn -> release. Interview: question implied by the first answer, answers ordered as an argument, b-roll as evidence.
RHYTHM: vary shot lengths (long-short-short-long), accelerate into the payoff, breathe after it. Cut on action, on the beat, on a breath. Music edits land on phrases; the drop gets the strongest image (`speedramp V1 at=3 to=4x dur=0.5` into it).
CONTINUITY OR IMPACT: match action and eye lines when the story should flow; break them (jump cuts `autocut silence c1` then `punchin V1 scale=1.12`, a smash cut, a whip) only when energy should spike.
SOUND LEADS PICTURE: J and L cuts between answers (`jcut c3 by=0.4`), sound design on every move that should be felt (`sfx whoosh at=20`), music ducked under speech (`duck A2 by=V1 amount=-12`), roles set so the mix and check know what is what (`sound c1 dialogue`), a deliberate ending (music resolves or fades on a phrase), loudness for the platform (`master lufs=-14 limiter=1`).
SPECIFICITY: the strongest edits are built from specific moments (a laugh, a hand, a look) found in transcripts and filmstrips, not from the first few seconds of each clip.

REVIEW RUBRIC (score 1-5 each; anything under 4 gets fixed before delivery)
Hook: frame one and the first two seconds earn attention. Clarity: the message survives sound off and a glance. Rhythm: something changes every 3-5 s on social, nothing drags anywhere. Craft: type readable, safe zones clear, easing on every move, no flash frames or gaps (check). Consistency: one look, one type system, the same moves for the same moments. Sound: dialogue clear, music under it, effects felt not noticed, loudness on target. Truth: every fact, name and claim on screen traceable to the brief or the footage.";

/// `help features`: when to use each feature, how (real commands), how to check it worked, and
/// when not to use it.
pub const FEATURES: &str = "Feature map: when to reach for each MoodCut feature, the command that does it, how to verify it, and when NOT to use it. Examples use c1 = the main clip on V1, c2 = b-roll above it on V2, c3 = the next shot on V1, c4 = music on A2, t5 = a title, m1-m9 = media; always use the ids from your own replies. Exact syntax: help <topic>.

STRUCTURE AND TIMING
Assemble: use for any story; `append m1[2-10] m2[0-4] m1[14-22]` lays clips end to end, `add m2[1-3] V2 at=9 fit=fill` places b-roll over speech starting on the word it shows. Verify: `view what=timeline detail=1`. Not: dumping whole clips; pick in and out points from filmstrips (`preview media=m2 n=8`) or transcripts.
Trim and refine: tighten every cut (`trim c1 start=+1 end=-0.5`, `cut c1 at=8`); move an edit point without changing length (`roll c1 +0.3`); change what a shot shows without moving it (`slip c3 +0.5`). Not: leaving air at the head or tail of a shot unless it is a deliberate breath.
Pauses and jump cuts: talking heads (`autocut silence c1`), then hide jumps by alternating framing (`punchin V1 scale=1.12`). Not: on emotional answers where the pause is the point.
Selects: `keep m1 0-12.5,15-30` builds a rough cut from source ranges you chose from a transcript.
Speed: slow motion for weight and detail (`speed c2 50%`, best on 50/60 fps footage), ramps into action and drops (`speedramp V1 at=3 to=4x dur=0.5`), a held moment (`freeze c1 at=5 dur=1.5`). Not: random speed changes with no motivation; slow motion on 24/25 fps without interp=blend.
Transitions: the cut is the default. Use one or two motivated types per piece: dissolve for time or memory (`tr V1 dissolve 0.4` sets every cut on V1), whip between places with a whoosh (`tr c1 c3 whip 0.3`), fadeblack for chapter breaks. Verify the handles in check. Not: a different transition on every cut; decorative wipes in professional work.
Markers and chapters: mark beats, drops and chapter starts (`marker add 12 'Chapter 2'`), then deliver YouTube chapters (`render preset=chapters`).
Nesting: group layers that move or get graded together (`precomp c2,t5 name='Insert'`). Not: as a tidy-up for unrelated layers; it hides them from the main timeline.

PICTURE: COMPOSITION, TYPE AND GRAPHICS
Brand kit first: one font, colors and accent drive every title, caption, template and graphic (`brand clean`; or the client's: brand font=, primary=, accent=). Not: picking colors per title.
Titles: short statements and hooks with the type rules above (`text 'Nobody tells you this' at=0.1 dur=1.8 size=96 enter=slamIn exit=popOut`); designed lower thirds for every new speaker (`title lowerthird text='Alex Rivera' sub='Product designer' style=minimal at=3`); data and progress (`title counter from=0 to=48 suffix=' h' at=10 dur=3`, `title progress pos=top`). Verify readability and safe zones with check. Not: paragraphs on screen; text over faces.
Captions: every speech-led social video (people watch muted) and accessibility (`captions c1 style=punch words=2` for creator energy, style=bold words=3 calmer). Captions go on last so they sit on top. Deliver a file for YouTube (`render preset=srt`). Not: two caption styles in one piece; captions in the bottom 25% of vertical video (platform UI).
HTML motion graphics: bespoke designed pieces (kinetic type, data visualizations, branded end cards, UI mockups) that templates can't make: the graphic tool renders HTML/CSS/JS frame-accurately with alpha (`graphic name=endcard dur=4 place='V3 at=22'` with html=...). Put every text and color in a {{field|default}} so people can edit it. Verify with the contact sheet it returns. Not: for a plain title a text layer does well.
Shapes, solids, adjustment layers: accents, bars and frames in the brand color (`shape rect w=520 h=6 pos=960,700 at=4 dur=2 enter=wipeInLeft`), backgrounds (`solid #05060a at=0 dur=3`), one look or effect over everything below (`adjust at=0 dur=26 fx=vignette`).
Layout tools: line up and space elements like a designer (`align t5 left to=safe`), parent things that move together (`null children=t5`). Verify with a preview at the moment they're all on screen.
Reframe: a vertical or square version of a landscape edit (`reframe 9:16`), then fix framing per shot and check target=reels. Not: as a substitute for composing graphics for the new frame.

MOTION
Animation presets: entrances, exits and loops with good default easing (`anim t5 slamIn`, `anim c2 kenburns amount=0.06` on stills and static shots). Choose 2-3 and reuse them. Not: a different preset on every element; kenburns on every shot.
Keyframes and easing: custom moves (`kf c2 scale 0:1 4:1.08@outCubic`); expressions for organic or procedural motion (`expr t5 rot 'wiggle(2,3)'`). Verify by previewing the start, middle and end of the move. Not: linear keys on anything that starts or stops.
3D and cameras: depth, parallax and push-ins on layered designs (`camera at=0 dur=8 move=pushin amount=200 layers=V2`; help 3d). Not: on a single flat video layer where a simple scale does the same.

FINISHING: MASKS, KEYS, TRACKING
Masks (help masks): isolate a region to cut out, reveal or limit an effect: shapes (`mask c1 ellipse x=.5 y=.4 w=.3 h=.4 feather=40`), effects limited to or outside a mask (`fx c1 blur radius=12 mask=-m1` blurs everything outside m1), editable paths and vertices (`mask c1.m1 topath; mask c1.m1 pt 2 corner`). Verify with `preview view=overlay id=c1.m1` (numbered vertices) or view=matte. Not: a hard-edged mask on organic shapes; feather them.
Qualifiers: change one color or tone range (skin, sky, a product color) without a shape (`mask c1 hsl hue=25 hueRange=40 window`); combine with a shape window to contain them. Verify with view=matte. Not: when a shape window alone isolates it.
Keying: green or blue screen (`fx c2 chromakey`), then clean the matte and edges (`fx c2 matteclean size=1`). Verify holes with check and edges with a preview at full width. Not: on footage that wasn't shot for keying; use a mask.
Mask tracking: a window or blur that must follow a moving subject (`track c1.m1 mode=similarity`; mode=perspective for planes like a wall or a screen, which keys the path itself). Verify: the reply names lost ranges; preview them and fix with a keyframed vertex. Not: on a static locked-off shot.
Point tracking: something that must stick to a point (a label, a sticker, an effect center): `track c1 region=.5,.5,.1,.1 to=null`, then parent layers to the null. Stabilize shaky footage: `track c1 region=.5,.5,.2,.2 to=stabilize smooth=1`. Not: stabilizing intentional handheld energy.
Screen and sign replacement: a planar track of a flat surface drives a layer's corner pin (`track c1 corners='.3,.3 .6,.32 .6,.6 .3,.58' to=cornerpin on=c2`). Pick corners on high-contrast edges, clockwise from top-left.
3D camera tracking: text, objects or graphics that must sit in a moving shot's 3D space (signs on buildings, titles on the floor, labels on a landscape). Solve (`camtrack c1`), look at the numbered points (`preview view=points id=c1`), set the floor (`camtrack c1 ground=p1,p2,p3`), attach (`camtrack c1 attach text 'OPEN DAILY' at=p1,p2,p3`). Verify the solve error in the reply (under 0.5 px sticks, above 1 px slides) and preview several moments. Not: on tripod pans with no parallax (a 2D point track or a nodal solve is enough), on shots with heavy motion blur or cuts.

FINISHING: COLOR (help grade, help scopes)
Correction: every shot before any look: log footage gets its input transform (`color c1 input=slog3`), then exposure and balance by the scopes (`color c1 exposure=0.3 contrast=12`). Shots that must match: `match c3 to=c1`, or against a saved reference (`still save hero from=c1; match c3 to=still:hero`). Verify with `preview t=4.2 scope=all` and check (it flags clipping, crushed shadows and shots that don't match).
Look: one creative look across the piece (`color V1 kodak`), saved and shared so new shots follow it (`grade save look from=c1`). Not: a look on uncorrected footage; different looks per shot.
Resolve nodes: precise, layered grading on hero shots: a node per job (balance, contrast, skin, sky, vignette), each with its own key and mix: `node c1 1 primaries gainR=1.03 gammaY=.02`, a window that follows the face (`node c1 2 window add circle x=.5 y=.45 w=.3 h=.4 feather=60`), a qualifier (`node c1 2 qualify hsl hue=25`), a parallel node (`node c1 add parallel`), a layer mixer for a glow (`node c1 add layer composite=softlight name=Glow`). Read the graph with `view what=grade id=c1`. Not: ten nodes on a shot one look already serves.
Effects: purposeful finishing (`fx c1 sharpen amount=0.3` on soft footage, `fx c1 softclip` on harsh highlights, `fx c2 glow radius=30 intensity=0.5` for a dreamy beat). Not: effects as decoration; anything that hides bad footage instead of cutting it.

SOUND (help mix, help sfx, help audio)
Roles and ducking: always set roles and duck music under speech (`sound c1 dialogue`, `sound A2 music`, `duck A2 by=V1 amount=-12`). Verify with check (it measures the real mix).
Music: the emotional engine: choose where it lifts and put the payoff there; cut to its beats (`beats c4`, then a beat-cut sequence on its own track: `montage m2,m3,m4 music=c4 beats=2 track=V3`). End on its ending or fade on a phrase. Not: music that stops dead; a track that fights the dialogue.
Sound design: whoosh on whips and fast moves, hit on slams and drops, riser into reveals (`sfx riser at=19`, `sfx hit at=1`, `sfx swish on=titles vol=-18dB`). Not: the same sound on every cut of a talking head.
Clean-up and mixing: noisy dialogue (`repair V1`), buses for groups with shared processing (`bus add B1 name=Dialogue`), tone shaping (`afx c1 eq bass=3`). Final loudness for the platform (`master lufs=-14 limiter=1`; -16 podcasts, -23 broadcast).

QUALITY AND DELIVERY
Check: after big changes and before every render (`check target=youtube`); fix=true applies safe fixes; craft notes are a senior editor's notes: act on them. Not: rendering with open errors.
Render: the preset for the destination (`render preset=youtube wait=true`, social, prores for a client master), plus deliverables (srt, chapters, xml to finish in Premiere or Resolve).";

#[cfg(test)]
mod tests {
    use super::*;

    /// The backticked spans of a guide.
    pub(crate) fn examples(s: &str) -> Vec<&str> {
        s.split('`').skip(1).step_by(2).collect()
    }

    #[test]
    fn guides_have_balanced_backticks_and_many_examples() {
        for g in [DIRECTION, FEATURES] {
            assert_eq!(g.matches('`').count() % 2, 0, "unbalanced backticks");
        }
        assert!(examples(DIRECTION).len() >= 20, "{}", examples(DIRECTION).len());
        assert!(examples(FEATURES).len() >= 60, "{}", examples(FEATURES).len());
    }
}
