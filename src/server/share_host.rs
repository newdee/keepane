//! What is shared (`share`, docs/design/share.md): each file or folder given
//! out, by its token, until its time is up, it is stopped, or (`--once`) it
//! went once. The phones' page asks what a link shares (`web-share-open`) and
//! says when all of it went (`web-share-sent`); the download itself is
//! `crate::share::serve`'s. Nothing of it is kept past the server.

use super::{ClientId, Outcome, PaneId, Server};
use crate::command::ShareWhat;
use chrono::{DateTime, Local};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(super) struct Share {
    id: u32,
    token: String,
    path: PathBuf,
    dir: bool,
    /// The file's name, or the folder's (its zip is this `.zip`).
    name: String,
    /// When it ends; None, when stopped or the server ends.
    until: Option<(Instant, DateTime<Local>)>,
    once: bool,
    /// The pane it was shared from.
    by: Option<PaneId>,
    /// Times all of it went.
    sent: u32,
    /// Addresses that opened it (each said once).
    seen: Vec<String>,
}

#[derive(Default)]
pub(super) struct Shares {
    list: Vec<Share>,
    last: u32,
}

impl Share {
    fn what(&self) -> &'static str {
        if self.dir { "folder" } else { "file" }
    }

    fn until_text(&self) -> String {
        match &self.until {
            Some((_, at)) => format!("until {}", at.format("%m-%d %H:%M")),
            None => "until stopped".to_string(),
        }
    }
}

impl Server {
    pub(super) fn exec_share(&mut self, what: ShareWhat, cid: Option<ClientId>) -> Outcome {
        match what {
            ShareWhat::List => Outcome::Text(self.shares_listed()),
            ShareWhat::Add { path, for_secs, once } => match self.share_add(&path, for_secs, once, cid) {
                Ok(text) => Outcome::Text(text),
                Err(e) => Outcome::Error(format!("share: {e}")),
            },
            ShareWhat::Stop(id) => {
                let gone: Vec<Share> = match id {
                    Some(id) => {
                        let Some(i) = self.shares.list.iter().position(|s| s.id == id) else {
                            return Outcome::Error(format!("share stop: no share {id} (share -l)"));
                        };
                        vec![self.shares.list.remove(i)]
                    }
                    None => std::mem::take(&mut self.shares.list),
                };
                let ids: Vec<String> = gone.iter().map(|s| s.id.to_string()).collect();
                if ids.is_empty() {
                    return Outcome::Text("nothing was shared".into());
                }
                let text = if ids.len() == 1 {
                    format!("share {} stopped: its link no longer works", ids[0])
                } else {
                    format!("shares {} stopped: their links no longer work", ids.join(", "))
                };
                self.share_note(&text);
                Outcome::Text(text)
            }
        }
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
        let for_ = match for_secs {
            None => Some(crate::share::DEFAULT_FOR),
            Some(0) => None,
            Some(s) => Some(Duration::from_secs(s)),
        };
        let until =
            for_.map(|d| (Instant::now() + d, Local::now() + chrono::Duration::from_std(d).unwrap_or_default()));
        self.shares.last += 1;
        let by = self.caller_pane(cid);
        let s = Share {
            id: self.shares.last,
            token,
            path: real.clone(),
            dir: meta.is_dir(),
            name,
            until,
            once,
            by,
            sent: 0,
            seen: Vec::new(),
        };
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
            lines.push(format!(
                "{}  {}  {}{once}, sent {}{from}  {}",
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

    /// `web-share-open`: what the link shares, as JSON, for the page to send.
    pub(super) fn share_open(&mut self, token: &str, peer: &str) -> Outcome {
        self.shares_tick();
        let Some(s) = self.shares.list.iter_mut().find(|s| crate::web::same_key(&s.token, token)) else {
            return Outcome::Error(crate::share::ENDED.into());
        };
        let json = serde_json::json!({ "path": s.path.to_string_lossy(), "dir": s.dir, "name": s.name }).to_string();
        if !s.seen.iter().any(|p| p == peer) {
            s.seen.push(peer.to_string());
            let text = format!("share {}: {peer} is downloading {}", s.id, s.name);
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
        let mut text = format!("share {}: {} sent to {peer}", s.id, s.name);
        if !note.is_empty() {
            text.push_str(&format!(" ({note})"));
        }
        if s.once {
            text.push_str("; it was for once: ended");
            self.shares.list.remove(i);
        }
        self.share_note(&text);
        Outcome::Ok
    }

    /// Every tick: a share whose time is up ends.
    pub(super) fn shares_tick(&mut self) {
        let now = Instant::now();
        let (gone, kept): (Vec<Share>, Vec<Share>) =
            std::mem::take(&mut self.shares.list).into_iter().partition(|s| s.until.is_some_and(|(t, _)| t <= now));
        self.shares.list = kept;
        for s in gone {
            self.share_note(&format!("share {} ended: {} (its time was up)", s.id, s.name));
        }
    }

    /// Said on the status line (someone getting a file should not go
    /// unnoticed), and kept in show-messages.
    fn share_note(&mut self, text: &str) {
        self.note_message(text);
        let now = Instant::now();
        for c in self.clients.values_mut().filter(|c| c.session.is_some()) {
            c.message = Some((text.to_string(), now));
        }
    }
}

#[cfg(test)]
mod tests;
