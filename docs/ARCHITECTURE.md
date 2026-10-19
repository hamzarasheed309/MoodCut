# MoodCut architecture

MoodCut has two kinds of user: a person working in the editor window, and an AI assistant working through MCP.
Both drive one engine through one edit language, so neither can do something the other can't see or undo.
The design aims for three things:

1. **Fast, cheap AI editing.** An assistant should finish a real edit in a handful of tool calls, using little
   context, and recover from its own mistakes without help.
2. **Professional range.** Editing (Premiere) and motion design (After Effects) in one timeline model, rendered
   with good quality.
3. **Simple to run on Windows.** One executable plus FFmpeg, no runtime, Python or Electron; an optional installer
   sets up shortcuts, FFmpeg and the AI apps in one pass.

## Stack and why

| Layer | Choice | Why |
| --- | --- | --- |
| Core | **Rust**, single binary (~10 MB) | Native speed for per-pixel work, safe threading, no runtime to install, easy Windows distribution. |
| Media I/O | **FFmpeg** as a child process (raw RGBA / PCM over pipes) | Decodes and encodes everything, has hardware encoders (NVENC/QSV/AMF), its scaler is SIMD-optimised, and it can be upgraded independently. Pipes avoid fragile FFI and licensing coupling. A portable build is downloaded on first run if none is found (into the data folder, no admin rights), and the engine switches to it without a restart. |
| Compositor | **tiny-skia** (vector raster, via resvg) + custom SIMD-friendly pixel loops + **rayon** | Predictable CPU rendering that matches the preview exactly, with no GPU driver issues. The hot paths (affine warps, blends, blurs, LUTs) are parallel and allocation-free. |
| Text | **rustybuzz** shaping + bundled Inter + system fonts | Correct kerning/ligatures/RTL, per-glyph layout for text animators. |
| Server | **tokio + axum**: MCP Streamable HTTP, REST, WebSocket | One async server feeds the GUI, MCP clients and tunnels. |
| GUI | **Preact + signals**, Vite, TypeScript, canvas timeline; embedded with rust-embed | About 58 KB gzipped with every panel and the dock. It opens as an Edge/Chrome `--app` window, so there's no WebView runtime to ship. |
| Protocol | **MCP 2025-06-18** over stdio and Streamable HTTP | Works with Claude Desktop, Claude Code, claude.ai and ChatGPT connectors and other MCP clients. |
| HTML graphics | **Headless Edge/Chrome** over the DevTools protocol (a small synchronous WebSocket client) | The best layout and animation engine there is, already installed on every Windows PC. No Node, Puppeteer or bundled Chromium. |
| Web access | **cloudflared** quick tunnel, downloaded on first use | A public HTTPS URL for web AI clients without an account, port forwarding or certificates. |

## Process layout

```
   Claude Desktop / Claude Code        claude.ai / ChatGPT (cloud)                 Cursor, VS Code, …
             │ stdio                              │ https://<tunnel>/mcp/<secret>          │ HTTP
             ▼                                    ▼                                        │
   moodcut mcp ──(proxies to a running     cloudflared ──► 127.0.0.1:7878 ◄────────────────┘
   editor if present)──────────────────────────────────►  moodcut gui / serve / connect
                                                          ├─ guard: local origin, or the secret for remote
                                                          ├─ /mcp        JSON-RPC (10 tools, 5 prompts)
   Editor window (Edge/Chrome --app) ◄── /api, /api/ws ───┤
                                                          ├─ Engine  (project, undo, autosave, log)
                                                          │   ├─ ops: MoodCut Script → edits
                                                          │   ├─ library: parallel cached probing, sheets, search
                                                          │   ├─ render: compositor + audio graph; export: SRT/VTT/chapters/XML/EDL
                                                          │   ├─ analyze: silence/scenes/beats/STT (cached)
                                                          │   ├─ check: measured QA of the edit + fixes
                                                          │   └─ gfx: HTML → headless Edge/Chrome → PNG frames
                                                          └─ ffmpeg / ffprobe / browser child processes
```

The **Engine** owns the open project. Every change, whether from the GUI, MCP or the CLI, is a MoodCut Script
run through `Engine::edit`:

