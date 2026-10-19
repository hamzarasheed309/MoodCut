# MoodCut one-line installer for Windows 10/11.
#
#   irm https://raw.githubusercontent.com/hamzarasheed309/MoodCut/HEAD/install.ps1 | iex
#
# Downloads the latest MoodCut installer and runs it for the current user (no
# admin rights): Start menu and desktop shortcuts, `moodcut` on PATH, .mcut
# files, a portable FFmpeg, and every AI app on this PC connected to MoodCut.
# Choose the steps with $env:MOODCUT_TASKS before running, e.g.
#   $env:MOODCUT_TASKS = "addtopath,ffmpeg"
# Everything is undone by Settings > Apps > MoodCut > Uninstall.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is much faster without its progress bar
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'hamzarasheed309/MoodCut'
$tasks = if ($env:MOODCUT_TASKS) { $env:MOODCUT_TASKS } else { 'desktopicon,addtopath,assoc,ffmpeg,connectai' }
$setup = Join-Path $env:TEMP 'MoodCut-Setup-x64.exe'
$sources = @(
  "https://github.com/$repo/releases/latest/download/MoodCut-Setup-x64.exe",
  "https://github.com/$repo/raw/HEAD/release/windows-x64/MoodCut-Setup-x64.exe"
)

if (-not [Environment]::Is64BitOperatingSystem) { throw 'MoodCut needs 64-bit Windows 10 or 11.' }

Write-Host ''
Write-Host '  MoodCut - the AI-native video editor' -ForegroundColor DarkYellow
Write-Host ''
$got = $false
foreach ($url in $sources) {
  try {
    Write-Host "Downloading the installer from $url ..."
    Invoke-WebRequest -Uri $url -OutFile $setup -UseBasicParsing
    if ((Get-Item $setup).Length -gt 1MB) { $got = $true; break }
  } catch {
    Write-Host "  not available there ($($_.Exception.Message))" -ForegroundColor DarkGray
  }
}
if (-not $got) { throw "Couldn't download MoodCut. Get it from https://github.com/$repo/releases" }

# Windows marks downloads; the installer isn't code-signed yet
Unblock-File $setup -ErrorAction SilentlyContinue

Write-Host "Installing (steps: $tasks) ..."
$p = Start-Process $setup -ArgumentList @('/SILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', "/TASKS=`"$tasks`"") -Wait -PassThru
Remove-Item $setup -ErrorAction SilentlyContinue
if ($p.ExitCode -ne 0) { throw "The installer stopped with code $($p.ExitCode)." }

$exe = Join-Path $env:LOCALAPPDATA 'Programs\MoodCut\moodcut.exe'
Write-Host ''
Write-Host 'MoodCut is installed.' -ForegroundColor Green
if (Test-Path $exe) {
  & $exe install-ai --list
  Write-Host ''
  Write-Host 'Restart your AI apps to load MoodCut, then ask: "use MoodCut to edit my footage".'
  Write-Host 'claude.ai / ChatGPT on the web: open MoodCut > Connect AI > Create web link.'
  Start-Process $exe -WindowStyle Minimized
}
