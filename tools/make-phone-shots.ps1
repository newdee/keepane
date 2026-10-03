<#
.SYNOPSIS
  Pictures of `keepane web` on a phone, for the README and the site.

.DESCRIPTION
  Starts a scratch server with a few panes doing something, runs
  `keepane web` on this machine only, and has Edge show the page as an iPhone
  would (390x844, touch, twice the pixels; tools/phone-shot.mjs through
  puppeteer-core, fetched into target/ on first use): the pane list, one
  pane, and the two side by side. Needs node, ffmpeg and Edge. Writes
  docs/img/phone.png (English page) and docs/img/phone-zh.png (Chinese).

.EXAMPLE
  pwsh -File tools/make-phone-shots.ps1
#>
[CmdletBinding()]
param(
    [string]$Work = "target/phone-shots",
    # The page light: docs/img/phone-light.png and phone-zh-light.png.
    [switch]$Light
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
$repo = (Get-Location).Path
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { throw "ffmpeg is not on the PATH" }
$edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { throw "Microsoft Edge not found" }
cargo build
if ($LASTEXITCODE -ne 0) { throw "build failed" }
$w = Join-Path $repo "target\debug\keepane.exe"

New-Item -ItemType Directory -Force $Work | Out-Null
$tmp = (Resolve-Path $Work).Path
$env:KEEPANE_SESSIONS_DIR = Join-Path $tmp "sessions"
$env:KEEPANE_CONFIG = Join-Path $tmp "empty.conf"
# The light pictures have the light terminal theme too.
Set-Content $env:KEEPANE_CONFIG $(if ($Light) { "set -g theme tokyo-day" } else { "" })
Remove-Item Env:KEEPANE -ErrorAction SilentlyContinue
Remove-Item Env:KEEPANE_PANE -ErrorAction SilentlyContinue

# Short prompts and nothing from this machine's history.
$prompt = Join-Path $tmp "prompt.ps1"
Set-Content $prompt -Encoding utf8 -Value @"
function global:prompt { 'PS> ' }
try { Set-PSReadLineOption -PredictionSource None -HistorySaveStyle SaveNothing } catch {}
Set-Location '$repo'
# A shell started with a script gets no prompt hook from keepane; this one
# installs it, so the pane reports its commands (the times in the picture).
Invoke-Expression (& '$w' __shell-hook | Out-String)
Clear-Host
"@
$shell = @("pwsh.exe", "-NoLogo", "-NoProfile", "-NoExit", "-File", $prompt)
$socket = "phone-shots"
& $w -L $socket kill-server 2>$null
& $w -L $socket new -d -s dev -n build -x 70 -y 40 @shell
# A background window that prints gets its alert mark, in the list too.
& $w -L $socket set -g monitor-activity on
& $w -L $socket split-window -v -t dev @shell
& $w -L $socket new-window -d -t dev -n logs @shell
& $w -L $socket new -d -s ops -n deploy -x 70 -y 40 @shell
function Wait-Screen($target, $what, [scriptblock]$ok) {
    $deadline = (Get-Date).AddSeconds(30)
    while (-not (& $ok ((& $w -L $socket capture-pane -p -t $target) -join "`n"))) {
        if ((Get-Date) -gt $deadline) { throw "timed out waiting for $what in $target" }
        Start-Sleep -Milliseconds 200
    }
}
# Type only into a shell that is up (its start clears the screen), and
# wait for what the line prints.
function Type-Line($target, $text, $expect) {
    # capture-pane drops the prompt's trailing space; empty rows follow it.
    Wait-Screen $target "the prompt" { param($s) $s -match "PS>\s*$" }
    & $w -L $socket send-keys -t $target -l -- $text
    & $w -L $socket send-keys -t $target Enter
    Wait-Screen $target "the output" { param($s) $s -match $expect }
}
Type-Line "dev:0.0" "Test-Path Cargo.toml" "True"
Type-Line "dev:0.0" "git --no-pager log --oneline --graph --decorate --color=always -12" "\* [0-9a-f]{7}.*\n.*\* [0-9a-f]{7}"
Type-Line "dev:0.1" "git status -sb | Select-Object -First 5" "## "
Type-Line "dev:1" "Get-ChildItem src | Select-Object -First 8 Name, Length" "Length"
# Something running, so the list names a program and not only shells.
Type-Line "ops:0" "ping -n 600 127.0.0.1" "127\.0\.0\.1.*(time|时间)"
Start-Sleep 1

$out = Join-Path $tmp "web.txt"
$web = Start-Process -FilePath $w -ArgumentList "-L", $socket, "web", "--bind", "127.0.0.1", "--port", "17682" `
    -RedirectStandardOutput $out -PassThru -WindowStyle Hidden
try {
    Start-Sleep 2
    $key = [regex]::Match((Get-Content $out -Raw), "#k=([A-Za-z0-9_-]+)").Groups[1].Value
    if (-not $key) { throw "keepane web printed no key: $(Get-Content $out -Raw)" }
    $pane = (& $w -L $socket list-panes -t dev:0 -F "#{pane_id}" | Select-Object -First 1).TrimStart("%")
    $shots = @(
        @{ Name = "phone-list"; View = "list"; Url = "http://127.0.0.1:17682/#k=$key" },
        @{ Name = "phone-pane"; View = "pane-detail"; Url = "http://127.0.0.1:17682/#k=$key&p=$pane" }
    )
    # puppeteer-core drives this machine's Edge as a phone would show the
    # page; it lives under target/, fetched on first use.
    $npm = Join-Path $tmp "npm"
    if (-not (Test-Path (Join-Path $npm "node_modules\puppeteer-core"))) {
        New-Item -ItemType Directory -Force $npm | Out-Null
        Set-Content (Join-Path $npm "package.json") '{"private":true,"type":"module"}' -Encoding utf8
        npm install --prefix $npm --no-audit --no-fund --silent puppeteer-core@24
        if ($LASTEXITCODE -ne 0) { throw "npm install puppeteer-core failed" }
    }
    Copy-Item tools/phone-shot.mjs $npm -Force
    $env:KEEPANE_PHONE_MODE = if ($Light) { "light" } else { "dark" }
    $suffix = if ($Light) { "-light" } else { "" }
    # The page in English for the README and the site, in Chinese for
    # README.zh-CN; each view drawn alone, then the two side by side on the
    # page's own background.
    foreach ($lang in @(@{ Code = "en"; Suffix = "" }, @{ Code = "zh-CN"; Suffix = "-zh" })) {
        foreach ($shot in $shots) {
            node (Join-Path $npm "phone-shot.mjs") $edge $shot.Url (Join-Path $tmp "$($shot.Name).png") $shot.View $lang.Code
            if ($LASTEXITCODE -ne 0) { throw "picture $($shot.Name) failed" }
        }
        # Put side by side on the page's own background (its top-left pixel).
        Add-Type -AssemblyName System.Drawing
        $first = [System.Drawing.Bitmap]::FromFile((Join-Path $tmp "phone-list.png"))
        $px = $first.GetPixel(2, $first.Height - 2); $first.Dispose()
        $bg = "0x{0:x2}{1:x2}{2:x2}" -f $px.R, $px.G, $px.B
        ffmpeg -v error -y -i (Join-Path $tmp "phone-list.png") -i (Join-Path $tmp "phone-pane.png") -filter_complex `
            "[0]pad=iw+60:ih:30:0:color=$bg[a];[1]pad=iw+30:ih:0:0:color=$bg[b];[a][b]hstack,scale=iw/2:-1:flags=lanczos" `
            "docs/img/phone$($lang.Suffix)$suffix.png"
        if ($LASTEXITCODE -ne 0) { throw "combine failed" }
    }
} finally {
    Stop-Process -Id $web.Id -Force -ErrorAction SilentlyContinue
    & $w -L $socket kill-server 2>$null
}
Get-ChildItem docs/img/phone*.png | Format-Table Name, Length -AutoSize
