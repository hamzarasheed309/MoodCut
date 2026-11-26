<!-- replaces-older-releases -->
MoodCut 1.3.0 makes AI Voice clearer, more reliable and better sounding, and lets AI apps use it too.

## Download

- **`MoodCut-Setup-x64.exe`**: the installer (recommended). No admin rights needed. FFmpeg is included.
- **`MoodCut-v1.3.0-windows-x64-portable.zip`**: unzip anywhere and run `MoodCutApp.exe`. FFmpeg is in the zip's
  `ffmpeg` folder.
- `moodcut-linux-x64.tar.gz`: the Linux command line and MCP server (FFmpeg from your package manager).
- `SHA256SUMS.txt`: checksums of every file.

Windows may show "Windows protected your PC" because the files aren't code-signed yet: choose **More info → Run
anyway**.

## What's new in AI Voice

**You can see what it's doing**
- Every clone and every line of speech shows a progress bar, what it is doing ("loading the model", "transcribing",
  "speaking part 2 of 3") and how long it has taken.
- The first Whisper download shows how much has arrived.

**Errors you can act on**
- Problems are explained in plain words with what to do: out of graphics memory, a broken or moved setup, no speech in
  the reference, no internet. A **Retry** or **Repair** button sits right there, and **Details** keeps the technical
  text for when you need it.
- **Repair** fixes the setup in place without downloading the model again. A setup folder that was moved to another
  drive is now repaired automatically the first time it's used.

**Better clones**
- Leave **To** empty and MoodCut picks the best 3-12 seconds of speech in the first 30 seconds, cut at pauses so no
  word is chopped.
- The transcript of every voice is shown, and **Fix transcript** corrects a misheard word. A correct transcript gives a
  more faithful voice.
- **Reduce background noise** helps with phone and room recordings.

**Better speech**
- **Takes**: generate 2 or 3 readings and keep the one you like. Each take has a seed, saved in the clip's note, so a
  reading can be repeated.
- Numbers are read as words: "2026" becomes "twenty twenty-six" and "40%" becomes "forty percent".
- **Even loudness**: every clip comes out at the same level (about -16 LUFS), even from a quiet phone recording.

**For AI apps**
- A new `voice` tool lets Claude and other AI apps clone voices, speak scripts and place the clips on the timeline,
  set up or repair OmniVoice, and follow the same progress you see. Cloning always needs the speaker's permission.
- `help voice` explains how to get good results.

## Measured on an RTX 4050 Laptop GPU (6 GB)

| Step | Time | GPU memory |
| --- | --- | --- |
| Loading the model (first use) | about 15 s | 1.9 GB |
| Cloning, with automatic transcription | about 9 s | 3.6 GB at most |
| Cloning, with a typed transcript | under 1 s | |
| One sentence of speech | about 3 s | |
| A 43-second narration in three parts | about 5 s | |

## Upgrading

Install over 1.2.x. Your OmniVoice download and your voices are kept. The first time AI Voice starts it adds one small
package (a few seconds), and a setup folder that was moved is repaired automatically.
