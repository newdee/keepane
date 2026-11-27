//! Group files (docs/design/chat.md §13): every file sent in a channel,
//! kept until it is deleted, listed by `chat-group-files`, deleted by
//! `chat-group-file-rm`. A reader on this machine is given its path; one on
//! another machine a share link, made when first needed and kept in the
//! channel's index (`group-files.json`, beside its day files).

use super::chat::{Attached, Caller, Who, copy_path, size_shown, write_whole};
use super::{Outcome, PaneId, Server};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The index of a channel's group files, in its folder.
const INDEX: &str = "group-files.json";

/// A group file: one sent with a line of the channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroupFile {
    /// Where it is under the channel's `files` folder: `day/line/name`.
    pub file: String,
    pub name: String,
    pub size: u64,
    /// Who sent it (its handle); empty when found by the folder after its
    /// line was gone.
    #[serde(default)]
    pub from: String,
    /// The pane that sent it (none: the person, or not known) and its
    /// machine's `host:port` (empty: this one): who may delete it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<PaneId>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub addr: String,
    /// The line it was sent with.
    pub seq: u64,
    /// When (the line's time, or its day when only the folder says).
    #[serde(default)]
    pub at: String,
    /// The share another machine's readers are given (`share -l`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<u32>,
}

impl GroupFile {
    fn of(file: String, name: String, size: u64, seq: u64, at: String, who: Option<&Who>) -> GroupFile {
        let ident = who.and_then(|w| w.ident.as_ref());
        GroupFile {
            file,
            name,
            size,
            from: who.map(|w| w.handle.clone()).unwrap_or_default(),
            pane: ident.map(|i| i.pane),
            addr: ident.map(|i| i.addr.clone()).unwrap_or_default(),
            seq,
            at,
            share: None,
        }
    }

    /// Whether `me` may delete it: the person here, or the pane that sent
    /// it (here, or on its machine).
    fn may_delete(&self, me: &Caller) -> bool {
        match me {
            Caller::You => true,
            Caller::YouThere { .. } => false,
            Caller::Pane(p) => self.addr.is_empty() && self.pane == Some(*p),
            Caller::There { ident, .. } => self.addr == ident.addr && self.pane == Some(ident.pane),
        }
    }
}

/// Where a reader is, for what it is given of a file: None, on this machine
/// (its path); else the `host:port` of its machine (a link on the address
/// nearest it): another machine's pane's; empty for the person there,
/// whose machine is not said.
pub(super) fn near_for(me: &Caller) -> Option<String> {
    match me {
        Caller::You | Caller::Pane(_) => None,
        Caller::There { ident, .. } => Some(ident.addr.clone()),
        Caller::YouThere { .. } => Some(String::new()),
    }
}

impl Server {
    fn group_index_path(&self, id: &str) -> Option<PathBuf> {
        self.chat.dir.as_ref().map(|d| d.join(id).join(INDEX))
    }

