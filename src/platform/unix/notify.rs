//! Desktop notifications: `notify-send` (freedesktop) on Linux, `osascript`
//! on macOS. No "go to pane" button: those links are Windows's.

/// Raise a notification; whether one was shown.
pub fn notify(title: &str, body: &str) -> bool {
    notify_with(title, body, None)
}

/// The same, with the action a Windows toast's button would take (not here).
pub fn notify_with(title: &str, body: &str, _action: Option<&str>) -> bool {
    if crate::legacy::var_os("KEEPANE_NO_TOAST").is_some() {
        return false;
    }
    let status = if cfg!(target_os = "macos") {
        let script = format!("display notification {:?} with title {:?}", body, title);
        std::process::Command::new("osascript").args(["-e", &script]).output()
    } else {
        std::process::Command::new("notify-send").args(["--app-name=keepane", title, body]).output()
    };
    status.is_ok_and(|o| o.status.success())
}

/// A link that would bring pane `pane` forward (Windows's `keepane://`).
pub fn go_to_pane_url(socket: &str, pane: u32) -> String {
    format!("keepane://focus/{socket}/{pane}")
}

/// The socket and pane of such a link.
pub fn parse_go_url(url: &str) -> Option<(String, u32)> {
    let rest = url.strip_prefix("keepane://focus/")?;
    let (socket, pane) = rest.trim_end_matches('/').rsplit_once('/')?;
    Some((socket.to_string(), pane.parse().ok()?))
}

/// Bring the terminal window forward: a terminal emulator's business here.
pub fn raise_console_window() -> bool {
    false
}
