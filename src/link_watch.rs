//! `keepane link watch host:port/pane`: a pane of a paired machine, live,
//! in the terminal it runs in (a pane here, usually: `C-b w`, Enter on a
//! pane of another machine, opens a window running it). An ordinary program
//! in an ordinary pane, as `ssh` with tmux in it is: the layout, `C-b v`,
//! zoom and copy mode treat it as any pane, and `resume` brings it back.
//!
//! It asks this machine's server (`link-view`, signed there) for the pane's
//! screen several times a second; the other machine answers only "same" when
//! nothing printed. Keys typed go over as key records (`link-keys`), made
//! into bytes there by the pane's own modes, when that machine gave this one
//! all of itself (`link allow --full`): typing into a shell is running any
//! command there. A machine that only shared the pane is watched read-only;
//! `q` then closes the watch.

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::console::{Console, InputEvent};
use crate::ipc::KeyRecord;

/// How often the screen is asked for while something happens (a key was
/// typed or the screen changed in the last `BUSY_FOR`), and when nothing does.
const ASK_BUSY: Duration = Duration::from_millis(100);
const ASK_IDLE: Duration = Duration::from_millis(500);
const BUSY_FOR: Duration = Duration::from_secs(3);
/// After a failed asking (the machine away, its web stopped).
const ASK_AFTER_ERROR: Duration = Duration::from_secs(2);
/// How long a note stays on the bottom row.
const NOTE_FOR: Duration = Duration::from_secs(4);

/// The other machine's screen as last heard.
struct Remote {
    parser: vt100::Parser,
    cursor: (u16, u16),
    cursor_shown: bool,
    typing: bool,
    dead: bool,
}

/// One answer of `link-view`.
#[derive(Debug, PartialEq)]
enum View {
    Same {
        stamp: String,
    },
    Screen {
        stamp: String,
        cols: u16,
        rows: u16,
        cursor: (u16, u16),
        cursor_shown: bool,
        screen: String,
        typing: bool,
        dead: bool,
    },
}

fn read_view(text: &str) -> Option<View> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let stamp = v["stamp"].as_str()?.to_string();
    if v["same"] == true {
        return Some(View::Same { stamp });
    }
    let num = |x: &serde_json::Value| x.as_u64().and_then(|n| u16::try_from(n).ok());
    Some(View::Screen {
        stamp,
        cols: num(&v["cols"])?.max(1),
        rows: num(&v["rows"])?.max(1),
        cursor: (num(&v["cursor"][0]).unwrap_or(0), num(&v["cursor"][1]).unwrap_or(0)),
        cursor_shown: v["cursor_shown"] != false,
        screen: v["screen"].as_str().unwrap_or_default().to_string(),
        typing: v["typing"] == true,
        dead: v["dead"] == true,
    })
}

/// Where the shown part of a remote screen starts on one axis: from the
/// start, unless the cursor would be past the view, then far enough in to
/// show it (a wider screen than the view follows its cursor).
fn offset(cursor: u16, remote: u16, view: u16) -> u16 {
    if remote <= view || cursor < view { 0 } else { (cursor + 1 - view).min(remote - view) }
}

/// The whole frame for a terminal of `cols` x `rows`: the remote screen
/// (cut to fit, following its cursor), or `message` centred when there is
/// none yet; `note` over the bottom row.
fn frame(remote: Option<&Remote>, cols: u16, rows: u16, message: &str, note: Option<&str>) -> String {
    let mut out = String::from("\x1b[?25l");
    let mut cursor_at: Option<(u16, u16)> = None;
    let lines: Vec<String> = match remote {
        Some(r) => {
            let s = r.parser.screen();
            let (rrows, rcols) = s.size();
            let (ox, oy) = (offset(r.cursor.0, rcols, cols), offset(r.cursor.1, rrows, rows));
            if r.cursor_shown && !r.dead {
                let (x, y) = (r.cursor.0.wrapping_sub(ox), r.cursor.1.wrapping_sub(oy));
                if x < cols && y < rows {
                    cursor_at = Some((x, y));
                }
            }
            s.rows_formatted(ox, cols)
                .skip(usize::from(oy))
                .take(usize::from(rows))
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .collect()
        }
        None => {
            let mut lines = vec![String::new(); usize::from(rows / 2)];
            let pad = usize::from(cols).saturating_sub(message.chars().count()) / 2;
            lines.push(format!("{}{message}", " ".repeat(pad)));
            lines
        }
    };
    for y in 0..rows {
        out.push_str(&format!("\x1b[{};1H\x1b[0m", y + 1));
        if let Some(l) = lines.get(usize::from(y)) {
            out.push_str(l);
        }
        out.push_str("\x1b[0m\x1b[K");
    }
    if let Some(n) = note {
        let n: String = n.chars().take(usize::from(cols)).collect();
        out.push_str(&format!("\x1b[{rows};1H\x1b[0;7m{n}\x1b[K\x1b[0m"));
    }
    if let Some((x, y)) = cursor_at {
        out.push_str(&format!("\x1b[{};{}H\x1b[?25h", y + 1, x + 1));
    }
    out
}