    /// The group files of channel `at`, oldest first: its index; for a
    /// channel from before it (or one whose index does not read, kept aside
    /// as `group-files.json.bad`), what its `files` folder holds, each with
    /// its line's sender while the line is still kept.
    pub(super) fn group_files(&self, at: usize) -> Vec<GroupFile> {
        let c = &self.chat.channels[at];
        let Some(path) = self.group_index_path(&c.id) else { return Vec::new() };
        if let Ok(text) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<Vec<GroupFile>>(&text) {
                Ok(list) => return list,
                Err(e) => {
                    log::warn!("chat: {}: {e}", path.display());
                    if let Err(e) = std::fs::rename(&path, path.with_extension("json.bad")) {
                        log::warn!("chat: {}: {e}", path.display());
                    }
                }
            }
        }
        let Some(root) = self.chat_files_dir(&c.id) else { return Vec::new() };
        let mut out = Vec::new();
        for (day, seq, name, size) in folder_files(&root) {
            let file = format!("{day}/{seq}/{name}");
            // Its line: the one of that number that sent it (another of
            // that number, as after a restart of the numbers, is not).
            let line = c.recent.iter().find(|l| l.seq == seq && l.files.iter().any(|f| f.file == file));
            let at = line.map_or_else(|| day.clone(), |l| l.at.clone());
            out.push(GroupFile::of(file, name, size, seq, at, line.and_then(|l| l.from.as_ref())));
        }
        out
    }

    fn group_files_save(&self, at: usize, list: &[GroupFile]) {
        let Some(path) = self.group_index_path(&self.chat.channels[at].id) else { return };
        let text = serde_json::to_string_pretty(list).expect("it serializes");
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            log::warn!("chat: {}: {e}", dir.display());
        }
        if let Err(e) = write_whole(&path, &text) {
            log::warn!("chat: {}: {e}", path.display());
        }
    }

    /// The files just sent with line `seq` of channel `at` by `who`, added
    /// to its group files.
    pub(super) fn group_files_add(&mut self, at: usize, seq: u64, at_time: &str, who: &Who, sent: &[Attached]) {
        if sent.is_empty() {
            return;
        }
        let mut list = self.group_files(at);
        for f in sent {
            if !list.iter().any(|g| g.file == f.file) {
                list.push(GroupFile::of(f.file.clone(), f.name.clone(), f.size, seq, at_time.to_string(), Some(who)));
            }
        }
        self.group_files_save(at, &list);
    }

    /// The link another machine's reader (at `near`) is given for group
    /// file `file` of channel `at`: its share's, made when it has none on.
    pub(super) fn group_file_link(&mut self, at: usize, file: &str, near: &str) -> Result<String, String> {
        let mut list = self.group_files(at);
        let i = list.iter().position(|g| g.file == file).ok_or("deleted")?;
        if let Some(url) = list[i].share.and_then(|id| self.share_link_near(id, near)) {
            return Ok(url);
        }
        let c = &self.chat.channels[at];
        let path = self.chat_files_dir(&c.id).and_then(|d| copy_path(&d, file)).ok_or("not a file of the channel")?;
        if !path.is_file() {
            return Err("deleted".into());
        }
        let origin = format!("group file #{} #{}", c.name, list[i].seq);
        let id = self.share_group_file(&path, &origin)?;
        list[i].share = Some(id);
        self.group_files_save(at, &list);
        self.share_link_near(id, near).ok_or_else(|| "the phone page is off".into())
    }

    /// For another machine's reader (at `near`): each file of `files` by its
    /// link, or why it has none.
    pub(super) fn group_file_links(
        &mut self,
        at: usize,
        files: impl IntoIterator<Item = String>,
        near: &str,
    ) -> HashMap<String, Result<String, String>> {
        files.into_iter().map(|f| (f.clone(), self.group_file_link(at, &f, near))).collect()
    }

    /// `chat-group-files CHAN [-J]`: channel `at`'s group files, the newest
    /// first, each with its path (to one here) or its link (to another
    /// machine's reader).
    pub(super) fn chat_group_files(&mut self, me: &Caller, at: usize, json: bool) -> Outcome {
        let mut list = self.group_files(at);
        list.reverse();
        let near = near_for(me);
        let here = near.is_none();
        let dir = self.chat_files_dir(&self.chat.channels[at].id);
        let links = match &near {
            Some(n) => self.group_file_links(at, list.iter().map(|g| g.file.clone()), n),
            None => HashMap::new(),
        };
        let place = |g: &GroupFile| -> Result<String, String> {
            if here {
                let p = dir.as_ref().and_then(|d| copy_path(d, &g.file)).ok_or("not a file of the channel")?;
                if p.is_file() { Ok(p.to_string_lossy().into_owned()) } else { Err("deleted".into()) }
            } else {
                links.get(&g.file).cloned().unwrap_or_else(|| Err("deleted".into()))
            }
        };
        let c = &self.chat.channels[at];
        if json {
            let files: Vec<serde_json::Value> = list
                .iter()
                .map(|g| {
                    let mut v = serde_json::json!({
                        "file": g.file, "name": g.name, "size": g.size, "from": g.from, "seq": g.seq, "at": g.at,
                    });
                    match (place(g), here) {
                        (Ok(p), true) => v["path"] = p.into(),
                        (Ok(u), false) => v["url"] = u.into(),
                        (Err(e), _) => v["missing"] = e.into(),
                    }
                    v
                })
                .collect();
            let channel = serde_json::json!({ "id": c.id, "name": c.name });
            return Outcome::Text(serde_json::json!({ "channel": channel, "group_files": files }).to_string());
        }
        if list.is_empty() {
            return Outcome::Text(format!("#{} has no group files", c.name));
        }
        let mut out = Vec::new();
        for g in &list {
            let day = g.at.get(..10).unwrap_or(&g.at);
            let from = if g.from.is_empty() { String::new() } else { format!("  {}", g.from) };
            out.push(format!("#{} {day}  {}  {}{from}", g.seq, g.name, size_shown(g.size)));
            out.push(match place(g) {
                Ok(p) => format!("    {p}"),
                Err(e) => format!("    ({e})"),
            });
        }
        Outcome::Text(out.join("\n"))
    }

    /// `chat-group-file-rm CHAN FILE`: a group file deleted (`FILE`: its
    /// `day/line/name`, or its name when no other has it), by the pane that
    /// sent it or the person; its share stopped. The line keeps naming it.
    pub(super) fn chat_group_file_rm(&mut self, me: &Caller, at: usize, word: &str) -> Outcome {
        let mut list = self.group_files(at);
        let found: Vec<usize> = match list.iter().position(|g| g.file == word) {
            Some(i) => vec![i],
            None => list.iter().enumerate().filter(|(_, g)| g.name == word).map(|(i, _)| i).collect(),
        };
        let name = self.chat.channels[at].name.clone();
        let i = match found.as_slice() {
            [i] => *i,
            [] => return Outcome::Error(format!("chat-group-file-rm: #{name} has no group file {word}")),
            more => {
                let files: Vec<&str> = more.iter().map(|i| list[*i].file.as_str()).collect();
                return Outcome::Error(format!(
                    "chat-group-file-rm: more than one is called {word}; give one of {}",
                    files.join(", ")
                ));
            }
        };
        if !list[i].may_delete(me) {
            let by = if list[i].from.is_empty() { "the pane that sent it" } else { list[i].from.as_str() };
            return Outcome::Error(format!("chat-group-file-rm: only {by} or the person deletes {}", list[i].name));
        }
        let g = list.remove(i);
        if let Some(dir) = self.chat_files_dir(&self.chat.channels[at].id)
            && let Some(path) = copy_path(&dir, &g.file)
        {
            if let Err(e) = std::fs::remove_file(&path)
                && path.exists()
            {
                return Outcome::Error(format!("chat-group-file-rm: {}: {e}", path.display()));
            }
            // Its line's folder and its day's, when nothing else is left in
            // them (a folder with files in it stays as it is).
            for up in path.ancestors().skip(1).take(2) {
                if std::fs::read_dir(up).is_ok_and(|mut d| d.next().is_none())
                    && let Err(e) = std::fs::remove_dir(up)
                {
                    log::warn!("chat: {}: {e}", up.display());
                }
            }
        }
        if let Some(id) = g.share {
            self.share_stop_for(id);
        }
        self.group_files_save(at, &list);
        Outcome::Text(format!("deleted group file {} (#{} of #{name})", g.name, g.seq))
    }

    /// The shares of channel `at`'s group files stopped: it is deleted.
    pub(super) fn group_files_unshare(&mut self, at: usize) {
        let ids: Vec<u32> = self.group_files(at).iter().filter_map(|g| g.share).collect();
        for id in ids {
            self.share_stop_for(id);
        }
    }
}

/// The files of a channel's `files` folder: `(day, line, name, size)` for
/// each `day/line/name` there, in that order.
fn folder_files(root: &Path) -> Vec<(String, u64, String, u64)> {
    let dirs = |p: &Path| -> Vec<(String, PathBuf)> {
        let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        v.sort();
        v
    };
    let mut out = Vec::new();
    for (day, day_path) in dirs(root).into_iter().filter(|(_, p)| p.is_dir()) {
        let mut lines: Vec<(u64, PathBuf)> =
            dirs(&day_path).into_iter().filter_map(|(n, p)| Some((n.parse::<u64>().ok()?, p))).collect();
        lines.sort();
        for (seq, line_path) in lines.into_iter().filter(|(_, p)| p.is_dir()) {
            for (name, path) in dirs(&line_path) {
                if let Ok(meta) = std::fs::metadata(&path)
                    && meta.is_file()
                {
                    out.push((day.clone(), seq, name, meta.len()));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
