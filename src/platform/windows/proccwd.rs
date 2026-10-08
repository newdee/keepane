//! The working directory of another process, read from its PEB.
//!
//! Windows has no API for it; the process parameters block another process
//! can read (same user, `PROCESS_VM_READ`) holds `CurrentDirectory`. That
//! is the directory `cmd.exe` and most programs `cd` in. PowerShell is the
//! exception: `Set-Location` moves its own idea of the location and not the
//! process's, which is why PowerShell panes get a prompt hook instead (see
//! `config::POWERSHELL_PROMPT_HOOK`) and this is the fallback for the rest.
//!
//! Offsets are those of the 64-bit PEB; keepane is built for x86_64 only.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base_address: usize,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: i32,
        info: *mut core::ffi::c_void,
        len: u32,
        out_len: *mut u32,
    ) -> i32;
}

/// PEB.ProcessParameters, and RTL_USER_PROCESS_PARAMETERS.CurrentDirectory
/// (a CURDIR: a UNICODE_STRING DosPath, then a handle) and .CommandLine (a
/// UNICODE_STRING), 64-bit layout.
const PEB_PROCESS_PARAMETERS: usize = 0x20;
const PARAMS_CURRENT_DIRECTORY: usize = 0x38;
const PARAMS_COMMAND_LINE: usize = 0x70;

fn read<T: Copy>(process: HANDLE, at: usize) -> Option<T> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut got = 0usize;
    let ok = unsafe {
        ReadProcessMemory(process, at as *const _, value.as_mut_ptr() as *mut _, std::mem::size_of::<T>(), &mut got)
    };
    if ok == 0 || got != std::mem::size_of::<T>() {
        return None;
    }
    Some(unsafe { value.assume_init() })
}

/// The process's current directory, or None when it cannot be read (gone,
/// another user's, a 32-bit process with a different layout, a path that
/// is not a drive path).
pub fn process_cwd(pid: u32) -> Option<String> {
    let path = params_string(pid, PARAMS_CURRENT_DIRECTORY)?;
    let path = path.trim_end_matches('\\').to_string();
    // "C:" alone means the drive's root.
    let path = if path.len() == 2 && path.ends_with(':') { format!("{path}\\") } else { path };
    if path.len() >= 2 && path.as_bytes()[1] == b':' { Some(path) } else { None }
}

/// The command line the process was started with, or None when it cannot
/// be read (as `process_cwd`).
pub fn process_command_line(pid: u32) -> Option<String> {
    params_string(pid, PARAMS_COMMAND_LINE)
}

/// A UNICODE_STRING of the process's parameters block, at `offset` in it.
fn params_string(pid: u32, offset: usize) -> Option<String> {
    const _: () = assert!(std::mem::size_of::<usize>() == 8, "64-bit PEB offsets");
    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if process.is_null() {
        return None;
    }
    let result = (|| {
        let mut info = std::mem::MaybeUninit::<ProcessBasicInformation>::uninit();
        let mut len = 0u32;
        let status = unsafe {
            NtQueryInformationProcess(
                process,
                0, // ProcessBasicInformation
                info.as_mut_ptr() as *mut _,
                std::mem::size_of::<ProcessBasicInformation>() as u32,
                &mut len,
            )
        };
        if status != 0 {
            return None;
        }
        let peb = unsafe { info.assume_init() }.peb_base_address;
        let params: usize = read(process, peb + PEB_PROCESS_PARAMETERS)?;
        // UNICODE_STRING: Length (bytes), MaximumLength, then the buffer
        // pointer at +8.
        let length: u16 = read(process, params + offset)?;
        let buffer: usize = read(process, params + offset + 8)?;
        if buffer == 0 || length == 0 || length > 32 * 1024 {
            return None;
        }
        let n = (length / 2) as usize;
        let mut wide = vec![0u16; n];
        let mut got = 0usize;
        let ok = unsafe {
            ReadProcessMemory(process, buffer as *const _, wide.as_mut_ptr() as *mut _, length as usize, &mut got)
        };
        if ok == 0 || got != length as usize {
            return None;
        }
        Some(String::from_utf16_lossy(&wide))
    })();
    unsafe { CloseHandle(process) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_directory_reads_back() {
        let got = process_cwd(std::process::id()).expect("this process");
        let want = std::env::current_dir().unwrap().to_string_lossy().trim_end_matches('\\').to_string();
        assert_eq!(got.to_ascii_lowercase(), want.to_ascii_lowercase());
    }

    #[test]
    fn our_own_command_line_reads_back() {
        let got = process_command_line(std::process::id()).expect("this process");
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_string_lossy().to_string();
        assert!(got.contains(&stem), "{got}");
    }

    #[test]
    fn a_dead_or_foreign_pid_is_none() {
        assert_eq!(process_command_line(0), None);
        assert_eq!(process_cwd(0), None);
        assert_eq!(process_cwd(4), None, "System: another user, no access");
        assert_eq!(process_cwd(u32::MAX), None);
    }

    /// A child that moved: its directory is the one it moved to, not the
    /// one it started in.
    #[test]
    fn a_child_that_changed_directory() {
        use std::process::{Command, Stdio};
        let mut child = Command::new("cmd.exe")
            .args(["/d", "/c", "cd /d C:\\Windows\\System32 & ping -n 6 127.0.0.1 >nul"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // ping keeps it alive while we look; the cd has happened once the
        // directory reads back as the new one.
        let started = std::time::Instant::now();
        let mut got = None;
        while started.elapsed() < std::time::Duration::from_secs(5) {
            got = process_cwd(child.id()).map(|s| s.to_ascii_lowercase());
            if got.as_deref() == Some("c:\\windows\\system32") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(got.as_deref(), Some("c:\\windows\\system32"));
    }
}
