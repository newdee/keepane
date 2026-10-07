//! The panel the prefix shows when the next key is late (`prefix-hint`):
//! what the keys of the prefix table do, by group. Read from the table as
//! it is (a key bound elsewhere moves with its command; a key bound to a
//! command of one's own has a group of its own), so it never says what a
//! key no longer does. `?` (`list-keys`) still lists them all.

use super::Binding;
use crate::keys::Key;
use std::collections::HashMap;

/// The groups, in the order the panel shows them.
pub const GROUPS: [&str; 5] = ["Panes", "Windows", "Sessions", "Tools", "Yours"];

/// One line of the panel: the keys (as the panel writes them) and what they do.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub keys: String,
    pub label: String,
}

/// A command the panel names, as `list-keys` writes it: its group and what
/// it does, in a few words. The others are left to `?`.
fn known(c: &str) -> Option<(usize, &'static str)> {
    let menu = c.starts_with("display-menu");
    Some(match c {
        _ if c.starts_with("split-window -h") => (0, "split side by side"),
        "split-window" | "split-window -v" => (0, "split above / below"),
        "select-pane -L" | "select-pane -R" | "select-pane -U" | "select-pane -D" => (0, "go to a pane"),
        "resize-pane -Z" => (0, "zoom"),
        _ if c.starts_with("resize-pane -") && !c.contains("-Z") => (0, "resize"),
        _ if menu && c.contains("split-window") => (0, "pane menu"),
        _ if c.ends_with("kill-pane") && !menu => (0, "close the pane"),
        "display-panes" => (0, "pane numbers"),
        "new-window" => (1, "new window"),
        "next-window" | "previous-window" => (1, "next / previous window"),
        _ if c.starts_with("select-window -t :") && c[18..].chars().all(|d| d.is_ascii_digit()) => (1, "go to window"),
        "last-window" => (1, "last window"),
        "choose-tree -w" => (1, "pick a window"),
        _ if menu && c.contains("new-window") => (1, "window menu"),
        _ if c.starts_with("command-prompt") && c.contains("rename-window") => (1, "rename window"),
        _ if c.ends_with("kill-window") && !menu => (1, "close the window"),
        "detach-client" => (2, "detach"),
        "choose-tree -s" => (2, "pick a session"),
        _ if c.starts_with("command-prompt") && c.contains("rename-session") => (2, "rename session"),
        _ if c.starts_with("save-session") => (2, "save sessions"),
        _ if c.starts_with("restore-session") => (2, "restore sessions"),
        "switch-client -p" | "switch-client -n" => (2, "previous / next session"),
        "dashboard" => (3, "dashboard"),
        "copy-mode" => (3, "scroll and copy"),
        "paste-buffer" => (3, "paste"),
        "copy-output" => (3, "copy last output"),
        "hints" => (3, "pick text on screen"),
        "choose-history" => (3, "history"),
        "choose-jobs" => (3, "jobs"),
        "command-prompt" => (3, "command"),
        // (`?`, every key, is the panel's own footer.)
        _ => return None,
    })
}

/// A key as the panel writes it: the arrows as arrows.
fn shown(k: &Key) -> String {
    match k.to_string().as_str() {
        "Up" => "↑".into(),
        "Down" => "↓".into(),
        "Left" => "←".into(),
        "Right" => "→".into(),
        s => s.to_string(),
    }
}

/// Where a key goes among an entry's keys: letters, capitals, digits,
/// marks, arrows (left, down, up, right, as h, j, k, l), then the rest.
fn order(s: &str) -> (u8, String) {
    let mut cs = s.chars();
    let class = match (cs.next(), cs.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => 0,
        (Some(c), None) if c.is_ascii_uppercase() => 1,
        (Some(c), None) if c.is_ascii_digit() => 2,
        (Some(c), None) if "←↓↑→".contains(c) => return (4, "←↓↑→".find(c).unwrap_or(0).to_string()),
        (Some(_), None) => 3,
        _ => 5,
    };
    (class, s.to_string())
}

