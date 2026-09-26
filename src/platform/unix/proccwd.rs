//! The working directory of another process: `/proc/<pid>/cwd` on Linux,
//! `proc_pidinfo` on macOS.

/// `pid`'s current directory, when it can be read (same user, alive).
#[cfg(not(target_os = "macos"))]
pub fn process_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
pub fn process_cwd(pid: u32) -> Option<String> {
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if n != size {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char) };
    Some(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_directory_reads_back() {
        let here = std::env::current_dir().unwrap();
        let got = process_cwd(std::process::id()).unwrap();
        assert_eq!(std::path::Path::new(&got).canonicalize().unwrap(), here.canonicalize().unwrap());
        assert_eq!(process_cwd(0x7fff_fff0), None, "no such process");
    }
}