/// What a watch says while it has no screen, from an error of `link-view`.
fn plain(error: &str) -> String {
    error.trim().lines().next().unwrap_or("no answer").to_string()
}

pub async fn run(socket: &str, target: &str) -> Result<i32> {
    let Some((addr, _)) = crate::link::split_remote(target) else {
        anyhow::bail!("keepane link watch: '{target}' is not host:port/pane");
    };
    let addr = addr.to_string();
    let mut console = Console::open()?;
    console.enter_raw()?;
    console.set_title(&format!("{target} (watch)"));
    let console = std::sync::Arc::new(console);
    // Console input blocks: read on a thread of its own.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    let reader = console.clone();
    std::thread::spawn(move || {
        while let Ok(events) = reader.read_events() {
            for e in events {
                if tx.send(e).is_err() {
                    return;
                }
            }
        }
    });
    let query = |argv: Vec<String>| async move {
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        crate::client::query(socket, &argv).await
    };
    let mut remote: Option<Remote> = None;
    let mut stamp: Option<String> = None;
    let mut message = format!("asking {target} for its screen…");
    let mut note: Option<(String, Instant)> = None;
    let mut busy = Instant::now();
    let mut next_ask = Instant::now();
    let (mut cols, mut rows) = console.size();
    let mut drawn = String::new();
    let result: Result<i32> = async {
        loop {
            let note_text = note.as_ref().filter(|(_, at)| at.elapsed() < NOTE_FOR).map(|(n, _)| n.clone());
            let f = frame(remote.as_ref(), cols, rows, &message, note_text.as_deref());
            if f != drawn {
                console.write_str(&f);
                drawn = f;
            }
            let wait = next_ask.saturating_duration_since(Instant::now()).min(Duration::from_millis(250));
            tokio::select! {
                ev = rx.recv() => {
                    let Some(ev) = ev else { return Ok(0) };
                    let mut keys: Vec<KeyRecord> = Vec::new();
                    let mut more = Some(ev);
                    while let Some(ev) = more.take().or_else(|| rx.try_recv().ok()) {
                        match ev {
                            InputEvent::Key(k) if k.down => keys.push(k),
                            InputEvent::Resize => (cols, rows) = console.size(),
                            _ => {}
                        }
                    }
                    if keys.is_empty() {
                        continue;
                    }
                    let typing = remote.as_ref().is_some_and(|r| r.typing);
                    if !typing {
                        if keys.iter().any(|k| k.ch == u16::from(b'q')) {
                            return Ok(0);
                        }
                        note = Some((format!(
                            "read-only: {addr} shares this pane for watching; typing needs `keepane link allow <this machine> --full` there · q closes"
                        ), Instant::now()));
                        continue;
                    }
                    let json = serde_json::to_string(&keys).expect("keys serialize");
                    match query(vec!["link-keys".into(), target.into(), json]).await {
                        Ok((0, _, _)) => {}
                        Ok((_, _, err)) => note = Some((plain(&err), Instant::now())),
                        Err(e) => note = Some((plain(&format!("{e:#}")), Instant::now())),
                    }
                    busy = Instant::now();
                    // What the keys did, at once.
                    next_ask = Instant::now();
                }
                _ = tokio::time::sleep(wait) => {
                    if Instant::now() < next_ask {
                        continue;
                    }
                    let mut argv = vec!["link-view".to_string(), target.to_string()];
                    if let (Some(s), Some(_)) = (&stamp, &remote) {
                        argv.extend(["-s".into(), s.clone()]);
                    }
                    let every = if busy.elapsed() < BUSY_FOR { ASK_BUSY } else { ASK_IDLE };
                    next_ask = Instant::now() + every;
                    match query(argv).await {
                        Ok((0, out, _)) => match read_view(&out) {
                            Some(View::Same { .. }) => {}
                            Some(View::Screen { stamp: st, cols: rc, rows: rr, cursor, cursor_shown, screen, typing, dead }) => {
                                let mut parser = vt100::Parser::new(rr, rc, 0);
                                parser.process(screen.as_bytes());
                                remote = Some(Remote { parser, cursor, cursor_shown, typing, dead });
                                stamp = Some(st);
                                busy = Instant::now();
                                next_ask = Instant::now() + ASK_BUSY;
                            }
                            // Said instead of a screen that is no longer sure.
                            None => {
                                remote = None;
                                stamp = None;
                                message = format!("{addr} answered with something that is not a screen");
                                next_ask = Instant::now() + ASK_AFTER_ERROR;
                            }
                        },
                        Ok((_, _, err)) => {
                            remote = None;
                            stamp = None;
                            message = plain(&err);
                            next_ask = Instant::now() + ASK_AFTER_ERROR;
                        }
                        Err(e) => {
                            remote = None;
                            message = plain(&format!("this machine's keepane: {e:#}"));
                            next_ask = Instant::now() + ASK_AFTER_ERROR;
                        }
                    }
                }
            }
        }
    }
    .await;
    console.restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(rows: u16, cols: u16, text: &str, cursor: (u16, u16)) -> Remote {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(text.as_bytes());
        Remote { parser, cursor, cursor_shown: true, typing: true, dead: false }
    }

    #[test]
    fn a_view_answer_reads_and_what_is_not_one_does_not() {
        assert_eq!(
            read_view(r#"{"stamp":"3 80x24 false","same":true}"#),
            Some(View::Same { stamp: "3 80x24 false".into() })
        );
        let v = read_view(
            r#"{"stamp":"s","cols":80,"rows":24,"cursor":[5,2],"cursor_shown":false,"screen":"hi","typing":true,"dead":false}"#,
        );
        assert_eq!(
            v,
            Some(View::Screen {
                stamp: "s".into(),
                cols: 80,
                rows: 24,
                cursor: (5, 2),
                cursor_shown: false,
                screen: "hi".into(),
                typing: true,
                dead: false
            })
        );
        assert_eq!(read_view("garbage"), None);
        assert_eq!(read_view(r#"{"stamp":"s","rows":24}"#), None, "no width: not a screen");
        // A size of nothing is one cell, not a screen that cannot be made.
        let Some(View::Screen { cols, rows, .. }) = read_view(r#"{"stamp":"s","cols":0,"rows":0}"#) else { panic!() };
        assert_eq!((cols, rows), (1, 1));
    }

    #[test]
    fn a_wider_or_taller_screen_follows_its_cursor() {
        assert_eq!(offset(5, 80, 100), 0, "it fits");
        assert_eq!(offset(10, 120, 80), 0, "the cursor is in view");
        assert_eq!(offset(100, 120, 80), 21, "moved in just far enough");
        assert_eq!(offset(119, 120, 80), 40, "never past the end");
    }

    #[test]
    fn the_frame_draws_the_screen_cut_to_fit_with_its_cursor() {
        let r = remote(3, 10, "abcdefghij\r\nsecond\r\nthird", (2, 1));
        let f = frame(Some(&r), 4, 2, "", None);
        assert!(f.contains("\x1b[1;1H\x1b[0mabcd"), "{f:?}");
        assert!(f.contains("\x1b[2;1H\x1b[0msecon") || f.contains("\x1b[2;1H\x1b[0msec"), "{f:?}");
        assert!(!f.contains("third"), "the third row is below the view");
        assert!(f.ends_with("\x1b[2;3H\x1b[?25h"), "the cursor where it is there: {f:?}");
        // Nothing heard yet: the message, centred; no cursor.
        let f = frame(None, 20, 3, "asking", None);
        assert!(f.contains("\x1b[2;1H\x1b[0m       asking"), "{f:?}");
        assert!(!f.contains("?25h"));
        // A note over the bottom row, cut to the width.
        let f = frame(Some(&r), 4, 2, "", Some("read-only: no"));
        assert!(f.contains("\x1b[2;1H\x1b[0;7mread\x1b[K"), "{f:?}");
        // A dead pane shows no cursor.
        let dead = Remote { dead: true, ..remote(3, 10, "x", (0, 0)) };
        assert!(!frame(Some(&dead), 10, 3, "", None).contains("?25h"));
    }
}
