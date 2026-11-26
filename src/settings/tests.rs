use super::*;
use crate::config::{Options, SHOWABLE};

/// `show-settings -J` as a server with these options answers it.
fn json_of(opts: &Options, file: Option<&str>) -> String {
    let d = Options::default();
    let options: Vec<serde_json::Value> = SHOWABLE
        .iter()
        .map(|n| {
            serde_json::json!({
                "name": n,
                "value": opts.get(n).unwrap_or_default(),
                "default": d.get(n).unwrap_or_default(),
                "source": if opts.get(n) == d.get(n) { "default" } else { "set" },
            })
        })
        .collect();
    serde_json::json!({ "file": file, "options": options }).to_string()
}

fn board(cols: u16, rows: u16) -> Board {
    let mut b = Board::new(cols, rows);
    b.set_settings(&json_of(&Options::default(), Some("~/.keepane/settings.conf")));
    b
}

/// The bottom line, as read.
fn foot(b: &Board) -> String {
    strip(b.lines().last().unwrap())
}

/// The whole screen, as read.
fn read(b: &Board) -> String {
    b.lines().iter().map(|l| strip(l)).collect::<Vec<_>>().join("\n")
}

fn k(c: char) -> Key {
    Key::ch(c)
}

fn special(code: KeyCode) -> Key {
    Key::plain(code)
}

fn set(name: &str, value: &str) -> Action {
    Action::Run(owned(&["set-option", "-g", name, value]))
}

/// Walk the keys to option `name`: its box by number, then down.
fn go(b: &mut Board, name: &str) {
    let o = optinfo::info(name).unwrap();
    b.key(k(char::from(b'1' + o.group as u8)));
    b.key(k('g'));
    while b.picked().map(|p| p.name) != Some(name) {
        assert_eq!(b.key(k('j')), Action::Redraw);
    }
}

/// ←/→ steps what can be picked: a choice to the next, on/off over, a
/// number by its step and never below 0.
#[test]
fn arrows_pick_what_can_be_picked() {
    let mut b = board(160, 45);
    assert_eq!(b.picked().unwrap().name, "theme");
    assert_eq!(b.key(k('l')), set("theme", "paper"));
    assert_eq!(b.key(special(KeyCode::Left)), set("theme", "tokyo-day"), "back from the first goes round");
    go(&mut b, "pane-frames");
    assert_eq!(b.key(k(' ')), set("pane-frames", "on"));
    assert_eq!(b.key(special(KeyCode::Enter)), set("pane-frames", "on"), "Enter flips on/off too");
    go(&mut b, "history-limit");
    assert_eq!(b.key(k('l')), set("history-limit", "6000"));
    assert_eq!(b.key(k('h')), set("history-limit", "4000"));
    go(&mut b, "base-index");
    assert_eq!(b.key(k('h')), set("base-index", "0"), "never below 0");
    go(&mut b, "clock-mode-colour");
    assert_eq!(b.key(k('l')), set("clock-mode-colour", "black"), "default is the first colour");
    // A value that is typed is not stepped: it says so.
    go(&mut b, "status-right");
    assert_eq!(b.key(k('l')), Action::Redraw);
    assert!(foot(&b).contains("Enter to type it"));
}

/// Enter types a value on the bottom line, the old one in it; Esc keeps
/// what was there.
#[test]
fn enter_types_a_value_on_the_bottom_line() {
    let mut b = board(160, 45);
    go(&mut b, "default-terminal");
    assert_eq!(b.key(special(KeyCode::Enter)), Action::Redraw);
    assert!(foot(&b).contains("default-terminal: xterm-256color"));
    for _ in 0.."256color".len() {
        b.key(special(KeyCode::BSpace));
    }
    for c in "direct".chars() {
        b.key(k(c));
    }
    assert_eq!(b.key(special(KeyCode::Enter)), set("default-terminal", "xterm-direct"));
    // Esc: nothing sent.
    b.key(special(KeyCode::Enter));
    b.key(k('x'));
    assert_eq!(b.key(special(KeyCode::Escape)), Action::Redraw);
    assert!(b.edit.is_none());
    // The cursor moves; a character goes where it is.
    go(&mut b, "window-status-separator");
    b.key(special(KeyCode::Enter));
    for c in "ac".chars() {
        b.key(k(c));
    }
    b.key(special(KeyCode::Left));
    b.key(k('b'));
    assert_eq!(b.key(special(KeyCode::Enter)), set("window-status-separator", "abc"));
}

/// A secret is never shown: not its value, not what is typed for it.
#[test]
fn a_secret_is_never_shown() {
    let mut opts = Options::default();
    opts.set("feishu-app-secret", "s3cret-value").unwrap();
    let mut b = Board::new(160, 45);
    b.set_settings(&json_of(&opts, Some("~/.keepane/settings.conf")));
    go(&mut b, "feishu-app-secret");
    b.key(special(KeyCode::Enter));
    for c in "typed-secret".chars() {
        b.key(k(c));
    }
    b.zoom = true;
    let screen = read(&b);
    assert!(!screen.contains("s3cret") && !screen.contains("typed-secret"), "{screen}");
    assert!(screen.contains("••••••••••••"), "typed as dots");
    let a = b.key(special(KeyCode::Enter));
    assert_eq!(a, set("feishu-app-secret", "typed-secret"));
    let Action::Run(argv) = a else { unreachable!() };
    b.done(&argv, 0, "", "");
    assert!(!read(&b).contains("typed-secret"), "nor said when set");
}

