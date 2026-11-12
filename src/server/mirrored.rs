//! Sessions of another machine, kept here as sessions of this server
//! (docs/design/link.md, decision 17). A copy (`link_mirror`) hands every
//! shape and every pane change here: the machine's sessions become sessions
//! here, named `name@host:port`, their windows and panes placed as they are
//! there and each pane's model fed the bytes the pane there printed. They
//! are attached to, looked at, switched between and typed into like any
//! other. What changes their shape (a split, a kill, a resize) is that
//! machine's to do, and looking at one from here never resizes it there:
//! a client smaller than the session sees its top left part.
//!
//! None of them is this machine's own: they are not saved, not told to the
//! machines that copy this one, and a bare name never names them (`name`
//! is this machine's session `name`; `host:port/name` the other's).

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::layout::{self, Node, Rect};
use super::pane::{Callbacks, Pane, RemotePane};
use super::{Chrome, PaneId, Server, Session, SessionId, Window, WindowId};
use crate::command::{Cmd, Target};
use crate::link_proto::{self as proto, ToServer, to_server};

/// Which machine's session a session is (`Session::remote`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RemoteSession {
    /// The machine's public key, as paired.
    pub key: String,
    /// `host:port`, as the session's name shows it.
    pub addr: String,
    /// Its id there.
    pub id: u32,
}

/// A window of another machine (`Window::remote`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RemoteWindow {
    /// Its id there.
    pub id: u32,
    /// Zoomed there: one pane drawn, the whole window's size. Which one is
    /// chosen here, as any pane is; the one zoomed there when it zooms.
    pub zoomed: bool,
    /// The pane zoomed there (its id here), to tell a new zoom.
    pub zoomed_pane: Option<PaneId>,
    /// Where its panes are there, as told: placed here again whenever the
    /// window area or what the panes give up around them changes here
    /// (`Window::relayout`).
    pub placed: Vec<Placed>,
}

/// A pane of another machine's window where it is there (its id here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Placed {
    pub id: PaneId,
    pub x: u32,
    pub y: u32,
    pub cols: u16,
    pub rows: u16,
}

/// A zoomed window of another machine draws the pane chosen here, at its
/// size, where the zoomed pane is drawn.
pub(super) fn zoom_on_active(w: &mut Window) {
    if !w.zoomed {
        return;
    }
    if let (Some(&(_, r)), Some(p)) = (w.rects.first(), w.pane(w.active)) {
        w.rects = vec![(w.active, Rect { x: r.x, y: r.y, w: p.cols, h: p.rows })];
        w.layout_rects = w.rects.clone();
    }
}

/// Where the keys typed into a pane of another machine go: that machine's
/// copy's connection (`link_mirror`), which sends them on while it is up and
/// lets them go while it is down (keys typed into a dead link are never
/// typed minutes later).
pub(super) struct RemoteInput {
    pub pane: u32,
    pub tx: UnboundedSender<ToServer>,
}

impl Write for RemoteInput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let input = proto::Input { pane: self.pane, bytes: buf.to_vec() };
        let _ = self.tx.send(ToServer { kind: Some(to_server::Kind::Input(input)) });
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A new model for a pane of another machine: a snapshot's or a screen's
/// (`set_remote_model`), and the bytes of output it has been fed.
pub(super) type Model = (vt100::Parser<Callbacks>, u64);

/// A size from the wire: 1 to 1000.
pub(super) fn dim(v: u32) -> Result<u16, String> {
    u16::try_from(v).ok().filter(|v| (1..=1000).contains(v)).ok_or_else(|| format!("a size of {v}"))
}

/// A pane's model as a snapshot or a screen change sends it, read back.
pub(super) fn model_of(snapshot: &[u8]) -> Option<vt100::Parser<Callbacks>> {
    let mut parser = vt100::Parser::new_with_callbacks(1, 1, 0, Callbacks::default());
    parser.screen_mut().restore(snapshot).then_some(parser)
}

