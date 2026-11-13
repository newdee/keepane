//! What is shared (`share`, docs/design/share.md): each file or folder given
//! out, by its token, until its time is up, it is stopped, or (`--once`) it
//! went once. The phones' page asks what a link shares (`web-share-open`) and
//! says when all of it went (`web-share-sent`); the download itself is
//! `crate::share::serve`'s.
//!
//! The list is kept like the saved sessions (`<sessions-dir>/share/
//! shares.json`, written on every change): a server that starts and brings
//! the sessions back (`restore-on-start`) brings the shares back too, each on
//! its own link again; one whose time was up meanwhile has ended. While no
//! server runs, no link works. The last `ENDED_KEPT` that ended are kept, so
//! what was shared and how it ended can still be seen (the dashboard's page
//! 2, `share -l -J`).

use super::{ClientId, Outcome, PaneId, Server};
use crate::command::ShareWhat;
use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

/// How many ended shares are kept to be seen.
const ENDED_KEPT: usize = 100;
/// How much of each share's story is kept (`Share::events`).
const EVENTS_KEPT: usize = 20;

/// How a share ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Ended {
    /// `share stop` (or a new link for it: `share again`).
    Stopped,
    /// Its time was up.
    Expired,
    /// For once, and it went.
    Used,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Share {
    pub(super) id: u32,
    token: String,
    path: PathBuf,
    dir: bool,
    /// The file's name, or the folder's (its zip is this `.zip`).
    name: String,
    /// When it was shared (Unix milliseconds).
    #[serde(default)]
    created: i64,
    /// When it ends (Unix milliseconds); None, when stopped.
    pub(super) until: Option<i64>,
    once: bool,
    /// The pane it was shared from.
    by: Option<PaneId>,
    /// Times all of it went.
    sent: u32,
    /// Addresses that opened it (each said once).
    seen: Vec<String>,
    /// The port its link was given on: the link is the same again only on
    /// it (0: not known).
    #[serde(default)]
    port: u16,
    /// How it ended, and when (Unix milliseconds).
    #[serde(default)]
    ended: Option<(Ended, i64)>,
    /// What happened to it (Unix milliseconds, what), oldest first: shared,
    /// opened, sent, extended, back, ended. The last `EVENTS_KEPT`.
    #[serde(default)]
    events: Vec<(i64, String)>,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Shares {
    pub(super) list: Vec<Share>,
    /// The ones that ended, the latest first.
    #[serde(default)]
    pub(super) ended: VecDeque<Share>,
    pub(super) last: u32,
}

fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

fn at_text(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%m-%d %H:%M").to_string()).unwrap_or_default()
}

impl Share {
    /// Something that happened to it, kept with it.
    fn event(&mut self, what: String) {
        self.events.push((now_ms(), what));
        let over = self.events.len().saturating_sub(EVENTS_KEPT);
        self.events.drain(..over);
    }

    fn what(&self) -> &'static str {
        if self.dir { "folder" } else { "file" }
    }

    fn until_text(&self) -> String {
        match self.until {
            Some(ms) => format!("until {}", at_text(ms)),
            None => "until stopped".to_string(),
        }
    }

    /// Its state in a word: active, missing (its file is gone; it comes
    /// back with it), stopped, expired, used.
    fn state(&self) -> &'static str {
        match self.ended {
            Some((Ended::Stopped, _)) => "stopped",
            Some((Ended::Expired, _)) => "expired",
            Some((Ended::Used, _)) => "used",
            None if !self.path.exists() => "missing",
            None => "active",
        }
    }
}

