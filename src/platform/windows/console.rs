//! Thin wrapper over the Win32 console API used by the client.

use crate::ipc::{KeyRecord, MouseRecord};
use anyhow::{Result, bail};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, GetConsoleMode, GetConsoleOutputCP, GetConsoleScreenBufferInfo, GetStdHandle,
    INPUT_RECORD, ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
    SetConsoleOutputCP, SetConsoleTitleW, WriteConsoleW,
};

const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
const ENABLE_LINE_INPUT: u32 = 0x0002;
const ENABLE_ECHO_INPUT: u32 = 0x0004;
const ENABLE_WINDOW_INPUT: u32 = 0x0008;
const ENABLE_MOUSE_INPUT: u32 = 0x0010;
const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
const ENABLE_WRAP_AT_EOL_OUTPUT: u32 = 0x0002;
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x0008;

const KEY_EVENT: u16 = 0x0001;
const MOUSE_EVENT: u16 = 0x0002;
const WINDOW_BUFFER_SIZE_EVENT: u16 = 0x0004;

/// A pane's pseudo console (ConPTY) keeps a screen of its own and repaints
/// from it: text a pane is to show must be printed into that screen, by a
/// process in the console (`keepane __replay`).
pub const PTY_HAS_SCREEN: bool = true;

/// Write `text` to this process's console screen (`CONOUT$`), VT sequences
/// interpreted, whatever the standard handles are: a process started into
/// a ConPTY gets no usable stdout, only the console itself. For the helper
/// that prints a resumed pane's saved output (`keepane __replay`).
pub fn write_to_console(text: &str) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::WriteConsoleW;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        bail!("no console: {}", std::io::Error::last_os_error());
    }
    let result = (|| {
        let mut mode = 0;
        if unsafe { GetConsoleMode(h, &mut mode) } != 0 {
            unsafe { SetConsoleMode(h, mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING) };
        }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut done = 0usize;
        while done < wide.len() {
            let chunk = (wide.len() - done).min(16 * 1024) as u32;
            let mut written = 0u32;
            let ok = unsafe { WriteConsoleW(h, wide[done..].as_ptr(), chunk, &mut written, std::ptr::null()) };
            if ok == 0 || written == 0 {
                bail!("WriteConsoleW failed: {}", std::io::Error::last_os_error());
            }
            done += written as usize;
        }
        Ok(())
    })();
    unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
    result
}

pub enum InputEvent {
    Key(KeyRecord),
    Mouse(MouseRecord),
    Resize,
}

pub struct Console {
    hin: HANDLE,
    hout: HANDLE,
    saved_in: u32,
    saved_out: u32,
    saved_cp: u32,
    raw: std::sync::atomic::AtomicBool,
}

// Handles are process-wide; the console is used from an input thread and the
// main thread concurrently, which the console API permits.
unsafe impl Send for Console {}
unsafe impl Sync for Console {}

impl Console {
    /// Open stdin/stdout as a console. Fails when either is redirected.
    pub fn open() -> Result<Console> {
        unsafe {
            let hin = GetStdHandle(STD_INPUT_HANDLE);
            let hout = GetStdHandle(STD_OUTPUT_HANDLE);
            if hin == INVALID_HANDLE_VALUE || hout == INVALID_HANDLE_VALUE || hin.is_null() || hout.is_null() {
                bail!("no console");
            }
            let mut saved_in = 0;
            let mut saved_out = 0;
            if GetConsoleMode(hin, &mut saved_in) == 0 {
                bail!("stdin is not a console");
            }
            if GetConsoleMode(hout, &mut saved_out) == 0 {
                bail!("stdout is not a console");
            }
            let saved_cp = GetConsoleOutputCP();
            Ok(Console { hin, hout, saved_in, saved_out, saved_cp, raw: std::sync::atomic::AtomicBool::new(false) })
        }
    }

    /// Switch to raw input, VT output, alternate screen.
    pub fn enter_raw(&mut self) -> Result<()> {
        unsafe {
            if SetConsoleMode(self.hin, self.raw_input_mode(true)) == 0 {
                bail!("SetConsoleMode(stdin) failed: {}", std::io::Error::last_os_error());
            }
            let out_mode = ENABLE_PROCESSED_OUTPUT
                | ENABLE_WRAP_AT_EOL_OUTPUT
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                | DISABLE_NEWLINE_AUTO_RETURN;
            if SetConsoleMode(self.hout, out_mode) == 0 {
                bail!("SetConsoleMode(stdout) failed: {}", std::io::Error::last_os_error());
            }
            SetConsoleOutputCP(65001);
            // Ctrl+C / Ctrl+Break must reach us as key events, not signals.
            SetConsoleCtrlHandler(None, 1);
        }
        self.raw.store(true, std::sync::atomic::Ordering::SeqCst);
        // Alternate screen, clear, ask the host terminal for win32-input-mode
        // (full key fidelity under Windows Terminal; ignored elsewhere).
        self.write_str("\x1b[?1049h\x1b[2J\x1b[H\x1b[?9001h");
        Ok(())
    }

    /// Title of the hosting window/tab (tmux `set-titles`).
    pub fn set_title(&self, title: &str) {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { SetConsoleTitleW(wide.as_ptr()) };
    }

