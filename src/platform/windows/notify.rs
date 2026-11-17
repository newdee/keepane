//! Desktop notifications, so a job that finishes in a window nobody is
//! looking at can say so outside the terminal too.
//!
//! First choice is a WinRT toast: it carries keepane's own name in the Action
//! Center and a "Go to pane" button that brings the terminal to the pane
//! the notification is about. A toast needs an AppUserModelID the shell
//! knows and, for the button, a URL protocol that starts keepane; both are
//! plain registry entries under the user's own hive (`HKCU\Software\
//! Classes`), written once, so the portable zip gets them too and no
//! administrator is involved.
//!
//! When the toast cannot be shown (no WinRT, a service, session 0), a tray
//! balloon (`Shell_NotifyIcon` with `NIF_INFO`) is the fallback; Windows 10
//! and 11 render it as a notification as well, only without the button.
//! The tray icon only exists while a notification is on screen: a
//! multiplexer has no business leaving something in the tray for the hours
//! it sits there doing nothing.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, IDI_APPLICATION, LoadIconW, MSG, PeekMessageW,
    PostQuitMessage, RegisterClassExW, TranslateMessage, WM_DESTROY, WNDCLASSEXW, WS_OVERLAPPED,
};

/// The AppUserModelID toasts are shown under, and the URL protocol the
/// "Go to pane" button opens.
pub const AUMID: &str = "keepane";
pub const PROTOCOL: &str = "keepane";

/// One notification: a bold first line, the text under it, and the URL a
/// "Go to pane" button opens (toasts only).
pub struct Note {
    pub title: String,
    pub body: String,
    pub action: Option<String>,
}

/// The thread that owns the balloon's window and tray icon. Created on the
/// first balloon and kept for the life of the server; it sits in a channel
/// receive, not a spin loop.
static SENDER: OnceLock<Option<Sender<Note>>> = OnceLock::new();

/// Show a desktop notification. Returns false when the desktop cannot be
/// reached at all (a service, session 0, a locked-down environment), which
/// is not an error worth failing a command over.
pub fn notify(title: &str, body: &str) -> bool {
    notify_with(title, body, None)
}

/// As `notify`, with a "Go to pane" button opening `action` (a `keepane://`
/// URL from `go_to_pane_url`) when the notification is a toast.
pub fn notify_with(title: &str, body: &str, action: Option<&str>) -> bool {
    if toast::show(title, body, action) {
        return true;
    }
    let tx = SENDER.get_or_init(spawn_thread);
    match tx {
        Some(tx) => tx
            .send(Note { title: title.to_string(), body: body.to_string(), action: action.map(str::to_string) })
            .is_ok(),
        None => false,
    }
}

/// The URL a notification's button opens to bring the terminal to a pane:
/// `keepane://go/<socket>/<pane id>`, the socket percent-encoded.
pub fn go_to_pane_url(socket: &str, pane: u32) -> String {
    let mut enc = String::new();
    for b in socket.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{PROTOCOL}://go/{enc}/{pane}")
}

/// The inverse of `go_to_pane_url`: (socket, pane id), or None for a URL
/// that is not one of ours.
pub fn parse_go_url(url: &str) -> Option<(String, u32)> {
    let rest = url.strip_prefix(&format!("{PROTOCOL}://"))?.trim_end_matches('/');
    let mut parts = rest.split('/');
    if parts.next()? != "go" {
        return None;
    }
    let socket = percent_decode(parts.next()?)?;
    let pane: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || socket.is_empty() {
        return None;
    }
    Some((socket, pane))
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Bring this process's console window to the front, for a client told to
/// (`focus-pane`). Windows only honours it when the caller may take the
/// foreground (it was launched by the user, as from a notification's
/// button), and Windows Terminal's hidden console window is not the tab,
/// so this is best effort: false when it did not happen.
pub fn raise_console_window() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SetForegroundWindow, ShowWindow};
    let hwnd = unsafe { GetConsoleWindow() };
    if hwnd.is_null() {
        return false;
    }
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd) != 0
    }
}

