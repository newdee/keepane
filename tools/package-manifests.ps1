# Writes the WinGet and Scoop manifests for one released version from what
# is on the GitHub release: the two .sha256 files and the MSI (its
# ProductCode is generated at build time, so it must be read back).
#
#   tools\package-manifests.ps1 -Version 0.5.0
#
# Output:
#   packaging\winget\manifests\n\newdee\keepane\<version>\newdee.keepane.yaml
#   packaging\winget\manifests\n\newdee\keepane\<version>\newdee.keepane.installer.yaml
#   packaging\winget\manifests\n\newdee\keepane\<version>\newdee.keepane.locale.en-US.yaml
#   packaging\scoop\keepane.json
#
# The winget tree is laid out as microsoft/winget-pkgs expects, so the
# directory can be copied into a fork as is; the scoop file installs with
# `scoop install <raw url of keepane.json>` or goes into a bucket.
#
# With -MsiPath and -ZipPath (the files just built, as in the release
# workflow) nothing is downloaded: the hashes and the ProductCode come from
# those files, and the URLs are still the release's. -UserMsiPath adds the
# per-user MSI (from 0.15.2 on); downloading, it is added when the release
# has one.
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [ValidatePattern('^\d+\.\d+\.\d+$')] [string] $Version,
    [string] $Repo = "newdee/keepane",
    [string] $OutDir = (Join-Path $PSScriptRoot "..\packaging"),
    [string] $MsiPath,
    [string] $ZipPath,
    [string] $UserMsiPath,
    # yyyy-MM-dd; the release's publish date when downloading, today otherwise.
    [string] $ReleaseDate
)
$ErrorActionPreference = "Stop"

$base = "https://github.com/$Repo/releases/download/v$Version"
$msiName = "keepane-$Version-windows-x86_64.msi"
$zipName = "keepane-v$Version-windows-x86_64.zip"
$userMsiName = "keepane-$Version-windows-x86_64-user.msi"
$userMsiSha = $null; $userProductCode = $null

function Get-Sha256FromRelease([string] $asset) {
    $text = (Invoke-RestMethod "$base/${asset}.sha256").Trim()
    if ($text -notmatch '^([0-9a-f]{64})\s+(\S+)$' -or $Matches[2] -ne $asset) {
        throw "${asset}.sha256 does not look like '<sha256>  ${asset}': $text"
    }
    $Matches[1]
}

function Get-MsiProductCode([string] $path) {
    $wi = New-Object -ComObject WindowsInstaller.Installer
    $db = $wi.GetType().InvokeMember("OpenDatabase", "InvokeMethod", $null, $wi, @($path, 0))
    $view = $db.GetType().InvokeMember("OpenView", "InvokeMethod", $null, $db, @("SELECT Value FROM Property WHERE Property='ProductCode'"))
    $view.GetType().InvokeMember("Execute", "InvokeMethod", $null, $view, $null) | Out-Null
    $rec = $view.GetType().InvokeMember("Fetch", "InvokeMethod", $null, $view, $null)
    $rec.GetType().InvokeMember("StringData", "GetProperty", $null, $rec, @(1))
}

