//! A pane sized to a phone (`web-fit`, the phone page's fit button): the
//! pane fills its window (zoomed) and its session takes the phone's columns
//! and rows, so a full-screen program draws for the phone. A session has one
//! size for everyone, so while it lasts the computer, and any other phone,
//! see that session at the phone's size too; the page says so. It ends when
//! the phone says so (`-u`), when no phone has shown the pane for
//! `LEFT_FOR`, when `keepane web` stops, or when someone sizes the session
//! by hand (`resize-window`); the session gets back the size it had.

use std::time::{Duration, Instant};

use super::layout::PaneId;
use super::{ClientId, Outcome, Server, SessionId};
use crate::command::Target;

/// A fitted pane nobody shows any more is put back after this: long enough
/// for a phone to go from pane to pane, or to reconnect.
const LEFT_FOR: Duration = Duration::from_secs(10);

pub(super) struct WebFit {
    pane: PaneId,
    /// The session's size while fitted: the phone's, with the status line
    /// and border rows added.
    cols: u16,
    rows: u16,
    /// The size it had before.
    saved: (u16, u16),
    /// The fit zoomed the pane (and so unzooms it after).
    zoomed: bool,
    /// When a phone was last seen not showing it.
    unwatched: Option<Instant>,
}

impl Server {
    /// The size a fit holds the session at, if one does.
    pub(super) fn web_fit_size(&self, sid: SessionId) -> Option<(u16, u16)> {
        self.web_fits.get(&sid).map(|f| (f.cols, f.rows))
    }

    /// `web-fit -t pane -x cols -y rows`, or `-u` to undo.
    pub(super) fn web_fit(
        &mut self,
        cid: Option<ClientId>,
        target: Option<&Target>,
        size: Option<(u16, u16)>,
    ) -> Outcome {
        let (sid, widx, pid) = match self.resolve(target, cid) {
            Ok(r) => r,
            Err(e) => return Outcome::Error(e),
        };
        let Some((cols, rows)) = size else {
            if self.web_fits.get(&sid).is_some_and(|f| f.pane == pid) {
                self.web_unfit(sid);
                return Outcome::Text(format!("%{pid}: back to its size"));
            }
            return Outcome::Text(format!("%{pid}: not fitted"));
        };
        let (cols, rows) = (cols.clamp(10, 1000), rows.clamp(3, 1000));
        // The rows the phone shows are the pane's: the session's have the
        // status line and a border row besides.
        let extra = u16::from(self.opts.status) + u16::from(self.border_rows().is_some());
        let saved = match self.web_fits.remove(&sid) {
            // Another pane of the session was fitted: it goes back to its
            // place, the session's own size is still the one to go back to.
            Some(old) => {
                self.unzoom_fitted(sid, &old);
                old.saved
            }
            None => match self.session(sid) {
                Some(s) => (s.cols, s.rows),
                None => return Outcome::Error("no such session".into()),
            },
        };
        let Some(w) = self.session_mut(sid).and_then(|s| s.windows.get_mut(widx)) else {
            return Outcome::Error("no such window".into());
        };
        let zoomed = w.panes.len() > 1 && !(w.zoomed && w.active == pid);
        if zoomed {
            w.active = pid;
            w.zoomed = true;
        }
        self.web_fits.insert(sid, WebFit { pane: pid, cols, rows: rows + extra, saved, zoomed, unwatched: None });
        self.resize_session(sid, cols, rows + extra);
        let name = self.session(sid).map(|s| s.name.clone()).unwrap_or_default();
        let text = format!("web: a phone fitted session {name} to {cols}x{rows} (until it leaves %{pid})");
        self.note_message(&text);
        let now = Instant::now();
        let terminals: Vec<ClientId> = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
        for c in &terminals {
            if let Some(c) = self.clients.get_mut(c) {
                c.message = Some((text.clone(), now));
            }
        }
        Outcome::Text(format!(
            "%{pid} fitted to {cols}x{rows}; {} terminal(s) on session {name} see it at this size until it is undone",
            terminals.len()
        ))
    }

    fn unzoom_fitted(&mut self, sid: SessionId, f: &WebFit) {
        if !f.zoomed {
            return;
        }
        if let Some(w) = self.session_mut(sid).and_then(|s| s.windows.iter_mut().find(|w| w.pane(f.pane).is_some()))
            && w.zoomed
            && w.active == f.pane
        {
            w.zoomed = false;
        }
    }

    /// The session back to the size it had, the pane back in its place.
    pub(super) fn web_unfit(&mut self, sid: SessionId) {
        let Some(f) = self.web_fits.remove(&sid) else { return };
        self.unzoom_fitted(sid, &f);
        self.resize_session(sid, f.saved.0, f.saved.1);
        // `smallest` / `largest` fold the clients again.
        self.fit_session(sid, None);
    }

    /// `resize-window`: whoever sized it by hand has the last word.
    pub(super) fn web_fit_dropped(&mut self, sid: SessionId) {
        if let Some(f) = self.web_fits.remove(&sid) {
            self.unzoom_fitted(sid, &f);
        }
    }

    /// Every tick: a fit no phone shows any more for `LEFT_FOR`, or whose
    /// pane is gone, or with `keepane web` off, ends.
    pub(super) fn web_fit_tick(&mut self) {
        let now = Instant::now();
        let sids: Vec<SessionId> = self.web_fits.keys().copied().collect();
        for sid in sids {
            let pane = self.web_fits[&sid].pane;
            let here = self.session(sid).is_some_and(|s| s.windows.iter().any(|w| w.pane(pane).is_some()));
            let watched = self.web.as_ref().is_some_and(|w| w.watching(&format!("%{pane}")));
            let over = if !here || self.web.is_none() {
                true
            } else if watched {
                self.web_fits.get_mut(&sid).expect("listed").unwatched = None;
                false
            } else {
                let f = self.web_fits.get_mut(&sid).expect("listed");
                now.duration_since(*f.unwatched.get_or_insert(now)) >= LEFT_FOR
            };
            if over {
                if here {
                    self.web_unfit(sid);
                } else {
                    // Its pane went away; the session keeps its windows.
                    let f = self.web_fits.remove(&sid).expect("listed");
                    self.resize_session(sid, f.saved.0, f.saved.1);
                    self.fit_session(sid, None);
                }
            }
        }
    }
}
