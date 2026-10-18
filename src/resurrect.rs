//! Saving and restoring sessions across server restarts and reboots (what
//! tmux-resurrect + tmux-continuum do for tmux), one file per session so a
//! single session can be resumed by name.
//!
//! Saved: window names, the pane layout tree with sizes, each pane's start
//! command and directory, the active window/pane and zoom, and the last
//! `save-history` lines each pane had on screen. Not saved: what the programs
//! themselves were doing, which no multiplexer can bring back.

use crate::server::layout::Node;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct SavedPane {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    /// What the pane had on screen and in its scrollback, so `resume` brings
    /// the output back and not just the command (`save-history` lines).
    /// Files written before this existed simply have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<String>,
    /// Its `rename-pane` name and work mode (docs/design/mailbox.md), so
    /// `%name` still finds it after a resume. Its inbox is not kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_mode: Option<String>,
    /// Its PowerShell history file (`Pane::shell_history`), so a resumed
    /// pane takes up the history it had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_history: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SavedNode {
    Pane { pane: SavedPane },
    Split { horizontal: bool, sizes: Vec<u16>, children: Vec<SavedNode> },
}

impl SavedNode {
    pub fn panes(&self) -> Vec<&SavedPane> {
        match self {
            SavedNode::Pane { pane } => vec![pane],
            SavedNode::Split { children, .. } => children.iter().flat_map(|c| c.panes()).collect(),
        }
    }

    /// Build a layout `Node` by assigning `ids` to the leaves in order.
    pub fn to_layout(&self, ids: &mut impl Iterator<Item = u32>) -> Option<Node> {
        Some(match self {
            SavedNode::Pane { .. } => Node::Leaf(ids.next()?),
            SavedNode::Split { horizontal, sizes, children } => {
                let mut nodes = Vec::with_capacity(children.len());
                for c in children {
                    nodes.push(c.to_layout(ids)?);
                }
                let sizes = if sizes.len() == nodes.len() { sizes.clone() } else { vec![1; nodes.len()] };
                Node::Split { horizontal: *horizontal, children: nodes, sizes }
            }
        })
    }