if ($ReleaseDate -and $ReleaseDate -notmatch '^\d{4}-\d{2}-\d{2}$') { throw "-ReleaseDate wants yyyy-MM-dd, not '$ReleaseDate'" }
if ($MsiPath -or $ZipPath) {
    # Local mode: the files just built. Their names must be the release's,
    # since the manifests point at the release URLs of those names.
    if (-not ($MsiPath -and $ZipPath)) { throw "-MsiPath and -ZipPath go together" }
    foreach ($pair in @(@($MsiPath, $msiName), @($ZipPath, $zipName))) {
        if (-not (Test-Path $pair[0])) { throw "no such file: $($pair[0])" }
        if ((Split-Path -Leaf $pair[0]) -ne $pair[1]) { throw "$($pair[0]) should be named $($pair[1]) for v$Version" }
    }
    $msiSha = (Get-FileHash $MsiPath -Algorithm SHA256).Hash.ToLower()
    $zipSha = (Get-FileHash $ZipPath -Algorithm SHA256).Hash.ToLower()
    $productCode = Get-MsiProductCode $MsiPath
    if ($UserMsiPath) {
        if (-not (Test-Path $UserMsiPath)) { throw "no such file: $UserMsiPath" }
        if ((Split-Path -Leaf $UserMsiPath) -ne $userMsiName) { throw "$UserMsiPath should be named $userMsiName for v$Version" }
        $userMsiSha = (Get-FileHash $UserMsiPath -Algorithm SHA256).Hash.ToLower()
        $userProductCode = Get-MsiProductCode $UserMsiPath
    }
    $releaseDate = if ($ReleaseDate) { $ReleaseDate } else { (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd") }
} else {
    try {
        $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/tags/v$Version"
    } catch {
        throw "no release v$Version on github.com/$Repo (tag the release first): $($_.Exception.Message)"
    }
    $releaseDate = if ($ReleaseDate) { $ReleaseDate } else { ([datetime] $release.published_at).ToUniversalTime().ToString("yyyy-MM-dd") }
    $msiSha = Get-Sha256FromRelease $msiName
    $zipSha = Get-Sha256FromRelease $zipName

    # The MSI itself, for its ProductCode; checked against the published hash.
    $tmp = Join-Path ([IO.Path]::GetTempPath()) "keepane-manifests-$PID"
    New-Item -ItemType Directory -Force $tmp | Out-Null
    try {
        $msiPath = Join-Path $tmp $msiName
        Invoke-WebRequest "$base/$msiName" -OutFile $msiPath
        $got = (Get-FileHash $msiPath -Algorithm SHA256).Hash.ToLower()
        if ($got -ne $msiSha) { throw "${msiName}: downloaded sha256 $got is not the published $msiSha" }
        $productCode = Get-MsiProductCode $msiPath
        # The per-user MSI, when this release has one.
        if ($release.assets.name -contains $userMsiName) {
            $userMsiSha = Get-Sha256FromRelease $userMsiName
            $userPath = Join-Path $tmp $userMsiName
            Invoke-WebRequest "$base/$userMsiName" -OutFile $userPath
            $got = (Get-FileHash $userPath -Algorithm SHA256).Hash.ToLower()
            if ($got -ne $userMsiSha) { throw "${userMsiName}: downloaded sha256 $got is not the published $userMsiSha" }
            $userProductCode = Get-MsiProductCode $userPath
        }
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
if ($productCode -notmatch '^\{[0-9A-F-]{36}\}$') { throw "odd ProductCode: $productCode" }
if ($userProductCode -and $userProductCode -notmatch '^\{[0-9A-F-]{36}\}$') { throw "odd per-user ProductCode: $userProductCode" }
$userInstaller = if ($userMsiSha) {
    "`n- Architecture: x64`n  Scope: user`n  InstallerUrl: $base/$userMsiName`n  InstallerSha256: $($userMsiSha.ToUpper())`n  ProductCode: '$userProductCode'"
} else { "" }

$wingetDir = Join-Path $OutDir "winget\manifests\n\newdee\keepane\$Version"
New-Item -ItemType Directory -Force $wingetDir | Out-Null
$scoopDir = Join-Path $OutDir "scoop"
New-Item -ItemType Directory -Force $scoopDir | Out-Null

$versionYaml = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.12.0.schema.json
PackageIdentifier: newdee.keepane
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.12.0
"@

$installerYaml = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.12.0.schema.json
PackageIdentifier: newdee.keepane
PackageVersion: $Version
InstallerType: wix
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
Commands:
- keepane
ReleaseDate: $releaseDate
Installers:
- Architecture: x64
  Scope: machine
  InstallerUrl: $base/$msiName
  InstallerSha256: $($msiSha.ToUpper())
  ProductCode: '$productCode'$userInstaller
ManifestType: installer
ManifestVersion: 1.12.0
"@

$localeYaml = @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.12.0.schema.json
PackageIdentifier: newdee.keepane
PackageVersion: $Version
PackageLocale: en-US
Publisher: newdee
PublisherUrl: https://github.com/newdee
PublisherSupportUrl: https://github.com/$Repo/issues
PackageName: keepane
PackageUrl: https://github.com/$Repo
License: MIT
LicenseUrl: https://github.com/$Repo/blob/master/LICENSE
ShortDescription: Persistent terminal panes, each with its own inbox: a terminal multiplexer with tmux's keys and commands.
Description: |-
  keepane: persistent terminal panes, each with its own inbox. The programs
  in them keep running after you detach. A named pane gets messages in
  an inbox and takes each one when it is ready: a shell runs it at its next
  prompt, a program reads it when it asks. Every message carries its sender
  and its task, and an event log and a dashboard show what became of it.
  Scripts, people and AI agents (through a built-in MCP server) send the
  same messages. tmux's keys, commands and tmux.conf syntax, and sessions
  that survive a reboot.
Moniker: keepane
Tags:
- terminal
- multiplexer
- tmux
- agents
- mcp
- conpty
ReleaseNotesUrl: https://github.com/$Repo/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: 1.12.0
"@

# LF line endings and no BOM: what winget-pkgs' validation wants.
$enc = New-Object System.Text.UTF8Encoding($false)
foreach ($pair in @(
    @("newdee.keepane.yaml", $versionYaml),
    @("newdee.keepane.installer.yaml", $installerYaml),
    @("newdee.keepane.locale.en-US.yaml", $localeYaml))) {
    [IO.File]::WriteAllText((Join-Path $wingetDir $pair[0]), ($pair[1] -replace "`r`n", "`n") + "`n", $enc)
}

$scoop = [ordered]@{
    version     = $Version
    description = "Persistent terminal panes, each with its own inbox: a terminal multiplexer with tmux's keys and commands."
    homepage    = "https://github.com/$Repo"
    license     = "MIT"
    url         = "$base/$zipName"
    hash        = $zipSha
    extract_dir = "keepane-v$Version-windows-x86_64"
    bin         = "keepane.exe"
    checkver    = "github"
    autoupdate  = [ordered]@{
        url         = "https://github.com/$Repo/releases/download/v`$version/keepane-v`$version-windows-x86_64.zip"
        extract_dir = "keepane-v`$version-windows-x86_64"
        hash        = [ordered]@{ url = "`$url.sha256" }
    }
}
$json = ($scoop | ConvertTo-Json -Depth 5) -replace "`r`n", "`n"
[IO.File]::WriteAllText((Join-Path $scoopDir "keepane.json"), $json + "`n", $enc)

"winget: $wingetDir"
"scoop:  $(Join-Path $scoopDir 'keepane.json')"
"msi sha256 $msiSha  product code $productCode"
"zip sha256 $zipSha"
