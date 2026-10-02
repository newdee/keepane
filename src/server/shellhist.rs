//! `shell-history`: a pane's own command history file read back as its
//! shell keeps it, entry by entry, with the messages delivered to it as
//! commands told apart by their envelope.
//!
//! Each shell writes its file its own way: PowerShell (PSReadLine) goes on
//! to the next line after a backtick; bash may put a `#<seconds>` line
//! before a command; zsh may start one with `: <seconds>:<took>;`, goes on
//! after a backslash, and keeps bytes past ASCII "metafied".

/// Which shell wrote a history file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Pwsh,
    Bash,
    Zsh,
}

impl Shell {
    /// The shell a program is, by its name (a Windows path or a Unix one,
    /// with `.exe` or without: the file may come from either side).
    pub fn of_program(program: &str) -> Option<Shell> {
        let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
        let name = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
        match name.to_ascii_lowercase().as_str() {
            "pwsh" | "powershell" => Some(Shell::Pwsh),
            "bash" | "sh" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            _ => None,
        }
    }
}

/// zsh writes a byte past ASCII as 0x83 and the byte xor 0x20.
fn unmetafy(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut it = bytes.iter();
    while let Some(&b) = it.next() {
        if b == 0x83 {
            if let Some(&n) = it.next() {
                out.push(n ^ 0x20);
            }
        } else {
            out.push(b);
        }
    }
    out
}

/// The entries of a history file, oldest first; an entry of several lines
/// keeps them, joined by `\n`.
pub fn entries(bytes: &[u8], shell: Shell) -> Vec<String> {
    let bytes = if shell == Shell::Zsh { unmetafy(bytes) } else { bytes.to_vec() };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let goes_on = match shell {
        Shell::Pwsh => Some('`'),
        Shell::Zsh => Some('\\'),
        Shell::Bash => None,
    };
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line = match (shell, &cur) {
            // A new zsh entry may carry its time; a line going on does not.
            (Shell::Zsh, None) => zsh_untimed(line),
            // bash's time of the next command.
            (Shell::Bash, _)
                if line.len() > 1 && line.starts_with('#') && line[1..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                continue;
            }
            _ => line,
        };
        let (body, more) = match goes_on {
            Some(c) if line.ends_with(c) => (&line[..line.len() - c.len_utf8()], true),
            _ => (line, false),
        };
        match cur.as_mut() {
            Some(e) => {
                e.push('\n');
                e.push_str(body);
            }
            None => cur = Some(body.to_string()),
        }
        if !more {
            out.extend(cur.take().filter(|e| !e.trim().is_empty()));
        }
    }
    out.extend(cur.filter(|e| !e.trim().is_empty()));
    out
}

/// `: 1690000000:0;command` (zsh's EXTENDED_HISTORY) as `command`.
fn zsh_untimed(line: &str) -> &str {
    let Some(rest) = line.strip_prefix(": ") else { return line };
    let Some((stamp, command)) = rest.split_once(';') else { return line };
    let ok = stamp.split_once(':').is_some_and(|(a, b)| {
        !a.is_empty() && !b.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
    });
    if ok { command } else { line }
}

/// A message delivered as a command: its id, who sent it (a pane's name
/// as `%name`, else its address, or `user`), and the command after the
/// envelope.
#[derive(Debug, PartialEq)]
pub struct Delivered {
    pub id: u64,
    pub from: String,
    pub command: String,
}

