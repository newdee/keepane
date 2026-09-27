<#
.SYNOPSIS
  Re-record the demos and re-cut every picture in docs/img from them.

.DESCRIPTION
  1. tests/demo_frames.rs plays four scripted sessions into the real keepane.exe
     and writes each screen as JSON: f0001.json ... for the animation, and
     still-<name>.json wherever the script marks a picture.
  2. tools/render-frames.ps1 draws them as PNGs.
  3. ffmpeg turns the frames into the GIF and MP4 (5 frames a second, as
     recorded). Nothing is scaled unless -Width is given: text drawn at its
     own size stays sharp, and the pages size the pictures themselves.

  Needs ffmpeg on the PATH. Takes a couple of minutes: the sessions run in
  real time.

.EXAMPLE
  pwsh -File tools/make-demos.ps1
.EXAMPLE
  pwsh -File tools/make-demos.ps1 -Only keepane-messages   # one demo, the others left as they are
#>
[CmdletBinding()]
param(
    # 0 keeps the rendered size.
    [int]$Width = 0,
    [string]$Work = "target/demos",
    # Only these demos (by name, such as keepane-messages); all when empty.
    [string[]]$Only = @()
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw "ffmpeg is not on the PATH" }

$takes = @(
    @{ Env = "KEEPANE_DEMO_OUT"; Test = "record_demo"; Name = "keepane-demo" },
    @{ Env = "KEEPANE_DEMO_OUT2"; Test = "record_alerts"; Name = "keepane-alerts" },
    @{ Env = "KEEPANE_DEMO_OUT3"; Test = "record_history"; Name = "keepane-history" },
    @{ Env = "KEEPANE_DEMO_OUT4"; Test = "record_messages"; Name = "keepane-messages" }
)
if ($Only.Count -gt 0) {
    $unknown = $Only | Where-Object { $_ -notin $takes.Name }
    if ($unknown) { throw "no demo named $($unknown -join ', ') (there are $($takes.Name -join ', '))" }
    $takes = $takes | Where-Object { $_.Name -in $Only }
}
foreach ($t in $takes) {
    $frames = Join-Path $Work "$($t.Name)-frames"
    $png = Join-Path $Work "$($t.Name)-png"
    foreach ($d in $frames, $png) {
        if (Test-Path $d) { Remove-Item -Recurse -Force $d }
        New-Item -ItemType Directory -Force $d | Out-Null
    }
    Set-Item "env:$($t.Env)" (Resolve-Path $frames).Path
    cargo test --release --test demo_frames -- --ignored --exact $t.Test --nocapture
    if ($LASTEXITCODE -ne 0) { throw "recording $($t.Test) failed" }
    pwsh -NoProfile -File tools/render-frames.ps1 -In $frames -Out $png
    if ($LASTEXITCODE -ne 0) { throw "rendering $($t.Name) failed" }

    $pattern = Join-Path $png "f%04d.png"
    $scale = if ($Width -gt 0) { "scale=${Width}:-2:flags=lanczos" } else { "null" }
    ffmpeg -v error -y -framerate 5 -i $pattern -vf "$scale,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" "docs/img/$($t.Name).gif"
    if ($LASTEXITCODE -ne 0) { throw "gif $($t.Name) failed" }
    ffmpeg -v error -y -framerate 5 -i $pattern -vf $scale -c:v libx264 -pix_fmt yuv420p -crf 26 -movflags +faststart "docs/img/$($t.Name).mp4"
    if ($LASTEXITCODE -ne 0) { throw "mp4 $($t.Name) failed" }

    foreach ($still in Get-ChildItem $png -Filter "still-*.png") {
        $name = $still.BaseName.Substring("still-".Length)
        ffmpeg -v error -y -i $still.FullName -vf $scale "docs/img/$name.png"
        if ($LASTEXITCODE -ne 0) { throw "still $name failed" }
    }
}
Get-ChildItem docs/img | Sort-Object Name | Format-Table Name, Length, LastWriteTime -AutoSize
