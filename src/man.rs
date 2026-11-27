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

    /// The version in Cargo.toml has its release notes, written by hand
    /// (`docs/releases/vX.md`, which the Release workflow publishes): what
    /// it changed, a line each, never a bare Full Changelog link. A version
    /// bumped without them fails here, before it is tagged.
    #[test]
    fn every_version_has_its_release_notes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("releases")
            .join(format!("v{}.md", env!("CARGO_PKG_VERSION")));
        let notes = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}: write what this version changed", path.display()));
        let changes = notes.lines().filter(|l| l.starts_with("- ")).count();
        assert!(changes > 0, "{} lists no change", path.display());
    }

    /// Every command keepane takes is on the page: a command added without
    /// a word there fails here.
    #[test]
    fn every_command_is_on_the_page() {
        for c in crate::command::COMMANDS.iter().chain(crate::completion::LOCAL) {
            assert!(MARKDOWN.contains(&format!("`{c}`")), "{c} is not in docs/keepane.1.md");
        }
    }

    /// Every option is on the page, in "Every option": one added without a
    /// line there fails here.
    #[test]
    fn every_option_is_on_the_page() {
        let table = &MARKDOWN[MARKDOWN.find("### Every option").expect("the table")..];
        let missing: Vec<&str> =
            crate::config::KNOWN.iter().copied().filter(|o| !table.contains(&format!("| `{o}` |"))).collect();
        assert!(missing.is_empty(), "not in docs/keepane.1.md's Every option: {missing:?}");
    }

    /// Every `KEEPANE_*` variable the code reads (as `"KEEPANE_…"` in
    /// `src/`) is under ENVIRONMENT.
    #[test]
    fn every_environment_variable_is_on_the_page() {
        let env = &MARKDOWN[MARKDOWN.find("## ENVIRONMENT").expect("the section")..];
        let env = &env[..env.find("\n## ").unwrap_or(env.len())];
        let mut read = std::collections::BTreeSet::new();
        let mut dirs = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for part in text.split("var_os(\"KEEPANE_").skip(1).chain(text.split("var(\"KEEPANE_").skip(1)) {
                        let name: String = part.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').collect();
                        read.insert(format!("KEEPANE_{name}"));
                    }
                }
            }
        }
        assert!(read.len() > 10, "{read:?}");
        // keepane's own, set for itself (a restart run detached, a replay
        // not waited on) or for the tests: not for anyone to set.
        const INTERNAL: [&str; 3] = ["KEEPANE_EXE", "KEEPANE_REPLAY_NO_WAIT", "KEEPANE_RESTART_DETACHED"];
        let missing: Vec<&String> = read
            .iter()
            .filter(|v| !INTERNAL.contains(&v.as_str()) && !v.starts_with("KEEPANE_TEST_"))
            .filter(|v| !env.contains(&format!("`{v}`")))
            .collect();
        assert!(missing.is_empty(), "not under ENVIRONMENT in docs/keepane.1.md: {missing:?}");
    }

    /// Every format variable (each name `Context::var` answers, as written
    /// in src/format.rs) is in "Every format variable".
    #[test]
    fn every_format_variable_is_on_the_page() {
        let src = include_str!("format.rs");
        let body = &src[src.find("pub fn var(").expect("var")..];
        let body = &body[..body.find("_ => return None").expect("its end")];
        let table = &MARKDOWN[MARKDOWN.find("### Every format variable").expect("the table")..];
        let names: Vec<&str> = body
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|n| n.len() > 1 && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .collect();
        assert!(names.len() > 100, "{names:?}");
        let missing: Vec<&&str> = names.iter().filter(|n| !table.contains(&format!("`{n}`"))).collect();
        assert!(missing.is_empty(), "not in docs/keepane.1.md's Every format variable: {missing:?}");
    }

    /// Every MCP tool is on the page.
    #[test]
    fn every_mcp_tool_is_on_the_page() {
        let missing: Vec<String> =
            crate::mcp::tool_names().into_iter().filter(|t| !MARKDOWN.contains(&format!("`{t}`"))).collect();
        assert!(missing.is_empty(), "not in docs/keepane.1.md: {missing:?}");
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
