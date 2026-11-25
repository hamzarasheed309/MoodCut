# Plan: masking, color grading and audio mixing, all driven over MCP

Status: implemented in the working tree on `claude/native-window-launch` (not compiled locally: this PC can't run cargo, so CI is the first build and test run). Written against commit `c495201`. See section 15 for what was built and what is still open.

## 1. Goal

Bring three areas up to the level people expect from the dedicated tools, without breaking what makes MoodCut
fast for an AI to drive:

| Area | Reference tool | What "done" means here |
| --- | --- | --- |
| Masking, roto, tracking, keying | After Effects | Windows on any effect, tracked masks, a matte view the AI can check, a usable keyer |
| Color grading | DaVinci Resolve | Color-managed input, serial nodes (window + qualifier + mix), shot matching, shared grades, measured scopes |
| Audio mixing | Adobe Audition | Buses and sends, automation, a real processor set, measured repair, audio scopes, stems |

Every feature must be reachable from MoodCut Script, so the editor window and MCP stay equal.

### Not in scope

- Full parity with any of the three tools. The target is the part used in most real jobs.
- A node-graph UI, ACES/OCIO config files, HDR mastering, Dolby Vision.
- Surround mixing (5.1/Atmos), VST/AU plug-in hosting, lasso spectral editing.
- A GPU compositor. Compositing stays on the CPU so preview and export keep matching.

## 2. Where the code is today

| Area | Already there | Missing or weak |
| --- | --- | --- |
| Masks ([masks.rs](../crates/moodcut/src/render/masks.rs), `MASK` in [schema.rs](../crates/moodcut/src/model/schema.rs)) | rect, ellipse, polygon, star, path; feather, expand, opacity, invert, four combine modes; path `d` keyframes morph; track mattes (alpha/luma) | No tracking of any kind. Only color effects accept `mask=`. No matte view. Every mask is re-rasterized for every effect on every frame |
| Keying | `chromakey`, `lumakey`, `choke`, `spill` | No matte clip levels, no auto screen color, no light wrap, no matte view |
| Color ([fx_color.rs](../crates/moodcut/src/render/fx_color.rs), [lut.rs](../crates/moodcut/src/render/lut.rs), [scopes.rs](../crates/moodcut/src/render/scopes.rs)) | `lumetri`, wheels, curves, hue curves, HSL secondary, channel mixer, `.cube` LUTs, per-effect `mask` + `mix`, four scopes with a measured readout | No input transform for log or HDR footage. 8-bit RGBA end to end. No shot matching, no shared grades, no reference stills. One LUT pass per effect |
| Audio ([audio.rs](../crates/moodcut/src/render/audio.rs), `AUDIO_EFFECTS`) | One FFmpeg filtergraph: clip gain keyframes, fades, pan, 30 effects, sidechain ducking, measured loudness + limiter, Essential Sound types | No buses or sends. Track volume and pan are static. Effect settings are static with no wet/dry. Reverb is three echoes. No noise-print denoise, no multiband, no audio scopes for the AI |
| MCP ([mcp/mod.rs](../crates/moodcut/src/mcp/mod.rs)) | 10 tools, about 11.6 KB of definitions; `edit` script, `preview scope=`, `analyze`, `check` | No way to look at a matte, sample a pixel, or measure a mix by frequency |

Three things found while reading the code that this plan fixes first (each needs a test to confirm, since
nothing was run):

1. **A grade window also cuts the clip.** In `render_layer` ([comp.rs:1008-1019](../crates/moodcut/src/render/comp.rs#L1008-L1019))
   `apply_masks` runs over every active mask before the effects, so a mask referenced by `color c1 … mask=m1` makes
   the rest of the clip transparent. It only behaves as a window on adjustment layers.
2. **Windowed effects copy the whole layer.** `apply_effect_windowed` ([comp.rs:63-97](../crates/moodcut/src/render/comp.rs#L63-L97))
   clones the buffer, runs the effect on all of it, then blends back, and rasterizes the mask again per effect.
3. **The audio filtergraph goes on the command line.** `-filter_complex <graph>` is passed as an argument
   ([encode.rs:329](../crates/moodcut/src/render/encode.rs#L329), [audio.rs:384](../crates/moodcut/src/render/audio.rs#L384)).
   Windows limits a command line to 32,767 characters, and each clip adds a gain expression, so a long timeline
   can fail to start FFmpeg. Buses and sends make graphs longer.

## 3. Rules every phase follows

These come from [ARCHITECTURE.md](ARCHITECTURE.md) and are what keeps AI editing cheap:

- **One edit path.** Each feature is a script verb or a schema property. Panels write script lines.
- **Schema first.** New parameters go in `schema.rs`, so validation, typo hints, the inspector and `help` come for free.
- **Still 10 tools.** New abilities are verbs, `preview`/`analyze`/`check` options and `help` topics, not new tools.
  Budget: tool definitions stay under 13 KB.
- **Results are ordinary data.** Tracking writes keyframes, matching writes an effect, repair writes an effect
  chain. Everything stays editable and undoable, and old builds can still open the file.
- **Measure, then act.** The AI can't watch or listen, so each area gets a measured readout and `check` rules.
- **Preview equals export.** Any shortcut taken for speed applies to both, or only to `draft` renders.
- **Optional FFmpeg filters are probed** with `ff.has_filter(...)` and have a fallback, as `pitch` and `exciter` do now.

## 4. Phase 0: shared foundations

Small, and everything else builds on it.

| # | Change | Where | Why |
| --- | --- | --- | --- |
| 0.1 | Mask property `cut` (default true). `mask c1 ellipse window` sets `cut=false`; `apply_masks` skips those | `schema.rs`, `masks.rs`, `exec_props.rs` | Fixes finding 1. Existing projects render as before |
| 0.2 | Matte cache: rasterized mask alpha keyed by (mask values at t, transform, buffer rect). A static mask is rasterized once, an animated one once per frame instead of once per effect | new `render/matte.rs` | Fixes half of finding 2 |
| 0.3 | Weighted apply: `Lut3::apply_weighted(pm, mask, amount)` and a region-limited path for other effects (process only the mask's bounding box, blend in place) | `lut.rs`, `comp.rs`, `pixel.rs` | No full-buffer clone; work scales with the window, not the frame |
| 0.4 | Accumulate masks in `u8` with a reused scratch buffer instead of a `Vec<f32>` per mask per frame; feather large radii at reduced resolution as the blur already does | `masks.rs` | Allocation-free hot path |
| 0.5 | Write filtergraphs to a temp file (`-filter_complex_script`, or `-/filter_complex` on FFmpeg 7+, probed) | `ffmpeg.rs`, `encode.rs`, `audio.rs`, `check.rs` | Fixes finding 3 |
| 0.6 | `moodcut bench`: fixed synthetic scenes (lavfi sources), prints fps and audio render time; CI stores the numbers as an artifact | `main.rs`, CI | A baseline before any optimization claim, and a regression alarm after |
| 0.7 | Persist the loudness cache to `<data>/cache` (it is an in-memory map today) | `audio.rs`, `cache.rs` | Re-opened projects don't re-measure the mix |

## 5. Track A: masking, tracking, keying

### A1. Mattes as a first-class thing

- **`mask=` on every in-place native effect**, not only the color category: `fx c1 blur radius=40 mask=m1` blurs a
  face, `fx c1 sharpen mask=m2` sharpens eyes. Geometry effects (`transform`, `cornerpin`, `tile`, `crop`) are excluded.
- **Mask expressions**: `mask=m1+m2` (union), `m1*m2` (intersection), `-m1` (inverse). Parsed once and validated
  at edit time with the existing "has no mask" error.
- **Pixel-driven mask kinds** `hsl` and `luma`: a mask whose alpha comes from the layer's own pixels (hue center
  and range, saturation and luma limits, softness, plus `clean` to blur and shrink the key). This is Resolve's
  qualifier. Combined with a shape (`m1*m2`) it gives "this color, inside this window".
- **Mask placement properties** `offset` and `scale` on all mask kinds, applied after the shape. The tracker
  writes to these, so shape keyframes (roto) and motion (tracking) stay separate.
- **Feather bias** `featherBias` -1..1 (inside to outside).
- **Vertex edits** for path masks: `mask c1.m1 pt 3 0.42,0.31 [at=2.0]` moves one point and keys the path, so
  the AI corrects roto without resending the whole `d` string.

### A2. Tracker (new `track` verb, new `render/track.rs`)

```text
track c1.m1 [range=a-b] [mode=pos|similarity|affine]      mask follows what is under it at the playhead
track c1 region=0.4,0.3,0.1,0.1 to=null                    new null layer with position keys (parent text to it)
track c1 corners='x,y x,y x,y x,y' to=cornerpin on=c5      four points drive c5's cornerpin (screen replacement)
```

- Frames are decoded gray at up to 640 px wide through the existing decoder (the proxy when one is ready).
- Pyramidal normalized cross-correlation, sub-pixel by parabola fit, template refreshed with a drift guard.
  `similarity` and `affine` track a grid of patches inside the region and fit by least squares. Rows run on rayon.
- Runs as an engine job like a render, cached in `Project.analysis` by media, range and region.
- Output is keyframes, simplified with a tolerance so a 10 s track is tens of keys, not 300.
- The reply reports confidence: `~c1.m1 tracked 0-8.4s, 31 keys; !lost 3.2-3.6 (conf 0.41)`, so the AI knows
  where to look and fix by hand.
- Later: `stabilize` from the same tracker (inverse motion as `transform` keys), and two-pass `vidstab` when the
  FFmpeg build has it.

### A3. Keyer

- `chromakey` gains `color=auto` (samples the dominant backdrop), matte `clipBlack`/`clipWhite`, and an edge blur.
- New `lightwrap` effect (background light bleeding onto the edge) and `matteclean` (despeckle).
- Garbage and holdout mattes are just mask expressions from A1.

### A4. Seeing the matte (the MCP half of masking)

- `preview view=matte|overlay id=c1.m1 [n=6]`: the matte as gray, or the mask outline with numbered vertices over
  the picture. This is what lets an AI do roto: look, move points, look again.
- `preview pick=x,y`: RGB, HSL and luma of a 5x5 patch. The AI needs this to set a qualifier or key color.

### A5. Open: automatic subject mattes

"Roto Brush"-style person segmentation needs a neural model and an inference runtime, which the single small
binary doesn't have. See decision D1.

## 6. Track B: color grading

### B1. Speed and quality of what exists

- **Fused grades.** Consecutive bakeable color effects with the same window and mix compose into one 33-point 3D LUT
  (cache key is the joined keys). A five-effect grade costs one lookup per pixel instead of five.
- **Tetrahedral interpolation** in `Lut3::lookup` and the fixed-point path (less hue skew than trilinear).
- **Cache policy.** The LUT cache clears every generated entry at 512. Animated grades produce a new key each frame
  and flush static ones. Change to LRU, and don't cache LUTs whose inputs are animated.
- **Dither** when a grade writes back to 8 bits, to stop banding in skies and gradients.

### B2. Color management

- Clip property `input=`: `rec709` (default), `slog3`, `logc3`, `logc4`, `vlog`, `clog3`, `dlog`, `hlg`, `pq`.
  Each is a shaper plus 3D LUT, which `lut.rs` already supports for `.cube` files.
- Probe `color_transfer` and `color_primaries` in ffprobe and store them on `Media`. HLG and PQ are detected
  automatically. Log profiles are untagged, so `analyze stats` adds a hint ("flat, low saturation: likely log").
- **Decode-time grade (precision without a 16-bit compositor).** When a clip's leading color effects are static
  and unwindowed (the input transform and primaries usually are), write the fused LUT as a `.cube` and apply it in
  the decoder with FFmpeg's `lut3d` on the source's native 10-bit data, before conversion to 8-bit RGBA. The
  log-to-display step, where 8 bits hurts most, then runs at source precision, on FFmpeg's threads, and the
  compositor is unchanged. Animated or windowed effects stay in the compositor.

### B3. Nodes, without a node graph

The effect stack already is a serial node chain. After A1 each color effect has a window, a qualifier and a mix,
which is what a Resolve serial node holds. On top of that:

- **Shared grades** (Resolve's group grade): `grade save day-ext from=c1`, `grade apply day-ext c2..c9`. Stored in
  `Project.grades`, rendered after the clip's own effects. Editing the grade changes every clip that uses it.
- **Timeline grade**: an adjustment layer, as now. Parallel and layer nodes: adjustment layers with a blend mode.
- **Stills**: `still save hero from=c1 [at=T]` keeps a reference frame and the grade. `preview ref=hero split=0.5`
  shows reference and current side by side.

### B4. Matching and automatic correction

- `match c2..c9 to=c1|still:hero [what=exposure,balance,sat]`: measures both sides (luma percentiles, channel means
  in shadows, mids and highlights, mean saturation, on three cached frames per clip), solves lift/gamma/gain per
  channel and saturation, and writes one `colorwheels` effect named `match`. The reply states the remaining difference.
- `color c1 wb=auto` and `color c1 wb=pick:0.4,0.6` (neutral point).
- `preview scope=… region=x,y,w,h` or `mask=c1.m1`: scope readout for a patch (skin, sky, product).
- `preview scope=falsecolor`: exposure zones as colors, which a vision model reads more reliably than a waveform.

### B5. New effects

`softclip` (highlight roll-off), `filmcurve` (S-curve with toe and shoulder), `halation`, range controls on
`colorwheels` (shadow and highlight pivots). Each is a per-pixel function, so it joins the fused LUT for free.

### B6. Open: true 10-bit pipeline and export

See decision D2.

## 7. Track C: audio mixing

### C1. Mixer model

- **Buses**: `bus add B1 name=Dialogue`, `track set A1 out=B1`. Buses carry volume, pan, effects and can feed other buses.
- **Sends**: `send A1 B2 -12dB [pre]` (a shared reverb or parallel compression bus).
- **Automation**: track and bus `vol` and `pan` become animatable (`kf A1 vol 0:-6 4:0`), using the same
  piecewise FFmpeg expression clip gain uses now (`db_expr`). Clip `pan` becomes animatable too.
- **Fade shapes**: `fade c1 in=0.5 curve=linear|log|exp|scurve`; equal-power crossfades for audio transitions.
- Model: `TrackKind::Bus`, `Track.out`, `Track.sends`. All serde-default, so old files open unchanged.

### C2. Processors

| Effect | Built on | Notes |
| --- | --- | --- |
| `mix=` on every audio effect | `asplit` + `amix` | Wet/dry, the audio twin of the color `mix` |
| `peq` to 8 bands with types | `equalizer`, `lowshelf`, `highshelf`, `highpass`, `lowpass` | Bands as `'f,g,q,type …'`, the same string style as curves |
| `multiband` | `acrossover` + `acompressor` per band, or `mcompand` | 3 or 4 bands |
| `limiter truepeak=1` | 4x oversampled `alimiter` | For -1 dBTP delivery |
| `reverb` rewrite | `afir` with impulse responses synthesized by [sfx.rs](../crates/moodcut/src/sfx.rs) (room, hall, plate, booth) | Replaces the three-echo reverb; old one stays as fallback |
| `denoise learn=a-b` | `afftdn` noise profile sampled from a quiet range | Audition's "capture noise print" |
| `declip` | `adeclip` | |
| `deess` upgrade | `adynamicequalizer` when present | |
| Stereo tools | `stereotools`, `pan` | Mid/side width, balance, phase invert, mono below a frequency |
| Racks | script presets | `afx A1 rack=podcast-voice` expands to ordinary effects, as animation presets do |

Automating a filter's own settings (an EQ gain that moves) is deferred: FFmpeg only supports it through stepped
`asendcmd` on some filters. Volume, pan and `mix` automation cover most needs.

### C3. Measured repair and matching

- `repair <clips>`: measures first, then adds only what is needed (dehum if a 50/60 Hz peak is found, declip if
  samples sit at full scale, denoise sized from the noise floor, high-pass if there is rumble). Reports what it found.
- `match c2 to=c1 eq|loudness`: band-energy difference becomes `peq` bands; loudness becomes clip gain.
- `roomtone <track>`: fills cut gaps with tone looped from the quietest stretch, so `autocut silence` doesn't
  leave dead air.

### C4. Audio scopes (the MCP half of mixing)

```text
preview scope=spectrum|spectrogram|loudness|phase|mix [target=c3|A1|B1|master] [range=a-b]
```

Each returns an image plus one measured line, like the Lumetri scopes do:
integrated, short-term max and momentary max LUFS, LRA, true peak, noise floor, clipped samples, DC offset, energy
in six bands (sub, bass, low-mid, mid, presence, air), stereo correlation. `scope=mix` gives one line per track
and bus, so the AI sees the balance (dialogue vs music vs effects) in a single call.
New `analyze` kinds on media or a clip range: `noise`, `spectrum`, `dynamics`.

### C5. Speed

- **Frozen stems.** A track or bus whose chain is expensive (`afir`, `afftdn`, `rubberband`) is rendered once to a
  WAV in the cache, keyed by the hash of its sub-graph and inputs. Preview and export then only mix. Any edit
  to that track changes the hash.
- **Preview graphs** leave out measurement-only stages and reuse cached loudness, as `plan_preview` does now.
- **Time-stretch**: `rubberband` for speed-changed clips on export when available; `atempo` for preview.

### C6. Delivery

`render preset=stems` (one WAV per bus, plus the full mix) and a loudness line in every render reply.

## 8. MCP surface, in one place

No new tools. Changes per tool:

| Tool | Additions |
| --- | --- |
| `edit` | Verbs: `track`, `grade`, `still`, `match`, `bus`, `send`, `repair`, `roomtone`. Extended: `mask` (`window`, `hsl`, `luma`, `pt`), `fx … mask=` on all effects, `color … input= wb=`, `afx … mix= rack=`, `fade … curve=`, `kf A1 vol` |
| `preview` | `view=matte\|overlay`, `id=`, `pick=x,y`, `ref=`, `split=`, `region=`, `scope=falsecolor`, audio scopes with `target=` |
| `analyze` | `noise`, `spectrum`, `dynamics`; `track` jobs report through the edit reply |
| `check` | Rules in section 9 |
| `view` | `what=mix` (buses, sends, chains, measured levels), `what=grade` (a clip's node list with windows) |
| `render` | `preset=stems` |
| `help` | New topics `masks`, `tracking`, `grade`, `mix`; `color`, `audio`, `scopes` extended |
| Prompts | `color-grade` updated; new `roto-and-track`, `audio-mix` |

The `edit` description's cheat sheet gets one line each for masks/tracking and for mixing, with detail left to
`help`. Each new workflow also gets a recipe in `mcp/craft.rs`, which the existing test runs against the engine.

How an AI would use it:

```text
# grade a log interview and match the b-roll to it
color c1 input=slog3
preview scope=all t=4                      -> luma 6-71%, cast +warm
color c1 exposure=0.4 temp=-8
mask c1 ellipse x=.48 y=.38 w=.2 h=.3 feather=60 window
track c1.m1                                -> tracked 0-12s, 22 keys
fx c1 curves master='0,0 .4,.46 1,1' mask=m1
still save hero from=c1
match c2..c9 to=still:hero                 -> residual dE 2.1

# mix it
bus add B1 name=Dialogue
bus add B2 name=Music
track set A1 out=B1
track set A2 out=B2
repair A1                                  -> hum 50 Hz, floor -52 dB: +dehum +denoise amount=10
afx B1 rack=podcast-voice
duck B2 by=B1
preview scope=mix                          -> B1 -18.2 LUFS, B2 -30.5 LUFS, master TP -1.3
master lufs=-14
```

## 9. New `check` rules

- Color: clipped or crushed shots, shots that differ from their neighbors in exposure or balance (with a
  `match` fix line), skin tone off the skin line, untransformed log footage.
- Masks: tracked mask with low-confidence ranges, mask that leaves the frame, key with holes or backdrop residue.
- Audio: dialogue bus outside its loudness range, dialogue-to-music ratio, per-track clipping, true peak over
  target, stereo correlation below zero (mono compatibility), noise floor above a threshold, hard-cut silence.

As now, measured problems carry fix lines and craft notes carry `try:` lines.

## 10. Editor panels

Each is a view over the project that writes the verbs above (the rule from ARCHITECTURE.md):

- **Monitor**: pen tool edits path vertices; Track button on a selected mask; matte / overlay / split view
  toggles; eyedropper (pick) for qualifiers, keys and white balance.
- **Color**: input transform menu, node list (effect order with window and qualifier chips), Match button,
  stills gallery, shared grade menu, false-color scope.
- **Audio**: bus strips and send knobs in the mixer, automation lanes on timeline tracks, EQ curve editor,
  spectrum and loudness panels, Repair button in Essential Sound.

## 11. Performance targets

Baselines come from `moodcut bench` (0.6). Until that exists these are targets, not measurements.

| Case | Target |
| --- | --- |
| Clip with a five-effect grade | Same frame time as one LUT effect today |
| Windowed effect, window covers 10% of the frame | Under a quarter of the full-frame cost |
| Static mask used by three effects | Rasterized once per render, not three times per frame |
| Tracking a 10 s 1080p clip, one region | A few seconds on four cores |
| Log clip with input transform | No extra compositor time (done in the decoder) |
| Preview start with a convolution reverb on a bus | Same as without, after the stem is frozen once |
| 200-clip timeline audio render on Windows | Starts (no command-line limit), graph built in milliseconds |

## 12. Verification

- Unit tests next to the code, following the existing patterns: every audio effect builds a valid filter, every
  recipe runs, schema names are unique. New: mask expression parser, window-does-not-cut, fused LUT equals the
  effects applied one by one, tracker recovers a known synthetic motion, `match` reduces a known offset,
  bus/send graph renders through FFmpeg.
- CI render tests use synthetic lavfi sources, so no media is committed.
- `docs/REFERENCE.md` is generated (`moodcut docs`) and CI fails when it is stale. Every phase regenerates it.
- This development PC cannot run cargo, so CI is the compile and test run for all Rust work. Regenerating
  `REFERENCE.md` needs a built binary, so add a CI step that uploads the regenerated file as an artifact to commit.
  `tsc` and the GUI build run locally.

## 13. Order of work

Each milestone ships its verbs, `help` text, a recipe, `check` rules and the panel control together.

| Milestone | Contents | Depends on |
| --- | --- | --- |
| M0 | Phase 0 (0.1 to 0.7) | |
| M1 | A1 mattes, A4 matte view and pick, B1 fused LUT and cache | M0 |
| M2 | A2 tracker, masks/tracking panel work | M1 |
| M3 | B2 color management with decode-time grade, B4 matching and auto white balance, B3 shared grades and stills | M1 |
| M4 | C1 mixer model, C4 audio scopes, `view what=mix` | M0 |
| M5 | C2 processors, C3 repair and matching, C5 frozen stems, C6 stems export | M4 |
| M6 | A3 keyer, B5 effects, remaining panels, new prompts | M1, M3 |

M1 and M4 don't touch the same files and can run in parallel. Riskiest pieces: the tracker (accuracy on real
footage), the decode-time grade (must look identical in preview and export), and frozen stems (cache invalidation).

## 14. Decisions needed

| # | Question | Recommendation |
| --- | --- | --- |
| D1 | Automatic subject mattes (person segmentation)? It needs an ONNX-style runtime and a model download, like the optional speech model | Not in this plan. Revisit after M2; tracked masks plus the keyer cover most jobs |
| D2 | A true 10-bit compositor and 10-bit/HDR export? tiny-skia is 8-bit, so this means a second pixel path through every effect | Defer. The decode-time grade (B2) recovers most of the visible benefit for SDR delivery |
| D3 | Shared grades: linked (edit once, all clips follow) or copied presets? | Linked, with `grade detach` to copy |
| D4 | Should masks referenced by an effect default to `window` in new projects? | Yes for masks created through the Color panel or with the `window` word; plain `mask` stays a cut |
| D5 | Start with color or audio after M1? | Color (M3): it shares the matte work, and audio (M4) can proceed in parallel |

## 15. Implementation status

Built (code, script verbs, `help` topics, MCP options, tests where noted):

- **M0**: mask `cut` / `window` (window-does-not-cut test), matte cache (`render/matte.rs`), region-limited weighted apply with in-place blend, u8 mask accumulator with low-res feathering, filtergraphs through a temp file (`Ff::graph_args`, 35 KB graph test), `moodcut bench` synthetic scenes plus a CI artifact, loudness cache persisted to `<data>/cache/loudness`. CI uploads the regenerated `REFERENCE.md` as an artifact and checks it at the end of the Linux job.
- **M1**: `mask=` on every in-place native effect, mask expressions, `hsl`/`luma` qualifiers, placement (`offset`, `scale`, `trot`), `featherBias`, `mask c1.m1 pt N x,y [at=]`; `preview view=matte|overlay id=` and `pick=x,y`; fused grades (`render/grade.rs`, equivalence test), LRU LUT cache that skips animated grades, ordered dither.
- **M2**: tracker (`render/track.rs`, synthetic-motion tests) as a cached analysis, `track c1.m1`, `track c1 region=… to=null`, `track c1 corners=… to=cornerpin on=`, simplified keys and `!lost` ranges.
- **M3**: `input=` transforms baked to `.cube` and applied with `lut3d` in the decoder, ffprobe `color` tags for `auto`; `color … wb=auto|pick:x,y`; `grade save|apply|detach|del|list` (linked, rendered after the clip's own effects); `still save|del|list`; `match … to=clip|still:`; `preview ref= split=`, `scope=falsecolor`, `region=`; `view what=grade`.
- **M4**: buses (`Comp.buses`, not a `TrackKind` variant, to avoid breaking exhaustive matches), `track set … out=`, `send` (pre/post), loop checks, track/bus/clip-pan automation, fade curves and equal-power audio crossfades, bus ducking; audio scopes and measurement (`render/audio_scope.rs`: BS.1770 loudness, LRA, true peak, floor, clipping, DC, six bands, correlation, hum), `scope=mix`, `view what=mix`, `analyze noise|spectrum|dynamics`; render test through FFmpeg.
- **M5**: `mix=` wet/dry on every audio effect, `peq bands=` (8), `multiband`, `limiter truepeak=1` and `master truepeak=1`, convolution `reverb` with synthesized IRs, `denoise learn=a-b`, `declip`, `stereo`, racks; `repair`, `roomtone`, `match … eq|loudness`; `render preset=stems`.
- **M6**: keyer `auto`, `clipBlack`/`clipWhite`, `edge`; `lightwrap`, `matteclean`, `softclip`, `filmcurve`, `halation`, colorwheels pivots/`sat`/`tag`; `check` rules for clipped/crushed/flat shots, unmatched neighbors, masks leaving the frame, clipping, phase, hum/noise, dialogue level and dialogue-to-music ratio; prompts `roto-and-track`, `audio-mix`, updated `color-grade`; recipes `grade` and `mix`; editor: bus strips, routing and sends in the mixer, true-peak toggle, Repair button, Color panel input/WB/stills/shared grades/window and Track buttons, Scopes panel false color, audio scopes and matte/overlay views.

Still open:

- C5 frozen stems (rendering expensive track chains to cached WAVs).
- Folding a clip's leading static primaries into the decode-time LUT (only the input transform runs in the decoder).
- `deess` with `adynamicequalizer` (its options differ between FFmpeg 6 and 7), stereo "mono below a frequency".
- `check`: skin-tone line, key holes, tracker confidence kept after the edit.
- Editor: an eyedropper on the monitor, automation lanes on timeline tracks, an EQ curve editor, a node list with window chips.
- `preview scope=… mask=c1.m1` (scoping by mask; `region=` works), `view=matte n=6` sheets, `stabilize` from the tracker.

## 16. Second pass: camera tracking, perspective tracking, Resolve nodes (2026-11-25)

Status: written on 2026-11-25. One `cargo check` of the section 15 code ran once in a Linux container and found 3
errors (fixed); a first `cargo test` run showed 179 of 185 tests passing, and the 6 failures were fixed. Everything
below was written after that and has **not** been compiled: CI is again the first build.

Built:

- **3D camera tracker** (`camtrack`, also `track c1 to=camera`): `render/klt.rs` (Shi-Tomasi corners, pyramidal
  Lucas-Kanade with a forward-backward check, streaming so only two frames are in memory), `render/sfm.rs`
  (keyframes, essential matrix by RANSAC, pose from points, triangulation, Schur-complement bundle adjustment with the
  focal length refined, a rotation-only solve for tripod shots, every frame posed from the points),
  `render/linalg.rs` (no new dependency). Output: a one-node camera layer keyed from the shot and solved 3D points.
  `camtrack c1 points`, `ground=pA,pB,pC [origin=]`, `attach text|solid|null|shape|<media>|<layer> at=...`
  (3D layers on the points' plane or facing the camera), `preview view=points id=c1` (numbered points, floor grid).
  Solves are recorded in `Project.camsolves` (ground plane, attached layers) and cached as the `camtrack` analysis.
- **Planar tracker** (`render/planar.rs`): KLT features in a region, a RANSAC fit per frame (translation, rotation,
  similarity, affine, perspective) against the reference frame, new features given reference positions through the
  current transform (no drift from chaining). Corner pins use it by default (`mode=points` keeps the four-point tracker);
  masks track with `mode=rotation|affine|perspective` (affine/perspective key the path; shape masks become paths);
  nulls follow rotation and scale with `mode=`.
- **Masks**: per-kind sub ids (the first mask is always m1, whatever effects exist), Resolve window names
  (circle, linear, curve), `mask c1.m1 topath`, `pt N first` (Set First Vertex), `pt N smooth|corner` (Convert Vertex),
  matte finesse `morph=shrink|grow|open|close morphR=` and `matteBlur=`, `expand` on qualifiers.
- **Resolve nodes**: effects sharing a `node` label render as one node (one key, one mix, fused into one LUT when
  possible); layer mixer nodes (`layer=1 composite=...`) next to parallel nodes; the `node` verb
  (`add serial|parallel|layer|outside`, tools per node, `window add`, `qualify`, `mix`, `off`, `reset`, `up|down`,
  `track`), node-numbered `view what=grade`.
- Fixed while testing: the point tracker's template refresh drifted up to half a pixel per refresh; false color in
  `scopes::render`; mask expressions like `m1 m2`; a duplicate `midDetail` alias.
- Help topics `masks`, `tracking`, `grade` rewritten for MCP; a `track` recipe; tests for every new verb.

Still open: the GUI has no controls for nodes, camera tracking or the points view yet; `check` rules for camera solve
error; lens distortion and rolling shutter in the solver; planar tracking of occluded regions (no manual hints);
`docs/REFERENCE.md` is stale until the CI artifact is committed. Don't build or test in Docker Desktop on the
development PC (it hung the VM shared with other projects).

## 17. Finishing the open items, bundled FFmpeg, creative direction, release 1.1.0 (2026-11-25)

Built:
- Editor: a **Tracker** panel (After Effects' Tracker: camera solve with lens, tripod and detail options, the solved
  points drawn through the solved camera with click-to-pick, ground plane, text/solid/null/shape attached to points;
  mask tracking with AE's five methods; track point to a null; stabilize; planar corner pin), a **Nodes** section in
  Lumetri Color (the clip's Resolve node graph: serial / parallel / layer mixer / outside nodes, bypass, mix, composite
  mode, tools per node, windows, qualifiers, the key expression, tracking, reorder, reset, delete), and a `points`
  view in the Scopes panel. Every control sends the same script line an AI would.
- `check`: camera solves with a high error (Info above 1 px, Warn above 2 px), solves missing poses for more than
  10% of frames, and layers attached to points with a high error. It reads only a solve already computed
  (`Engine::cached_analysis_with`), so a check never starts a solve. `camtrack c1 points` replies with the solve
  error and lens.
- FFmpeg ships with MoodCut: `installer/get-ffmpeg.ps1` puts ffmpeg.exe, ffprobe.exe, the build's license and a
  SOURCE.txt in `dist\ffmpeg`; the installer requires it and installs it to `{app}\ffmpeg\bin` (where MoodCut looks
  first), the separate "download FFmpeg" step is gone, and the portable zip carries the same folder. CI asserts the
  installed `moodcut doctor` uses the bundled copy.
- The creative layer for AI clients: `help direction` (ground rules against guessing, the director's loop of brief,
  concept, treatment, build, review, deliver and report, design direction, editing direction, a review rubric) and
  `help features` (every feature: when to use it, the command, how to verify it, when not to). The MCP server
  instructions, the help tool, `help workflow` and the edit-folder prompt point to them, and a `creative-direction`
  prompt runs the whole loop. A test runs every backticked example of both guides (script lines on a fixture
  project, tool calls against the tool schemas), so the guides can't teach a command that doesn't exist.
- CI commits the regenerated `docs/REFERENCE.md` on branch pushes (it can't be generated on this PC); tags and pull
  requests still fail when it is stale. Version 1.1.0; the release notes live in `release/NOTES-v1.1.0.md`.

Still open: lens distortion and rolling shutter in the solver; planar tracking of occluded regions; AI-model features
(roto brush, magic mask, face tracking) need a model runtime MoodCut doesn't ship.
