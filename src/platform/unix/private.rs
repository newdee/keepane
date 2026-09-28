//! A file kept to this user: mode 0600.

use anyhow::{Context, Result};
use std::path::Path;

pub fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 600 {}", path.display()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_file_is_the_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("keepane-private-{}", std::process::id()));
        std::fs::write(&path, "secret").unwrap();
        super::restrict(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
    }
}