/// The registry entries a toast needs, under the user's own hive.
pub mod registration {
    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
        RegGetValueW, RegSetValueExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);

    impl Key {
        fn create(path: &str) -> Result<Key> {
            let mut h: HKEY = std::ptr::null_mut();
            let rc = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide(path).as_ptr(),
                    0,
                    std::ptr::null(),
                    0,
                    KEY_SET_VALUE,
                    std::ptr::null(),
                    &mut h,
                    std::ptr::null_mut(),
                )
            };
            if rc != 0 {
                bail!("cannot create HKCU\\{path} (error {rc})");
            }
            Ok(Key(h))
        }

        fn set(&self, name: Option<&str>, value: &str) -> Result<()> {
            let data = wide(value);
            let name_w = name.map(wide);
            let rc = unsafe {
                RegSetValueExW(
                    self.0,
                    name_w.as_ref().map(|n| n.as_ptr()).unwrap_or(std::ptr::null()),
                    0,
                    REG_SZ,
                    data.as_ptr() as *const u8,
                    (data.len() * 2) as u32,
                )
            };
            if rc != 0 {
                bail!("cannot write {} (error {rc})", name.unwrap_or("(default)"));
            }
            Ok(())
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe { RegCloseKey(self.0) };
        }
    }

    /// What the protocol runs: keepane itself, headless so no console flashes
    /// up, with the URL as its one argument.
    pub fn protocol_command(exe: &str) -> String {
        format!("conhost.exe --headless \"{exe}\" \"%1\"")
    }

    /// Register `aumid` (the name toasts are shown under) and `protocol`
    /// (the URL scheme whose links start `exe`). Idempotent.
    pub fn register(aumid: &str, protocol: &str, exe: &str) -> Result<()> {
        let app = Key::create(&format!("Software\\Classes\\AppUserModelId\\{aumid}"))?;
        app.set(Some("DisplayName"), "keepane")?;
        let proto = Key::create(&format!("Software\\Classes\\{protocol}"))?;
        proto.set(None, &format!("URL:{protocol} protocol"))?;
        proto.set(Some("URL Protocol"), "")?;
        let cmd = Key::create(&format!("Software\\Classes\\{protocol}\\shell\\open\\command"))?;
        cmd.set(None, &protocol_command(exe))?;
        Ok(())
    }

    /// The command a URL protocol runs, when it is registered.
    pub fn command_of(protocol: &str) -> Option<String> {
        let path = wide(&format!("Software\\Classes\\{protocol}\\shell\\open\\command"));
        let mut size = 0u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != 0 || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut _,
                &mut size,
            )
        };
        if rc != 0 {
            return None;
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Remove both again. Not an error when they are not there.
    pub fn unregister(aumid: &str, protocol: &str) -> Result<()> {
        for path in [format!("Software\\Classes\\AppUserModelId\\{aumid}"), format!("Software\\Classes\\{protocol}")] {
            let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(&path).as_ptr()) };
            if rc != 0 && rc != ERROR_FILE_NOT_FOUND {
                bail!("cannot delete HKCU\\{path} (error {rc})");
            }
        }
        Ok(())
    }
}

mod toast {
    use std::sync::OnceLock;

    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::HSTRING;

    /// Whether this process registered (or found) the entries a toast needs.
    static REGISTERED: OnceLock<bool> = OnceLock::new();

