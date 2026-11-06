# Makes MoodCutApp.exe: a copy of moodcut.exe that runs as a Windows GUI program, so opening
# the editor from the Start menu, the desktop or a .mcut file never flashes a console window.
# (moodcut.exe stays a console program: terminals, scripts and AI apps need its stdin/stdout.)
#
# The only difference is the "subsystem" field in the PE header: 3 = console, 2 = windows.
#
#   pwsh installer\make-app-exe.ps1 [-In target\release\moodcut.exe] [-Out target\release\MoodCutApp.exe]
param(
    [string]$In = "target\release\moodcut.exe",
    [string]$Out = "target\release\MoodCutApp.exe"
)
$ErrorActionPreference = "Stop"
$bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $In))
$pe = [BitConverter]::ToInt32($bytes, 0x3C)
if ([System.Text.Encoding]::ASCII.GetString($bytes, $pe, 2) -ne "PE") { throw "$In is not a Windows executable" }
# PE signature (4) + COFF header (20) + 68 bytes into the optional header (same for PE32 and PE32+)
$sub = $pe + 24 + 68
if ([BitConverter]::ToUInt16($bytes, $sub) -ne 3) { throw "$In is not a console program (subsystem $([BitConverter]::ToUInt16($bytes, $sub)))" }
$bytes[$sub] = 2
$bytes[$sub + 1] = 0
[System.IO.File]::WriteAllBytes($Out, $bytes)
Write-Host "wrote $Out (windows subsystem)"
