//! `board-pick`: where the dashboard was left (its page and the row picked
//! there), so the next one, opened by `C-b v` in a new process, opens there.

use super::{ClientId, Outcome, Server};

impl Server {
    /// `board-pick`: the page and row kept, printed (`PAGE ROW`, `PAGE`
    /// with no row, nothing when none); `board-pick PAGE [ROW]`, those
    /// kept. Only the person moves it: a pane's agent may read it, not
    /// change it (as `chat-pick`).
    pub(super) fn board_pick(&mut self, cid: Option<ClientId>, place: Option<(u8, String)>) -> Outcome {
        let Some(place) = place else {
            return Outcome::Text(match &self.board_place {
                Some((page, row)) if row.is_empty() => page.to_string(),
                Some((page, row)) => format!("{page} {row}"),
                None => String::new(),
            });
        };
        if self.agent_caller(cid).is_some() {
            return Outcome::Error("board-pick: where the person left the dashboard is theirs to change".into());
        }
        self.board_place = Some(place);
        Outcome::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Cmd, parse_line};

    fn run(s: &mut Server, line: &str) -> String {
        let Ok(Some(cmd @ Cmd::BoardPick { .. })) = parse_line(line) else { panic!("{line}") };
        match s.exec(cmd, None) {
            Outcome::Text(t) => t,
            Outcome::Ok => "ok".into(),
            Outcome::Error(e) => format!("error: {e}"),
            _ => "other".into(),
        }
    }

    /// Nothing kept at first; what is kept is printed back, the row left
    /// out when none; the latest wins.
    #[test]
    fn the_place_kept_is_printed_back() {
        let (pane_tx, _p) = std::sync::mpsc::channel::<super::super::PaneEvent>();
        let (events, _e) = tokio::sync::mpsc::unbounded_channel::<super::super::Event>();
        let mut s = Server::new(pane_tx, "board-test".into(), events);
        assert_eq!(run(&mut s, "board-pick"), "", "none yet");
        assert_eq!(run(&mut s, "board-pick 1 %7"), "ok");
        assert_eq!(run(&mut s, "board-pick"), "1 %7");
        assert_eq!(run(&mut s, "board-pick 2"), "ok");
        assert_eq!(run(&mut s, "board-pick"), "2", "no row on the chat's page");
        assert_eq!(run(&mut s, "board-pick 1 10.0.0.2:7681/%4"), "ok");
        assert_eq!(run(&mut s, "board-pick"), "1 10.0.0.2:7681/%4");
    }
}