    fn registered() -> bool {
        *REGISTERED.get_or_init(|| {
            let Ok(exe) = std::env::current_exe() else { return false };
            match super::registration::register(super::AUMID, super::PROTOCOL, &exe.to_string_lossy()) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("toast registration: {e:#}");
                    false
                }
            }
        })
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    /// The toast's XML: two lines of text, and a button when there is
    /// somewhere to go.
    pub fn xml(title: &str, body: &str, action: Option<&str>) -> String {
        let (launch, actions) = match action {
            Some(url) => (
                format!(" launch=\"{}\" activationType=\"protocol\"", escape(url)),
                format!(
                    "<actions><action content=\"Go to pane\" arguments=\"{}\" activationType=\"protocol\"/></actions>",
                    escape(url)
                ),
            ),
            None => (String::new(), String::new()),
        };
        format!(
            "<toast{launch}><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>{actions}</toast>",
            escape(title),
            escape(body)
        )
    }

    fn show_inner(title: &str, body: &str, action: Option<&str>) -> windows::core::Result<()> {
        // WinRT wants the thread initialised; an "already done" answer is fine.
        unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
        }
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml(title, body, action)))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(super::AUMID))?;
        notifier.Show(&toast)
    }

    /// True when the toast went out; false (and a log line) otherwise, so
    /// the caller can fall back.
    pub fn show(title: &str, body: &str, action: Option<&str>) -> bool {
        // Tests and CI: no toasts, and no registry entries pointing at a
        // test binary (the balloon needs neither).
        if std::env::var_os("KEEPANE_NO_TOAST").is_some() {
            return false;
        }
        if !registered() {
            return false;
        }
        match show_inner(title, body, action) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("toast: {e}");
                false
            }
        }
    }
}

fn spawn_thread() -> Option<Sender<Note>> {
    let (tx, rx) = channel::<Note>();
    let (ready_tx, ready_rx) = channel::<bool>();
    let spawned = std::thread::Builder::new().name("notify".into()).spawn(move || {
        let Some(hwnd) = message_window() else {
            let _ = ready_tx.send(false);
            return;
        };
        let _ = ready_tx.send(true);
        // A notification stays up for a few seconds; the icon is added just
        // before it and taken away once Windows has had time to show it.
        while let Ok(note) = rx.recv() {
            if !show(hwnd, &note) {
                log::warn!("notification failed: {}", note.title);
            }
            // Keep draining while more arrive so a burst shares one icon.
            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            loop {
                pump(hwnd);
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(next) => {
                        show(hwnd, &next);
                    }
                    Err(_) if std::time::Instant::now() >= deadline => break,
                    Err(_) => {}
                }
            }
            remove_icon(hwnd);
        }
        unsafe { DestroyWindow(hwnd) };
    });
    if spawned.is_err() {
        return None;
    }
    // If the desktop is unreachable the thread says so instead of leaving
    // every later notification to queue up in a channel nobody reads.
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(true) => Some(tx),
        _ => None,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Copy into one of the fixed-size fields of NOTIFYICONDATAW, truncating on
/// a character boundary and leaving the terminating zero.
fn put(dst: &mut [u16], s: &str) {
    let src = wide(s);
    let n = src.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&src[..n]);
    dst[n] = 0;
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = hwnd;
    d.uID = 1;
    d
}

fn show(hwnd: HWND, note: &Note) -> bool {
    let mut d = icon_data(hwnd);
    d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_INFO;
    d.uCallbackMessage = 0;
    d.hIcon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    put(&mut d.szTip, "keepane");
    put(&mut d.szInfoTitle, &note.title);
    put(&mut d.szInfo, &note.body);
    d.dwInfoFlags = NIIF_INFO;
    // ADD first: modifying an icon that is not there does nothing.
    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &d) } != 0;
    if added {
        return true;
    }
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) != 0 }
}

fn remove_icon(hwnd: HWND) {
    let d = icon_data(hwnd);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
}

