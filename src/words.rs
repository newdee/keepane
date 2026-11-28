//! A command line as words: what `command` parses keepane's commands from,
//! and what `config` reads a theme's `set -g` lines with. On its own, so the
//! options do not need the commands to read a line.

/// Split a command line into words, honouring double/single quotes and
/// backslash escapes. A `#` at the start of a word begins a comment.
///
/// Backslashes are only escapes before a quote, another backslash,
/// whitespace, `#` or `;`, so Windows paths like `C:\src` survive unquoted.
pub fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '#' if !in_word => break,
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err("unterminated double quote".into()),
                        Some('"') => break,
                        Some('\\') => match chars.peek() {
                            Some(&e @ ('"' | '\\')) => {
                                chars.next();
                                cur.push(e);
                            }
                            _ => cur.push('\\'),
                        },
                        Some(o) => cur.push(o),
                    }
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return Err("unterminated single quote".into()),
                        Some('\'') => break,
                        Some(o) => cur.push(o),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.peek() {
                    Some(&e) if e == '"' || e == '\'' || e == '\\' || e == '#' || e == ';' || e.is_whitespace() => {
                        chars.next();
                        cur.push(e);
                    }
                    _ => cur.push('\\'),
                }
            }
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}