impl Server {
    pub(super) fn exec_share(&mut self, what: ShareWhat, cid: Option<ClientId>) -> Outcome {
        match what {
            ShareWhat::List { json: false } => Outcome::Text(self.shares_listed()),
            ShareWhat::List { json: true } => Outcome::Text(self.shares_json()),
            ShareWhat::Add { path, for_secs, once } => match self.share_add(&path, for_secs, once, cid) {
                Ok(text) => Outcome::Text(text),
                Err(e) => Outcome::Error(format!("share: {e}")),
            },
            ShareWhat::Stop(id) => {
                let ids: Vec<u32> = match id {
                    Some(id) if self.shares.list.iter().any(|s| s.id == id) => vec![id],
                    Some(id) => return Outcome::Error(format!("share stop: no share {id} (share -l)")),
                    None => self.shares.list.iter().map(|s| s.id).collect(),
                };
                if ids.is_empty() {
                    return Outcome::Text("nothing was shared".into());
                }
                for id in &ids {
                    self.share_end(*id, Ended::Stopped);
                }
                self.shares_save();
                let ids: Vec<String> = ids.iter().map(u32::to_string).collect();
                let text = if ids.len() == 1 {
                    format!("share {} stopped: its link no longer works", ids[0])
                } else {
                    format!("shares {} stopped: their links no longer work", ids.join(", "))
                };
                self.share_note(&text);
                Outcome::Text(text)
            }
            ShareWhat::Extend { id, for_secs } => match self.share_extend(id, for_secs) {
                Ok(text) => Outcome::Text(text),
                Err(e) => Outcome::Error(format!("share extend: {e}")),
            },
            ShareWhat::Again(id) => {
                let Some(s) = self.share_by_id(id).cloned() else {
                    return Outcome::Error(format!(
                        "share again: no share {id} (share -l -J lists the ended ones too)"
                    ));
                };
                let for_secs = s.until.map(|u| ((u - s.created).max(1000) / 1000) as u64).or(Some(0));
                if self.shares.list.iter().any(|x| x.id == id) {
                    self.share_end(id, Ended::Stopped);
                }
                match self.share_add(&s.path.to_string_lossy(), for_secs, s.once, cid) {
                    Ok(text) => Outcome::Text(format!("share {id} ended; shared again on a new link:\n{text}")),
                    Err(e) => {
                        self.shares_save();
                        Outcome::Error(format!("share again: {e}"))
                    }
                }
            }
        }
    }

    /// A share by its number, ended or not.
    fn share_by_id(&self, id: u32) -> Option<&Share> {
        self.shares.list.iter().chain(self.shares.ended.iter()).find(|s| s.id == id)
    }

    /// End share `id` (`how`), kept among the ended (the latest first, at
    /// most `ENDED_KEPT`); written by the caller.
    fn share_end(&mut self, id: u32, how: Ended) {
        let Some(i) = self.shares.list.iter().position(|s| s.id == id) else { return };
        let mut s = self.shares.list.remove(i);
        s.ended = Some((how, now_ms()));
        s.event(match how {
            Ended::Stopped => "stopped".into(),
            Ended::Expired => "its time was up".into(),
            Ended::Used => "ended: it was for once".into(),
        });
        self.shares.ended.push_front(s);
        self.shares.ended.truncate(ENDED_KEPT);
    }

    /// `share extend ID [--for TIME]`: its time from now (the default when
    /// not said), on the same link; one whose time was up comes back. A
    /// stopped one, or one that went once, is shared again only on a new
    /// link (`share again`).
    fn share_extend(&mut self, id: u32, for_secs: Option<u64>) -> Result<String, String> {
        let until = match for_secs {
            None => Some(now_ms() + crate::share::DEFAULT_FOR.as_millis() as i64),
            Some(0) => None,
            Some(s) => Some(now_ms() + (s as i64).saturating_mul(1000)),
        };
        if let Some(s) = self.shares.list.iter_mut().find(|s| s.id == id) {
            s.until = until;
            let what = format!("extended, {}", s.until_text());
            s.event(what);
            let text = format!("share {id}: {}, on the same link", s.until_text());
            self.shares_save();
            return Ok(text);
        }
        let Some(i) = self.shares.ended.iter().position(|s| s.id == id) else {
            return Err(format!("no share {id} (share -l -J)"));
        };
        match self.shares.ended[i].ended {
            Some((Ended::Expired, _)) => {}
            Some((how, _)) => {
                let how = if how == Ended::Stopped { "was stopped" } else { "went once" };
                return Err(format!("share {id} {how}: `share again {id}` shares it on a new link"));
            }
            None => {}
        }
        if self.web.is_none() {
            let o = crate::web::kept_on(&self.web_on).unwrap_or_default();
            if let Outcome::Error(e) = self.web_serve(o) {
                return Err(format!("the link needs the phones' page, which did not start: {e}"));
            }
        }
        let mut s = self.shares.ended.remove(i).expect("found");
        s.ended = None;
        s.until = until;
        let what = format!("back on its link, {}", s.until_text());
        s.event(what);
        let text = format!("share {id}: back, {}, on the same link", s.until_text());
        self.shares.list.push(s);
        self.shares_save();
        self.share_note(&text);
        Ok(text)
    }

