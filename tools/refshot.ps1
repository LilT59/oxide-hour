<#
.SYNOPSIS
  Reference screenshots of the retail game next to ours, for visual comparison.

.DESCRIPTION
  Captures one frame of a running game window with ffmpeg's gfxcapture filter (Windows.Graphics.Capture:
  real GPU frames, works while the window is covered). Capture only: no input, no injection.

  Workflow: start retail Zero Hour windowed (e.g. `generals.exe -win`), set up the scene by hand (map,
  camera), then run this script. With -Ours it also renders our viewer with -ViewerArgs and writes a
  side-by-side image.

  Screenshots contain game art: they are written outside the repo (default %TEMP%\zr-refshots) and the
  script refuses an output directory inside the repo (AGENTS.md: no game assets in the repo).

  Needs an ffmpeg with gfxcapture (ffmpeg 8+; e.g. `winget install Gyan.FFmpeg`). Running it with no
  window found lists the processes that own a window, to pick -Exe or -Title from.

.EXAMPLE
  pwsh tools/refshot.ps1 -Name alpine_base
.EXAMPLE
  pwsh tools/refshot.ps1 -Name alpine_base -Ours -ViewerArgs '--quick','--sim-frames','300'
.EXAMPLE
  pwsh tools/refshot.ps1 -Title '(?i)command & conquer' -Name menu
#>
param(
    # Regex on the window's executable name. Steam Zero Hour runs as game.dat (generals.exe is the launcher).
    [string]$Exe = '(?i)^(game[.]dat|generals[.]exe)$',
    # Regex on the window title; overrides -Exe when given. The default is the title because
    # gfxcapture's window_exe never matched Steam's Game.dat window (2026-10-13); pass -Title '' to use -Exe.
    [string]$Title = '(?i)generals zero hour',
    [string]$Name = (Get-Date -Format 'yyyyMMdd-HHmmss'),
    [string]$OutDir = (Join-Path $env:TEMP 'zr-refshots'),
    # Also render our viewer and write a side-by-side image.
    [switch]$Ours,
    [string[]]$ViewerArgs = @('--quick'),
    [string]$Viewer = (Join-Path $PSScriptRoot '..\target\slot-1\release\zr-viewer.exe')
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
New-Item -ItemType Directory -Force $OutDir | Out-Null
$out = (Resolve-Path $OutDir).Path
if ($out.StartsWith($repo, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to write screenshots inside the repo ($out): they contain game art."
}

if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw 'ffmpeg not found on PATH.' }
if (-not ((ffmpeg -hide_banner -filters 2>$null) -match 'gfxcapture')) {
    throw 'This ffmpeg has no gfxcapture filter (need ffmpeg 8+).'
}

# lavfi treats ':' and '\' specially; the default regexes avoid both.
$select = if ($Title) { "window_title=$Title" } else { "window_exe=$Exe" }
$retail = Join-Path $out "$Name.retail.png"
ffmpeg -hide_banner -loglevel error -y -f lavfi `
    -i "gfxcapture=${select}:capture_cursor=0:max_framerate=30,hwdownload,format=bgra" `
    -frames:v 1 $retail 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0 -or -not (Test-Path $retail)) {
    Write-Host "No matching window captured ($select). Windows open now:"
    Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } |
        Select-Object Id, ProcessName, MainWindowTitle | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
    Write-Host 'gfxcapture only delivers a frame when the window redraws: it must not be minimized.'
    exit 1
}
Write-Host "retail: $retail"

if ($Ours) {
    if (-not (Test-Path $Viewer)) {
        throw "Viewer not built: $Viewer (cargo build --release -p zr-viewer with CARGO_TARGET_DIR=<repo>/target/slot-1)."
    }
    $oursPng = Join-Path $out "$Name.ours.png"
    Push-Location $repo
    try { & $Viewer @ViewerArgs --screenshot $oursPng | Out-Null } finally { Pop-Location }
    if (-not (Test-Path $oursPng)) { throw "Viewer wrote no screenshot ($oursPng)." }
    Write-Host "ours:   $oursPng"

    # Side by side at the retail frame's height.
    $pair = Join-Path $out "$Name.pair.png"
    ffmpeg -hide_banner -loglevel error -y -i $retail -i $oursPng `
        -filter_complex '[1:v][0:v]scale2ref=oh*mdar:ih[o][r];[r][o]hstack=inputs=2' `
        -frames:v 1 $pair 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0) { Write-Host "pair:   $pair" } else { Write-Warning 'side-by-side failed; the two PNGs are still there.' }
}