/// Where a window of another machine's panes are drawn here: where they
/// are there, from the top left of this session's window area. Their
/// rectangles there include what the panes give up around them
/// (`pane-border-status`, `pane-frames`); taken as given up as they are
/// here, so the same options on both machines draw the same picture.
/// `zoomed`: the pane zoomed there, the only one drawn.
pub(super) fn place(placed: &[Placed], zoomed: Option<PaneId>, area: Rect, chrome: Chrome) -> Vec<(PaneId, Rect)> {
    let panes: Vec<&Placed> = placed.iter().filter(|p| zoomed.is_none_or(|z| z == p.id)).collect();
    let (pad_x, pad_y) = match chrome {
        Chrome::Frames => (1, 1),
        Chrome::Status(true) => (0, 1),
        Chrome::Status(false) | Chrome::Lines => (0, 0),
    };
    let ox = panes.iter().map(|p| p.x).min().unwrap_or(0).saturating_sub(pad_x);
    let oy = panes.iter().map(|p| p.y).min().unwrap_or(0).saturating_sub(pad_y);
    let at = |v: u32, o: u32, base: u16| base.saturating_add(u16::try_from(v - o).unwrap_or(u16::MAX));
    panes
        .into_iter()
        .map(|p| (p.id, Rect { x: at(p.x, ox, area.x), y: at(p.y, oy, area.y), w: p.cols, h: p.rows }))
        .collect()
}

impl Server {
    /// A pane of machine `key`'s copy, by its id there.
    pub(super) fn mirror_pane_mut(&mut self, key: &str, id: u32) -> Option<&mut Pane> {
        self.sessions
            .iter_mut()
            .filter(|s| s.remote.as_ref().is_some_and(|r| r.key == key))
            .flat_map(|s| s.windows.iter_mut())
            .flat_map(|w| w.panes.iter_mut())
            .find(|p| p.remote.is_some_and(|r| r.id == id))
    }

    /// The panes of machine `key`'s copy, by their ids there.
    pub(super) fn mirror_panes(&self, key: &str) -> Vec<&Pane> {
        let mut panes: Vec<&Pane> = self
            .sessions
            .iter()
            .filter(|s| s.remote.as_ref().is_some_and(|r| r.key == key))
            .flat_map(|s| s.windows.iter())
            .flat_map(|w| w.panes.iter())
            .collect();
        panes.sort_by_key(|p| p.remote.map(|r| r.id));
        panes
    }

