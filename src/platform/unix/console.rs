//! The client's terminal: termios raw mode, output as VT, input as bytes
//! made into the same key and mouse records a Windows console gives
//! (`crate::vtinput`), resizes from `SIGWINCH`.

use crate::ipc::{KeyRecord, MouseRecord};
use crate::vtinput::{Input, Parser};
use anyhow::{Result, bail};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// A pty passes bytes through and keeps no screen: keepane's screen model is
/// the pane's only one.
pub const PTY_HAS_SCREEN: bool = false;

/// Write `text` to this process's terminal (`/dev/tty`), whatever stdout is:
/// the helper that prints a resumed pane's saved output (`keepane __replay`).
pub fn write_to_console(text: &str) -> Result<()> {
    use std::io::Write;
    let mut tty = std::fs::OpenOptions::new().write(true).open("/dev/tty")?;
    tty.write_all(text.as_bytes())?;
    Ok(())
}

pub enum InputEvent {
    Key(KeyRecord),
    Mouse(MouseRecord),
    Resize,
}

/// The write end of the pipe `SIGWINCH` writes a byte to.
static WINCH_W: AtomicI32 = AtomicI32::new(-1);
static WINCH_R: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_winch(_sig: libc::c_int) {
    let fd = WINCH_W.load(Ordering::SeqCst);
    if fd >= 0 {
        let b = 1u8;
        unsafe { libc::write(fd, &b as *const u8 as *const libc::c_void, 1) };
    }
}

fn watch_resizes() {
    if WINCH_R.load(Ordering::SeqCst) >= 0 {
        return;
    }
    let mut fds = [0 as libc::c_int; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
        WINCH_W.store(fds[1], Ordering::SeqCst);
        WINCH_R.store(fds[0], Ordering::SeqCst);
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_winch as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGWINCH, &sa, std::ptr::null_mut());
    }
}

/// How long an unfinished escape sequence may wait for the rest before
/// what came is taken as it is (a lone Escape key).
const ESCAPE_WAIT_MS: libc::c_int = 30;

pub struct Console {
    saved: libc::termios,
    raw: AtomicBool,
    parser: Mutex<Parser>,
    /// A terminal without 24-bit colour: what is written has them made 256.
    downgrade: Option<Mutex<crate::truecolor::Downgrade>>,
}

