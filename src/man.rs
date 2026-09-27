//! keepane's manual page. It is written as Markdown (`docs/keepane.1.md`),
//! which people and programs read as it is (`keepane man`); `keepane man
//! --roff` gives it to `man` as roff, made here from what that page uses: headings, paragraphs,
//! definition lists (a line, then `: ` and what it means), indented code,
//! tables (as tagged paragraphs: no `tbl` needed), and `code`, *emphasis*
//! and <links> in the text. The same page and version give the same bytes.

/// The page, as written.
pub const MARKDOWN: &str = include_str!("../docs/keepane.1.md");

/// The page as roff, for `man`.
pub fn roff(md: &str) -> String {
    let mut out = String::new();
    let mut para: Vec<String> = Vec::new();
    let lines: Vec<&str> = md.lines().collect();
    let flush = |out: &mut String, para: &mut Vec<String>| {
        if !para.is_empty() {
            out.push_str(".PP\n");
            out.push_str(&inline(&para.join(" ")));
            out.push('\n');
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(title) = line.strip_prefix("# ") {
            let (name, section) = title.rsplit_once(' ').unwrap_or((title, "1"));
            out.push_str(&format!(
                ".TH {name} {section} \"\" \"keepane {}\" \"User Commands\"\n",
                env!("CARGO_PKG_VERSION")
            ));
            i += 1;
        } else if let Some(h) = line.strip_prefix("### ") {
            flush(&mut out, &mut para);
            out.push_str(&format!(".SS {}\n", inline(h)));
            i += 1;
        } else if let Some(h) = line.strip_prefix("## ") {
            flush(&mut out, &mut para);
            out.push_str(&format!(".SH {}\n", inline(h)));
            i += 1;
        } else if line.starts_with("    ") {
            flush(&mut out, &mut para);
            out.push_str(".PP\n.RS 4\n.nf\n");
            while i < lines.len() && (lines[i].starts_with("    ") || lines[i].is_empty()) {
                if lines[i].is_empty() && !lines.get(i + 1).is_some_and(|l| l.starts_with("    ")) {
                    break;
                }
                out.push_str(&literal(lines[i].strip_prefix("    ").unwrap_or_default()));
                out.push('\n');
                i += 1;
            }
            out.push_str(".fi\n.RE\n");
        } else if line.starts_with('|') {
            flush(&mut out, &mut para);
            // Header, rule, then a tagged paragraph a row.
            let mut rows = lines[i..].iter().take_while(|l| l.starts_with('|')).skip(2);
            let n = lines[i..].iter().take_while(|l| l.starts_with('|')).count();
            for row in rows.by_ref() {
                let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
                out.push_str(&format!(".TP\n{}\n{}\n", inline(cells[0]), inline(&cells[1..].join(" "))));
            }
            i += n;
        } else if line.is_empty() {
            flush(&mut out, &mut para);
            i += 1;
        } else if lines.get(i + 1).is_some_and(|l| l.starts_with(": ")) {
            // A term and what it means (continued on lines indented by two).
            flush(&mut out, &mut para);
            let mut def = vec![lines[i + 1][2..].to_string()];
            i += 2;
            while i < lines.len() && lines[i].starts_with("  ") && !lines[i].starts_with("    ") {
                def.push(lines[i].trim().to_string());
                i += 1;
            }
            out.push_str(&format!(".TP\n{}\n{}\n", inline(line), inline(&def.join(" "))));
        } else {
            para.push(line.trim().to_string());
            i += 1;
        }
    }
    flush(&mut out, &mut para);
    out
}

/// A line of text in roff: `code` bold, *emphasis* italic, <link> as its
/// address, and nothing that roff would take as its own.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut code = false;
    let mut em = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                out.push_str(if code { "\\fR" } else { "\\fB" });
                code = !code;
            }
            '*' if !code => {
                out.push_str(if em { "\\fR" } else { "\\fI" });
                em = !em;
            }
            '<' if !code && chars.peek().is_some_and(|n| n.is_ascii_alphabetic()) => {
                let link: String = chars.by_ref().take_while(|&n| n != '>').collect();
                out.push_str(&escape(&link, false));
            }
            _ => out.push_str(&escape(&c.to_string(), code)),
        }
    }
    if code || em {
        out.push_str("\\fR");
    }
    start_safe(out)
}

/// A line of code, as it is.
fn literal(text: &str) -> String {
    start_safe(escape(text, true))
}

/// Backslashes as roff's `\e`; in code, `-` as a minus sign, not a hyphen.
fn escape(text: &str, code: bool) -> String {
    let text = text.replace('\\', "\\e");
    if code { text.replace('-', "\\-") } else { text }
}

/// A line that begins with `.` or `'` would be a request to roff.
fn start_safe(line: String) -> String {
    if line.starts_with('.') || line.starts_with('\'') { format!("\\&{line}") } else { line }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command keepane takes is on the page: a command added without
    /// a word there fails here.
    #[test]
    fn every_command_is_on_the_page() {
        for c in crate::command::COMMANDS.iter().chain(crate::completion::LOCAL) {
            assert!(MARKDOWN.contains(&format!("`{c}`")), "{c} is not in docs/keepane.1.md");
        }
    }

    #[test]
    fn markdown_becomes_roff() {
        let md = "# KEEPANE 1\n\n## NAME\n\nkeepane - a *thing*\nthat `runs -x`.\n\n### Sub\n\n\
                  `-L` *name*\n: A server\n  of its own.\n\n    keepane new -d\n    .not a request\n\n\
                  | Key | Does |\n|---|---|\n| `c` | new window |\n\nSee <https://example.com> and a\\b.\n.dot\n";
        let r = roff(md);
        let want = format!(
            ".TH KEEPANE 1 \"\" \"keepane {}\" \"User Commands\"\n\
             .SH NAME\n.PP\nkeepane - a \\fIthing\\fR that \\fBruns \\-x\\fR.\n\
             .SS Sub\n\
             .TP\n\\fB\\-L\\fR \\fIname\\fR\nA server of its own.\n\
             .PP\n.RS 4\n.nf\nkeepane new \\-d\n\\&.not a request\n.fi\n.RE\n\
             .TP\n\\fBc\\fR\nnew window\n\
             .PP\nSee https://example.com and a\\eb. .dot\n",
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(r, want);
    }

    /// The real page: a title, its sections, no line roff would misread,
    /// and the same bytes every time.
    #[test]
    fn the_page_as_man_reads_it() {
        let r = roff(MARKDOWN);
        assert!(r.starts_with(".TH KEEPANE 1 "), "{}", &r[..60]);
        for s in ["NAME", "SYNOPSIS", "DESCRIPTION", "COMMANDS", "KEYS", "ENVIRONMENT", "FILES", "SEE ALSO"] {
            assert!(r.contains(&format!("\n.SH {s}\n")), "{s}");
        }
        for line in r.lines() {
            if line.starts_with('.') {
                let req = line.split(' ').next().unwrap();
                assert!(
                    [".TH", ".SH", ".SS", ".PP", ".TP", ".RS", ".RE", ".nf", ".fi"].contains(&req),
                    "not a request of ours: {line}"
                );
            }
            assert!(!line.contains('`'), "a backtick left: {line}");
        }
        assert_eq!(roff(MARKDOWN), r);
        // The page with LF endings and with CRLF ones (a Windows checkout
        // has either, as git is set): the same page.
        let lf = MARKDOWN.replace("\r\n", "\n");
        assert_eq!(roff(&lf), r);
        assert_eq!(roff(&lf.replace('\n', "\r\n")), r);
        assert!(!r.contains('\r'), "no carriage return reaches man");
    }
}