    /// Mirror of `to_layout`: describe a live layout with a pane lookup.
    pub fn from_layout(node: &Node, pane: &impl Fn(u32) -> Option<SavedPane>) -> Option<SavedNode> {
        Some(match node {
            Node::Leaf(id) => SavedNode::Pane { pane: pane(*id)? },
            Node::Split { horizontal, children, sizes } => SavedNode::Split {
                horizontal: *horizontal,
                sizes: sizes.clone(),
                children: children.iter().map(|c| SavedNode::from_layout(c, pane)).collect::<Option<Vec<_>>>()?,
            },
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SavedWindow {
    pub name: String,
    pub layout: SavedNode,
    /// Index of the active pane in leaf order.
    pub active: usize,
    pub zoomed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SavedSession {
    pub name: String,
    pub windows: Vec<SavedWindow>,
    pub current: usize,
    /// (columns, rows) the session had, used when nothing attaches to size
    /// the restored one. Older files have none: 80x24 then.
    #[serde(default)]
    pub size: Option<(u16, u16)>,
}

/// One file: one session.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SavedFile {
    pub version: u32,
    pub saved_at: String,
    pub session: SavedSession,
}

impl SavedFile {
    pub fn new(session: SavedSession) -> SavedFile {
        SavedFile { version: FORMAT_VERSION, saved_at: chrono::Local::now().to_rfc3339(), session }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<SavedFile, String> {
        let f: SavedFile = serde_json::from_str(s).map_err(|e| format!("saved session: {e}"))?;
        if f.version != FORMAT_VERSION {
            return Err(format!("saved session: version {} (this keepane writes {FORMAT_VERSION})", f.version));
        }
        Ok(f)
    }

    /// Write atomically (temp file + rename) so a crash mid-write never
    /// leaves a truncated file behind; a write or rename that fails takes
    /// the temp file away with it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let written = std::fs::write(&tmp, self.to_json())
            .map_err(|e| format!("{}: {e}", tmp.display()))
            .and_then(|()| std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display())));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }

    pub fn load(path: &Path) -> Result<SavedFile, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        SavedFile::from_json(&text)
    }
}

/// Default directory of the auto-saved sessions (`KEEPANE_SESSIONS_DIR` wins,
/// which is how the tests keep their servers out of the real one).
pub fn default_dir() -> PathBuf {
    if let Some(d) = crate::legacy::var("KEEPANE_SESSIONS_DIR")
        && !d.is_empty()
    {
        return PathBuf::from(d);
    }
    crate::logger::log_dir().join("sessions")
}

/// File for a session name (names are free-form; the file name is not).
pub fn file_for(dir: &Path, session: &str) -> PathBuf {
    let safe: String = session
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    // Two different names could sanitize alike; a short hash keeps them apart.
    let mut h: u32 = 2166136261;
    for b in session.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    dir.join(format!("{safe}-{h:08x}.json"))
}

/// Every saved session in `dir`, newest first: (name, saved_at, path).
pub fn list(dir: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json")
            && let Ok(f) = SavedFile::load(&p)
        {
            out.push((f.session.name, f.saved_at, p));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The saved file for a session name, if any.
pub fn find(dir: &Path, session: &str) -> Option<PathBuf> {
    let p = file_for(dir, session);
    if p.is_file() {
        return Some(p);
    }
    // Files written by hand or renamed: fall back to scanning.
    list(dir).into_iter().find(|(n, _, _)| n == session).map(|(_, _, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SavedFile {
        let pane = |c: &str| SavedPane { argv: vec![c.into()], cwd: Some("C:\\src".into()), ..Default::default() };
        SavedFile::new(SavedSession {
            name: "main".into(),
            current: 1,
            size: Some((120, 40)),
            windows: vec![
                SavedWindow {
                    name: "shell".into(),
                    layout: SavedNode::Split {
                        horizontal: true,
                        sizes: vec![40, 39],
                        children: vec![
                            SavedNode::Pane { pane: pane("pwsh.exe") },
                            SavedNode::Split {
                                horizontal: false,
                                sizes: vec![10, 12],
                                children: vec![
                                    SavedNode::Pane { pane: pane("wsl.exe") },
                                    SavedNode::Pane { pane: pane("cmd.exe") },
                                ],
                            },
                        ],
                    },
                    active: 2,
                    zoomed: false,
                },
                SavedWindow {
                    name: "logs".into(),
                    layout: SavedNode::Pane { pane: pane("pwsh.exe") },
                    active: 0,
                    zoomed: true,
                },
            ],
        })
    }

    #[test]
    fn json_roundtrip_and_layout_mapping() {
        let f = sample();
        assert_eq!(SavedFile::from_json(&f.to_json()).unwrap(), f);
        let w = &f.session.windows[0];
        assert_eq!(
            w.layout.panes().iter().map(|p| p.argv[0].as_str()).collect::<Vec<_>>(),
            ["pwsh.exe", "wsl.exe", "cmd.exe"]
        );
        let node = w.layout.to_layout(&mut (10..)).unwrap();
        assert_eq!(node.panes(), vec![10, 11, 12]);
        let lookup = |id: u32| w.layout.panes().get((id - 10) as usize).map(|p| (*p).clone());
        assert_eq!(SavedNode::from_layout(&node, &lookup).unwrap(), w.layout);
        assert!(w.layout.to_layout(&mut (0..2)).is_none(), "too few ids must not yield a partial tree");
    }

    #[test]
    fn per_session_files() {
        let dir = std::env::temp_dir().join(format!("keepane-resurrect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let f = sample();
        let path = file_for(&dir, "main");
        f.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists(), "temp file must be renamed away");
        assert_eq!(SavedFile::load(&path).unwrap(), f);
        // Odd names get a safe file name that still maps back.
        let odd = "会话 name/with:stuff";
        let mut g = sample();
        g.session.name = odd.into();
        g.save(&file_for(&dir, odd)).unwrap();
        assert_eq!(find(&dir, odd).unwrap(), file_for(&dir, odd));
        assert_ne!(file_for(&dir, "a b"), file_for(&dir, "a_b"), "distinct names never collide");
        let names: Vec<String> = list(&dir).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"main".to_string()) && names.contains(&odd.to_string()));
        // Bad files are skipped by list() and reported by load().
        std::fs::write(dir.join("junk.json"), "{ not json").unwrap();
        assert_eq!(list(&dir).len(), 2);
        assert!(SavedFile::load(&dir.join("junk.json")).unwrap_err().contains("saved session"));
        std::fs::write(
            dir.join("old.json"),
            r#"{"version":99,"saved_at":"","session":{"name":"x","windows":[],"current":0}}"#,
        )
        .unwrap();
        assert!(SavedFile::load(&dir.join("old.json")).unwrap_err().contains("version 99"));
        assert!(find(&dir, "nope").is_none());
        // A save that cannot land (a folder in its place) says so and leaves
        // no temp file behind.
        let blocked = dir.join("blocked.json");
        std::fs::create_dir_all(&blocked).unwrap();
        assert!(f.save(&blocked).is_err());
        assert!(!blocked.with_extension("json.tmp").exists(), "temp file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_mismatch_is_repaired() {
        let n = SavedNode::Split {
            horizontal: true,
            sizes: vec![1],
            children: vec![SavedNode::Pane { pane: SavedPane::default() }; 3],
        };
        match n.to_layout(&mut (1..)).unwrap() {
            Node::Split { sizes, .. } => assert_eq!(sizes, vec![1, 1, 1]),
            _ => panic!(),
        }
    }
}
