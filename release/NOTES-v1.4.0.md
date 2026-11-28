<!-- replaces-older-releases -->
MoodCut 1.4.0 moves AI Voice to ElevenLabs, adds AI Video (Runway, fal.ai and Higgsfield), and gives the editor the look and feel of
Premiere Pro.

## Download

- **`MoodCut-Setup-x64.exe`**: the installer (recommended). No admin rights needed. FFmpeg is included.
- **`MoodCut-v1.4.0-windows-x64-portable.zip`**: unzip anywhere and run `MoodCutApp.exe`. FFmpeg is in the zip's
  `ffmpeg` folder.
- `moodcut-linux-x64.tar.gz`: the Linux command line and MCP server (FFmpeg from your package manager).
- `SHA256SUMS.txt`: checksums of every file.

Windows may show "Windows protected your PC" because the files aren't code-signed yet: choose **More info → Run
anyway**.

## AI Voice, now with ElevenLabs

- Speech in seconds, in your own cloned voices or any voice from ElevenLabs' library, with search and a sample to
  listen to. 30+ languages; the Eleven v4, v3, Multilingual v2 and Flash models.
- Shape the delivery with **Stability**, **Similarity**, **Style** and **Speed**, make 1-3 takes, one clip per
  paragraph, at even loudness. Clips land in the Project panel and, if you like, at the playhead.
- **Clone a voice** from a clip's In/Out or a file (up to two minutes, with the speaker's permission; paid ElevenLabs
  plans).
- The header shows how many characters your plan has left; each job shows what it costs.
- No more 10 GB local install: the old OmniVoice setup is no longer used and can be deleted (by default
  `D:\MoodCut\OmniVoice` or another drive's `MoodCut\OmniVoice`).

## New: AI Video

- Generate shots with **Runway** (Gen-4.5, Veo 3.1, Kling 3.0 and 4, Seedance 2.5, Gemini Omni) or **fal.ai** (Veo
  3.1, Kling, Seedance, Hailuo, Wan, or any fal video model) or **Higgsfield** (its own Cinema Studio 4,
  Kling 3.0, Seedance 2.5, Hailuo 3).
- Text to video, or **start from a frame**: the Program monitor at the playhead, a still, a video frame or a file; most
  models also take a **last frame**, for transitions and moves that land on a composition.
- Duration, aspect, resolution and sound per model; the panel tells you what each model is good at and how long it
  usually takes.
- **Real progress**: the provider's percentage when it gives one, otherwise an estimate marked "~" with the time left;
  the queue position while waiting; the cost in credits. Up to three jobs run at once, and jobs still running when you
  close MoodCut are picked up when you open it again.
- The finished clip is downloaded, imported (tagged "ai-video", with the prompt as its note) and optionally placed on
  the timeline.

## Your keys, your accounts

- **Edit → AI Services** holds your ElevenLabs, Runway, fal.ai and Higgsfield API keys. They're tested when you save them, stored
  encrypted for your Windows user, and never shown to AI apps or web pages.
- Errors say what happened and what to do, with the button that fixes it: a rejected key (**Open AI Services**), no
  credits left (**Open billing**), a plan that can't clone, a busy service (**Retry**), a content filter, or a setting
  a model doesn't take.

## AI apps

- The `voice` tool speaks and clones through ElevenLabs; the new `video` tool generates shots, waits for them (or
  hands back a job id to `wait` on) and places them. Both report progress, cost and plain-language errors, and never
  ask for keys in chat. `help voice` and `help video` are their guides.

## Looks and works like Premiere Pro

- A **menu bar** with File, Edit, Clip, Sequence, Markers, Graphics, AI, View, Window and Help, every command with its
  shortcut. **Help → Keyboard Shortcuts** (F1) lists them all; Ctrl+O, Ctrl+I and Ctrl+Alt+N work too.
- Premiere's greys and blue: the focused panel is outlined in blue, workspace tabs and panel tabs are underlined.
- **Hot text** in Effect Controls: blue values you drag to scrub (Shift faster, Alt finer), click to type, or step
  with the arrow keys.
- The timeline: a timecode ruler, the playhead's timecode in blue, Premiere's label colors (violet video, teal audio,
  rose graphics), green markers, track headers with lock, eye, mute and solo icons, and the selected clip's track
  highlighted. Alt+wheel zooms around the pointer, smoothly on a trackpad.
- Monitors with Premiere's transport buttons: marker, In, Out, go to In/Out, step, play, lift and extract; insert and
  overwrite in the Source monitor.
- An **Audio Meters** panel beside the timeline, with a dB scale, peak hold and clip light.
- Workspaces were refreshed for these panels, so a custom arrangement from 1.3 is reset once.
