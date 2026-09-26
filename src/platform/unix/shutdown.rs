//! Save everything before the server is stopped: `SIGTERM` (a logout, a
//! reboot, `systemctl --user stop`) and `SIGHUP` run the hook, then the
//! server exits. The handler only writes a byte to a pipe; a thread of its
//! own reads it and runs the hook.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

static PIPE_W: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(_sig: libc::c_int) {
    let fd = PIPE_W.load(Ordering::SeqCst);
    if fd >= 0 {
        let b = 1u8;
        unsafe { libc::write(fd, &b as *const u8 as *const libc::c_void, 1) };
    }
}

/// Run `on_end` when the server is told to stop, then exit.
pub fn watch(_name: &str, on_end: impl Fn() + Send + Sync + 'static) {
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_err() {
        return;
    }
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        log::warn!("shutdown: no pipe: {}", std::io::Error::last_os_error());
        return;
    }
    PIPE_W.store(fds[1], Ordering::SeqCst);
    let read_fd = fds[0];
    let _ = std::thread::Builder::new().name("shutdown".into()).spawn(move || {
        let mut b = 0u8;
        loop {
            let n = unsafe { libc::read(read_fd, &mut b as *mut u8 as *mut libc::c_void, 1) };
            if n == 1 {
                log::info!("server told to stop: saving");
                on_end();
                std::process::exit(0);
            }
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
    });
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGHUP, &sa, std::ptr::null_mut());
    }
}