/// Enter on the prefix waits for the new key and sets it by its name.
#[test]
fn the_prefix_is_pressed_not_typed() {
    let mut b = board(160, 45);
    go(&mut b, "prefix");
    b.key(special(KeyCode::Enter));
    assert!(foot(&b).contains("press the new prefix key"));
    assert_eq!(b.key(Key::ctrl('a')), set("prefix", "C-a"));
    b.key(special(KeyCode::Enter));
    assert_eq!(b.key(special(KeyCode::Escape)), Action::Redraw, "Esc: no new prefix");
}

/// A change is marked until written; `w` writes the marked ones, and
/// quitting with some unwritten asks first.
#[test]
fn changes_are_marked_until_written() {
    let mut b = board(160, 45);
    go(&mut b, "mouse");
    let Action::Run(argv) = b.key(k(' ')) else { panic!() };
    b.done(&argv, 0, "", "");
    go(&mut b, "history-limit");
    let Action::Run(argv) = b.key(k('r')) else { panic!() };
    assert_eq!(argv, owned(&["set-option", "-gu", "history-limit"]));
    b.done(&argv, 0, "", "");
    assert_eq!(b.unsaved.iter().cloned().collect::<Vec<_>>(), owned(&["history-limit", "mouse"]));
    assert!(strip(&b.lines()[0]).contains("2 not written"));
    // A change refused is not marked, and the reason is said.
    let argv = owned(&["set-option", "-g", "agent-pane-limit", "999"]);
    b.done(&argv, 1, "", "bad agent-pane-limit '999' (panes, 0 to 256)");
    assert!(!b.unsaved.contains("agent-pane-limit"));
    assert!(foot(&b).contains("bad agent-pane-limit"));
    // q asks; Esc stays.
    assert_eq!(b.key(k('q')), Action::Redraw);
    assert!(foot(&b).contains("2 changes not written"));
    assert_eq!(b.key(special(KeyCode::Escape)), Action::Redraw);
    assert_eq!(b.key(k('q')), Action::Redraw);
    assert_eq!(b.key(k('w')), Action::RunThenQuit(owned(&["save-settings", "history-limit", "mouse"])));
    // w writes; written, nothing is marked and q quits at once.
    let save = owned(&["save-settings", "history-limit", "mouse"]);
    assert_eq!(b.key(k('w')), Action::Run(save.clone()));
    b.done(&save, 0, "saved 2 options to ~/.keepane/settings.conf", "");
    assert!(b.unsaved.is_empty());
    assert_eq!(b.key(k('w')), Action::Redraw, "nothing to write");
    assert_eq!(b.key(k('q')), Action::Quit);
}

/// No settings file (the config was named): `w` says so and sends nothing.
#[test]
fn no_file_nothing_written() {
    let mut b = Board::new(160, 45);
    b.set_settings(&json_of(&Options::default(), None));
    b.unsaved.insert("mouse".into());
    assert_eq!(b.key(k('w')), Action::Redraw);
    assert!(foot(&b).contains("no settings file"));
    assert!(strip(&b.lines()[0]).contains("no settings file"));
}

/// Reloading waits for a y, saying what is lost.
#[test]
fn reload_waits_for_a_yes() {
    let mut opts = Options::default();
    opts.set("mouse", "off").unwrap();
    let mut b = Board::new(160, 45);
    b.set_settings(&json_of(&opts, Some("~/.keepane/settings.conf")));
    assert_eq!(b.key(k('R')), Action::Redraw);
    assert!(foot(&b).contains("1 set by hand are lost"));
    assert_eq!(b.key(k('n')), Action::Redraw, "anything but y: nothing");
    b.key(k('R'));
    let reload = owned(&["reload-settings"]);
    assert_eq!(b.key(k('y')), Action::Run(reload.clone()));
    b.unsaved.insert("mouse".into());
    b.done(&reload, 0, "settings back to the defaults", "");
    assert!(b.unsaved.is_empty());
}

/// Whatever is pressed, on whichever option, the board sends only the
/// commands it may, and a reload only after a yes.
#[test]
fn only_its_own_commands_go_out() {
    let keys: Vec<Key> = "jkhlgGrzmwRq/ 0123456789xyn"
        .chars()
        .map(k)
        .chain([KeyCode::Enter, KeyCode::Escape, KeyCode::Tab, KeyCode::Left, KeyCode::Right].map(special))
        .collect();
    for o in OPTIONS {
        for key in &keys {
            let mut b = board(160, 45);
            go(&mut b, o.name);
            let mut sent = Vec::new();
            for press in [*key, *key, special(KeyCode::Enter)] {
                match b.key(press) {
                    Action::Run(argv) | Action::RunThenQuit(argv) => sent.push(argv),
                    _ => {}
                }
            }
            for argv in sent {
                assert!(ACTIONS.contains(&argv[0].as_str()), "{}: {key:?} sent {argv:?}", o.name);
                if argv[0] == "reload-settings" {
                    assert_eq!(*key, k('y'), "a reload without R then y");
                }
            }
        }
    }
}

