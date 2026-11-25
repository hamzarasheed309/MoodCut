<!-- replaces-older-releases -->
MoodCut 1.1.0 adds After Effects-style masking and tracking, DaVinci Resolve-style color grading and audio mixing,
all of it controllable from AI apps over MCP. FFmpeg is now included, and AI apps get creative-direction guidance.

## Download

- **`MoodCut-Setup-x64.exe`**: the installer (recommended). No admin rights needed. **FFmpeg is included**, so
  video import, playback and export work as soon as setup finishes.
- **`MoodCut-v1.1.0-windows-x64-portable.zip`**: unzip anywhere and run `MoodCutApp.exe`. FFmpeg is in the zip's
  `ffmpeg` folder.
- `moodcut-linux-x64.tar.gz`: the Linux command line and MCP server (FFmpeg from your package manager).
- `SHA256SUMS.txt`: checksums of every file.

Windows may show "Windows protected your PC" because the files aren't code-signed yet: choose **More info → Run
anyway**.

## What's new

**Tracking**
- 3D camera tracker (`camtrack`): solves a shot's camera into a keyframed 3D camera and numbered track points. You
  can set the ground plane and place text, solids, nulls or shapes on the points. Tripod pans are solved as a
  camera that only rotates.
- Planar (perspective) tracking for corner pins and screen replacements. Masks track with After Effects' methods:
  position, rotation, scale, skew or perspective.
- Point tracking to a null, stabilizing, and lost-track ranges named in the reply.

**Masks and keys**
- Masks can cut out a region or limit an effect to it, combined with mask expressions such as `m1*-m2`.
- Feather, expand, per-vertex editing, converting shapes to paths, and HSL and luma qualifiers with matte finesse.
- Matte and overlay views show a mask's matte and its numbered vertices.
- Keyer refinements: light wrap and matte cleanup.

**Color**
- A node graph like DaVinci Resolve's: serial, parallel, layer-mixer and outside nodes, each with its own window,
  qualifier and mix.
- Primaries, color wheels and curves.
- Camera log input transforms, auto white balance, shot matching to a clip or a saved still, and shared grades.
- False color and scopes measured on a region of the frame.

**Audio**
- Buses, sends and automation.
- Parametric and multiband EQ, true-peak limiting and convolution reverb.
- Noise repair and room tone.
- Audio scopes (loudness, spectrum, phase) and stems export.

**Editor**
- A new Tracker panel.
- A Nodes section in Lumetri Color.
- A points view in the Scopes panel.
- Mixer bus strips and routing.

**For AI apps**
- `help direction` and `help features` guide AI apps through the work like a creative director: brief, concept and
  treatment first, then design and editing direction, then a review checklist. `help features` says when to use
  each feature and how to check its result.
- The guidance is tied to what the editor can actually do: AI apps are told to use only real commands and ids,
  describe only what they have seen, and report only what MoodCut confirmed. A test runs every example in both guides.
- A new `creative-direction` prompt runs the whole process.
- `check` now also reviews camera solves, keys, mask tracking, grades and the mix.

## Upgrading from 1.0.0

Install over the old version; projects open as before. FFmpeg is no longer a separate step in the installer (it is
always included); `install.ps1` ignores an `ffmpeg` entry in `MOODCUT_TASKS`.
