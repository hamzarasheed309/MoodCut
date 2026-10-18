# MoodCut

**A local video editor and motion graphics engine that ChatGPT and Claude can drive over MCP.**
Premiere-style editing and After Effects-style motion design in one small Windows executable,
with FFmpeg doing the heavy lifting and a compact edit language built so an AI can edit fast and cheaply.

![MoodCut editor](docs/screenshot.jpg)

Ask your assistant *"cut the pauses out of talk.mp4, add bold captions, put a lower third with my name at 3s,
duck the music under my voice and export a 9:16 version"*. MoodCut makes the edit in one atomic batch, shows it
live in the editor window, and lets you keep editing by hand. Every click in the GUI goes through the same
engine, so the AI always sees what you changed.

## What it can do

**Editing (Premiere-style)**: multi-track timeline, ripple/insert/overwrite, razor, trim, slip, slide, speed
ramps and reverse, freeze frames, time remapping, frame-blend/optical-flow slow motion, nested sequences,
24 transitions (plus FFmpeg's xfade set), Lumetri-style grading with 15 looks and `.cube` LUTs, audio gain
automation, crossfades, sidechain ducking, EBU R128 loudness and a limiter, 15 audio effects, markers, and
export presets for MP4/H.264, HEVC, AV1, ProRes (incl. 4444 alpha), WebM (incl. alpha), GIF, PNG sequences and
audio formats. NVENC, QuickSync and AMF can be used for encoding.

**Motion design (After Effects-style)**: keyframes on every property with 30+ easing curves and custom béziers,
keyframe loops, expressions (`wiggle`, `linear`, `ease`, `random`, `noise`…), parenting, 2.5D layers (X/Y
rotation and depth with perspective), corner pin, motion blur, masks (feather, expand, modes), track mattes,
adjustment layers, shape layers with trim paths and repeaters, per-character text animators, 49 video effects
(glow, blur, chroma key, wave warp, glitch, light leaks…), 50+ animation presets and 15 editable motion graphics
templates (lower thirds, kinetic titles, counters, bar charts, callouts, progress bars…).

**AI workflows**: silence removal (jump cuts), scene detection, beat detection with beat-synced montages,
transcription with word-timed animated captions (whisper.cpp, openai-whisper, FFmpeg 8's whisper filter or an
`.srt`), transcript-driven rough cuts, and automatic reframing to 9:16 / 1:1 / 4:5.

## Quick start (Windows)

1. Download `moodcut.exe` from the [latest build](../../actions/workflows/ci.yml) (artifact `moodcut-windows-x64`)
   or a [release](../../releases), and put it somewhere on your `PATH`.
2. Install FFmpeg if you don't have it: `moodcut setup` (uses `winget`), then check everything with `moodcut doctor`.
3. Run `moodcut`. The editor opens in its own window (Edge/Chrome app mode), with no browser chrome.

Projects are plain JSON (`.mcut`) and autosave. Optional: install [whisper.cpp](https://github.com/ggerganov/whisper.cpp)
(`whisper-cli` on `PATH`, model in `%APPDATA%\MoodCut\models` or `MOODCUT_WHISPER_MODEL`) for captions.

## Connect your AI

The **Connect AI** button in the editor shows these with your actual port.

| Client | How |
| --- | --- |
| **Claude Desktop** | `moodcut install-claude`, then restart Claude. |
| **Claude Code** | `claude mcp add moodcut -- moodcut mcp` |
| **ChatGPT** (Settings → Connectors → Developer mode) | ChatGPT needs a public HTTPS URL: run `moodcut serve --public` (prints a token) and a tunnel such as `cloudflared tunnel --url http://localhost:7878`, then add `https://<tunnel>/mcp/<token>`. |
| **Any MCP client over HTTP** (Cursor, VS Code, LM Studio…) | `http://127.0.0.1:7878/mcp` (Streamable HTTP) |

`moodcut mcp` (stdio) attaches to the editor if it's already open, so AI edits appear live in the window; otherwise
it runs the engine itself and serves the editor at `http://127.0.0.1:7878`.

## How the AI edits

MoodCut exposes **8 tools** whose definitions total about 5 KB, so connecting it costs little context:

| Tool | Purpose |
| --- | --- |
| `project` | new / open / save / undo / redo / history |
| `media` | import files, folders or globs; list media with duration, size and fps |
| `edit` | apply a **MoodCut Script**: many edits per call, all-or-nothing |
| `view` | compact text views: timeline, one item, media, markers, transcript |
| `preview` | one frame, or a labelled contact sheet of N frames as a small JPEG |
| `analyze` | silence, scenes, loudness, beats, transcript, black frames, exposure stats (cached) |
| `render` | export with a preset; progress and cancel |
| `help` | the reference, one topic at a time, generated from the engine's schema |

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

See the [MoodCut Script reference](docs/REFERENCE.md) (this is exactly what the AI gets from `help`) and the
[examples](examples/):

- [`motion-graphics.mcs`](examples/motion-graphics.mcs): a 12-second animated explainer with no footage
- [`talking-head.mcs`](examples/talking-head.mcs): jump cuts, grade, captions, lower third, music ducking
- [`beat-montage.mcs`](examples/beat-montage.mcs): beat-synced promo with titles and overlays
- [`vertical-short.mcs`](examples/vertical-short.mcs): turn an edit into a 9:16 Short/Reel

```sh
moodcut run explainer.mcut @examples/motion-graphics.mcs --create 1920x1080@30
moodcut render explainer.mcut
```

## Command line

| Command | |
| --- | --- |
| `moodcut` / `moodcut gui [project]` | open the editor (also serves MCP at `/mcp`) |
| `moodcut serve [--port 7878] [--public] [--token T] [--allow DIR]…` | headless editor + MCP server; `--public` listens on all interfaces and requires a token, `--allow` limits file access |
| `moodcut mcp` | MCP over stdio (for Claude Desktop / Claude Code) |
| `moodcut run <project> <script or @file> [--create WxH@fps]` | apply a script and save |
| `moodcut render <project> [-o out] [-p preset] [--range a-b] [--scale 0.5] [--hw auto]` | export |
| `moodcut frame <project> -t 3.5 -o frame.png` | render one frame |
| `moodcut doctor` / `moodcut setup` | check / install FFmpeg and optional tools |
| `moodcut install-claude` | register with Claude Desktop |

Environment: `MOODCUT_FFMPEG` (folder with ffmpeg/ffprobe), `MOODCUT_HOME` (data folder, default
`%APPDATA%\MoodCut`), `MOODCUT_WHISPER_BIN`, `MOODCUT_WHISPER_MODEL`.

## Building from source

Requires Rust (stable) and Node.js 20+.

```sh
cd gui && npm ci && npm run build && cd ..   # GUI -> gui/dist (embedded into the exe)
cargo build --release                         # target/release/moodcut(.exe)
cargo test
```

`npm run dev` in `gui/` serves the GUI with hot reload and proxies `/api` (including the WebSocket), `/mcp` and
`/fonts` to a running `moodcut serve`. `moodcut docs > docs/REFERENCE.md` regenerates the reference. The same
code builds and runs on Linux (tested) and macOS (untested).

## Architecture

One Rust executable contains the engine, MCP server, HTTP/WebSocket API, CLI and the embedded GUI. FFmpeg (a
separate process) handles decoding, encoding and audio, while a multi-threaded compositor does
everything visual. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the design and the reasons behind it.

## License

MIT. The bundled Inter font is under the SIL Open Font License (`crates/moodcut/assets/fonts/OFL.txt`).
