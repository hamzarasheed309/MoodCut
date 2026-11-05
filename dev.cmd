@echo off
rem Starts the MoodCut engine (port 7878) and the GUI dev server (Vite, hot reload)
rem together, then opens the editor in the browser. Double-click it or run `dev` from the repo root.
rem Close this window (or press Ctrl+C) to stop Vite; close the "MoodCut engine" window to stop the engine.
setlocal
cd /d "%~dp0"

set "ENGINE=target\release\moodcut.exe"
if not exist "%ENGINE%" (
  echo %ENGINE% not found. Build it first: cargo build --release
  pause
  exit /b 1
)

rem Vite proxies /api, /mcp and /fonts to 127.0.0.1:7878, so reuse an engine that is already there.
netstat -ano | findstr /r /c:"127.0.0.1:7878 .*LISTENING" >nul
if errorlevel 1 (
  echo Starting the MoodCut engine on http://127.0.0.1:7878 ...
  start "MoodCut engine" /min "%ENGINE%" serve
) else (
  echo The MoodCut engine is already running on port 7878.
)

where npm >nul 2>nul
if errorlevel 1 (
  echo npm not found on PATH. Install Node.js 20+ or add a portable Node to PATH.
  pause
  exit /b 1
)

cd gui
if not exist node_modules (
  echo Installing GUI dependencies...
  call npm ci || (pause & exit /b 1)
)

echo Starting the GUI dev server...
call npm run dev -- --open
