//! Page 2 of the dashboard: what this machine shares (`share -l -J`): the
//! shares and how each stands ([1]), what happened to them ([2]), and the
//! one picked, in full ([4]).

use super::{fit, paint};
use chrono::{Local, TimeZone};

/// One share as `share -l -J` gives it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ShareRow {
    pub id: u32,
    /// active, missing, stopped, expired, used.
    pub state: String,
    /// file or folder.
    pub kind: String,
    pub name: String,
    pub path: String,
    pub created: i64,
    pub until: Option<i64>,
    pub once: bool,
    pub sent: u64,
    pub seen: Vec<String>,
    pub from: Option<String>,
    pub links: Vec<String>,
    pub port_moved: bool,
    pub ended: Option<i64>,
    pub events: Vec<(i64, String)>,
}

/// The times a share is given for, as the menus offer them: what is shown,
/// what `--for` takes.
pub const FOR: [(&str, &str); 4] = [("1 hour", "1h"), ("24 hours", "24h"), ("7 days", "7d"), ("no end", "0")];

/// `share -l -J` read: the shares (the running ones first, then the ended,
/// the latest first). Whether the page is served shows in each one's links.
pub fn parse(json: &str) -> Vec<ShareRow> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let strs = |x: &serde_json::Value| -> Vec<String> {
        x.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    v["shares"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| ShareRow {
                    id: s["id"].as_u64().unwrap_or(0) as u32,
                    state: s["state"].as_str().unwrap_or_default().into(),
                    kind: s["kind"].as_str().unwrap_or_default().into(),
                    name: s["name"].as_str().unwrap_or_default().into(),
                    path: s["path"].as_str().unwrap_or_default().into(),
                    created: s["created"].as_i64().unwrap_or(0),
                    until: s["until"].as_i64(),
                    once: s["once"].as_bool().unwrap_or(false),
                    sent: s["sent"].as_u64().unwrap_or(0),
                    seen: strs(&s["seen"]),
                    from: s["from"].as_str().map(String::from),
                    links: strs(&s["links"]),
                    port_moved: s["port_moved"].as_bool().unwrap_or(false),
                    ended: s["ended"].as_i64(),
                    events: s["events"]
                        .as_array()
                        .map(|e| e.iter().filter_map(|x| Some((x[0].as_i64()?, x[1].as_str()?.to_string()))).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A time, as the list says it: the day and the minute.
pub fn at(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%m-%d %H:%M").to_string()).unwrap_or_default()
}

impl ShareRow {
    /// It still has a link (its file may be gone for now).
    pub fn running(&self) -> bool {
        matches!(self.state.as_str(), "active" | "missing")
    }

    /// Its state, in its colour: active green, missing yellow, stopped
    /// faint, expired faint yellow, used blue.
    pub fn state_styled(&self) -> String {
        let sgr = match self.state.as_str() {
            "active" => "32",
            "missing" => "33",
            "expired" => "2;33",
            "used" => "34",
            _ => "2",
        };
        paint(sgr, &self.state)
    }

    /// How long it has left (`23h`, `6d`, `∞`), yellow under an hour;
    /// nothing once ended.
    pub fn left(&self, now_ms: i64) -> String {
        if !self.running() {
            return String::new();
        }
        match self.until {
            None => "∞".into(),
            Some(u) => {
                let secs = ((u - now_ms) / 1000).max(0);
                let text = crate::format::human_duration(secs);
                if secs < 3600 { paint("33", &text) } else { text }
            }
        }
    }

    /// Its line in [1]: number, state, kind, name, time left, times sent.
    pub fn line(&self, now_ms: i64, width: usize) -> String {
        let name = if self.kind == "folder" { format!("{}/", self.name) } else { self.name.clone() };
        let tail = format!("{} {:>3}", super::fit_right(&self.left(now_ms), 6), self.sent);
        let head = format!("{:>3} {} {:<6}", self.id, fit(&self.state_styled(), 7), self.kind);
        let room = width.saturating_sub(super::width_of(&head) + super::width_of(&tail) + 2);
        format!("{head} {} {tail}", fit(&name, room))
    }

    /// [4]: everything of it, labelled, and the code to scan for its link.
    pub fn detail(&self, now_ms: i64, width: usize) -> Vec<String> {
        let label = |l: &str| paint("2", &format!("{l:<8}"));
        let mut out = vec![
            format!("{}{}  {}", label("state"), self.state_styled(), paint("2", &format!("share {}", self.id))),
            format!("{}{} ({})", label("path"), self.path, self.kind),
        ];
        match self.links.first() {
            Some(l) => {
                out.push(format!("{}{l}", label("link")));
                for l in &self.links[1..] {
                    out.push(format!("{}{l}", label("also")));
                }
                if self.port_moved {
                    out.push(format!(
                        "{}{}",
                        label(""),
                        paint(
                            "33",
                            "served on another port than its link was given on: the old link does not reach it"
                        )
                    ));
                }
            }
            None if self.running() => out.push(format!("{}{}", label("link"), paint("2", "the phone page is off"))),
            None => {}
        }
        if self.state == "missing" {
            out.push(format!(
                "{}{}",
                label(""),
                paint("33", "its file is gone: the link answers nothing until it is back")
            ));
        }
        let until = match (self.ended, self.until) {
            (Some(e), _) => format!("ended {}", at(e)),
            (None, Some(u)) => {
                format!("{} ({} left)", at(u), crate::format::human_duration(((u - now_ms) / 1000).max(0)))
            }
            (None, None) => "until stopped".into(),
        };
        out.push(format!("{}{until}{}", label("until"), if self.once { " · once" } else { "" }));
        out.push(format!("{}{}", label("shared"), at(self.created)));
        let seen = if self.seen.is_empty() { String::new() } else { format!(" · opened by {}", self.seen.join(", ")) };
        out.push(format!("{}{} time(s){seen}", label("sent"), self.sent));
        if let Some(f) = &self.from {
            out.push(format!("{}pane {f}", label("from")));
        }
        out.push(String::new());
        out.push(paint("2", "what happened"));
        for (t, what) in self.events.iter().rev() {
            out.push(format!("  {}  {what}", paint("2", &at(*t))));
        }
        // The code to scan, when there is room for it.
        if let Some(code) = self.links.first().and_then(|l| crate::web::qr_text(l).ok()) {
            let code: Vec<String> = code.lines().map(String::from).collect();
            if code.iter().all(|l| super::width_of(l) <= width) {
                out.push(String::new());
                out.extend(code);
            }
        }
        out
    }
}

/// [2]: what happened to every share, the latest first: when, which, what.
pub fn activity(rows: &[ShareRow]) -> Vec<String> {
    let mut all: Vec<(i64, u32, &str, &str)> =
        rows.iter().flat_map(|r| r.events.iter().map(move |(t, w)| (*t, r.id, r.name.as_str(), w.as_str()))).collect();
    all.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    all.into_iter()
        .map(|(t, id, name, what)| format!("{}  {} {name}  {what}", paint("2", &at(t)), paint("1", &id.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"served":true,"shares":[
        {"id":3,"state":"active","kind":"file","name":"report.pdf","path":"C:\\r.pdf","created":1000,"until":7200000,
         "once":false,"sent":1,"seen":["10.0.0.9"],"from":"%4","links":["http://10.0.0.5:7681/s/abc"],"port":7681,
         "port_moved":false,"ended":null,"events":[[1000,"shared, until x"],[5000,"opened by 10.0.0.9"]]},
        {"id":1,"state":"stopped","kind":"folder","name":"logs","path":"/l","created":10,"until":null,"once":true,
         "sent":0,"seen":[],"from":null,"links":[],"port":0,"port_moved":false,"ended":3000,
         "events":[[10,"shared"],[3000,"stopped"]]}]}"#;

    #[test]
    fn the_list_reads_back_with_states_times_and_what_happened() {
        let rows = parse(JSON);
        assert_eq!(rows.len(), 2);
        let (a, b) = (&rows[0], &rows[1]);
        assert!(a.running() && !b.running());
        assert_eq!(
            (a.id, a.sent, a.seen.as_slice(), a.from.as_deref()),
            (3, 1, &["10.0.0.9".to_string()][..], Some("%4"))
        );
        assert_eq!(super::super::strip(&a.left(0)), "2h00m");
        assert_eq!(b.left(0), "", "ended: no time left");
        // Under an hour left: yellow.
        assert!(a.left(7_200_000 - 1_800_000).contains("\x1b[33m") && !a.left(0).contains("\x1b[33m"));
        let line = super::super::strip(&a.line(0, 40));
        assert!(
            line.starts_with("  3 active  file") && line.contains("report.pdf") && line.ends_with(" 2h00m   1"),
            "{line:?}"
        );
        assert!(super::super::strip(&b.line(0, 40)).contains("logs/"));
        // What happened, every share's, the latest first.
        let act: Vec<String> = activity(&rows).iter().map(|l| super::super::strip(l)).collect();
        assert_eq!(act.len(), 4);
        assert!(act[0].ends_with("3 report.pdf  opened by 10.0.0.9") && act[1].ends_with("1 logs  stopped"), "{act:?}");
        let detail: Vec<String> = a.detail(0, 200).iter().map(|l| super::super::strip(l)).collect();
        assert!(detail.iter().any(|l| l.starts_with("link    http://10.0.0.5:7681/s/abc")), "{detail:?}");
        assert!(detail.iter().any(|l| l.contains("opened by 10.0.0.9")));
        assert!(detail.iter().any(|l| l.contains('█')), "the code to scan");
        // A list that is not one: nothing, not a panic.
        assert_eq!(parse("nothing is shared"), Vec::new());
    }
}
