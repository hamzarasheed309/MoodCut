# MoodCut architecture

MoodCut has two kinds of user: a person working in the editor window, and an AI assistant working through MCP.
Both drive one engine through one edit language, so neither can do something the other can't see or undo.
The design aims for three things:

1. **Fast, cheap AI editing.** An assistant should finish a real edit in a handful of tool calls, using little
   context, and recover from its own mistakes without help.
2. **Professional range.** Editing (Premiere) and motion design (After Effects) in one timeline model, rendered
   with good quality.
3. **Simple to run on Windows.** One executable plus FFmpeg. No runtime, installer, Python or Electron.

## Stack and why

| Layer | Choice | Why |
| --- | --- | --- |
| Core | **Rust**, single binary (~10 MB) | Native speed for per-pixel work, safe threading, no runtime to install, easy Windows distribution. |
| Media I/O | **FFmpeg** as a child process (raw RGBA / PCM over pipes) | Decodes and encodes everything, has hardware encoders (NVENC/QSV/AMF), its scaler is SIMD-optimised, and it can be upgraded independently. Pipes avoid fragile FFI and licensing coupling. |
| Compositor | **tiny-skia** (vector raster, via resvg) + custom SIMD-friendly pixel loops + **rayon** | Predictable CPU rendering that matches the preview exactly, with no GPU driver issues. The hot paths (affine warps, blends, blurs, LUTs) are parallel and allocation-free. |
| Text | **rustybuzz** shaping + bundled Inter + system fonts | Correct kerning/ligatures/RTL, per-glyph layout for text animators. |
| Server | **tokio + axum**: MCP Streamable HTTP, REST, WebSocket | One async server feeds the GUI, MCP clients and tunnels. |
| GUI | **Preact + signals**, Vite, TypeScript, canvas timeline; embedded with rust-embed | 26 KB gzipped. It opens as an Edge/Chrome `--app` window, so there's no WebView runtime to ship. |
| Protocol | **MCP 2025-06-18** over stdio and Streamable HTTP | Works with Claude Desktop, Claude Code, ChatGPT connectors and other MCP clients. |

## Process layout

```
             Claude Desktop / Claude Code            ChatGPT (via HTTPS tunnel)      Cursor, VS Code, …
                       │ stdio                                │ HTTP + token               │ HTTP
                       ▼                                      ▼                            ▼
              moodcut mcp ──(proxies to a running editor if present)──►  moodcut gui / serve  :7878
                                                                         ├─ /mcp        JSON-RPC (8 tools)
   Editor window (Edge/Chrome --app) ◄── /api, /api/ws (live events) ────┤
                                                                         ├─ Engine  (project, undo, autosave, log)
                                                                         │   ├─ ops: MoodCut Script → edits
                                                                         │   ├─ render: compositor + audio graph
                                                                         │   └─ analyze: silence/scenes/beats/STT (cached)
                                                                         └─ ffmpeg / ffprobe child processes
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

## Data model (`src/model`)

```
Project ─ media[] (m1…: probed path, kind, size, fps, duration, audio layout)
        ─ comps[] (s1…: size, fps, background, markers, master audio)
             └─ tracks[] (V1… bottom→top, A1…; hidden/locked/mute/solo/gain)
                   └─ items[] (c clip, t text, g shape, o solid, j adjustment, n nested comp)
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
- `outline.rs`: the compact text views returned by `view`.

## Rendering (`src/render`)

Per output frame, `comp.rs` evaluates each visible layer bottom-up:

1. **Evaluate** properties at the frame time (keyframes → expressions → parent chain → 2.5D camera).
2. **Rasterize** the layer into a tight bounding-box buffer in output pixels:
   - media is decoded by FFmpeg already scaled to the size it will be drawn at (including upscales), so the
     compositor never resamples a 4K frame just to show it at 640 px. Decoders are per clip, prefetch on a
     background thread, and read reverse playback in chunks.
   - nested comps render recursively at the needed scale.
   - text, shapes and masks are drawn as vectors at final resolution.
   - a layer whose first effect is a large blur is rasterized and blurred at reduced resolution, which costs
     nothing visible and makes blurred backgrounds (9:16 reframes) about 4× faster.