/// An entry's keys, short: the one-character ones run together (`hjkl←↓↑→`,
/// `0-9` for all ten digits), else the first two.
fn keys_text(mut keys: Vec<String>) -> String {
    keys.sort_by_key(|k| order(k));
    keys.dedup();
    let digits: Vec<&String> = keys.iter().filter(|k| k.len() == 1 && k.as_bytes()[0].is_ascii_digit()).collect();
    let all_digits = digits.len() == 10;
    let single: Vec<&String> = keys.iter().filter(|k| k.chars().count() == 1).collect();
    if !single.is_empty() {
        let mut out = String::new();
        for k in single {
            if all_digits && k.as_bytes()[0].is_ascii_digit() {
                if k == "0" {
                    out.push_str("0-9");
                }
                continue;
            }
            out.push_str(k);
        }
        return out;
    }
    keys.into_iter().take(2).collect::<Vec<_>>().join(" ")
}

/// The panel's groups for the prefix table `binds`, `defaults` being the
/// table keepane starts with: each known command once, its keys together;
/// a key bound to a command of one's own under Yours; the defaults the panel
/// does not name left out. Empty groups are dropped.
pub fn groups(binds: &HashMap<Key, Binding>, defaults: &HashMap<Key, Binding>) -> Vec<(&'static str, Vec<Entry>)> {
    // (group, label) -> keys, in the order the labels first appear in `known`.
    let mut by: Vec<((usize, String), Vec<String>)> = Vec::new();
    let mut add = |at: (usize, String), key: String| match by.iter_mut().find(|(a, _)| *a == at) {
        Some((_, keys)) => keys.push(key),
        None => by.push((at, vec![key])),
    };
    let mut keys: Vec<&Key> = binds.keys().collect();
    keys.sort_by_key(|k| order(&shown(k)));
    for key in keys {
        let b = &binds[key];
        let c = b.cmd.to_string();
        match known(&c) {
            Some((g, label)) => add((g, label.to_string()), shown(key)),
            // One's own: what was bound, as written (shortened).
            None if defaults.get(key).is_none_or(|d| d.cmd != b.cmd) => {
                let label: String = c.chars().take(28).collect();
                add((4, if c.chars().count() > 28 { format!("{label}…") } else { label }), shown(key));
            }
            None => {}
        }
    }
    // Within a group, in the order `known` lists the commands.
    let rank = |g: usize, label: &str| -> usize {
        if g == 4 {
            return 0;
        }
        LABEL_ORDER.iter().position(|l| *l == label).unwrap_or(usize::MAX)
    };
    let mut out: Vec<(&'static str, Vec<Entry>)> = GROUPS.iter().map(|g| (*g, Vec::new())).collect();
    by.sort_by_key(|((g, label), _)| (*g, rank(*g, label)));
    for ((g, label), keys) in by {
        out[g].1.push(Entry { keys: keys_text(keys), label });
    }
    out.retain(|(_, e)| !e.is_empty());
    out
}

/// The panel's footer: the key that lists every binding (`?` unless bound
/// elsewhere), or nothing when no key does.
pub fn footer(binds: &HashMap<Key, Binding>) -> String {
    let keys: Vec<String> =
        binds.iter().filter(|(_, b)| b.cmd.to_string() == "list-keys").map(|(k, _)| shown(k)).collect();
    if keys.is_empty() { String::new() } else { format!("{} every key", keys_text(keys)) }
}

/// The labels in the order the panel lists them (as in `known`).
const LABEL_ORDER: &[&str] = &[
    "split side by side",
    "split above / below",
    "go to a pane",
    "zoom",
    "resize",
    "pane menu",
    "close the pane",
    "pane numbers",
    "new window",
    "next / previous window",
    "go to window",
    "last window",
    "pick a window",
    "window menu",
    "rename window",
    "close the window",
    "detach",
    "pick a session",
    "rename session",
    "save sessions",
    "restore sessions",
    "previous / next session",
    "dashboard",
    "scroll and copy",
    "paste",
    "copy last output",
    "pick text on screen",
    "history",
    "jobs",
    "command",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(g: &[(&str, Vec<Entry>)], name: &str) -> Vec<(String, String)> {
        g.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, e)| e.iter().map(|e| (e.keys.clone(), e.label.clone())).collect())
            .unwrap_or_default()
    }

    #[test]
    fn the_defaults_by_group() {
        let d = super::super::default_bindings();
        let g = groups(&d, &d);
        assert_eq!(g.iter().map(|(n, _)| *n).collect::<Vec<_>>(), ["Panes", "Windows", "Sessions", "Tools"]);
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            entries(&g, "Panes"),
            [
                s("%", "split side by side"),
                s("\"", "split above / below"),
                s("hjkl←↓↑→", "go to a pane"),
                s("z", "zoom"),
                s("HJKL", "resize"),
                s(">", "pane menu"),
                s("x", "close the pane"),
                s("q", "pane numbers"),
            ]
        );
        assert_eq!(
            entries(&g, "Windows"),
            [
                s("c", "new window"),
                s("np", "next / previous window"),
                s("0-9", "go to window"),
                s("Tab", "last window"),
                s("w", "pick a window"),
                s("<", "window menu"),
                s(",", "rename window"),
                s("&", "close the window"),
            ]
        );
        assert_eq!(
            entries(&g, "Sessions"),
            [
                s("d", "detach"),
                s("s", "pick a session"),
                s("$", "rename session"),
                s("C-s", "save sessions"),
                s("C-r", "restore sessions"),
                s("()", "previous / next session"),
            ]
        );
        let tools = entries(&g, "Tools");
        assert_eq!(tools.first(), Some(&s("v", "dashboard")));
        assert_eq!(tools.last(), Some(&s(":", "command")));
        assert!(tools.contains(&s("y", "copy last output")));
        assert!(tools.iter().all(|(k, _)| k != "?"), "`?` is the footer's");
    }

    #[test]
    fn a_key_bound_elsewhere_moves_with_its_command() {
        let d = super::super::default_bindings();
        let mut b = d.clone();
        // `bind N new-window; unbind c`, and a binding of one's own.
        let parse = |l: &str| crate::command::parse_line(l).unwrap().unwrap();
        b.remove(&Key::parse("c").unwrap());
        b.insert(Key::parse("N").unwrap(), Binding { cmd: parse("new-window"), repeat: false });
        b.insert(Key::parse("g").unwrap(), Binding { cmd: parse("display-popup -E lazygit"), repeat: false });
        let g = groups(&b, &d);
        assert_eq!(entries(&g, "Windows")[0], ("N".to_string(), "new window".to_string()));
        assert_eq!(entries(&g, "Yours"), [("g".to_string(), "display-popup -E lazygit".to_string())]);
        // A default the panel does not name is not one's own.
        assert!(entries(&g, "Yours").iter().all(|(k, _)| k != "r"));
        // The footer follows the key that lists them all, and goes with it.
        assert_eq!(footer(&d), "? every key");
        b.insert(Key::parse("K").unwrap(), Binding { cmd: parse("list-keys"), repeat: false });
        b.remove(&Key::parse("?").unwrap());
        assert_eq!(footer(&b), "K every key");
        b.remove(&Key::parse("K").unwrap());
        assert_eq!(footer(&b), "");
    }

    #[test]
    fn nothing_bound_and_a_long_command_of_ones_own() {
        let d = super::super::default_bindings();
        assert!(groups(&HashMap::new(), &d).is_empty(), "every key unbound: no groups");
        let parse = |l: &str| crate::command::parse_line(l).unwrap().unwrap();
        let long = format!("display-popup -E \"{}\"", "x".repeat(60));
        let b = HashMap::from([(Key::parse("g").unwrap(), Binding { cmd: parse(&long), repeat: false })]);
        let yours = entries(&groups(&b, &d), "Yours");
        assert_eq!(yours.len(), 1);
        assert_eq!(yours[0].1.chars().count(), 29, "28 characters and …: {:?}", yours[0].1);
        assert!(yours[0].1.ends_with('…'));
    }
}
