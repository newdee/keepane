//! `keepane update`: on Windows the MSI or Scoop. Here Homebrew, the release
//! archive or `cargo install` puts a new version in place; the command says
//! which, and never installs anything itself.

use anyhow::Result;
use std::path::Path;

pub async fn run(_socket: &str, _args: &[String]) -> Result<i32> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default();
    println!("{}", advice(&exe));
    Ok(0)
}

/// What to do for a new version, for the keepane at `exe` (its real path:
/// Homebrew's `bin/keepane` is a link into its Cellar).
fn advice(exe: &Path) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let brewed =
        exe.components().map(|c| c.as_os_str()).collect::<Vec<_>>().windows(2).any(|w| w == ["Cellar", "keepane"]);
    if brewed {
        format!("keepane {version}, installed by Homebrew: `brew upgrade keepane`, then `keepane restart-server`")
    } else {
        format!(
            "keepane {version}: update it the way it was installed: `brew upgrade keepane` (Homebrew), \
             the archive for this system from https://github.com/newdee/keepane/releases/latest, or \
             `cargo install --git https://github.com/newdee/keepane --locked`; then `keepane restart-server`"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brewed_keepane_is_told_to_upgrade_with_brew() {
        for cellar in [
            "/opt/homebrew/Cellar/keepane/0.17.0/bin/keepane",
            "/home/linuxbrew/.linuxbrew/Cellar/keepane/0.17.0/bin/keepane",
        ] {
            let a = advice(Path::new(cellar));
            assert!(a.contains("installed by Homebrew: `brew upgrade keepane`"), "{a}");
        }
        // Not Homebrew: every way is named, brew among them.
        for other in ["/home/me/.local/bin/keepane", "/home/me/.cargo/bin/keepane", "/opt/Cellar-keepane/keepane", ""] {
            let a = advice(Path::new(other));
            assert!(a.contains("the way it was installed") && a.contains("releases/latest"), "{a}");
            assert!(a.contains("`brew upgrade keepane` (Homebrew)"), "{a}");
        }
    }
}