3. **Effects, masks and mattes** run on that buffer. 3D layers and corner pins use a perspective warp.
4. **Blend** into the frame with the layer's blend mode, using parallel premultiplied loops. Transitions
   composite both sides with their own progress curve.

The frame buffer is shared, not copied, between steps where possible (`Arc` frames, zero-copy for unscaled
layers). LUTs use a fixed-point 3D table, and vignette masks and fonts are cached.

Audio is rendered as a single FFmpeg filtergraph per export: clip trims, speed, gain automation (as volume
expressions), pans, crossfades, sidechain ducking, per-clip/track/master effects, loudness normalization and a
limiter. Export pipes raw frames into FFmpeg's encoder (x264/x265/SVT-AV1/ProRes/VP9 or NVENC/QSV/AMF).

Preview frames and contact sheets use the same renderer at reduced scale, so what the AI checks with `preview` is
what gets exported.

**Measured on a 4-core cloud VM** (1080p30): a title/transition demo exports at about 28 fps with x264
`medium` and about 51 fps with the draft preset. The talking-head example composites at 39–107 fps, and a
blurred-background 9:16 reframe of a beat montage at about 12 fps (2.8 fps before the reduced-resolution blur).

## Built for AI use

These choices keep a typical edit to a few calls and a few thousand tokens:

| Choice | Effect |
| --- | --- |
| **8 coarse tools** (~5 KB of definitions) | Small fixed cost per conversation; the `edit` description carries a cheat sheet so most edits need no `help` call. |
| **Batch script instead of one tool per operation** | 20 edits in one call instead of 20 round trips. |
| **Atomic scripts with line-numbered errors and suggestions** | No half-applied state to inspect; the fix is usually one corrected line. |
| **Diff replies** (`+t4 …`, `~c1..c3.f1 …`) | The AI learns the new ids and effects without re-reading the project. Repeated changes collapse into id ranges, which also work as targets (`fx c2..c9 blur`). |
| **Compact `view`** | A whole timeline in a few lines; `detail=` and `what=item` go deeper only when needed. |
| **References inside scripts** (`$`, `$2`, `as=name`, `c3.end+0.5`, `@marker`, `end`) | Later lines can use earlier results without another round trip. |
| **`help(topic)` on demand, generated from the schema** | The full reference (~30 KB) is never sent up front, and it is always accurate. |
| **Macros backed by cached analysis** | `autocut silence c1` replaces dozens of manual cuts; analysis runs once per file. |
| **Contact-sheet previews as small JPEGs** | The model checks 6–16 moments in one image instead of one call per frame. |
| **Live shared state** | The person can correct things in the GUI mid-conversation and the AI sees it in the next `view`. |

## Security

The server binds to `127.0.0.1` by default and rejects cross-origin browser requests, so websites can't drive it.
`--public` binds to all interfaces and requires a random token (as a bearer header, or in the URL path for
ChatGPT connectors). `--allow DIR` restricts which folders the API and AI can read or write.

## Source map

```
crates/moodcut/src/
  main.rs          CLI (clap)
  engine.rs        project state, transactions, undo, autosave, event log, renders
  server.rs        axum: MCP HTTP, REST API, WebSocket, static GUI, file dialogs
  mcp/             MCP protocol, tool definitions, help docs, stdio transport
  model/           project, values, anim/easing, expressions, schema, time parsing
  ops/             MoodCut Script: lexer, executor, presets, templates, macros, outline
  render/          ffmpeg, decode, compositor, text, shapes, masks, effects, LUT, warp, audio, encode
  analyze.rs       silence, scenes, loudness, beats, black frames, transcription
  setup.rs         doctor / setup / install-claude
gui/src/           Preact app: Timeline (canvas), Monitor, Inspector, LeftPanel, Console, Modals
```
