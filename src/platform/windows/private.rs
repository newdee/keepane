//! A file kept to this user: its DACL gives full access to the user and
//! SYSTEM and nothing to anyone else (the same descriptor as the server's
//! pipe, `winsec`), protected so nothing is inherited from the folder.

use anyhow::{Result, bail};
use std::path::Path;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW};

pub fn restrict(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let sddl = super::winsec::owner_only_sddl(&super::winsec::current_user_sid()?);
    let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let mut sd = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            std::ptr::null_mut(),
        ) == 0
        {
            bail!("ConvertStringSecurityDescriptorToSecurityDescriptor: {}", std::io::Error::last_os_error());
        }
        let ok = SetFileSecurityW(file.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, sd);
        LocalFree(sd);
        if ok == 0 {
            bail!("SetFileSecurity: {}", std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_file_is_the_users_alone() {
        let path = std::env::temp_dir().join(format!("keepane-private-{}", std::process::id()));
        std::fs::write(&path, "secret").unwrap();
        super::restrict(&path).unwrap();
        // The descriptor read back names this user and SYSTEM, nobody else.
        let out = std::process::Command::new("icacls").arg(&path).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let grants = text.lines().filter(|l| l.contains(":(")).count();
        assert_eq!(grants, 2, "{text}");
        assert!(text.contains("NT AUTHORITY\\SYSTEM:(F)") || text.contains("SYSTEM:(F)"), "{text}");
        let _ = std::fs::remove_file(&path);
    }
}
