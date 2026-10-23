//! Save everything when Windows shuts down, restarts or the user logs off.
//!
//! Windows tells a process the session is ending in one of two ways, and a
//! keepane server may be in either situation: started by a client it has no
//! console (`DETACHED_PROCESS`), so only a top-level window of its own gets
//! `WM_QUERYENDSESSION` / `WM_ENDSESSION`; started at logon it sits under
//! a headless conhost, whose console control handler gets
//! `CTRL_SHUTDOWN_EVENT` / `CTRL_LOGOFF_EVENT`. A process that has neither a
//! window nor a console is simply killed, and the last half minute of pane
//! history and every directory change since the last autosave go with it.
//!
//! So the server keeps a hidden window on a thread of its own and registers
//! a console handler too; whichever fires asks the server (through the
//! callback given to `watch`) to save now, and returns once it has. Both may
//! fire, and Windows asks more than once (the question, then the end): the
//! server saves on the first and lets the later ones be, since by then the
//! shutdown may be killing the panes' shells, and a save would keep a layout
//! without them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Console::{
    CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::Shutdown::ShutdownBlockReasonCreate;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassExW, TranslateMessage,
    WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSEXW, WS_OVERLAPPED,
};

/// The window class of the hidden window; its title is the socket name, so
/// a test (or a curious `FindWindowW`) can tell servers apart.
pub const WINDOW_CLASS: &str = "keepane-shutdown";

type Hook = Box<dyn Fn() + Send + Sync>;

std::thread_local! {
    /// The server a hidden window belongs to, on that window's thread.
    static OWNER: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// What to run when the session ends: one entry per server in this process
/// (one, outside the tests), by its name. The console's signal runs them all;
/// a window's, its own server's only. A server that is gone left a callback
/// that finds nobody listening and returns at once.
static HOOKS: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());
static CTRL: Once = Once::new();
/// How many times the end of the session was signalled, for tests.
static FIRED: AtomicUsize = AtomicUsize::new(0);

/// Run the hooks of server `only`, or every one.
fn fire(only: Option<&str>) {
    for (name, h) in HOOKS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if only.is_none_or(|o| o == name) {
            h();
        }
    }
    FIRED.fetch_add(1, Ordering::SeqCst);
}

/// Start watching for the end of the session. `on_end` must save and return
/// (Windows waits only a few seconds for a window to answer, and the
/// blocking reason set here is what it shows while it does).
pub fn watch(name: &str, on_end: impl Fn() + Send + Sync + 'static) {
    HOOKS.lock().unwrap_or_else(|e| e.into_inner()).push((name.to_string(), Box::new(on_end)));
    CTRL.call_once(|| unsafe {
        SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    });
    let title = name.to_string();
    if let Err(e) = std::thread::Builder::new().name("shutdown-watch".into()).spawn(move || window_thread(&title)) {
        log::warn!("shutdown watcher: {e}");
    }
}

