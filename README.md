# MoodCut

**A local video editor and motion graphics engine that ChatGPT and Claude can drive over MCP.**
Premiere-style editing and After Effects-style motion design in one small Windows executable,
with FFmpeg doing the heavy lifting and a compact edit language built so an AI can edit fast and cheaply.

![MoodCut editor](docs/screenshot.jpg)

Point your assistant at a folder of footage and ask for the finished piece:

> *"Here's D:\Footage\Summer (about 150 clips) and my interview talk.mp4. Make a 30-second 1080p highlight:
> open on the interview, b-roll over it, a beat-cut drone montage to the music, an animated title and a lower
> third with my name, captions and YouTube chapters. Check it and give me the MP4, captions and a Premiere XML."*

The assistant imports and browses the library, reads the transcript, cuts the story, designs the graphics as
HTML, grades and mixes, runs MoodCut's quality check (fixing what it reports), and renders the deliverables,
in about 30 tool calls. You watch the edit build live in the editor window and can take over by hand at any point.
Every click in the GUI goes through the same engine, so the AI always sees what you changed.

## What it can do

**Editing (Premiere-style)**: multi-track timeline, ripple/insert/overwrite, razor, trim, slip, slide, speed
ramps and reverse, freeze frames, time remapping, frame-blend/optical-flow slow motion, nested sequences,
24 transitions (plus FFmpeg's xfade set), Lumetri-style grading with 15 looks and `.cube` LUTs, blur-fill for
portrait footage in landscape edits, audio gain automation, crossfades, sidechain ducking, measured loudness
normalization (hits the LUFS target exactly) and a limiter, 15 audio effects, and markers.

**Motion design (After Effects-style)**: keyframes on every property with 30+ easing curves and custom béziers,
keyframe loops, expressions (`wiggle`, `linear`, `ease`, `random`, `noise`…), parenting, 2.5D layers (X/Y
rotation and depth with perspective), corner pin, motion blur, masks (feather, expand, modes), track mattes,
adjustment layers, shape layers with trim paths and repeaters, per-character text animators, 49 video effects
(glow, blur, chroma key, wave warp, glitch, light leaks…), 50+ animation presets and 15 editable motion graphics
templates (lower thirds, kinetic titles, counters, bar charts, callouts, progress bars…).

**HTML motion graphics**: the AI (or you) designs a title, lower third, chart or full-screen animation as
HTML/CSS/JS, the same way Claude builds web artifacts. MoodCut renders it frame by frame in headless Edge/Chrome
on a virtual clock, so CSS animations and transitions, Web Animations, GSAP, anime.js, Lottie, canvas, SVG and
`<video>` all land on exact frames. The result is a transparent video (or a still) added to the media library
and, optionally, placed on the timeline. Templates take `{{name|default}}` variables, so one design makes many
variants. See [examples/graphics](examples/graphics).

**Large libraries**: import whole drives of footage at once (parallel, cached probing; about 150 files in 2–3 s),
skip unreadable files with a report, tag and annotate media, filter by kind, duration, orientation, tags or
words, browse poster-frame sheets of 48 clips per image and per-clip filmstrips, batch-analyze, and search
transcripts across the library.

**AI workflows**: silence removal (jump cuts), scene detection, beat detection with beat-synced montages,
transcription with word-timed animated captions (whisper.cpp, openai-whisper, FFmpeg 8's whisper filter or an
`.srt`), transcript-driven rough cuts, and automatic reframing to 9:16 / 1:1 / 4:5.

**Quality check**: `check` measures the edit instead of trusting it: black gaps and flash frames, clips that
are covered or offline, text outside title-safe or with poor contrast (WCAG ratio measured on the rendered frames),
upscaled or letterboxed footage, frame-rate judder, transitions without source handles, captions that are too fast,
music not ducked under speech, loudness and true peak (measured on the actual mix), and whether the result matches
what was asked (duration, size, captions, music, shot lengths). Each problem comes with a script line that fixes it;
`fix=true` applies them as one undoable edit, and a contact sheet shows the problem frames with safe-area guides.

**Deliverables**: MP4/H.264 (YouTube and social presets), HEVC, AV1, ProRes (incl. 4444 alpha), WebM (incl.
alpha), GIF, PNG sequences and audio formats, with NVENC, QuickSync and AMF encoding. Besides the video:
SRT and WebVTT captions, a YouTube chapter list from markers, an FCP7 XML timeline that Premiere Pro and DaVinci
Resolve open with the original media, and a CMX3600 EDL.

## Quick start (Windows)

1. Get [`release/windows-x64/moodcut.exe`](release/windows-x64/) from this repository (or the `moodcut-windows-x64`
   artifact of the [latest CI build](../../actions/workflows/ci.yml)). It's a single 64-bit exe with nothing to install;
   put it wherever you like, ideally in a folder on your `PATH` so AI clients and terminals can start it.
2. Double-click it. The editor opens in its own window (Edge app mode, no browser chrome).
3. The first time, MoodCut offers to **Install FFmpeg**: one click downloads a portable build (about 100 MB) into
   `%APPDATA%\MoodCut\ffmpeg`, with no admin rights or PATH changes, and it's used right away. From a terminal, `moodcut setup`
   does the same. An FFmpeg you already have (on `PATH`, winget, Chocolatey, Scoop, or next to the exe) is found automatically.
   HTML graphics use the Microsoft Edge that comes with Windows.

Check everything with `moodcut doctor`, then connect your AI (below).

Projects are plain JSON (`.mcut`) and autosave. Rendered graphics go next to the project in
`<project> assets/graphics`. Optional: install [whisper.cpp](https://github.com/ggerganov/whisper.cpp)
(`whisper-cli` on `PATH`, model in `%APPDATA%\MoodCut\models` or `MOODCUT_WHISPER_MODEL`) for captions.

## Connect your AI

The **Connect AI** button in the editor shows all of these with your actual port.

### Claude on the web (claude.ai) and ChatGPT

Web apps run in the cloud, so they need an HTTPS address that reaches your PC. One command sets it up:

```sh
moodcut connect
```

It opens the editor, starts a free [Cloudflare quick tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/do-more-with-tunnels/trycloudflare/)
(it downloads `cloudflared` the first time; no account needed), and prints a connector URL like
`https://quiet-river-1234.trycloudflare.com/mcp/<secret>` (also copied to the clipboard). In the editor,
**Connect AI → Create web link** does the same thing.

- **claude.ai** (Pro, Max, Team, Enterprise): Settings → Connectors → *Add custom connector* → paste the URL → Add.
  Then enable MoodCut in a chat's tools menu.
- **ChatGPT** (plans with developer mode): Settings → Connectors (*Apps* in newer versions) → Advanced → turn on
  *Developer mode* → *Create* → paste the URL, authentication *No authentication*.

Keep the window open while you work. The secret in the URL is the only way in: requests through the tunnel
without it are refused, and it stays the same across restarts (`moodcut connect --new-token` replaces it). The
quick-tunnel hostname changes each time you run `connect`; if you have your own public address (a named Cloudflare
tunnel, Tailscale Funnel or a reverse proxy to port 7878), use `moodcut connect --url https://your.host` to keep
one connector URL for good.

### Desktop and local clients

| Client | How |
| --- | --- |
| **Claude Desktop** | `moodcut install-claude`, then restart Claude. |
| **Claude Code** | `claude mcp add moodcut -- moodcut mcp` |
| **Any MCP client over HTTP** (Cursor, VS Code, LM Studio…) | `http://127.0.0.1:7878/mcp` (Streamable HTTP) |

`moodcut mcp` (stdio) attaches to the editor if it's already open, so AI edits appear live in the window; otherwise
it runs the engine itself and serves the editor at `http://127.0.0.1:7878`.

### Ready-made prompts

MoodCut also offers MCP prompts that clients show as slash commands or prompt pickers: **edit-folder** (a folder
of footage to a finished video), **talking-head**, **social-cutdown** (vertical versions), **motion-graphic** and
**review** (check an edit and fix it). Each one runs the full workflow below with your arguments.

## How the AI edits

MoodCut exposes **10 tools** whose definitions total about 8.7 KB (~2,200 tokens), so connecting it costs little context:

| Tool | Purpose |
| --- | --- |
| `project` | new / open / save / undo / redo / history |
| `media` | import files, folders, globs or URLs; list with filters and paging; browse disk folders |
| `edit` | apply a **MoodCut Script**: many edits per call, all-or-nothing |
| `view` | compact text views: timeline, one item, media, markers, transcript, library search |
| `preview` | a frame, a contact sheet of the edit, poster sheets of the library or a clip's filmstrip, as small JPEGs |
| `analyze` | silence, scenes, loudness, beats, transcript, black frames, exposure stats, for one clip or many (cached) |
| `graphic` | design a motion graphic in HTML/CSS/JS; returns the media id, layout checks, JS errors and a frame sheet |
| `check` | measured quality check with expectations and one-step fixes |
| `render` | export a video or a deliverable (captions, chapters, XML, EDL); progress and cancel |
| `help` | the reference, one topic at a time, generated from the engine's schema; `help('workflow')` is the playbook for a complete edit |

Instead of one tool call per operation, the assistant sends a short script and gets back a diff:

```text
text 'SUMMER SALE' at=0.3 dur=3 style=title enter=pop exit=blurOut
title lowerthird text='Jane Doe' sub='Founder' at=4
tr V1 dissolve 0.4
color V1 cinematic
```
```text
ok rev 5 (4 ops)
+t4 text 'SUMMER SALE' V2 0.3-3.3
+n8 lowerthird V2 4-9 (precomp s2: g5,t6,t7)
~V1 2 transition(s) dissolve
~c1..c3.f1 contrast=20 highlights=-15 shadows=8 sat=92 stint=#16707f55 htint=#ff9d5a44
+c1..c3.f2 vignette
s1 dur=12s
```

`view` then returns the whole timeline in a few lines:

```text
s1 'Main' 1920x1080@30 dur=12 [main]
V2: t4 text 'SUMMER SALE' 0.3-3.3 size=120 txanim:in,out | n8 comp s2 4-9
V1: c1 m1 0-4 fx:lumetri,vignette >dissolve:0.4 | c2 m2 4-8 fx:lumetri,vignette >dissolve:0.4 | c3 m3 8-12 fx:lumetri,vignette
A1: -
```

Errors name the line and suggest fixes (`unknown animation 'type'. Did you mean 'typeOut' or 'typewriter'?`),
and a failed script changes nothing. Times can be seconds, timecode, frames, markers (`@drop`) or expressions
like `c3.end+0.5`, and `$` refers to the item just created, so scripts rarely need a round trip to look up ids.

### Motion graphics as HTML

```json
{"tool": "graphic", "html": "<div class='lt'><b>{{name}}</b><span>{{role|Director}}</span></div><style>…@keyframes…</style>",
 "name": "Lower third", "dur": 4, "vars": {"name": "Jane Doe"}, "place": "V4 at=0.4"}
```
```text
+m158 graphic 'Lower third' 1920x1080 30fps 4s alpha
+c38 clip m158 V4 0.4-4.4
rendered 120 frames in 9.3s -> D:/Edits/promo assets/graphics/lower-third-2a6eaae7.mov
content box 115,830-281,957
```

The reply includes a sheet of frames so the AI sees its design, plus warnings for text outside title-safe,
content clipped by the frame, an empty render, JavaScript errors and unset `{{variables}}`. `id=` re-renders a
graphic in place after a fix (every clip using it updates); `from=` makes a variant with other variables.
Library media can be used inside the page as `src="mc://m3"`. `help('graphics')` has the patterns.

### Checking the work

```text
check s1 'Main' 1920x1080@30 30s: 0 error(s), 1 warning(s), 1 note(s)
27 shots (avg 1.11s, 52 cuts/min) · 6 captions · 2 graphics/overlays · mix -14 LUFS / -3 dBTP
W1 @4.05s c2,c3 from 640x360 sources shown at up to 3x their resolution: soft/pixelated (use higher-res media, smaller scale, or a smaller comp)
N1 @8.62s 26 clips (c5,c6,c7,c8…) are portrait in a landscape comp: black bars on the sides
   fix: set c5..c30 fit=blur
expected: dur 30s ✓ · size 1920x1080 ✓ · captions ✓ · music ✓
1 fix line(s) available: check fix=true applies them in one undoable edit, or edit them yourself
```

See the [MoodCut Script reference](docs/REFERENCE.md) (this is exactly what the AI gets from `help`) and the
[examples](examples/):

- [`motion-graphics.mcs`](examples/motion-graphics.mcs): a 12-second animated explainer with no footage
- [`talking-head.mcs`](examples/talking-head.mcs): jump cuts, grade, captions, lower third, music ducking
- [`beat-montage.mcs`](examples/beat-montage.mcs): beat-synced promo with titles and overlays
- [`vertical-short.mcs`](examples/vertical-short.mcs): turn an edit into a 9:16 Short/Reel
- [`graphics/`](examples/graphics): HTML lower third and kinetic title templates

```sh
moodcut run explainer.mcut @examples/motion-graphics.mcs --create 1920x1080@30
moodcut check explainer.mcut
moodcut render explainer.mcut
```

## Command line

| Command | |
| --- | --- |
| `moodcut` / `moodcut gui [project]` | open the editor (also serves MCP at `/mcp`) |
| `moodcut connect [project] [--url URL] [--new-token]` | editor + HTTPS tunnel + connector URL for claude.ai / ChatGPT |
| `moodcut serve [--port 7878] [--public] [--token T] [--allow DIR]…` | headless editor + MCP server; `--public` listens on all interfaces and requires a token, `--allow` limits file access |
| `moodcut mcp` | MCP over stdio (for Claude Desktop / Claude Code) |
| `moodcut run <project> <script or @file> [--create WxH@fps]` | apply a script and save |
| `moodcut render <project> [-o out] [-p preset] [--range a-b] [--scale 0.5] [--hw auto]` | export (presets include `srt`, `vtt`, `chapters`, `xml`, `edl`) |
| `moodcut frame <project> -t 3.5 -o frame.png` | render one frame |
| `moodcut graphic <file.html> -o out.mov [--dur 5] [--size 1920x1080] [--var k=v]… [--project p --place "V3 at=2"]` | render an HTML motion graphic (`--dur 0 --at 2` for a still) |
| `moodcut check <project> [--fix] [--dur 30] [--size 1080x1920] [--strict]` | quality-check an edit; `--strict` fails on errors (CI) |
| `moodcut doctor` / `moodcut setup [--force]` | check the setup / install (or update) the portable FFmpeg |
| `moodcut install-claude` | register with Claude Desktop |

Environment: `MOODCUT_FFMPEG` (folder with ffmpeg/ffprobe, or `none` to run without), `MOODCUT_HOME` (data
folder, default `%APPDATA%\MoodCut`), `MOODCUT_BROWSER` (Edge/Chrome for HTML graphics), `MOODCUT_CLOUDFLARED`,
`MOODCUT_WHISPER_BIN`, `MOODCUT_WHISPER_MODEL`.

Without FFmpeg, MoodCut still opens, edits projects and renders HTML graphics as PNG stills; anything that needs
to decode or encode media explains how to install it, and the editor shows the one-click install.

## Building from source

Requires Rust (stable) and Node.js 20+.

```sh
cd gui && npm ci && npm run build && cd ..   # GUI -> gui/dist (embedded into the exe)
cargo build --release                         # target/release/moodcut(.exe)
cargo test
```

`npm run dev` in `gui/` serves the GUI with hot reload and proxies `/api` (including the WebSocket), `/mcp` and
`/fonts` to a running `moodcut serve`. `moodcut docs > docs/REFERENCE.md` regenerates the reference (CI checks
it is current). The same code builds and runs on Linux (tested in CI) and macOS (untested).

## Architecture

One Rust executable contains the engine, MCP server, HTTP/WebSocket API, CLI and the embedded GUI. FFmpeg (a
separate process) handles decoding, encoding and audio, a multi-threaded compositor does everything visual, and
headless Edge/Chrome rasterizes HTML graphics. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design
and the reasons behind it.

## License

MIT. The bundled Inter font is under the SIL Open Font License (`crates/moodcut/assets/fonts/OFL.txt`).
