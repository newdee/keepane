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

/// The command line `pid` was started with, its words joined by spaces.
#[cfg(not(target_os = "macos"))]
pub fn process_command_line(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let words: Vec<String> =
        raw.split(|b| *b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// macOS: as `ps` gives it (its own sysctl takes more code than it is worth
/// for a once-a-few-seconds question).
#[cfg(target_os = "macos")]
pub fn process_command_line(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()]).output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !line.is_empty()).then_some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_command_line_reads_back() {
        let got = process_command_line(std::process::id()).unwrap();
        let exe = std::env::current_exe().unwrap();
        assert!(got.contains(exe.file_name().unwrap().to_string_lossy().as_ref()), "{got}");
        assert_eq!(process_command_line(0x7fff_fff0), None, "no such process");
    }

    #[test]
    fn our_own_directory_reads_back() {
        let here = std::env::current_dir().unwrap();
        let got = process_cwd(std::process::id()).unwrap();
        assert_eq!(std::path::Path::new(&got).canonicalize().unwrap(), here.canonicalize().unwrap());
        assert_eq!(process_cwd(0x7fff_fff0), None, "no such process");
    }
}