unsafe extern "system" fn ctrl_handler(kind: u32) -> i32 {
    if matches!(kind, CTRL_SHUTDOWN_EVENT | CTRL_LOGOFF_EVENT | CTRL_CLOSE_EVENT) {
        fire(None);
        return 1;
    }
    0
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        // Save on the question already: the answer may be followed by the
        // end itself with very little time in between.
        WM_QUERYENDSESSION => {
            OWNER.with(|o| fire(Some(&o.borrow())));
            1
        }
        WM_ENDSESSION => {
            if w != 0 {
                OWNER.with(|o| fire(Some(&o.borrow())));
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn window_thread(title: &str) {
    OWNER.with(|o| *o.borrow_mut() = title.to_string());
    let class = wide(WINDOW_CLASS);
    let mut wc: WNDCLASSEXW = unsafe { std::mem::zeroed() };
    wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
    wc.lpfnWndProc = Some(wnd_proc);
    wc.lpszClassName = class.as_ptr();
    unsafe { RegisterClassExW(&wc) }; // already ours when a test runs two servers
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            wide(title).as_ptr(),
            WS_OVERLAPPED, // never shown: no WS_VISIBLE, and nothing calls ShowWindow
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
    if hwnd.is_null() {
        log::warn!("shutdown watcher: no window; sessions will not be saved at shutdown");
        return;
    }
    unsafe { ShutdownBlockReasonCreate(hwnd, wide("keepane is saving its sessions").as_ptr()) };
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The hidden window of the server named `name`, once it is up.
pub fn window(name: &str) -> Option<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
    let h = unsafe { FindWindowW(wide(WINDOW_CLASS).as_ptr(), wide(name).as_ptr()) };
    if h.is_null() { None } else { Some(h) }
}

/// How many times the end of the session was signalled (tests).
pub fn fired() -> usize {
    FIRED.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    /// The hooks are the process's: the tests that signal take turns.
    static TURN: Mutex<()> = Mutex::new(());

    /// Windows sends WM_QUERYENDSESSION to every top-level window; sending
    /// it ourselves runs the same path: the hook, on the window's thread,
    /// before the message returns.
    #[test]
    fn the_hidden_window_runs_the_hook_when_the_session_ends() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        static SAVES: AtomicUsize = AtomicUsize::new(0);
        watch("unit-test", || {
            SAVES.fetch_add(1, Ordering::SeqCst);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let hwnd = loop {
            if let Some(h) = window("unit-test") {
                break h;
            }
            assert!(Instant::now() < deadline, "the window never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let before = SAVES.load(Ordering::SeqCst);
        let ok = unsafe { SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0) };
        assert_eq!(ok, 1, "shutdown is allowed to go on");
        assert_eq!(SAVES.load(Ordering::SeqCst), before + 1, "saved on the question");
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 1, 0) };
        assert_eq!(SAVES.load(Ordering::SeqCst), before + 2, "and on the end itself");
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 0, 0) };
        assert_eq!(SAVES.load(Ordering::SeqCst), before + 2, "a cancelled end saves nothing more");
        // The console handler only answers the session-ending events.
        assert_eq!(unsafe { ctrl_handler(CTRL_SHUTDOWN_EVENT) }, 1);
        assert_eq!(unsafe { ctrl_handler(CTRL_LOGOFF_EVENT) }, 1);
        assert_eq!(unsafe { ctrl_handler(0) }, 0, "C-c is not ours");
        assert_eq!(SAVES.load(Ordering::SeqCst), before + 4);
        assert!(fired() >= 4);
    }

    /// A window's message is for its own server only (a test process runs
    /// many, and each must save when it is asked, not when another is); the
    /// console's signal is for every one.
    #[test]
    fn a_window_asks_its_own_server_only() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        static MINE: AtomicUsize = AtomicUsize::new(0);
        static OTHER: AtomicUsize = AtomicUsize::new(0);
        watch("unit-mine", || {
            MINE.fetch_add(1, Ordering::SeqCst);
        });
        watch("unit-other", || {
            OTHER.fetch_add(1, Ordering::SeqCst);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let hwnd = loop {
            if let (Some(h), Some(_)) = (window("unit-mine"), window("unit-other")) {
                break h;
            }
            assert!(Instant::now() < deadline, "the windows never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let (mine, other) = (MINE.load(Ordering::SeqCst), OTHER.load(Ordering::SeqCst));
        unsafe { SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0) };
        unsafe { SendMessageW(hwnd, WM_ENDSESSION, 1, 0) };
        assert_eq!(MINE.load(Ordering::SeqCst), mine + 2);
        assert_eq!(OTHER.load(Ordering::SeqCst), other, "not the other server's");
        unsafe { ctrl_handler(CTRL_LOGOFF_EVENT) };
        assert_eq!((MINE.load(Ordering::SeqCst), OTHER.load(Ordering::SeqCst)), (mine + 3, other + 1));
    }
}
