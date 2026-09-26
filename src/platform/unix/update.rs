//! `keepane update`: on Windows the MSI or Scoop. Here the release archive or
//! `cargo install` puts a new version in place; the command says where, and
//! never installs anything itself.

use anyhow::Result;

pub async fn run(_socket: &str, _args: &[String]) -> Result<i32> {
    println!(
        "keepane {}: update it the way it was installed: the archive for this system from \
         https://github.com/newdee/keepane/releases/latest, or \
         `cargo install --git https://github.com/newdee/keepane --locked`; then \
         `keepane restart-server`",
        env!("CARGO_PKG_VERSION")
    );
    Ok(0)
}