    /// Raw input mode: no line editing/echo, window events, and either mouse
    /// capture for keepane or quick-edit so the host terminal selects text.
    fn raw_input_mode(&self, mouse: bool) -> u32 {
        let base = (self.saved_in
            & !(ENABLE_PROCESSED_INPUT
                | ENABLE_LINE_INPUT
                | ENABLE_ECHO_INPUT
                | ENABLE_QUICK_EDIT_MODE
                | ENABLE_MOUSE_INPUT
                | ENABLE_VIRTUAL_TERMINAL_INPUT))
            | ENABLE_WINDOW_INPUT
            | ENABLE_EXTENDED_FLAGS;
        if mouse { base | ENABLE_MOUSE_INPUT } else { base | ENABLE_QUICK_EDIT_MODE }
    }

    /// Toggle mouse capture while in raw mode (the server's `mouse` option).
    pub fn set_mouse(&self, mouse: bool) {
        if !self.raw.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        unsafe {
            SetConsoleMode(self.hin, self.raw_input_mode(mouse));
        }
    }

    /// Undo `enter_raw`. Safe to call more than once and from any thread.
    pub fn restore(&self) {
        if !self.raw.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        self.write_str("\x1b[?9001l\x1b[0m\x1b[?25h\x1b[?1049l");
        unsafe {
            SetConsoleMode(self.hin, self.saved_in);
            SetConsoleMode(self.hout, self.saved_out);
            SetConsoleOutputCP(self.saved_cp);
            SetConsoleCtrlHandler(None, 0);
        }
    }

    /// Visible window size in cells (cols, rows).
    pub fn size(&self) -> (u16, u16) {
        unsafe {
            let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
            if GetConsoleScreenBufferInfo(self.hout, &mut info) == 0 {
                return (80, 24);
            }
            let w = info.srWindow.Right - info.srWindow.Left + 1;
            let h = info.srWindow.Bottom - info.srWindow.Top + 1;
            (w.max(1) as u16, h.max(1) as u16)
        }
    }

    pub fn write_str(&self, s: &str) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let mut off = 0usize;
        while off < wide.len() {
            let chunk = &wide[off..];
            let mut written = 0u32;
            let ok = unsafe { WriteConsoleW(self.hout, chunk.as_ptr(), chunk.len() as u32, &mut written, null_mut()) };
            if ok == 0 || written == 0 {
                break;
            }
            off += written as usize;
        }
    }

    pub fn write_bytes(&self, b: &[u8]) {
        self.write_str(&String::from_utf8_lossy(b));
    }

    /// Blocking read of a batch of console input events.
    pub fn read_events(&self) -> Result<Vec<InputEvent>> {
        let mut buf: [INPUT_RECORD; 128] = unsafe { std::mem::zeroed() };
        let mut n = 0u32;
        let ok = unsafe { ReadConsoleInputW(self.hin, buf.as_mut_ptr(), buf.len() as u32, &mut n) };
        if ok == 0 {
            bail!("ReadConsoleInputW failed: {}", std::io::Error::last_os_error());
        }
        let mut out = Vec::with_capacity(n as usize);
        for rec in &buf[..n as usize] {
            match rec.EventType {
                KEY_EVENT => {
                    let k = unsafe { rec.Event.KeyEvent };
                    out.push(InputEvent::Key(KeyRecord {
                        down: k.bKeyDown != 0,
                        repeat: k.wRepeatCount,
                        vk: k.wVirtualKeyCode,
                        sc: k.wVirtualScanCode,
                        ch: unsafe { k.uChar.UnicodeChar },
                        ctrl: k.dwControlKeyState,
                    }));
                }
                MOUSE_EVENT => {
                    let m = unsafe { rec.Event.MouseEvent };
                    out.push(InputEvent::Mouse(MouseRecord {
                        x: m.dwMousePosition.X,
                        y: m.dwMousePosition.Y,
                        buttons: m.dwButtonState,
                        ctrl: m.dwControlKeyState,
                        flags: m.dwEventFlags,
                    }));
                }
                WINDOW_BUFFER_SIZE_EVENT => out.push(InputEvent::Resize),
                _ => {}
            }
        }
        Ok(out)
    }
}

/// `keepane show-keys`: each key as the console hands it over (key code,
/// character, modifier flags) and the key keepane makes of it, until `q`.
/// When the prefix does nothing in some terminal, this shows whether it
/// arrives at all, and in what form.
pub fn show_keys() -> Result<i32> {
    let mut c = Console::open()?;
    c.enter_raw()?;
    c.write_str("keepane show-keys: press keys (the prefix, for one); q quits.\r\n\r\n");
    // Raw mode draws on the alternate screen, which goes away on quitting:
    // the lines are printed again on the normal screen, to copy from.
    let mut seen = Vec::new();
    let result = (|| -> Result<()> {
        loop {
            for ev in c.read_events()? {
                let InputEvent::Key(k) = ev else { continue };
                if !k.down {
                    continue;
                }
                let read =
                    crate::keys::key_from_record(&k).map(|key| key.to_string()).unwrap_or_else(|| "(nothing)".into());
                let ctrl = k.ctrl & (crate::keys::LEFT_CTRL_PRESSED | crate::keys::RIGHT_CTRL_PRESSED) != 0;
                let line = format!(
                    "vk=0x{:02X} char=0x{:04X} flags=0x{:04X}{}  ->  {read}",
                    k.vk,
                    k.ch,
                    k.ctrl,
                    if ctrl { " (Ctrl)" } else { "" },
                );
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