impl Console {
    /// The terminal on stdin and stdout. Fails when either is redirected.
    pub fn open() -> Result<Console> {
        unsafe {
            if libc::isatty(0) != 1 {
                bail!("stdin is not a terminal");
            }
            if libc::isatty(1) != 1 {
                bail!("stdout is not a terminal");
            }
            let mut saved: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut saved) != 0 {
                bail!("tcgetattr: {}", std::io::Error::last_os_error());
            }
            let downgrade = (!crate::truecolor::host_has_truecolor(|k| std::env::var(k).ok())).then(Mutex::default);
            Ok(Console { saved, raw: AtomicBool::new(false), parser: Mutex::new(Parser::new()), downgrade })
        }
    }

    /// Raw input, the alternate screen, bracketed paste and mouse reports.
    pub fn enter_raw(&mut self) -> Result<()> {
        let mut t = self.saved;
        unsafe {
            libc::cfmakeraw(&mut t);
            if libc::tcsetattr(0, libc::TCSANOW, &t) != 0 {
                bail!("tcsetattr: {}", std::io::Error::last_os_error());
            }
        }
        watch_resizes();
        self.raw.store(true, Ordering::SeqCst);
        self.write_str("\x1b[?1049h\x1b[2J\x1b[H\x1b[?2004h");
        self.set_mouse(true);
        Ok(())
    }

    /// Title of the hosting window/tab (tmux `set-titles`).
    pub fn set_title(&self, title: &str) {
        self.write_str(&format!("\x1b]2;{title}\x07"));
    }

    /// Mouse reports on or off while in raw mode (the server's `mouse`
    /// option): off, the terminal selects text itself.
    pub fn set_mouse(&self, mouse: bool) {
        if !self.raw.load(Ordering::SeqCst) {
            return;
        }
        self.write_str(if mouse { "\x1b[?1000h\x1b[?1002h\x1b[?1006h" } else { "\x1b[?1006l\x1b[?1002l\x1b[?1000l" });
    }

    /// Undo `enter_raw`. Safe to call more than once and from any thread.
    pub fn restore(&self) {
        if !self.raw.swap(false, Ordering::SeqCst) {
            return;
        }
        self.write_str("\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?2004l\x1b[0m\x1b[?25h\x1b[?1049l");
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.saved) };
    }

    /// Size in cells (cols, rows).
    pub fn size(&self) -> (u16, u16) {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } != 0 || ws.ws_col == 0 {
            return (80, 24);
        }
        (ws.ws_col, ws.ws_row.max(1))
    }

    pub fn write_str(&self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    pub fn write_bytes(&self, b: &[u8]) {
        let filtered;
        let b = match &self.downgrade {
            Some(d) => {
                filtered = d.lock().unwrap_or_else(|e| e.into_inner()).filter(b);
                &filtered[..]
            }
            None => b,
        };
        let mut off = 0;
        while off < b.len() {
            let n = unsafe { libc::write(1, b[off..].as_ptr() as *const libc::c_void, b.len() - off) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            if n == 0 {
                return;
            }
            off += n as usize;
        }
    }

    /// Blocking read of a batch of input events.
    pub fn read_events(&self) -> Result<Vec<InputEvent>> {
        loop {
            let waiting = self.parser.lock().unwrap_or_else(|e| e.into_inner()).waiting();
            let winch = WINCH_R.load(Ordering::SeqCst);
            let mut fds = [
                libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: winch, events: libc::POLLIN, revents: 0 },
            ];
            let nfds = if winch >= 0 { 2 } else { 1 };
            let r = unsafe { libc::poll(fds.as_mut_ptr(), nfds, if waiting { ESCAPE_WAIT_MS } else { -1 }) };
            if r < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                bail!("poll: {}", std::io::Error::last_os_error());
            }
            let mut out = Vec::new();
            if r == 0 {
                // The rest of a sequence did not come: what came is what it is.
                let inputs = self.parser.lock().unwrap_or_else(|e| e.into_inner()).flush();
                out.extend(inputs.into_iter().map(to_event));
                return Ok(out);
            }
            if nfds == 2 && fds[1].revents & libc::POLLIN != 0 {
                let mut buf = [0u8; 64];
                while unsafe { libc::read(winch, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) } > 0 {}
                out.push(InputEvent::Resize);
            }
            if fds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                let mut buf = [0u8; 4096];
                let n = unsafe { libc::read(0, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n == 0 {
                    bail!("the terminal is gone");
                }
                if n < 0 {
                    if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                        bail!("read: {}", std::io::Error::last_os_error());
                    }
                } else {
                    let inputs = self.parser.lock().unwrap_or_else(|e| e.into_inner()).feed(&buf[..n as usize]);
                    out.extend(inputs.into_iter().map(to_event));
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
    }
}

fn to_event(i: Input) -> InputEvent {
    match i {
        Input::Key(k) => InputEvent::Key(k),
        Input::Mouse(m) => InputEvent::Mouse(m),
    }
}

/// `keepane show-keys`: each key as keepane reads it, until `q`.
pub fn show_keys() -> Result<i32> {
    let mut c = Console::open()?;
    c.enter_raw()?;
    c.set_mouse(false);
    c.write_str("keepane show-keys: press keys (the prefix, for one); q quits.\r\n\r\n");
    let mut seen = Vec::new();
    let result = (|| -> Result<()> {
        loop {
            for ev in c.read_events()? {
                let InputEvent::Key(k) = ev else { continue };
                let read =
                    crate::keys::key_from_record(&k).map(|key| key.to_string()).unwrap_or_else(|| "(nothing)".into());
                let line = format!("vk=0x{:02X} char=0x{:04X} flags=0x{:04X}  ->  {read}", k.vk, k.ch, k.ctrl);
                c.write_str(&format!("{line}\r\n"));
                seen.push(line);
                if read == "q" {
                    return Ok(());
                }
            }
        }
    })();
    c.restore();
    for line in &seen {
        println!("{line}");
    }
    result.map(|()| 0)
}

impl Drop for Console {
    fn drop(&mut self) {
        self.restore();
    }
}
