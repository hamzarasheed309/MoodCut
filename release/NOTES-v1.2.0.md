<!-- replaces-older-releases -->
MoodCut 1.2.0 adds **AI Voice**, a new editor tab. Clone a voice from a few seconds of speech, then type a script
and hear it in that voice. It runs on your own computer with [OmniVoice](https://huggingface.co/k2-fsa/OmniVoice),
and the speech goes straight into your project.

## Download

- **`MoodCut-Setup-x64.exe`**: the installer (recommended). No admin rights needed. FFmpeg is included.
- **`MoodCut-v1.2.0-windows-x64-portable.zip`**: unzip anywhere and run `MoodCutApp.exe`. FFmpeg is in the zip's
  `ffmpeg` folder.
- `moodcut-linux-x64.tar.gz`: the Linux command line and MCP server (FFmpeg from your package manager).
- `SHA256SUMS.txt`: checksums of every file.

Windows may show "Windows protected your PC" because the files aren't code-signed yet: choose **More info → Run
anyway**.

## What's new: AI Voice

Open it from **Window → AI Voice**. It is also part of the Editing, Audio and AI workspaces.

**Setup, once**
- MoodCut looks for the OmniVoice model by itself: on every drive, in usual model folders and in the Hugging Face cache.
- If it isn't there, choose **Locate** to point at a model folder you already have, or **Download**. Download asks
  which drive to use, shows each drive's free space, and puts everything in `<drive>\MoodCut\OmniVoice`:
  - the model (about 3.3 GB)
  - a private Python with PyTorch (about 6.5 GB; the GPU version when you have an NVIDIA card)

  Nothing is installed system-wide and your system drive stays free. The download shows its progress, can be
  cancelled, and continues where it stopped.

**Clone a voice**
- Use a clip from your project (mark In and Out in the Source monitor) or any audio or video file. 3 to 10 seconds
  of one person speaking clearly works best.
- You can type what is said in the clip. If you leave it empty, MoodCut transcribes it automatically (a one-time
  1.6 GB download).
- Confirm that it is your voice or that you have the speaker's permission.
- Cloned voices are saved once and work in every project.

**Generate speech**
- Choose a voice, type the script, and pick the language, speed and quality.
- A blank line starts a new clip, so a whole voice-over comes out as separate clips.
- Each clip lands in the **Project** panel, tagged `voice`, with its script as the note. You can also have the clips
  placed on the timeline at the playhead.
- Undo removes them like any other edit.

An NVIDIA GPU makes generation fast. It also works on the CPU, more slowly. The model unloads after 10 idle minutes
to free your GPU memory.

**License:** the OmniVoice model weights are licensed CC BY-NC 4.0, for non-commercial use only. Its audio tokenizer
is under the Boson Higgs Audio 2 Community License. MoodCut shows this before downloading.

## Upgrading from 1.1.0

Install over the old version; projects open as before.
