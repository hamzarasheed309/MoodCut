# MoodCut for Windows

`windows-x64/moodcut.exe` is the complete program: editor, MCP server for Claude and ChatGPT, command line and
HTML graphics renderer in one file. It is built and tested by CI on Windows (unit tests, a render, the quality
check and exports on a machine without FFmpeg, where MoodCut installs its own); `windows-x64/BUILD.txt` names the
source commit and the exe's SHA-256.

## Install

1. Copy `moodcut.exe` to a folder of your choice, for example `C:\Tools\MoodCut\`. Adding that folder to your
   `PATH` lets AI clients and terminals start it by name.
2. Double-click it. Windows may show *"Windows protected your PC"* because the exe isn't code-signed:
   choose **More info → Run anyway** (once).
3. The editor opens. Click **Install FFmpeg** when MoodCut offers it (about 100 MB, no admin rights).
4. Connect your AI: **Connect AI** in the editor, or
   - Claude Desktop: `moodcut install-claude`, then restart Claude
   - claude.ai or ChatGPT on the web: `moodcut connect`, then paste the printed URL into the connector settings

Requirements: Windows 10 or 11 (64-bit) and Microsoft Edge (preinstalled). Everything MoodCut stores (settings,
the portable FFmpeg, caches) lives in `%APPDATA%\MoodCut`; projects live wherever you save them.

See the [main README](../README.md) for what MoodCut can do and how the AI drives it.