/// The envelope at the start of `entry`, if it has one: PowerShell's
/// `<# … #> command`, a POSIX shell's `: '…'; command`, the header either
/// `[keepane id=… from=…]` or the JSON one of before 0.17.
pub fn delivered(entry: &str) -> Option<Delivered> {
    let (header, command) = match entry.strip_prefix("<# ") {
        Some(rest) => rest.split_once(" #> ")?,
        None => entry.strip_prefix(": '")?.split_once("'; ")?,
    };
    let (id, from, name) = if let Some(inner) = header.strip_prefix("[keepane ").and_then(|h| h.strip_suffix(']')) {
        let field = |k: &str| inner.split(' ').find_map(|f| f.strip_prefix(k)?.strip_prefix('=')).map(String::from);
        (field("id")?.parse().ok()?, field("from")?, field("name"))
    } else if header.starts_with("{\"keepane\"") {
        let v: serde_json::Value = serde_json::from_str(header).ok()?;
        (v["id"].as_u64()?, v["from"].as_str()?.to_string(), v["name"].as_str().map(String::from))
    } else {
        return None;
    };
    let from = name.map(|n| format!("%{n}")).unwrap_or(from);
    Some(Delivered { id, from, command: command.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_shells_entries() {
        let ps = b"git status\r\nif ($x) {`\r\n  ls`\r\n}\r\ncargo test\r\n";
        assert_eq!(entries(ps, Shell::Pwsh), ["git status", "if ($x) {\n  ls\n}", "cargo test"]);
        let bash = b"#1700000000\nls -l\n#1700000005\nmake\nplain\n";
        assert_eq!(entries(bash, Shell::Bash), ["ls -l", "make", "plain"]);
        let zsh = b": 1700000000:0;echo a\\\nb\n: 1700000001:2;ls\nno-stamp\n";
        assert_eq!(entries(zsh, Shell::Zsh), ["echo a\nb", "ls", "no-stamp"]);
        // zsh's metafied UTF-8: 0x83 then the byte xor 0x20.
        let meta: Vec<u8> =
            "echo ".bytes().chain([0xE4u8, 0xB8, 0xAD].iter().flat_map(|b| [0x83, b ^ 0x20])).chain(*b"\n").collect();
        assert_eq!(entries(&meta, Shell::Zsh), ["echo 中"]);
        // A byte-order mark and blank lines are not entries.
        assert_eq!(entries("\u{feff}a\r\n\r\nb".as_bytes(), Shell::Pwsh), ["a", "b"]);
        // A file cut in the middle of an entry still gives it.
        assert_eq!(entries(b"x`\r\ny`", Shell::Pwsh), ["x\ny"]);
    }

    #[test]
    fn a_shell_is_known_by_its_program_from_either_side() {
        assert_eq!(Shell::of_program("C:\\Program Files\\PowerShell\\7\\pwsh.exe"), Some(Shell::Pwsh));
        assert_eq!(Shell::of_program("powershell.EXE"), Some(Shell::Pwsh));
        assert_eq!(Shell::of_program("/usr/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::of_program("/bin/bash"), Some(Shell::Bash));
        assert_eq!(Shell::of_program("cmd.exe"), None);
        assert_eq!(Shell::of_program("claude"), None);
    }

    #[test]
    fn messages_are_told_by_their_envelope() {
        let fields =
            "<# [keepane id=12 task=3 from=$1:@3.%7 name=builder mode=ai to=$1:@4.%9 via=shell hop=1] #> cargo test";
        assert_eq!(
            delivered(fields),
            Some(Delivered { id: 12, from: "%builder".into(), command: "cargo test".into() })
        );
        let posix = ": '[keepane id=4 task=4 from=user to=$1:@3.%2 via=shell hop=0]'; make";
        assert_eq!(delivered(posix), Some(Delivered { id: 4, from: "user".into(), command: "make".into() }));
        let json = "<# {\"keepane\":1,\"id\":2,\"task\":2,\"from\":\"user\",\"to\":\"$1:@3.%2\",\"via\":\"shell\",\"hop\":0} #> Get-Item x";
        assert_eq!(delivered(json), Some(Delivered { id: 2, from: "user".into(), command: "Get-Item x".into() }));
        // A comment of one's own, or a damaged header, is a command.
        assert_eq!(delivered("<# my note #> ls"), None);
        assert_eq!(delivered("<# [keepane id=x from=user] #> ls"), None);
        assert_eq!(delivered(": 'just a note'; ls"), None);
        assert_eq!(delivered("git log"), None);
    }
}
