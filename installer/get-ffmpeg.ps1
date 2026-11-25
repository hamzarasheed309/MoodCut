# Puts a portable FFmpeg (ffmpeg.exe + ffprobe.exe, with its license) in dist\ffmpeg, where the
# installer and the portable zip pick it up, so MoodCut works the moment it is installed: nothing
# to download on first run. MoodCut finds it at <folder of moodcut.exe>\ffmpeg\bin.
#
#   pwsh installer\get-ffmpeg.ps1 [-Out dist\ffmpeg]
#
# FFmpeg is GPL software run as a separate program; dist\ffmpeg\SOURCE.txt records which build was
# bundled and where its source code is, next to the license that ships with it.
param(
    [string]$Out = "dist\ffmpeg"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # Invoke-WebRequest is many times slower with a progress bar

$builds = @(
    @{ Url = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"; By = "gyan.dev (release essentials build)"; Source = "https://www.gyan.dev/ffmpeg/builds/ (each build lists the exact FFmpeg source and library versions); FFmpeg source: https://ffmpeg.org/download.html" },
    @{ Url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"; By = "BtbN/FFmpeg-Builds (win64 gpl)"; Source = "https://github.com/BtbN/FFmpeg-Builds (build scripts and the exact source revisions); FFmpeg source: https://ffmpeg.org/download.html" }
)

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("moodcut-ffmpeg-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    $got = $null
    foreach ($b in $builds) {
        $zip = Join-Path $tmp "ffmpeg.zip"
        try {
            Write-Host "downloading $($b.Url)"
            Invoke-WebRequest -Uri $b.Url -OutFile $zip -UseBasicParsing -MaximumRedirection 10
            $x = Join-Path $tmp "x"
            if (Test-Path $x) { Remove-Item -Recurse -Force $x }
            Expand-Archive -Path $zip -DestinationPath $x -Force
            $exe = Get-ChildItem $x -Recurse -Filter ffmpeg.exe | Select-Object -First 1
            if (-not $exe -or -not (Test-Path (Join-Path $exe.DirectoryName "ffprobe.exe"))) { throw "no ffmpeg.exe and ffprobe.exe in the archive" }
            $lines = @(& $exe.FullName -hide_banner -version)
            $code = $LASTEXITCODE
            $first = "$($lines | Select-Object -First 1)"
            if ($code -ne 0 -or $first -notlike "ffmpeg version*") { throw "the downloaded ffmpeg.exe doesn't run" }
            $got = @{ Bin = $exe.DirectoryName; Root = (Split-Path $exe.DirectoryName -Parent); Version = $first; Build = $b }
            break
        } catch {
            Write-Warning "FFmpeg from $($b.Url) failed: $_"
        }
    }
    if (-not $got) { throw "couldn't download FFmpeg from any source" }

    if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
    New-Item -ItemType Directory -Force (Join-Path $Out "bin") | Out-Null
    Copy-Item (Join-Path $got.Bin "ffmpeg.exe"), (Join-Path $got.Bin "ffprobe.exe") (Join-Path $Out "bin")
    $lic = Get-ChildItem $got.Root -File | Where-Object { $_.Name -match '^(LICENSE|COPYING)' } | Select-Object -First 1
    if ($lic) { Copy-Item $lic.FullName (Join-Path $Out "LICENSE.txt") } else { throw "the FFmpeg build has no LICENSE file; refusing to bundle it without one" }
    @(
        "FFmpeg bundled with MoodCut",
        "",
        $got.Version,
        "Built by: $($got.Build.By)",
        "Downloaded from: $($got.Build.Url)",
        "Downloaded at: $((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))",
        "",
        "FFmpeg is free software by the FFmpeg developers (https://ffmpeg.org), licensed under the",
        "GNU General Public License version 3 for this build (it includes GPL libraries such as x264",
        "and x265); the license is in LICENSE.txt next to this file. MoodCut runs ffmpeg.exe and",
        "ffprobe.exe as separate programs and is not a derivative of FFmpeg.",
        "",
        "Source code: $($got.Build.Source)"
    ) | Set-Content -Encoding utf8 (Join-Path $Out "SOURCE.txt")
    $mb = [math]::Round(((Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum) / 1MB, 1)
    Write-Host "bundled $($got.Version) in $Out ($mb MB)"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