    /// Machine `key`'s sessions here made what `shape` says they are there:
    /// new ones made, gone ones dropped, the rest changed in place, each
    /// window and pane kept (with what it shows) when it is still there,
    /// wherever it moved. What a client chose to look at (a window, a pane)
    /// stays chosen while it is there. `models`: new models of panes (a
    /// snapshot's), by their ids there.
    pub(super) fn mirror_shape(
        &mut self,
        key: &str,
        addr: &str,
        input: &UnboundedSender<ToServer>,
        shape: &proto::Shape,
        mut models: HashMap<u32, Model>,
    ) {
        let mine = |s: &Session| s.remote.as_ref().is_some_and(|r| r.key == key);
        // What is there now, taken out, to go back where the shape has it.
        let mut panes: HashMap<u32, Pane> = HashMap::new();
        let mut windows: HashMap<u32, Window> = HashMap::new();
        let mut chosen: HashMap<SessionId, (Option<WindowId>, Option<WindowId>)> = HashMap::new();
        for s in self.sessions.iter_mut().filter(|s| mine(s)) {
            chosen.insert(s.id, (s.window().map(|w| w.id), s.last));
            for mut w in s.windows.drain(..) {
                for p in w.panes.drain(..) {
                    if let Some(r) = p.remote {
                        panes.insert(r.id, p);
                    }
                }
                if let Some(r) = &w.remote {
                    windows.insert(r.id, w);
                }
            }
        }
        let history = self.opts.history_limit;
        let chrome = self.chrome();
        let mut kept: HashSet<SessionId> = HashSet::new();
        for rs in &shape.sessions {
            let (cols, rows) = (dim(rs.cols).unwrap_or(80), dim(rs.rows).unwrap_or(24));
            let area = self.window_area(cols, rows);
            let mut built: Vec<Window> = Vec::new();
            let mut current = None;
            for rw in rs.windows.iter().filter(|w| !w.panes.is_empty()) {
                let mut w = match windows.remove(&rw.id) {
                    Some(w) => w,
                    None => {
                        let id = self.alloc_id();
                        new_window(id, rw.id)
                    }
                };
                let mut ids = Vec::new();
                for rp in &rw.panes {
                    let mut p = match panes.remove(&rp.id) {
                        Some(p) => p,
                        None => {
                            let (c, r) = (dim(rp.cols).unwrap_or(80), dim(rp.rows).unwrap_or(24));
                            let parser = vt100::Parser::new_with_callbacks(r, c, history, Callbacks::default());
                            let writer = Box::new(RemoteInput { pane: rp.id, tx: input.clone() });
                            let id = self.alloc_id();
                            Pane::remote(id, RemotePane { id: rp.id, at: 0 }, parser, writer, self.pane_tx.clone())
                        }
                    };
                    if let Some((parser, at)) = models.remove(&rp.id) {
                        p.set_remote_model(parser, at);
                    }
                    p.title = rp.title.clone();
                    p.command = rp.command.clone();
                    p.cwd = rp.cwd.clone();
                    p.exit_code = rp.exited;
                    ids.push(p.id);
                    w.panes.push(p);
                }
                let theirs = rw.panes.iter().position(|p| p.id == rw.active).map_or(ids[0], |i| ids[i]);
                // Zoomed there anew (or on another pane): the zoomed pane is
                // the one looked at here too.
                let zoomed_pane = rw.zoomed.then_some(theirs);
                let zoomed_anew = zoomed_pane.is_some() && w.remote.as_ref().and_then(|r| r.zoomed_pane) != zoomed_pane;
                if !ids.contains(&w.active) || zoomed_anew {
                    w.active = theirs;
                }
                w.last_pane = w.last_pane.filter(|id| ids.contains(id));
                w.name = rw.name.clone();
                let placed = rw
                    .panes
                    .iter()
                    .zip(&ids)
                    .map(|(p, &id)| Placed {
                        id,
                        x: p.x,
                        y: p.y,
                        cols: dim(p.cols).unwrap_or(1),
                        rows: dim(p.rows).unwrap_or(1),
                    })
                    .collect();
                w.remote = Some(Box::new(RemoteWindow { id: rw.id, zoomed: rw.zoomed, zoomed_pane, placed }));
                let mut node = layout::parse_layout(&rw.layout)
                    .ok()
                    .filter(|n| n.panes().len() == ids.len())
                    .or_else(|| layout::Preset::EvenHorizontal.build(&ids))
                    .unwrap_or(Node::Leaf(ids[0]));
                node.set_panes(&ids);
                w.layout = node;
                w.relayout(area, chrome);
                if rw.index == rs.current {
                    current = Some(built.len());
                }
                built.push(w);
            }
            if built.is_empty() {
                continue;
            }
            let at = self.sessions.iter().position(|s| mine(s) && s.remote.as_ref().is_some_and(|r| r.id == rs.id));
            let sid = match at {
                Some(i) => self.sessions[i].id,
                None => {
                    let sid = self.alloc_id();
                    self.sessions.push(Session {
                        id: sid,
                        name: String::new(),
                        windows: Vec::new(),
                        cur: 0,
                        last: None,
                        cols,
                        rows,
                        created: Instant::now(),
                        created_at: chrono::Local::now().timestamp(),
                        last_used: Instant::now(),
                        remote: Some(RemoteSession { key: key.to_string(), addr: addr.to_string(), id: rs.id }),
                    });
                    sid
                }
            };
            kept.insert(sid);
            let (was, last) = chosen.get(&sid).copied().unwrap_or((None, None));
            let s = self.session_mut(sid).expect("just found or made");
            s.name = format!("{}@{addr}", rs.name);
            if let Some(r) = s.remote.as_mut() {
                r.addr = addr.to_string();
            }
            (s.cols, s.rows) = (cols, rows);
            s.cur = was.and_then(|id| built.iter().position(|w| w.id == id)).or(current).unwrap_or(0);
            s.last = last.filter(|id| built.iter().any(|w| w.id == *id));
            s.windows = built;
        }
        let gone: Vec<SessionId> =
            self.sessions.iter().filter(|s| mine(s) && !kept.contains(&s.id)).map(|s| s.id).collect();
        for sid in gone {
            self.drop_mirror_session(sid, &format!("the session is gone from {addr}"));
        }
    }