/// Drain the window's queue; the balloon needs a pumping thread to appear.
fn pump(_hwnd: HWND) {
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, 1) } != 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// A window nobody sees, only there because a tray icon needs an owner.
fn message_window() -> Option<HWND> {
    let class = wide("keepane-notify\0");
    let name = wide("keepane\0");
    let mut wc: WNDCLASSEXW = unsafe { std::mem::zeroed() };
    wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
    wc.lpfnWndProc = Some(wnd_proc);
    wc.lpszClassName = class.as_ptr();
    // A second server in the same process would re-register; either the
    // class is ours already or CreateWindowExW will tell us.
    unsafe { RegisterClassExW(&wc) };
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            name.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() { None } else { Some(hwnd) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text fields are fixed-size; long input must be cut, never
    /// overflow, and always stay zero-terminated.
    #[test]
    fn long_text_is_truncated_safely() {
        let mut buf = [0u16; 8];
        put(&mut buf, "hello world, this is much too long");
        assert_eq!(buf[7], 0, "zero terminated");
        assert_eq!(String::from_utf16_lossy(&buf[..7]), "hello w");

        let mut small = [0u16; 1];
        put(&mut small, "x");
        assert_eq!(small[0], 0, "nothing fits but the terminator");

        let mut exact = [0u16; 4];
        put(&mut exact, "abc");
        assert_eq!(String::from_utf16_lossy(&exact[..3]), "abc");
        assert_eq!(exact[3], 0);
    }

    /// Asking for a notification must never take the server down, whatever
    /// the desktop situation is (this test runs in session 0 on CI).
    #[test]
    fn notify_never_panics() {
        // No toast from a test: it would register this test binary as the
        // keepane:// handler on the developer's machine.
        unsafe { std::env::set_var("KEEPANE_NO_TOAST", "1") };
        let _ = notify("keepane", "test notification");
        let _ = notify("keepane", "");
        let _ = notify("", "body only");
        let _ = notify_with("keepane", "with a button", Some("keepane://go/default/3"));
    }

    #[test]
    fn go_urls_round_trip() {
        assert_eq!(go_to_pane_url("default", 7), "keepane://go/default/7");
        assert_eq!(parse_go_url("keepane://go/default/7"), Some(("default".into(), 7)));
        assert_eq!(parse_go_url("keepane://go/default/7/"), Some(("default".into(), 7)), "a trailing slash is fine");
        // Odd socket names survive the trip.
        for sock in ["work", "my sock", "a/b", "中文", "x%y"] {
            let url = go_to_pane_url(sock, 42);
            assert!(url.starts_with("keepane://go/"), "{url}");
            assert_eq!(parse_go_url(&url), Some((sock.to_string(), 42)), "{url}");
        }
        for bad in [
            "keepane://go/",
            "keepane://go//3",
            "keepane://stop/default/3",
            "http://go/default/3",
            "keepane://go/default/x",
            "keepane://go/default/3/extra",
            "keepane://go/%zz/3",
        ] {
            assert_eq!(parse_go_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn toast_xml_is_escaped_and_has_a_button_only_with_a_url() {
        let x = toast::xml("a & b", "<done> \"ok\"", Some("keepane://go/s/1"));
        assert!(x.contains("<text>a &amp; b</text>"), "{x}");
        assert!(x.contains("<text>&lt;done&gt; &quot;ok&quot;</text>"), "{x}");
        assert!(x.contains("content=\"Go to pane\" arguments=\"keepane://go/s/1\" activationType=\"protocol\""), "{x}");
        assert!(x.starts_with("<toast launch=\"keepane://go/s/1\" activationType=\"protocol\">"), "{x}");
        let plain = toast::xml("t", "b", None);
        assert!(!plain.contains("<actions>") && plain.starts_with("<toast><visual>"), "{plain}");
        // It parses as XML (the toast API would refuse it otherwise).
        assert_eq!(x.matches('<').count(), x.matches('>').count());
    }

    /// The real registry, with throwaway names so nothing of the user's is
    /// touched: register, register again, remove, remove again.
    #[test]
    fn registration_round_trip() {
        let id = format!("keepane-unit-test-{}", std::process::id());
        let proto = format!("keepanetest{}", std::process::id());
        let _ = registration::unregister(&id, &proto);
        registration::register(&id, &proto, r"C:\x\keepane.exe").unwrap();
        registration::register(&id, &proto, r"C:\x\keepane.exe").unwrap();
        // What the protocol runs: headless, quoted, the URL as %1.
        assert_eq!(
            registration::protocol_command(r"C:\x y\keepane.exe"),
            "conhost.exe --headless \"C:\\x y\\keepane.exe\" \"%1\""
        );
        registration::unregister(&id, &proto).unwrap();
        registration::unregister(&id, &proto).unwrap();
    }
}
