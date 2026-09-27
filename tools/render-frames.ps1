<#
.SYNOPSIS
  Render the frames recorded by tests/demo_frames.rs as PNGs.

.DESCRIPTION
  Each frame is a JSON grid of coloured text runs taken from a real keepane
  session. This draws them with a monospace font inside a terminal-looking
  window, so the result is a picture of what the terminal actually showed.

.EXAMPLE
  $env:KEEPANE_DEMO_OUT = "target/demo-frames"
  cargo test --release --test demo_frames -- --ignored --nocapture
  pwsh -File tools/render-frames.ps1 -In target/demo-frames -Out target/demo-png
  # f0001.png ... for the animation, still-<name>.png for the pictures.
#>
[CmdletBinding()]
param(
    [string]$In = "target/demo-frames",
    [string]$Out = "target/demo-png",
    # Falls back to Cascadia Mono where Maple Mono is not installed.
    [string]$FontName = "Maple Mono NF CN",
    [double]$FontSize = 15,
    # Only render these frame numbers (1-based). Empty means all of them.
    [int[]]$Only = @(),
    [string]$Title = "keepane",
    # Width of a panel beside the terminal for frames that carry a `side`
    # (the tour: a chapter's title and lines, or the phone); 0: none.
    [int]$Panel = 0
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$installed = (New-Object System.Drawing.Text.InstalledFontCollection).Families.Name
if ($installed -notcontains $FontName) {
    Write-Warning "$FontName is not installed; using Cascadia Mono"
    $FontName = "Cascadia Mono"
}

# Tokyo Night, the palette the recordings' keepane theme (themes/tokyo-night.conf)
# is made for.
$palette = @(
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
    "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"
)
$defaultFg = "#c0caf5"
$defaultBg = "#1a1b26"
$chromeBg = "#16161e"

function To-Color([string]$spec, [string]$fallback) {
    if (-not $spec -or $spec -eq "default") { $spec = $fallback }
    if ($spec.StartsWith("#")) { return [System.Drawing.ColorTranslator]::FromHtml($spec) }
    $i = [int]$spec
    if ($i -lt 16) { return [System.Drawing.ColorTranslator]::FromHtml($palette[$i]) }
    if ($i -ge 232) {
        # 24-step grey ramp
        $v = 8 + 10 * ($i - 232)
        return [System.Drawing.Color]::FromArgb($v, $v, $v)
    }
    # 6x6x6 colour cube
    $n = $i - 16
    $steps = @(0, 95, 135, 175, 215, 255)
    return [System.Drawing.Color]::FromArgb($steps[[math]::Floor($n / 36)], $steps[[math]::Floor(($n % 36) / 6)], $steps[$n % 6])
}

$inDir = (Resolve-Path $In).Path
$frames = @(Get-ChildItem $inDir -Filter "*.json" | Where-Object { $_.Name -match '^(f\d+|still-.+)\.json$' } | Sort-Object Name)
if ($frames.Count -eq 0) { throw "no frames in $inDir" }
New-Item -ItemType Directory -Force $Out | Out-Null
$outDir = (Resolve-Path $Out).Path

# Cell metrics: a monospace font, measured once on a long run of characters.
$probe = New-Object System.Drawing.Bitmap 10, 10
$pg = [System.Drawing.Graphics]::FromImage($probe)
$pg.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
$font = New-Object System.Drawing.Font $FontName, $FontSize, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$fontBold = New-Object System.Drawing.Font $FontName, $FontSize, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
$fmt = [System.Drawing.StringFormat]::GenericTypographic
$sample = "M" * 40
$cellW = [math]::Round($pg.MeasureString($sample, $font, [System.Drawing.PointF]::new(0, 0), $fmt).Width / 40, 2)
$cellH = [math]::Ceiling($font.GetHeight($pg)) + 2
$pg.Dispose(); $probe.Dispose()

$pad = 18            # padding around the grid
$barH = 30           # window title bar

$first = Get-Content $frames[0].FullName -Raw | ConvertFrom-Json
$gridW = [int][math]::Ceiling($cellW * $first.cols)
$gridH = [int]($cellH * $first.rows)
$termW = $gridW + 2 * $pad
$imgW = $termW + $Panel
$imgH = $gridH + 2 * $pad + $barH

# The panel beside the terminal: the chapter's number and title, then either
# its lines or the phone's picture in a phone-shaped frame.
$sideFont = New-Object System.Drawing.Font "Segoe UI", 17, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$sideTitle = New-Object System.Drawing.Font "Segoe UI Semibold", 27, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$sideNum = New-Object System.Drawing.Font "Segoe UI Semibold", 15, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$phones = @{}
function Draw-Side($g, $side) {
    $x0 = $termW + 26
    $w = $Panel - 48
    $accent = [System.Drawing.ColorTranslator]::FromHtml("#7aa2f7")
    $text = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml("#c0caf5"))
    $muted = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml("#a9b1d6"))
    # The chapter's number as a chip, then its title.
    $chip = New-Object System.Drawing.SolidBrush $accent
    $g.FillRectangle($chip, [float]$x0, [float]($barH + 22), 44, 26)
    $dark = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml("#1a1b26"))
    $g.DrawString(('{0:00}' -f [int]$side.n), $sideNum, $dark, [float]($x0 + 10), [float]($barH + 25))
    $g.DrawString($side.title, $sideTitle, $text, [float]($x0 - 2), [float]($barH + 56))
    $y = $barH + 108
    if ($side.phone) {
        if (-not $phones.ContainsKey($side.phone)) { $phones[$side.phone] = [System.Drawing.Image]::FromFile($side.phone) }
        $img = $phones[$side.phone]
        $h = $imgH - $y - 20
        $pw = [int]($h * $img.Width / $img.Height)
        $px = $termW + ($Panel - $pw) / 2
        # The phone: a dark body a little larger than its screen.
        $body = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml("#0c0d12"))
        $path = New-Object System.Drawing.Drawing2D.GraphicsPath
        $r = 26; $bx = $px - 8; $by = $y - 8; $bw = $pw + 16; $bh = $h + 16
        $path.AddArc($bx, $by, $r, $r, 180, 90); $path.AddArc($bx + $bw - $r, $by, $r, $r, 270, 90)
        $path.AddArc($bx + $bw - $r, $by + $bh - $r, $r, $r, 0, 90); $path.AddArc($bx, $by + $bh - $r, $r, $r, 90, 90)
        $path.CloseFigure()
        $g.FillPath($body, $path)
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.DrawImage($img, [float]$px, [float]$y, [float]$pw, [float]$h)
        $body.Dispose(); $path.Dispose()
    } else {
        foreach ($line in $side.lines) {
            $g.DrawString($line, $sideFont, $muted, [float]$x0, [float]$y)
            $y += 27
        }
    }
    $text.Dispose(); $muted.Dispose(); $chip.Dispose(); $dark.Dispose()
}