/// The boxes stand where they stand: three, two or one abreast by width,
/// never overlapping, never reordered, all nine kept when a filter empties
/// some; zoomed, one fills the board and z puts it back.
#[test]
fn the_boxes_stay_where_they_are() {
    for (cols, across) in [(160u16, 3usize), (114, 3), (113, 2), (80, 2), (75, 1), (40, 1)] {
        let b = board(cols, 45);
        let grid = b.grid();
        let xs: std::collections::BTreeSet<usize> = grid.iter().map(|(_, r)| r.x).collect();
        assert_eq!(xs.len(), across, "{cols} columns");
        for (i, (g, r)) in grid.iter().enumerate() {
            assert!(r.x + r.w <= usize::from(cols) && r.h >= BOX_LEAST, "{cols}: box {g} {r:?}");
            for (h, s) in grid.iter().skip(i + 1) {
                let apart = r.x + r.w <= s.x || s.x + s.w <= r.x || r.y + r.h <= s.y || s.y + s.h <= r.y;
                assert!(apart, "{cols}: boxes {g} and {h} overlap");
            }
        }
        let order: Vec<usize> = grid.iter().map(|(g, _)| *g).collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    }
    let mut b = board(160, 45);
    assert_eq!(b.grid().len(), 9);
    let before = b.grid();
    b.key(k('/'));
    for c in "mouse".chars() {
        b.key(k(c));
    }
    b.key(special(KeyCode::Enter));
    assert_eq!(b.grid(), before, "a filter moves no box");
    let screen = read(&b);
    assert_eq!(screen.matches("no match").count(), 8, "{screen}");
    assert_eq!(b.items(3).iter().map(|o| o.name).collect::<Vec<_>>(), vec!["mouse"]);
    b.key(special(KeyCode::Escape));
    assert!(b.filter.is_empty(), "Esc clears the filter");
    // Zoom and back: the same option picked.
    go(&mut b, "keep-zoom");
    b.key(k('z'));
    assert_eq!(b.grid().len(), 1);
    assert!(read(&b).contains("set by"), "the table's heading");
    b.key(special(KeyCode::Escape));
    assert_eq!(b.grid(), before);
    assert_eq!(b.picked().unwrap().name, "keep-zoom");
    // A short window: the board scrolls by boxes and keeps the one in use in view.
    let mut b = board(80, 20);
    b.key(k('9'));
    assert!(b.grid().iter().any(|(g, _)| *g == 8));
    assert!(b.grid().len() < 9);
}

/// Every row is the window's width, whatever the window, the state, or
/// the text typed.
#[test]
fn every_row_fits_the_window() {
    for (cols, rows) in [(160u16, 45u16), (114, 30), (90, 24), (60, 20), (30, 10), (12, 4)] {
        for zoom in [false, true] {
            let mut b = board(cols, rows);
            b.zoom = zoom;
            go(&mut b, "status-right");
            b.key(special(KeyCode::Enter));
            for (i, l) in b.lines().iter().enumerate() {
                assert_eq!(width_of(l), usize::from(cols), "{cols}x{rows} zoom {zoom} row {i}: {l:?}");
            }
            assert_eq!(b.lines().len(), usize::from(rows));
            b.key(special(KeyCode::Escape));
            b.key(k('R'));
            for l in b.lines() {
                assert_eq!(width_of(&l), usize::from(cols));
            }
        }
    }
}

/// A click picks the option under it; a click on a box's top line zooms
/// it; the wheel moves in the box under it.
#[test]
fn the_mouse_picks_and_zooms() {
    let mut b = board(160, 45);
    let (_, r) = b.grid()[3];
    let click = |x: usize, y: usize| MouseRecord { x: x as i16, y: y as i16, buttons: 1, ctrl: 0, flags: 0 };
    b.mouse(&click(r.x + 3, r.y + 2));
    assert_eq!((b.group, b.picked().unwrap().name), (3, "history-limit"), "the second line of box 4");
    b.mouse(&MouseRecord {
        x: r.x as i16 + 3,
        y: r.y as i16 + 2,
        buttons: (-120i32 as u32) << 16,
        ctrl: 0,
        flags: 0x4,
    });
    assert_eq!(b.picked().unwrap().name, "base-index");
    b.mouse(&click(r.x + 3, r.y));
    assert!(b.zoom);
    b.mouse(&click(5, 1));
    assert!(!b.zoom, "the zoomed box's top line puts it back");
}

/// What the server answers that does not read is said, not shown as
/// options.
#[test]
fn an_answer_that_does_not_read_is_said() {
    let mut b = board(120, 30);
    b.set_settings("unknown command: show-settings");
    assert!(foot(&b).contains("unknown command"));
    assert!(b.picked().is_some(), "what was there stays");
}