    /// A session that is this machine's own; another machine's copied here
    /// is that machine's to change (`what`: what was to be done).
    pub(super) fn own_session(&self, sid: SessionId, what: &str) -> Result<(), String> {
        match self.session(sid).map(|s| (&s.name, &s.remote)) {
            Some((name, Some(r))) => Err(format!("{name} is a session of {}: only that machine can {what}", r.addr)),
            _ => Ok(()),
        }
    }

    /// Every session of machine `key` dropped: its copy stopped.
    pub(super) fn mirror_forget(&mut self, key: &str, why: &str) {
        let ids: Vec<SessionId> =
            self.sessions.iter().filter(|s| s.remote.as_ref().is_some_and(|r| r.key == key)).map(|s| s.id).collect();
        for sid in ids {
            self.drop_mirror_session(sid, why);
        }
    }

    /// A session of another machine taken away; its clients are detached
    /// (nothing ran here to end).
    fn drop_mirror_session(&mut self, sid: SessionId, why: &str) {
        let Some(pos) = self.sessions.iter().position(|s| s.id == sid) else { return };
        let s = self.sessions.remove(pos);
        let clients: Vec<_> = self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
        for cid in clients {
            self.detach(cid, why);
        }
        log::info!("session {}: {why}", s.name);
    }

    /// The session named `host:port/name` (or `host:port/$id`, its id
    /// there) among the sessions of other machines kept here.
    pub(super) fn mirror_session(&self, machine: &str, name: &str) -> Option<SessionId> {
        let id = name.strip_prefix('$').and_then(|n| n.parse::<u32>().ok());
        self.sessions
            .iter()
            .filter(|s| s.remote.as_ref().is_some_and(|r| r.addr == machine))
            .find(|s| {
                let r = s.remote.as_ref().expect("filtered");
                id == Some(r.id) || s.name.strip_suffix(&format!("@{machine}")) == Some(name)
            })
            .map(|s| s.id)
    }

    /// Why `cmd` may not run where it would: a session of another machine,
    /// whose shape is that machine's to change (its windows, its panes, its
    /// size, its names). Looking, choosing what to look at, copying out and
    /// typing in are free. A command not named here changes no shape (a
    /// new one that does is named here when it is made).
    pub(super) fn mirror_refusal(&self, cmd: &Cmd, cid: Option<super::ClientId>) -> Option<String> {
        let (targets, what): (Vec<Option<&Target>>, &str) = match cmd {
            Cmd::KillSession { target, all_but: false } => (vec![target.as_ref()], "kill it"),
            Cmd::RenameSession { target, .. } => (vec![target.as_ref()], "rename it"),
            Cmd::NewWindow { target, .. } => (vec![target.as_ref()], "add a window to it"),
            Cmd::KillWindow { target, .. } => (vec![target.as_ref()], "kill a window of it"),
            Cmd::RenameWindow { target, .. } => (vec![target.as_ref()], "rename a window of it"),
            Cmd::SwapWindow { src, dst, .. } => (vec![src.as_ref(), dst.as_ref()], "move its windows"),
            Cmd::SplitWindow { target, .. } => (vec![target.as_ref()], "split a pane of it"),
            Cmd::KillPane { target, .. } => (vec![target.as_ref()], "kill a pane of it"),
            Cmd::PaneTitle { target, title: Some(_), .. } => (vec![target.as_ref()], "name a pane of it"),
            Cmd::ResizeWindow { target, .. } => (vec![target.as_ref()], "resize it"),
            Cmd::ResizePane { target, .. } => (vec![target.as_ref()], "resize or zoom a pane of it"),
            Cmd::SwapPane { target, source, .. } => (vec![target.as_ref(), source.as_ref()], "move its panes"),
            Cmd::SelectLayout { target, .. } => (vec![target.as_ref()], "lay out a window of it"),
            Cmd::BreakPane { target, .. } => (vec![target.as_ref()], "move its panes"),
            Cmd::JoinPane { src, dst, .. } => (vec![src.as_ref(), dst.as_ref()], "move its panes"),
            Cmd::RotateWindow { target, .. } => (vec![target.as_ref()], "move its panes"),
            Cmd::RespawnPane { target, .. } => (vec![target.as_ref()], "start a pane of it again"),
            Cmd::PipePane { target, .. } => (vec![target.as_ref()], "pipe a pane of it"),
            Cmd::Record { target, .. } => (vec![target.as_ref()], "record a pane of it"),
            Cmd::SaveSession { target, all: false } => (vec![target.as_ref()], "save it"),
            Cmd::SetWorkMode { target, .. } => (vec![target.as_ref()], "set a pane of it to work"),
            Cmd::RenamePane { target, .. } => (vec![target.as_ref()], "name a pane of it"),
            Cmd::SetCwd { target, .. } => (vec![target.as_ref()], "set the directory of a pane of it"),
            Cmd::WebFit { target, .. } => (vec![target.as_ref()], "size it to a phone"),
            _ => return None,
        };
        for t in targets {
            // One that names nothing is the command's to say.
            let Ok(sid) = self.resolve_session(t, cid) else { continue };
            if let Some(s) = self.session(sid)
                && let Some(r) = &s.remote
            {
                return Some(format!(
                    "{} is a session of {}: only that machine can {what} (attached there: `keepane link attach {} {}`)",
                    s.name,
                    r.addr,
                    r.addr,
                    s.name.strip_suffix(&format!("@{}", r.addr)).unwrap_or(&s.name)
                ));
            }
        }
        None
    }
}

