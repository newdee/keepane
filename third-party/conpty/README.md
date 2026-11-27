# ConPTY's license

`conpty.dll` and `OpenConsole.exe` ship beside `keepane.exe` on Windows (the zip and both MSIs): the console host each pane runs in, newer than Windows' own, which drops sequences it does not know, kitty's pictures among them (docs/design/kitty-graphics.md, section 11). They come from the NuGet package `Microsoft.Windows.Console.ConPTY` (MIT, https://github.com/microsoft/terminal); `tools/fetch-conpty.ps1` downloads a fixed version (its SHA-256 checked) into `vendor/conpty`, which git does not keep, and `build.rs` puts them beside the built program.

`LICENSE` here is that project's license text, shipped beside them as `ConPTY-LICENSE`.
