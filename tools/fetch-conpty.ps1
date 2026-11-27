# Fetches the console host keepane ships beside keepane.exe on Windows into
# vendor/conpty/ (docs/design/kitty-graphics.md 11, item 9).
#
#   pwsh tools/fetch-conpty.ps1
#
# Each pane is a Windows console, and Windows' own console host redraws what
# a program prints from its own grid of characters: a sequence it does not
# know, such as a picture sent with kitty's graphics protocol, never reaches
# keepane. Microsoft publishes the newer host Windows Terminal uses (MIT
# licence) as a NuGet package: conpty.dll and OpenConsole.exe, which a
# program puts beside its own .exe; the pty library keepane uses loads that
# conpty.dll before Windows' own. The same version UnifyTerm ships. The
# version and the package's hash are fixed here; a download that differs is
# refused.
$ErrorActionPreference = 'Stop'

$Version = '1.25.261109001'
$Sha256 = '3D77F535DF848E44225C2A8912AC844FECB23F3057732B639F3034B587496F09'
$Package = 'microsoft.windows.console.conpty'
$Url = "https://api.nuget.org/v3-flatcontainer/$Package/$Version/$Package.$Version.nupkg"

$root = Split-Path $PSScriptRoot
$download = Join-Path $root "target\conpty-download"
$nupkg = Join-Path $download "conpty.$Version.nupkg"
New-Item -ItemType Directory -Force $download | Out-Null

if (-not (Test-Path $nupkg) -or (Get-FileHash $nupkg -Algorithm SHA256).Hash -ne $Sha256) {
    Invoke-WebRequest $Url -OutFile $nupkg -TimeoutSec 180
}
$hash = (Get-FileHash $nupkg -Algorithm SHA256).Hash
if ($hash -ne $Sha256) {
    Remove-Item $nupkg
    throw "conpty ${Version}: the download's SHA-256 is $hash, not $Sha256. Nothing was installed."
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($nupkg)
try {
    foreach ($arch in 'x64', 'arm64') {
        $out = Join-Path $root "vendor\conpty\$arch"
        New-Item -ItemType Directory -Force $out | Out-Null
        foreach ($entry in "runtimes/win-$arch/native/conpty.dll", "build/native/runtimes/$arch/OpenConsole.exe") {
            $file = $zip.GetEntry($entry)
            if (-not $file) { throw "conpty ${Version}: $entry is not in the package" }
            $target = Join-Path $out (Split-Path $entry -Leaf)
            [IO.Compression.ZipFileExtensions]::ExtractToFile($file, $target, $true)
            "{0,9} bytes  vendor\conpty\{1}\{2}" -f $file.Length, $arch, (Split-Path $entry -Leaf)
        }
    }
} finally {
    $zip.Dispose()
}
Set-Content (Join-Path $root "vendor\conpty\VERSION") "Microsoft.Windows.Console.ConPTY $Version (MIT), from $Url`n" -NoNewline
"conpty $Version is in vendor\conpty. The next cargo build puts it beside keepane.exe."
