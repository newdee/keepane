<#
.SYNOPSIS
  Build the keepane MSI from an already built release binary.

.DESCRIPTION
  Stages the files the installer ships (keepane.exe, conpty.dll,
  OpenConsole.exe, README.md, LICENSE, ConPTY-LICENSE, keepane.conf.example) into a temporary directory and runs WiX over
  installer/keepane.wxs. WiX 3.14 is used from -WixBin, from PATH, from the usual
  "WiX Toolset v3.x" install, or downloaded (no install) when none is found.

.EXAMPLE
  cargo build --release
  pwsh -File installer/build-msi.ps1 -Version 0.10.0
  pwsh -File installer/build-msi.ps1 -Scope user    # per user: no administrator rights to install
#>
[CmdletBinding()]
param(
    # Product version; must be x.y.z (MSI does not take anything else).
    [string]$Version,
    # Directory holding the built keepane.exe.
    [string]$ExeDir = "target/release",
    # Where the .msi lands.
    [string]$OutDir = "target",
    # Directory holding candle.exe / light.exe.
    [string]$WixBin,
    # machine: Program Files, system PATH (needs administrator rights);
    # user: %LOCALAPPDATA%\Programs, the user's PATH (does not).
    [ValidateSet("machine", "user")] [string]$Scope = "machine"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

if (-not $Version) {
    $Version = (Select-String -Path "$root/Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    throw "version must be x.y.z, got '$Version'"
}

function Resolve-Wix {
    if ($WixBin) { return $WixBin }
    $candle = Get-Command candle.exe -ErrorAction SilentlyContinue
    if ($candle) { return (Split-Path -Parent $candle.Source) }
    $installed = Get-ChildItem "${env:ProgramFiles(x86)}" -Filter "WiX Toolset v3.*" -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
    if ($installed -and (Test-Path "$($installed.FullName)/bin/candle.exe")) { return "$($installed.FullName)/bin" }
    # No WiX anywhere: use the official binaries zip, unpacked next to the build.
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) "wix314-binaries"
    if (-not (Test-Path "$dir/candle.exe")) {
        $zip = "$dir.zip"
        Write-Host "downloading WiX 3.14 binaries..."
        Invoke-WebRequest -Uri "https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip" -OutFile $zip
        Expand-Archive -Path $zip -DestinationPath $dir -Force
    }
    return $dir
}

$wix = Resolve-Wix
$exe = Join-Path $(if ([IO.Path]::IsPathRooted($ExeDir)) { $ExeDir } else { Join-Path $root $ExeDir }) "keepane.exe"
if (-not (Test-Path $exe)) { throw "no keepane.exe at $exe (cargo build --release first)" }
# A stale build would go in under this version's name.
$exeVersion = ((& $exe -V) -split ' ')[-1].Trim()
if ($exeVersion -ne $Version) { throw "$exe is keepane $exeVersion, not $Version (cargo build --release first)" }

# Everything the MSI ships, in one directory, so the .wxs has a single root.
$stage = Join-Path ([System.IO.Path]::GetTempPath()) "keepane-msi-stage-$PID"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe $stage
foreach ($f in @("README.md", "LICENSE", "keepane.conf.example")) { Copy-Item (Join-Path $root $f) $stage }
# The console host beside it (third-party/conpty; build.rs put it there).
foreach ($f in @("conpty.dll", "OpenConsole.exe")) {
    $p = Join-Path (Split-Path -Parent $exe) $f
    if (-not (Test-Path $p)) { throw "no $f beside $exe (pwsh tools/fetch-conpty.ps1, then cargo build --release)" }
    Copy-Item $p $stage
}
Copy-Item (Join-Path $root "third-party/conpty/LICENSE") (Join-Path $stage "ConPTY-LICENSE")

$obj = Join-Path $stage "keepane.wixobj"
$outDirFull = if ([IO.Path]::IsPathRooted($OutDir)) { $OutDir } else { Join-Path $root $OutDir }
New-Item -ItemType Directory -Force $outDirFull | Out-Null
$suffix = if ($Scope -eq "user") { "-user" } else { "" }
$msi = Join-Path $outDirFull "keepane-$Version-windows-x86_64$suffix.msi"
$scopeVar = if ($Scope -eq "user") { "perUser" } else { "perMachine" }

try {
    & "$wix/candle.exe" -nologo -arch x64 "-dVersion=$Version" "-dSourceDir=$stage" "-dScope=$scopeVar" `
        (Join-Path $root "installer/keepane.wxs") -o $obj
    if ($LASTEXITCODE -ne 0) { throw "candle failed" }
    # ICE61 fires on same-version upgrades, which MajorUpgrade allows on purpose.
    # ICE91 warns that a per-user file is not copied to every profile, which a
    # per-user package does not want anyway.
    $ices = @("-sice:ICE61") + $(if ($Scope -eq "user") { @("-sice:ICE91") } else { @() })
    & "$wix/light.exe" -nologo -ext WixUIExtension @ices -spdb -b $root $obj -o $msi
    if ($LASTEXITCODE -ne 0) { throw "light failed" }
}
finally {
    # A failed build must not leave a copy of the staged files behind.
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}

Write-Host "built $msi"