foreach ($file in $frames) {
    $n = if ($file.BaseName -match '^f(\d+)$') { [int]$Matches[1] } else { 0 }
    if ($Only.Count -gt 0 -and $Only -notcontains $n) { continue }
    $frame = Get-Content $file.FullName -Raw | ConvertFrom-Json
    $bmp = New-Object System.Drawing.Bitmap $imgW, $imgH
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

    # Window chrome: title bar with the usual three dots.
    $g.Clear([System.Drawing.ColorTranslator]::FromHtml($chromeBg))
    $bgBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml($defaultBg))
    $g.FillRectangle($bgBrush, 0, $barH, $termW, $imgH - $barH)
    foreach ($dot in @(@{x = 18; c = "#e06c75" }, @{x = 38; c = "#e5c07b" }, @{x = 58; c = "#98c379" })) {
        $b = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml($dot.c))
        $g.FillEllipse($b, $dot.x - 5, ($barH / 2) - 5, 10, 10)
        $b.Dispose()
    }
    $titleBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml("#565f89"))
    $titleFont = New-Object System.Drawing.Font "Segoe UI", 12, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
    $titleSize = $g.MeasureString($Title, $titleFont)
    $g.DrawString($Title, $titleFont, $titleBrush, ($termW - $titleSize.Width) / 2, ($barH - $titleSize.Height) / 2)

    $y = 0
    foreach ($line in $frame.lines) {
        $x = 0.0
        foreach ($run in $line) {
            $text = $run.t
            if (-not $text) { $x += 0; continue }
            $fg = To-Color $run.fg $defaultFg
            $bg = To-Color $run.bg $defaultBg
            if ($run.i) { $tmp = $fg; $fg = $bg; $bg = $tmp }
            $w = $cellW * $text.Length
            if ("$($run.bg)" -ne "default" -or $run.i) {
                $b = New-Object System.Drawing.SolidBrush $bg
                $g.FillRectangle($b, [float]($pad + $x), [float]($barH + $pad + $y * $cellH), [float]$w, [float]$cellH)
                $b.Dispose()
            }
            $b = New-Object System.Drawing.SolidBrush $fg
            $f = if ($run.b) { $fontBold } else { $font }
            $g.DrawString($text, $f, $b, [float]($pad + $x), [float]($barH + $pad + $y * $cellH), $fmt)
            $b.Dispose()
            $x += $w
        }
        $y++
    }

    # A block cursor, so the recording looks alive.
    if ($frame.cursor_visible) {
        $cx = $frame.cursor[0]; $cy = $frame.cursor[1]
        $b = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml($defaultFg))
        $g.FillRectangle($b, [float]($pad + $cellW * $cx), [float]($barH + $pad + $cy * $cellH), [float]$cellW, [float]$cellH)
        $b.Dispose()
    }

    if ($Panel -gt 0 -and $frame.side) { Draw-Side $g $frame.side }

    $g.Dispose()
    $bmp.Save((Join-Path $outDir ($file.BaseName + ".png")), [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

Write-Host "rendered $(@(Get-ChildItem $outDir -Filter *.png).Count) frames at ${imgW}x${imgH} into $outDir"
