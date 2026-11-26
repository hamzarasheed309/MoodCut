<!-- replaces-older-releases -->
MoodCut 1.2.1 makes AI Voice work on 6 GB laptop GPUs and keeps your computer responsive while it runs.

## Download

- **`MoodCut-Setup-x64.exe`**: the installer (recommended). No admin rights needed. FFmpeg is included.
- **`MoodCut-v1.2.1-windows-x64-portable.zip`**: unzip anywhere and run `MoodCutApp.exe`. FFmpeg is in the zip's
  `ffmpeg` folder.
- `moodcut-linux-x64.tar.gz`: the Linux command line and MCP server (FFmpeg from your package manager).
- `SHA256SUMS.txt`: checksums of every file.

Windows may show "Windows protected your PC" because the files aren't code-signed yet: choose **More info → Run
anyway**.

## Fixed

- **Cloning a voice could run out of GPU memory** on 6 GB cards (such as an RTX 4050 Laptop GPU), after running for
  minutes. It now uses a fraction of the memory:
  - OmniVoice runs in inference mode, so no memory is spent on training data.
  - Whisper, which transcribes a reference you didn't type out, now runs on the CPU and is unloaded right after.
  - A clone without an end time now uses the first 10 seconds of the reference instead of 15.
  - If memory does run short, AI Voice frees what it can and tries once more.

## Gentler on your computer

- OmniVoice stays inside your graphics card's own memory. If something doesn't fit, it stops at once with a clear
  message, instead of spilling into system memory and slowing the whole PC for minutes.
- It runs at below-normal priority and leaves two CPU cores free, so the editor and other apps stay responsive.
- As before, the model unloads after 10 idle minutes and frees the GPU.

## Upgrading

Install over 1.2.0. Your OmniVoice download, the Python setup and your cloned voices are kept; nothing is downloaded
again.
