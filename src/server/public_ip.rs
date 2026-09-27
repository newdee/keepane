//! `#{public_ip}`: the address the internet sees this machine at. Only an
//! outside service knows it, so it is asked (`api.ipify.org`, with `curl`,
//! on a thread of its own) only while a format that is drawn uses it: a
//! status line without `#{public_ip}` never asks anyone. Asked every ten
//! minutes; a failed ask (offline) is tried again in a minute, and until an
//! answer comes the variable is empty.

use super::{Event, Server};
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(10 * 60);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct PublicIp {
    pub value: String,
    due: Option<Instant>,
    asking: bool,
}

impl Server {
    /// Whether any format that is drawn asks for the public address.
    fn public_ip_wanted(&self) -> bool {
        let o = &self.opts;
        [
            &o.status_left,
            &o.status_right,
            &o.window_status_format,
            &o.window_status_current_format,
            &o.pane_border_format,
        ]
        .iter()
        .any(|f| f.contains("public_ip"))
    }

    /// Every tick: ask when a format wants it and it is time.
    pub(super) fn public_ip_tick(&mut self) {
        if !self.public_ip_wanted() || self.public_ip.asking || self.public_ip.due.is_some_and(|d| Instant::now() < d) {
            return;
        }
        self.public_ip.asking = true;
        let tx = self.events.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::PublicIp(ask()));
        });
    }

    pub(super) fn public_ip_answered(&mut self, ip: Option<String>) {
        self.public_ip.asking = false;
        self.public_ip.due = Some(Instant::now() + if ip.is_some() { EVERY } else { RETRY });
        self.public_ip.value = ip.unwrap_or_default();
        for c in self.clients.values_mut() {
            c.last_grid = None;
        }
    }
}

/// The address, asked of api.ipify.org; None when it could not be.
fn ask() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args(["-fsS", "-m", "5", "-H", "User-Agent: keepane", "https://api.ipify.org"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// An address, and nothing else: whatever else a service says is not shown.
fn parse(text: &str) -> Option<String> {
    text.trim().parse::<std::net::IpAddr>().ok().map(|ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_address_is_taken() {
        assert_eq!(parse("203.0.113.7\n").as_deref(), Some("203.0.113.7"));
        assert_eq!(parse("2001:db8::1").as_deref(), Some("2001:db8::1"));
        assert_eq!(parse("<html>rate limited</html>"), None);
        assert_eq!(parse(""), None);
    }

    /// Nobody is asked while no format uses it; once one does, it is asked;
    /// an answer is kept for ten minutes, a failure retried sooner. (The
    /// ask itself is not run here: a test must not reach the internet.)
    #[test]
    fn it_is_asked_only_while_a_format_uses_it() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<Event>();
        let mut s = Server::new(pane_tx, "t".into(), events);
        assert!(!s.public_ip_wanted(), "not in the default look");
        s.public_ip_tick();
        assert!(!s.public_ip.asking);
        s.opts.status_right = "#{public_ip} %H:%M".into();
        assert!(s.public_ip_wanted());
        s.public_ip_answered(Some("203.0.113.7".into()));
        assert_eq!(s.public_ip.value, "203.0.113.7");
        s.public_ip_tick();
        assert!(!s.public_ip.asking, "not due again for ten minutes");
        let ok_due = s.public_ip.due.unwrap();
        s.public_ip_answered(None);
        assert_eq!(s.public_ip.value, "", "unknown, not stale");
        assert!(s.public_ip.due.unwrap() < ok_due, "a failure is tried again sooner");
    }
}
