//! On Windows, puts the console host `tools/fetch-conpty.ps1` fetched
//! (conpty.dll and OpenConsole.exe, in vendor/conpty/<arch>) beside the
//! built program and the test programs, where the pty library looks for it
//! before Windows' own: Windows' own drops what it does not know, the
//! pictures panes are sent among it (docs/design/kitty-graphics.md 11).
//! Without them the build goes on and Windows' own is used.

use std::path::Path;

/// conpty.dll starts OpenConsole.exe from its own directory.
const FILES: [&str; 2] = ["conpty.dll", "OpenConsole.exe"];

fn main() {
    println!("cargo:rerun-if-changed=vendor/conpty/VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x64",
        Ok("aarch64") => "arm64",
        _ => return,
    };
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/conpty").join(arch);
    // OUT_DIR is target/<profile>/build/<package>-<hash>/out, or with a
    // newer cargo target/<profile>/build/<package>/<hash>/out: the folder
    // the program is put in is the one above `build`, named for the profile.
    let Ok(out_dir) = std::env::var("OUT_DIR") else { return };
    let profile_name = std::env::var("PROFILE").unwrap_or_default();
    let Some(profile) = Path::new(&out_dir)
        .ancestors()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "build"))
        .filter_map(Path::parent)
        .find(|dir| dir.file_name().is_some_and(|name| name == profile_name.as_str()))
    else {
        println!(
            "cargo:warning=no {profile_name}/build folder above {out_dir}: conpty.dll and OpenConsole.exe are not put beside keepane.exe, so Windows' own console host will be used."
        );
        return;
    };
    for name in FILES {
        let source = vendor.join(name);
        if !source.is_file() {
            println!(
                "cargo:warning=vendor/conpty/{arch}/{name} is missing: Windows' own console host will be used (it drops kitty's pictures). Run tools/fetch-conpty.ps1."
            );
            return;
        }
        // Beside the program, and beside the test programs (target/<profile>/deps).
        for dir in [profile.to_path_buf(), profile.join("deps")] {
            if dir.is_dir()
                && let Err(error) = std::fs::copy(&source, dir.join(name))
            {
                println!("cargo:warning=copy {name} to {}: {error}", dir.display());
            }
        }
    }
}