1. The script is parsed into commands and validated against the schema.
2. It runs against a **copy** of the project. If any line fails, the copy is thrown away and the error names
   the line (`line 3 \`fx c2 glw\`: unknown effect 'glw'. Did you mean 'glow'?`). Scripts are atomic.
3. On success the copy replaces the project, the previous version goes onto the undo stack (200 levels),
   the revision number increments, the project autosaves, and an event goes out over WebSocket. The GUI updates,
   and the activity log shows the edit as coming from the AI or from the GUI.

Because the GUI edits through the same path (its inspector and timeline generate script lines), undo/redo,
the activity log and what the AI sees in `view` stay consistent.

## Editor (`gui/src`)

The window is a dock (`dock.ts`, rendered by `components/Dock.tsx`): a tree of row/column splits whose leaves are
tab groups of panels, plus floating windows. Split sizes are fractions of the free space, fixed pixels, or 0 to fit
the content (the Tools strip measures its height and asks for the columns it needs). Dragging a tab hit-tests the
tab bars and groups under the pointer: a tab bar or a group's center adds it as a tab, an edge splits that group,
nothing floats it. Workspaces are preset layouts (Assembly, Editing, Color, Effects, Audio, Motion, Graphics, AI);
each keeps its own arrangement in `localStorage`, and below 900 px a one-column layout of tabs is shown instead.
`panels.tsx` maps panel ids to components, so a panel is just a component that reads the store.

Every panel is a thin view over the project that writes script lines (`actions.ts` holds the shared actions and the
Premiere/After Effects shortcuts):

| Panel | Script it writes |
| --- | --- |
| Tools (V A B N R C Y U H Z T G) + timeline | `move` (`mode=overwrite` for nudges), `trim … ripple`, `roll … to=`, `fit`, `cut`, `slip`, `slide`, `workarea` |
| Program monitor | transform gizmo → `nudge <id> pos=/scale=/rot= [key at=]`; Type → `text … pos=`; Shape → `shape …` or (Alt) `mask <id> …` |
| Source monitor | `add m1[in-out] <targeted track> at=<playhead> mode=insert\|overwrite` |
| Lumetri Color | `color <id> k=v`, `fx <id> curves\|huecurves\|colorwheels\|secondary\|vignette\|autolevels` |
| Audio (Essential Sound, Track Mixer) | `sound`, `track set`, `master`, `afx <track\|master>` |
| Effect Controls | `set`, `kf`/`kf+`/`kf-`, `fx`, `mask`, `anim`, `align`, `distribute`, `ease`, `stagger` |
| Markers, History | `marker add\|set\|del`, `undo n` / `redo n` |

The editor also shares what the person is doing: the selection, playhead and active comp go to the engine over
the WebSocket (`ops::Ui`), so scripts can say `sel` and `playhead` (`color sel cinematic`, `text 'Hi' at=playhead`);
with no editor window open they are empty. Overlays and panels read from the renderer instead of re-implementing
it: `/api/layer` returns a layer's on-screen corners and anchor from the same `layer_geom` the compositor uses
(parenting, 3D and cameras included), `/api/scope` returns the Lumetri scope image and the measured readout that
MCP's `preview scope=` gives the AI, and `/api/history` lists the undo/redo labels. The peak meter is a Web Audio
analyser on the program monitor's streamed preview.

Playback moves the playhead every frame, so nothing heavy depends on it: the timeline's playhead is a positioned
element over the canvas (the clips are only redrawn when the edit, zoom or scroll changes), frame loads, the
transform gizmo, scopes and markers follow a `pausedPlayhead` signal that stays constant while playing, and the
timecode is its own tiny component. State, log and history fetches are coalesced, so an edit's reply and its
WebSocket notice cost one fetch.

## Finishing and craft (`ops/pro.rs`, `sfx.rs`, `mcp/craft.rs`)