/// A window of another machine, empty: its panes go in next.
fn new_window(id: WindowId, remote: u32) -> Window {
    Window {
        id,
        name: String::new(),
        layout: Node::Leaf(0),
        panes: Vec::new(),
        active: 0,
        last_pane: None,
        zoomed: false,
        layout_preset: None,
        synchronized: false,
        rects: Vec::new(),
        layout_rects: Vec::new(),
        alert_activity: false,
        alert_bell: false,
        alert_silence: false,
        alert_mail: false,
        last_output: Instant::now(),
        anim: None,
        remote: Some(Box::new(RemoteWindow { id: remote, zoomed: false, zoomed_pane: None, placed: Vec::new() })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panes 10 and 11 (10x4) at those places there.
    fn window(panes: &[(u32, u32)]) -> Vec<Placed> {
        panes.iter().zip([10, 11]).map(|(&(x, y), id)| Placed { id, x, y, cols: 10, rows: 4 }).collect()
    }

    /// Where the panes go: from the top left of the window area here, the
    /// row or column a border or frame takes there taken as taken here.
    #[test]
    fn panes_go_where_they_are_there_from_the_window_area_here() {
        let area = Rect { x: 0, y: 1, w: 80, h: 23 };
        let rects = |w: &[Placed], chrome| place(w, None, area, chrome);
        let side = window(&[(0, 0), (11, 0)]);
        assert_eq!(
            rects(&side, Chrome::Lines),
            [(10, Rect { x: 0, y: 1, w: 10, h: 4 }), (11, Rect { x: 11, y: 1, w: 10, h: 4 })]
        );
        // A status line at the top there: its area starts a row down.
        let down = window(&[(0, 1), (11, 1)]);
        assert_eq!(rects(&down, Chrome::Lines)[0].1.y, 1);
        // Border rows above the panes, here as there: the row stays.
        let bordered = window(&[(0, 2), (11, 2)]);
        assert_eq!(rects(&bordered, Chrome::Status(true))[0].1.y, 2);
        // Frames: a column and a row each side.
        let framed = window(&[(1, 1), (12, 1)]);
        assert_eq!(rects(&framed, Chrome::Frames)[0].1, Rect { x: 1, y: 2, w: 10, h: 4 });
        // Zoomed there: only the zoomed pane, the others' places ignored.
        let zoomed = window(&[(0, 0), (0, 0)]);
        assert_eq!(place(&zoomed, Some(11), area, Chrome::Lines), [(11, Rect { x: 0, y: 1, w: 10, h: 4 })]);
    }
}
