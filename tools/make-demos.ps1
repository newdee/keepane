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
    [string[]]$Only = @(),
    # The light pictures: recorded with `theme tokyo-day`, the phone page
    # light, drawn on a light window; written as <name>-light.*.
    [switch]$Light
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw "ffmpeg is not on the PATH" }

$takes = @(
    @{ Env = "KEEPANE_DEMO_OUT"; Test = "record_demo"; Name = "keepane-demo" },
    @{ Env = "KEEPANE_DEMO_OUT2"; Test = "record_alerts"; Name = "keepane-alerts" },
    @{ Env = "KEEPANE_DEMO_OUT3"; Test = "record_history"; Name = "keepane-history" },
    @{ Env = "KEEPANE_DEMO_OUT4"; Test = "record_messages"; Name = "keepane-messages" },
    @{ Env = "KEEPANE_DEMO_OUT6"; Test = "record_chart"; Name = "keepane-chart" },
    # Stand-in agents (tests/demo-agent.mjs, run by node): needs node on the PATH.
    @{ Env = "KEEPANE_DEMO_OUT7"; Test = "record_agents"; Name = "keepane-agents" },
    @{ Env = "KEEPANE_DEMO_OUT8"; Test = "record_fix"; Name = "keepane-fix" },
    @{ Env = "KEEPANE_DEMO_OUT9"; Test = "record_resume"; Name = "keepane-resume" },
    # The tour: a panel beside the terminal, and the phone (Edge through
    # puppeteer-core, as tools/make-phone-shots.ps1 does).
    @{ Env = "KEEPANE_DEMO_OUT5"; Test = "record_tour"; Name = "keepane-tour"; Panel = 380 }
)
if ($Only.Count -gt 0) {
    $unknown = $Only | Where-Object { $_ -notin $takes.Name }
    if ($unknown) { throw "no demo named $($unknown -join ', ') (there are $($takes.Name -join ', '))" }
    $takes = $takes | Where-Object { $_.Name -in $Only }
}
if ($takes.Name -contains "keepane-tour") {
    $npm = "target/phone-shots/npm"
    if (-not (Test-Path (Join-Path $npm "node_modules/puppeteer-core"))) {
        New-Item -ItemType Directory -Force $npm | Out-Null
        Set-Content (Join-Path $npm "package.json") '{"private":true,"type":"module"}' -Encoding utf8
        npm install --prefix $npm --no-audit --no-fund --silent puppeteer-core@24
        if ($LASTEXITCODE -ne 0) { throw "npm install puppeteer-core failed" }
    }
    $env:KEEPANE_PHONE_NPM = (Resolve-Path $npm).Path
}
$suffix = ""
if ($Light) {
    $env:KEEPANE_DEMO_THEME = "tokyo-day"
    $env:KEEPANE_PHONE_MODE = "light"
    $suffix = "-light"
} else {
    Remove-Item Env:KEEPANE_DEMO_THEME -ErrorAction SilentlyContinue
    Remove-Item Env:KEEPANE_PHONE_MODE -ErrorAction SilentlyContinue
}
foreach ($t in $takes) {
    $frames = Join-Path $Work "$($t.Name)$suffix-frames"
    $png = Join-Path $Work "$($t.Name)$suffix-png"
    foreach ($d in $frames, $png) {
        if (Test-Path $d) { Remove-Item -Recurse -Force $d }
        New-Item -ItemType Directory -Force $d | Out-Null
    }
    Set-Item "env:$($t.Env)" (Resolve-Path $frames).Path
    cargo test --release --test demo_frames -- --ignored --exact $t.Test --nocapture
    if ($LASTEXITCODE -ne 0) { throw "recording $($t.Test) failed" }
    $panel = if ($t.Panel) { $t.Panel } else { 0 }
    pwsh -NoProfile -File tools/render-frames.ps1 -In $frames -Out $png -Panel $panel -Light:$Light
    if ($LASTEXITCODE -ne 0) { throw "rendering $($t.Name) failed" }

    $pattern = Join-Path $png "f%04d.png"
    $scale = if ($Width -gt 0) { "scale=${Width}:-2:flags=lanczos" } else { "null" }
    ffmpeg -v error -y -framerate 5 -i $pattern -vf "$scale,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" "docs/img/$($t.Name)$suffix.gif"
    if ($LASTEXITCODE -ne 0) { throw "gif $($t.Name) failed" }
    ffmpeg -v error -y -framerate 5 -i $pattern -vf $scale -c:v libx264 -pix_fmt yuv420p -crf 26 -movflags +faststart "docs/img/$($t.Name)$suffix.mp4"
    if ($LASTEXITCODE -ne 0) { throw "mp4 $($t.Name) failed" }

    foreach ($still in Get-ChildItem $png -Filter "still-*.png") {
        $name = $still.BaseName.Substring("still-".Length)
        ffmpeg -v error -y -i $still.FullName -vf $scale "docs/img/$name$suffix.png"
        if ($LASTEXITCODE -ne 0) { throw "still $name failed" }
    }
}
Get-ChildItem docs/img | Sort-Object Name | Format-Table Name, Length, LastWriteTime -AutoSize