- **Sound design** (`sfx.rs`): 13 effects synthesized in Rust (filtered-noise whooshes with a moving band and stereo
  pan, risers with rising oscillators and noise, kick/sub impacts, bells, keystrokes...), deterministic per
  (kind, length, pitch, seed). Each instance gets a new seed and a slight pitch change so repeats never sound copied,
  and each kind declares where its moment is (a whoosh's peak, a riser's end, a hit's attack) so `sfx … at=T`
  places the clip with that point on T. Files are written next to the project's graphics and imported as media
  tagged `sfx`; they go on audio tracks above dialogue and music.
- **Speed ramps** write `remap` keyframes: the speed multiplier is eased in and out, integrated over item time and
  composed with any earlier ramp; the clip keeps its length, the peak speed drops if the source runs out, slow
  sections get frame blending. **Punch-ins** scale alternate clips around a focus point. **J/L cuts** detach the
  audio (found again later by its sync offset) and extend it before or after the cut with a short fade.
- **Brand kit** (`Project.brand`): defaults for text, captions, title templates and shapes, and CSS variables in
  HTML graphics; what a command asks for explicitly still wins.
- **Craft guidance** (`mcp/craft.rs`): the style rules, platform specs, sound and brand topics and the recipes. A
  test runs every recipe against the engine with mock media, so the AI is never taught a line that fails.
- **Craft notes** in `check`: platform UI zones (with move fixes) for vertical short-form targets, then taste: weak
  hook, fade from black, slow pacing, static shots, linear keyframes, transition variety, silent transitions and
  titles, inconsistent type, missing brand or captions, length. Their suggestions are printed as `try:` and are
  never applied by `fix=true`.

## Data model (`src/model`)

```
Project ─ media[] (m1…: probed path, kind, size, fps, duration, audio layout)
        ─ comps[] (s1…: size, fps, background, markers, work area, master audio)
             └─ tracks[] (V1… bottom→top, A1…; hidden/locked/mute/solo/gain)
                   └─ items[] (c clip, t text, g shape, o solid, j adjustment, n nested comp, l null, k camera)
                        props:  name → Anim (static value | keyframes with easing | expression)
                        fx[] / afx[] (f1…), masks[] (m1…), shapes[] (s1…), text animators[] (x1…)
                        transitions, parent, track matte, speed/remap
```

- **Schema-driven properties.** Every layer kind, effect, shape and animator declares its parameters (type,
  range, default, unit, animatable) in `schema.rs`. The same table validates edits, gives typo suggestions,
  drives the GUI inspector and generates the `help` text, so docs and engine can't drift apart.
- **Anim** values hold a static value, keyframes (each with its own easing curve, including custom béziers, and
  optional cycle/ping-pong looping), and an optional expression evaluated on top (`expr.rs`: a small
  JIT-free interpreter with `time`, `value`, `wiggle`, `linear`, `ease`, `noise`…).
- **Short, stable ids** (`c12`, `c12.f1`) are cheap to send and quote, and are never reused.

## Edit language (`src/ops`)

MoodCut Script is line-based, `verb args key=value`, chosen because it is what LLMs write most reliably and
compactly. Notable pieces:

- `lex.rs`: tokenizer with quoting rules safe for Windows paths (`'C:\new\clip.mp4'` stays intact).
- `exec.rs` / `exec_props.rs`: placement (auto track picking, overwrite/insert/ripple), trimming, splits,
  speed, properties, keyframes, effects, masks, transitions, audio.
- `presets.rs`: 50+ animation presets that expand into ordinary keyframes/animators (so they stay editable).
- `templates.rs`: motion graphics templates built as editable precomps out of ordinary script. Overlays
  (light leak, grain…) are placed directly above existing layers instead.
- `macros.rs`: multi-step operations that use analysis: `autocut`, `captions`, `montage`, `beats`, `reframe`,
  `keep`, `duck`, `precomp`.
- `tools.rs`: the Premiere / After Effects tools as commands (rolling edit, lift/extract, swap, paste
  attributes, stagger/sequence layers, align/distribute, nudge, keyframe assistant, nulls and cameras, work
  area, Essential Sound) and the selectors (`sel`, `after:T`, `kind:text`, `V2:after:10`).
- `outline.rs`: the compact text views returned by `view`.

## Rendering (`src/render`)

Per output frame, `comp.rs` evaluates each visible layer bottom-up:

1. **Evaluate** properties at the frame time (keyframes → expressions → parent chain → camera). Time effects
   (posterize time, echo) evaluate the layer at other times.
2. **Rasterize** the layer into a tight bounding-box buffer in output pixels:
   - media is decoded by FFmpeg already scaled to the size it will be drawn at (including upscales), so the
     compositor never resamples a 4K frame just to show it at 640 px. Decoders are per clip, prefetch on a
     background thread, and read reverse playback in chunks.
   - nested comps render recursively at the needed scale.
   - text, shapes and masks are drawn as vectors at final resolution.
   - a layer whose first effect is a large blur is rasterized and blurred at reduced resolution, which costs
     nothing visible and makes blurred backgrounds (9:16 reframes) about 4× faster.
3. **Effects, masks and mattes** run on that buffer. 3D layers and corner pins use a perspective warp.
4. **Blend** into the frame with the layer's blend mode (32 modes, including stencil/silhouette), using parallel
   premultiplied loops. 3D layers are drawn back to front as seen from the active camera; 2D layers keep track
   order. Transitions composite both sides with their own progress curve.

The frame buffer is shared, not copied, between steps where possible (`Arc` frames, zero-copy for unscaled
layers). LUTs use a fixed-point 3D table, and vignette masks and fonts are cached.

Audio is rendered as a single FFmpeg filtergraph per export: clip trims, speed, gain automation (as volume
expressions), pans, crossfades, sidechain ducking, per-clip/track/master effects, loudness normalization and a
limiter. For a loudness target the pre-master mix is measured once (EBU R128, cached per mix) and given the exact
gain, followed by a peak limiter; single-pass `loudnorm` can't settle on short programs and lands several LU low.
Export pipes raw frames into FFmpeg's encoder (x264/x265/SVT-AV1/ProRes/VP9 or NVENC/QSV/AMF). Captions, chapters
and interchange timelines (FCP7 XML for Premiere Pro and Resolve, CMX3600 EDL) are written straight from the model.

Preview frames and contact sheets use the same renderer at reduced scale, so what the AI checks with `preview` is
what gets exported.

**Measured on a 4-core cloud VM** (1080p30): a title/transition demo exports at about 28 fps with x264
`medium` and about 51 fps with the draft preset. The talking-head example composites at 39–107 fps, and a
blurred-background 9:16 reframe of a beat montage at about 12 fps (2.8 fps before the reduced-resolution blur).

## Preview performance (`render/proxy.rs`, `render/encode.rs`, `server.rs`)

Previews follow the techniques Premiere Pro and After Effects use, so heavy footage plays on the CPU without loading
the graphics card:

- **Proxies.** Footage that is slow to decode at preview sizes (more than 1080p, HEVC/AV1/VP9/ProRes at 720p and up, 50+ fps
  at 1080p) gets a proxy when imported or when its project opens: H.264 at up to 1280 px, GOP 15, no B-frames,
  `-tune fastdecode`, source timestamps kept (`-fps_mode passthrough`) so variable-frame-rate clips stay in sync. One job
  runs at a time, at below-normal priority (`nice` / `BELOW_NORMAL_PRIORITY_CLASS`) with two threads. Proxies live in
  `<data>/cache/proxies`, named by the source's path, size and date plus the proxy size, so a session finds existing ones
  without a database and a changed file gets a new one. Only the editor's frame server knows about proxies
  (`FrameServer::for_preview`); exports build their own frame server and always decode the originals. The compositor
  swaps the path in `raster()` for draft renders when a proxy is ready and at most 1.5x upscaled, except for clips with
  decode-time FFmpeg effects (deshake, deinterlace, custom filters), which work in source pixels. Modes: `auto` (heavy
  footage), `all`, `off`; the "Toggle Proxies" switch and the mode are kept in `<data>/settings.json`. A finished proxy
  emits a `proxy` event so the editor reloads its frame.
- **Playback resolution.** The monitor's *Auto* picks the smallest of Full, 1/2, 1/4, 1/8 that still covers the monitor's
  device pixels; paused frames are requested at the displayed size (never above the comp size).
- **Dropped frames.** The preview stream measures render time against frame duration and repeats the last frame when it
  falls behind (at least one new picture a second), so playback runs in real time with sound in sync instead of
  stalling. Time spent blocked on the pipe doesn't count: that means the player is ahead.
- **Frame cache.** `/api/frame` keeps about 96 MB of encoded frames keyed by revision, proxy generation, frame number and
  size, so scrubbing back over frames is instant and any edit or proxy change invalidates them.
- **Peak files.** Each audio file is decoded once to 200 peaks per second (`<data>/cache/peaks`, like Premiere's `.pek`),
  and every waveform (a clip's source range at the width it's drawn, the Source monitor, library sheets) is drawn from
  it. Thumbnails are kept on disk too, and video posters decode from the proxy when one exists.
- **Bounded background work.** Imports probe on their own pool of at most 8 threads; thumbnails and peak files have
  small concurrency limits; HTML graphics render with `--disable-gpu` (SwiftShader for WebGL).

## AI apps and launching (`clients.rs`, `mcp/stdio.rs`, `server.rs`)

`clients.rs` knows where each AI app keeps its MCP servers: Claude Desktop (`%APPDATA%\Claude`, plus the Microsoft Store
build's virtualized `Packages\Claude_*\LocalCache\Roaming\Claude`), Claude Code (`claude mcp add --scope user`, or
`~/.claude.json`), Cursor, VS Code (`servers` with `type: stdio`), Windsurf, LM Studio, Gemini CLI and Codex (TOML). A
registration reads the file as JSON or JSONC, sets only the `moodcut` entry, keeps a one-time backup and writes atomically;
a file it can't parse is never overwritten (the error carries the snippet to paste). `status()` also notices entries
that point at another copy of MoodCut. The CLI (`install-ai`, `uninstall-ai`), the editor's Connect AI dialog
(`/api/ai/*`, local requests only) and the installer all use it.

A running editor writes `<data>/instance.json` with its port. `moodcut mcp` attaches to the editor on `--port` or on that
port; when the editor goes away mid-session it opens the same project in its own engine (and serves the web GUI again)
instead of answering errors. A second `moodcut` / `moodcut gui file.mcut` asks the running editor to open the file and
shows its window, and `moodcut connect` gives a running editor the web link; otherwise each picks a free port.

## Installer (`installer/moodcut.iss`)

Inno Setup, per-user by default (`PrivilegesRequired=lowest`, `/ALLUSERS` on the command line for everyone). Tasks:
shortcuts (started minimized so the console host never flashes), `PATH`, the `.mcut` association (`moodcut gui "%1"`),
FFmpeg (downloaded by the installer with a progress bar from gyan.dev or the BtbN mirror, then unpacked by
`moodcut setup --from <zip> --dest {app}\ffmpeg\bin`, which MoodCut searches first), and `moodcut install-ai --quiet`.
The uninstaller runs `moodcut uninstall-ai`, removes the PATH entry and FFmpeg, and offers to delete the data folder.
CI compiles it, installs it silently into a folder with a space, checks FFmpeg, PATH and the association, and uninstalls.

## Brand (`brand.rs`, `gui/src/brand.tsx`)

The logo is four interlocking reels (ember `#E76C33` and white on ink `#010101`), defined once in `brand.rs`. A bolder
variant keeps it legible at 16-32 px. `moodcut brand <repo>` regenerates the SVG logos (wordmark outlined by usvg),
web icons, the multi-size `.ico` (DIB entries plus a PNG at 256 px) and the installer bitmaps. The editor uses neutral
greys so colors read true while grading, with ember for focus and actions.

## Media library (`src/library.rs`)

Real projects start from a drive of footage, so importing and finding media has to scale. Folders are walked
recursively (skipping hidden and MoodCut's own asset folders), files are probed in parallel, and probe results
are cached in memory and on disk by path, size and modification time, so re-importing a known card is instant.
Unreadable files are skipped and reported in one line. Media carry tags and notes the AI writes as it looks
(`note m3 'drone over beach at sunset'`), and every media argument takes selectors (`m1..m40`, `tag:broll`,
`kind:video`, `q:drone`, `all`). `media list` filters and pages; `preview media=…` returns poster-frame sheets
(48 clips per image, a third of the way in so fades and slates are skipped) or a filmstrip of one clip; `analyze` runs over many
media at once; `view what=search` searches transcripts and notes across the library.

## HTML motion graphics (`src/gfx.rs`, `assets/gfx_shim.js`)

The `graphic` tool lets the AI design the way it designs web pages. MoodCut finds Edge or Chrome, launches it
headless with a throwaway profile, and talks to it over the DevTools protocol:

1. The page is loaded from a throwaway localhost server that serves the graphic's folder (so relative assets
   work), the bundled fonts and library media (`src="mc://m3"`). `{{name|default}}` variables are filled first.
2. A shim injected before any page script replaces the clocks: `performance.now`, `Date`, `requestAnimationFrame`,
   timers and `document.timeline` all read a virtual time. Before each frame MoodCut seeks it: due timers run in
   order, rAF callbacks fire, CSS animations and transitions and Web Animations are paused at the exact time, SVG
   animations and `<video>` elements are seeked, and GSAP's ticker is stepped. Renders are deterministic and
   independent of machine speed.
3. Each frame is captured as a PNG with a transparent background and piped to FFmpeg as PNG-in-MOV (RGBA), or
   saved as a PNG still. The file is named by a hash of its inputs, so unchanged graphics never re-render.
4. The reply measures what was drawn: the box of painted content (text, images, shapes, backgrounds; not layout
   wrappers) checked against title-safe and the frame edge, blank renders, JavaScript errors and unset variables,
   plus a sheet of frames cropped to the content for small graphics.

The source is kept with the media, so `id=` re-renders a graphic in place after a fix and `from=` makes variants.

## Checking edits (`src/check.rs`)

An AI can't watch its edit, so `check` measures it instead. It walks the timeline for structural problems
(black gaps, 1–2 frame gaps and flash frames, layers fully covered by opaque clips, offline media, transitions
without enough source handles), renders isolated frames of each text and graphic layer to measure its painted
box and its contrast against what is behind it (WCAG ratio), compares media resolution and frame rate with the
comp, measures caption reading speed, finds music that isn't ducked under speech, measures integrated loudness
and true peak of the real mix, and compares the result with what was asked (`expect` duration, size, aspect, fps,
loudness, captions, music, shot lengths). Issues come as one line each (repeats grouped), most with a script line
that fixes them; `fix=true` applies those as one undoable edit and re-measures where needed (title-safe moves are
computed from the measured box). A sheet shows the problem frames with title-safe guides.

## Built for AI use

These choices keep a typical edit to a few calls and a few thousand tokens:

| Choice | Effect |
| --- | --- |
| **10 coarse tools** (~11.6 KB, ~2,900 tokens of definitions) | Small fixed cost per conversation; the `edit` description carries a cheat sheet so most edits need no `help` call. |
| **Batch script instead of one tool per operation** | 20 edits in one call instead of 20 round trips. |
| **Atomic scripts with line-numbered errors and suggestions** | No half-applied state to inspect; the fix is usually one corrected line. |
| **Diff replies** (`+t4 …`, `~c1..c3.f1 …`) | The AI learns the new ids and effects without re-reading the project. Repeated changes collapse into id ranges, which also work as targets (`fx c2..c9 blur`). |
| **Compact `view`** | A whole timeline in a few lines; `detail=` and `what=item` go deeper only when needed. |
| **References inside scripts** (`$`, `$2`, `as=name`, `c3.end+0.5`, `@marker`, `end`) | Later lines can use earlier results without another round trip. |
| **`help(topic)` on demand, generated from the schema** | The full reference (~30 KB) is never sent up front, and it is always accurate. |
| **Macros backed by cached analysis** | `autocut silence c1` replaces dozens of manual cuts; analysis runs once per file. |
| **Contact-sheet previews as small JPEGs** | The model checks 6–16 moments in one image instead of one call per frame. |
| **Poster sheets and filmstrips of the library** | 48 clips per image: the AI surveys a shoot in a few calls and writes notes so it never looks twice. |
| **Graphics as HTML** | Models already write excellent HTML/CSS; the reply shows the frames and flags layout errors. |
| **Measured `check` with fix lines** | The AI verifies its own edit against what was asked and repairs it without guessing. |
| **`help('workflow')` and MCP prompts** | A step-by-step playbook from library to delivery, so one request becomes a finished edit. |
| **Live shared state** | The person can correct things in the GUI mid-conversation and the AI sees it in the next `view`. |

A full one-request edit, driven through MCP like a client would (import a 157-file library and an interview, read
the transcript, cut the story with b-roll, a beat-cut montage, color, captions, music ducking and loudness, two HTML
graphics, check with expectations and fix, render the MP4 plus SRT, VTT, chapters, XML and EDL), takes 28 tool
calls and about 11,000 characters of tool output plus six small JPEG sheets, in under a minute on a 4-core VM
including the renders.

## Security and remote access

The server binds to `127.0.0.1` by default. Browser requests must come from the editor's own origin, so other
websites can't drive it; the one exception is `/mcp` from pages served on this computer (any `localhost` port, such as MCP
Inspector), which gets CORS headers, while `/api` stays same-origin. Request bodies can be up to 256 MB (HTML graphics
with embedded images). A request counts as remote when it arrives through a proxy or tunnel (forwarding headers,
or a `Host` that isn't local); remote requests need the secret token, either as a bearer header or in the URL
path (`/mcp/<token>`, which is how web connectors carry it), and without a configured token remote access is off.
`moodcut connect` keeps a random 32-character token in the data folder so connector URLs survive restarts
(`--new-token` rotates it), starts a Cloudflare quick tunnel to the local port, and prints the connector URL; the
editor's **Create web link** does the same and is only available from the local machine. OAuth discovery paths
answer 404 so clients fall back to the token URL. `--public` binds to all interfaces with a token, and
`--allow DIR` restricts which folders the API and AI can read or write.

## Source map

```
crates/moodcut/src/
  main.rs          CLI (clap)
  engine.rs        project state, transactions, undo, autosave, event log, renders
  library.rs       import at scale, probe cache, selectors, poster sheets, filmstrips, search
  gfx.rs           HTML motion graphics: browser discovery, DevTools client, asset server, frame capture
  sfx.rs           procedural sound effects (synthesis, WAV writer)
  check.rs         measured quality check, expectations, fixes
  export.rs        SRT / WebVTT / YouTube chapters / FCP7 XML / CMX3600 EDL
  tunnel.rs        remote token, cloudflared quick tunnel, connector instructions
  server.rs        axum: MCP HTTP, REST API, WebSocket, static GUI, file dialogs, access guard
  mcp/             MCP protocol, tool definitions, prompts, help docs, craft guidance and recipes, stdio transport
  model/           project, values, anim/easing, expressions, schema, time parsing
  ops/             MoodCut Script: lexer, executor, tools, pro finishing, presets, templates, macros, outline
  render/          ffmpeg, decode, compositor, text, shapes, masks, effects, LUT, warp, scopes, audio, encode
  analyze.rs       silence, scenes, loudness, beats, black frames, transcription
  setup.rs         doctor / setup (download or --from archive) / install-ai
  clients.rs       AI app registration: Claude Desktop, Claude Code, Cursor, VS Code, Windsurf, LM Studio, Gemini, Codex
  brand.rs         the logo and the generator for icons, .ico and installer art
  render/proxy.rs  preview proxies: heavy-footage detection, background builds, Toggle Proxies
assets/gfx_shim.js virtual clock injected into HTML graphics
installer/         Inno Setup script and wizard artwork; install.ps1 / install.sh at the root are the one-liners
gui/src/           Preact app: dock.ts + Dock (workspaces), panels.tsx (registry), Timeline (canvas), Tools, Monitor
                   (program/source/scopes/gizmo), Inspector (+Align, Character), ColorPanel, AudioPanel, BrandPanel,
                   Markers, History, LeftPanel (Project, Effects), Console, Modals (Connect AI), Home, GraphicEditor,
                   CheckPanel; brand.tsx; actions.ts
```
