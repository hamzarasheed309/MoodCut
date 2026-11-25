# MoodCut for Windows

`windows-x64/MoodCut-Setup-x64.exe` is the installer. `windows-x64/MoodCutApp.exe` is the editor as a Windows app (its
own native window, no console) and `windows-x64/moodcut.exe` the same program as a command line: editor, MCP server for
Claude, ChatGPT and other AI apps, and HTML graphics renderer. Each works as a portable file. Both are built
and tested by CI on Windows (unit tests, a render, the quality check and exports on a machine without FFmpeg, and a
silent install, setup and uninstall of the installer); `windows-x64/BUILD.txt` names the source commit and the SHA-256
of each file.

## Install

**Installer (recommended):** run `MoodCut-Setup-x64.exe`. No admin rights are needed. FFmpeg is included, so nothing
else has to be downloaded or installed. Leave the options ticked to get shortcuts, `moodcut` on PATH, `.mcut` files
opening in MoodCut and your AI apps connected (Claude Desktop, Claude Code, Cursor, VS Code, Windsurf, LM Studio, Gemini CLI, Codex). Restart those apps
afterwards. Silent install for IT or scripts:

```
MoodCut-Setup-x64.exe /VERYSILENT /SUPPRESSMSGBOXES /TASKS="desktopicon,addtopath,assoc,connectai"
```

Add `/ALLUSERS` to install for everyone (asks for admin rights). Uninstall from Settings → Apps → MoodCut: it removes
MoodCut from the AI apps' settings and PATH too.

**Portable:** unzip `MoodCut-<version>-windows-x64-portable.zip` from the
[latest release](https://github.com/hamzarasheed309/MoodCut/releases/latest) anywhere (ideally a folder on your `PATH`)
and double-click `MoodCutApp.exe`. FFmpeg is in the zip's `ffmpeg` folder, where MoodCut looks first. Click
**Connect AI** to set up your AI apps. (A bare `MoodCutApp.exe` without that folder offers to download FFmpeg.)

Windows may show *"Windows protected your PC"* because the files aren't code-signed yet: choose **More info → Run
anyway** (once).

Requirements: Windows 10 (1803) or 11, 64-bit, and the Microsoft WebView2 runtime (preinstalled on Windows 11; the
installer adds it on Windows 10 if missing). The editor always runs in MoodCut's own native app window; if WebView2
is unavailable, MoodCut displays a setup error instead of opening a browser. The frontend, engine, and HTTP MCP
endpoint start together in the same app process. Everything MoodCut stores (settings,
proxies, caches, a downloaded FFmpeg if you ever used the bare exe) lives in `%APPDATA%\MoodCut`; projects live wherever
you save them.

See the [main README](../README.md) for what MoodCut can do and how the AI drives it.