    fn share_add(
        &mut self,
        path: &str,
        for_secs: Option<u64>,
        once: bool,
        cid: Option<ClientId>,
    ) -> Result<String, String> {
        let asked = super::expand_home(path);
        let p = std::path::Path::new(&asked);
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            PathBuf::from(self.client_cwd(cid).unwrap_or_default()).join(p)
        };
        let meta = std::fs::metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if !meta.is_file() && !meta.is_dir() {
            return Err(format!("{}: not a file or a folder", p.display()));
        }
        let real = crate::git::real(&p);
        // Handed to the page as JSON text: a path that is not text (bytes
        // a Unix file name may hold) cannot be.
        if real.to_str().is_none() {
            return Err(format!("{}: the path is not UTF-8 text; rename it to share it", real.display()));
        }
        let name = real
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "shared".into());
        // The link is the phones' page's: on, if it was not, as it was last
        // served (else as `keepane web` does), for as long as this server
        // runs (not kept for a restore).
        let mut web_note = String::new();
        if self.web.is_none() {
            let o = crate::web::kept_on(&self.web_on).unwrap_or_default();
            if let Outcome::Error(e) = self.web_serve(o) {
                return Err(format!("the link needs the phones' page, which did not start: {e}"));
            }
            web_note = "\nThe phone page was off: it serves now (`keepane web stop` ends it, and these links).".into();
        }
        let w = self.web.as_ref().expect("served");
        let token = crate::web::new_key().map_err(|e| format!("{e:#}"))?;
        let urls: Vec<String> =
            w.addrs().iter().map(|ip| format!("http://{}/s/{token}", crate::link::hostport(*ip, w.port()))).collect();
        let port = w.port();
        let for_ = match for_secs {
            None => Some(crate::share::DEFAULT_FOR),
            Some(0) => None,
            Some(s) => Some(Duration::from_secs(s)),
        };
        let created = now_ms();
        let until = for_.map(|d| created + d.as_millis() as i64);
        self.shares.last += 1;
        let by = self.caller_pane(cid);
        let s = Share {
            id: self.shares.last,
            token,
            path: real.clone(),
            dir: meta.is_dir(),
            name,
            created,
            until,
            once,
            by,
            sent: 0,
            seen: Vec::new(),
            port,
            ended: None,
            events: Vec::new(),
        };
        let mut s = s;
        s.event(format!("shared, {}{}", s.until_text(), if once { ", once" } else { "" }));
        let size = if s.dir { String::new() } else { format!(", {}", super::human_size(meta.len())) };
        let how = match for_ {
            Some(d) => format!("{} ({})", s.until_text(), crate::share::for_text(d)),
            None => s.until_text(),
        };
        let once_text = if once { ", once" } else { "" };
        let from = by.map(|p| format!(" from pane %{p}")).unwrap_or_default();
        self.share_note(&format!("share {}: {} {}{from}, {how}{once_text}", s.id, s.what(), s.name));
        let mut out = crate::web::qr_text(&urls[0]).map(|q| q + "\n").unwrap_or_default();
        out.push_str(&format!("share {}: {} ({}{size}), {how}{once_text}\n", s.id, real.display(), s.what()));
        out.push_str(&format!("  {}\n", urls[0]));
        for u in &urls[1..] {
            out.push_str(&format!("  also {u}\n"));
        }
        let zip = if s.dir { " (as a zip)" } else { "" };
        out.push_str(&format!(
            "Anyone with the link can download it{zip}; `keepane share stop {}` ends it.{web_note}",
            s.id
        ));
        self.shares.list.push(s);
        self.shares_save();
        Ok(out)
    }

    fn shares_listed(&self) -> String {
        if self.shares.list.is_empty() {
            return "nothing is shared (`keepane share PATH` shares a file or a folder)".into();
        }
        let base = self.web.as_ref().and_then(|w| w.addrs().first().map(|ip| (*ip, w.port())));
        let mut lines = Vec::new();
        for s in &self.shares.list {
            let once = if s.once { ", once" } else { "" };
            let from = s.by.map(|p| format!(", from pane %{p}")).unwrap_or_default();
            let missing = if s.path.exists() { "" } else { ", missing (its file is gone)" };
            lines.push(format!(
                "{}  {}  {}{once}, sent {}{from}{missing}  {}",
                s.id,
                s.what(),
                s.until_text(),
                s.sent,
                s.path.display()
            ));
            match base {
                Some((ip, port)) => lines.push(format!("   http://{}/s/{}", crate::link::hostport(ip, port), s.token)),
                None => lines.push("   (the phone page is off: `keepane web` serves the links again)".into()),
            }
        }
        lines.join("\n")
    }

    /// `share -l -J`: every share, the ended ones after (the latest first),
    /// each with its state, its links as served now, and whether its port
    /// is not the one its link was given on.
    fn shares_json(&self) -> String {
        let served = self.web.as_ref().map(|w| (w.addrs(), w.port()));
        let one = |s: &Share| {
            let links: Vec<String> = match (&served, s.ended) {
                (Some((addrs, port)), None) => addrs
                    .iter()
                    .map(|ip| format!("http://{}/s/{}", crate::link::hostport(*ip, *port), s.token))
                    .collect(),
                _ => Vec::new(),
            };
            let moved = served.as_ref().is_some_and(|(_, port)| s.port != 0 && *port != s.port);
            serde_json::json!({
                "id": s.id, "state": s.state(), "kind": s.what(), "name": s.name, "path": s.path.to_string_lossy(),
                "created": s.created, "until": s.until, "once": s.once, "sent": s.sent, "seen": s.seen,
                "from": s.by.map(|p| format!("%{p}")), "links": links, "port": s.port, "port_moved": moved,
                "ended": s.ended.map(|(_, at)| at), "events": s.events,
            })
        };
        let all: Vec<serde_json::Value> = self.shares.list.iter().chain(self.shares.ended.iter()).map(one).collect();
        serde_json::json!({ "served": served.is_some(), "shares": all }).to_string()
    }

    /// `web-share-open`: what the link shares, as JSON, for the page to send.
    pub(super) fn share_open(&mut self, token: &str, peer: &str) -> Outcome {
        self.shares_tick();
        let Some(s) = self.shares.list.iter_mut().find(|s| crate::web::same_key(&s.token, token)) else {
            return Outcome::Error(crate::share::ENDED.into());
        };
        // Its file gone (moved, a drive not there): not now; back with it.
        if !s.path.exists() {
            return Outcome::Error("what this link shares is not on that machine now".into());
        }
        let json = serde_json::json!({ "path": s.path.to_string_lossy(), "dir": s.dir, "name": s.name }).to_string();
        if !s.seen.iter().any(|p| p == peer) {
            s.seen.push(peer.to_string());
            s.event(format!("opened by {peer}"));
            let text = format!("share {}: {peer} is downloading {}", s.id, s.name);
            self.shares_save();
            self.share_note(&text);
        }
        Outcome::Text(json)
    }

    /// `web-share-sent`: all of it went; once, it ends here.
    pub(super) fn share_sent(&mut self, token: &str, peer: &str, note: &str) -> Outcome {
        let Some(i) = self.shares.list.iter().position(|s| crate::web::same_key(&s.token, token)) else {
            return Outcome::Ok;
        };
        let s = &mut self.shares.list[i];
        s.sent += 1;
        s.event(format!("sent to {peer}"));
        let mut text = format!("share {}: {} sent to {peer}", s.id, s.name);
        if !note.is_empty() {
            text.push_str(&format!(" ({note})"));
        }
        if s.once {
            text.push_str("; it was for once: ended");
            let id = s.id;
            self.share_end(id, Ended::Used);
        }
        self.shares_save();
        self.share_note(&text);
        Outcome::Ok
    }

    /// Every tick: a share whose time is up ends.
    pub(super) fn shares_tick(&mut self) {
        let now = now_ms();
        let due: Vec<(u32, String)> = self
            .shares
            .list
            .iter()
            .filter(|s| s.until.is_some_and(|u| u <= now))
            .map(|s| (s.id, s.name.clone()))
            .collect();
        if due.is_empty() {
            return;
        }
        for (id, name) in due {
            self.share_end(id, Ended::Expired);
            self.share_note(&format!("share {id} ended: {name} (its time was up)"));
        }
        self.shares_save();
    }

    /// Where the list is kept: a folder of its own in the sessions
    /// directory (each `*.json` beside the saved sessions is read as one).
    pub(super) fn shares_file(&self) -> PathBuf {
        self.sessions_dir().join("share").join("shares.json")
    }

    /// Write the list (atomically: a temp file renamed over it).
    fn shares_save(&mut self) {
        let path = self.shares_file();
        let written = serde_json::to_string_pretty(&self.shares).map_err(|e| e.to_string()).and_then(|text| {
            std::fs::create_dir_all(path.parent().expect("in a folder")).map_err(|e| e.to_string())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                e.to_string()
            })
        });
        if let Err(e) = written {
            log::warn!("share: cannot keep the list in {}: {e}", path.display());
        }
    }

    /// The list as kept, at start (with the sessions): the shares whose
    /// time is not up are back, each on its own link; the others have
    /// ended. How many are back.
    pub(super) fn shares_load(&mut self) -> usize {
        let path = self.shares_file();
        let Ok(text) = std::fs::read_to_string(&path) else { return 0 };
        let mut kept: Shares = match serde_json::from_str(&text) {
            Ok(k) => k,
            Err(e) => {
                // Kept aside, not written over by the next share: one file
                // holds every share, so what it had would be gone.
                let bad = path.with_extension("json.bad");
                let aside = std::fs::rename(&path, &bad).map_or_else(
                    |r| format!("; it could not be moved aside ({r})"),
                    |()| format!("; kept as {}", bad.display()),
                );
                let said = format!("share: {} is not a list keepane reads ({e}){aside}", path.display());
                log::warn!("{said}");
                self.note_message(&said);
                return 0;
            }
        };
        // What this server shared already (none, at start) stays; numbers go on.
        kept.last = kept.last.max(self.shares.last);
        self.shares = kept;
        let now = now_ms();
        let due: Vec<u32> =
            self.shares.list.iter().filter(|s| s.until.is_some_and(|u| u <= now)).map(|s| s.id).collect();
        for id in &due {
            self.share_end(*id, Ended::Expired);
        }
        if !due.is_empty() {
            self.shares_save();
        }
        self.shares.list.len()
    }

    /// At start, after the sessions and the phones' page are back: the
    /// shares too, the page served for them if it is not, and said.
    pub(super) fn shares_resume(&mut self) {
        let n = self.shares_load();
        if n == 0 {
            return;
        }
        if self.web.is_none() {
            let o = crate::web::kept_on(&self.web_on).unwrap_or_default();
            if let Outcome::Error(e) = self.web_serve(o) {
                log::warn!("share: {n} share(s) back, but the phones' page did not start: {e}");
                return;
            }
        }
        for s in &mut self.shares.list {
            s.event("back: the server started again".into());
        }
        self.shares_save();
        let port = self.web.as_ref().map_or(0, |w| w.port());
        let moved = self.shares.list.iter().filter(|s| s.port != 0 && s.port != port).count();
        let mut text = format!("share: {n} share(s) back on their links");
        if moved > 0 {
            text.push_str(&format!(" ({moved} given on another port: the page serves on {port} now)"));
        }
        self.share_note(&text);
    }

    /// Said on the status line (someone getting a file should not go
    /// unnoticed), and kept in show-messages.
    fn share_note(&mut self, text: &str) {
        self.note_message(text);
        let now = std::time::Instant::now();
        for c in self.clients.values_mut().filter(|c| c.session.is_some()) {
            c.message = Some((text.to_string(), now));
        }
    }
}

#[cfg(test)]
mod tests;
