//! A file shown on the page (a group file, a file shared): what it is is
//! judged by its first bytes, never by its name, and only pictures, PDF and
//! UTF-8 text are given; the page shows a picture from a blob, text as text
//! (docs/design/chat.md §13). Nothing given here is run: `nosniff` and a
//! sandboxing policy go with each.

use super::Response;
use std::path::Path;
use tokio::io::AsyncReadExt;

/// The most of a text file shown: the rest is said, not sent.
pub const TEXT_MOST: usize = 256 * 1024;
/// The largest picture or PDF shown (read whole into memory): a bigger one
/// is saved, not shown (a share can be any size).
pub const SHOW_MOST: u64 = 50 * 1024 * 1024;
/// How much of a file says whether it is text (a NUL in it: not).
const SNIFF: usize = 8 * 1024;
/// What goes with each preview: nothing in it runs, nothing loads.
const POLICY: &str = "sandbox; default-src 'none'";
/// The header that says a text was cut, with the whole file's size.
pub const CUT_HEADER: &str = "X-Keepane-Cut";

/// What a file is, by its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture(&'static str),
    Pdf,
    Text,
}

/// What `head` (a file's first bytes, all of it when `whole`) is: a
/// picture, a PDF, UTF-8 text, or nothing shown.
pub fn kind(head: &[u8], whole: bool) -> Option<Kind> {
    let starts = |magic: &[u8]| head.starts_with(magic);
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some(Kind::Picture("image/png"));
    }
    if starts(b"\xff\xd8\xff") {
        return Some(Kind::Picture("image/jpeg"));
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some(Kind::Picture("image/gif"));
    }
    if starts(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        return Some(Kind::Picture("image/webp"));
    }
    if starts(b"%PDF-") {
        return Some(Kind::Pdf);
    }
    if head[..head.len().min(SNIFF)].contains(&0) {
        return None;
    }
    match std::str::from_utf8(head) {
        Ok(_) => Some(Kind::Text),
        // Cut inside a character at the end of what was read: text still.
        Err(e) if e.error_len().is_none() && !whole => Some(Kind::Text),
        Err(_) => None,
    }
}

/// The file at `path`, to be shown: a picture or a PDF whole, text up to
/// `TEXT_MOST` (`CUT_HEADER` says the whole size when cut), else 415.
pub async fn file(path: &Path) -> Response {
    let Ok(mut f) = tokio::fs::File::open(path).await else {
        return Response::text(404, "no such file (deleted from the group files, or moved)");
    };
    let size = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut head = Vec::new();
    if let Err(e) = (&mut f).take(TEXT_MOST as u64).read_to_end(&mut head).await {
        return Response::text(500, &format!("{}: {e}", path.display()));
    }
    let whole = head.len() as u64 >= size;
    let mut r = match kind(&head, whole) {
        Some(Kind::Text) => {
            let ok = match std::str::from_utf8(&head) {
                Ok(_) => head.len(),
                Err(e) => e.valid_up_to(),
            };
            head.truncate(ok);
            let mut r = Response::new(200, "text/plain; charset=utf-8", head);
            if !whole {
                r.headers.push((CUT_HEADER, size.to_string()));
            }
            r
        }
        Some(_) if size > SHOW_MOST => {
            return Response::text(415, "no preview: larger than 50 MiB (save it to see it)");
        }
        Some(k) => {
            if let Err(e) = f.read_to_end(&mut head).await {
                return Response::text(500, &format!("{}: {e}", path.display()));
            }
            let mime = match k {
                Kind::Picture(m) => m,
                _ => "application/pdf",
            };
            Response::new(200, mime, head)
        }
        None => return Response::text(415, "no preview: not a picture, a PDF or UTF-8 text"),
    };
    r.headers.push(("Content-Security-Policy", POLICY.into()));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_what_its_first_bytes_say_not_its_name() {
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n\0\0", true), Some(Kind::Picture("image/png")));
        assert_eq!(kind(b"\xff\xd8\xff\xe0\0", true), Some(Kind::Picture("image/jpeg")));
        assert_eq!(kind(b"GIF89a\0", true), Some(Kind::Picture("image/gif")));
        assert_eq!(kind(b"RIFF\x10\0\0\0WEBPVP8 ", true), Some(Kind::Picture("image/webp")));
        assert_eq!(kind(b"RIFF\x10\0\0\0WAVEfmt ", true), None, "a sound is no picture");
        assert_eq!(kind(b"%PDF-1.7\n", true), Some(Kind::Pdf));
        assert_eq!(kind("报告\n第二行".as_bytes(), true), Some(Kind::Text));
        assert_eq!(kind(b"", true), Some(Kind::Text), "empty: text, no lines");
        // An SVG or a page is text: shown as text, never drawn.
        assert_eq!(kind(b"<svg onload=alert(1)>", true), Some(Kind::Text));
        assert_eq!(kind(b"<html><script>x</script>", true), Some(Kind::Text));
        assert_eq!(kind(b"MZ\x90\0\x03", true), None, "a program");
        assert_eq!(kind(b"\xb1\xa8\xb8\xe6", true), None, "GBK: not UTF-8");
        // Cut inside a character: text when there is more, not when whole.
        let cut = &"报".as_bytes()[..2];
        assert_eq!(kind(cut, false), Some(Kind::Text));
        assert_eq!(kind(cut, true), None);
    }

    #[tokio::test]
    async fn a_preview_carries_its_type_its_policy_and_at_most_the_text_shown() {
        let dir = std::env::temp_dir().join(format!("keepane-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let header = |r: &Response, k: &str| r.headers.iter().find(|(h, _)| *h == k).map(|(_, v)| v.clone());
        // A long text whose cut falls inside a character (two bytes, then
        // three-byte ones: 256 KiB - 2 is no multiple of 3): cut before it.
        let long = format!("ab{}", "报".repeat(TEXT_MOST / 3 + 10));
        assert_ne!((TEXT_MOST - 2) % 3, 0);
        std::fs::write(dir.join("long.txt"), &long).unwrap();
        let r = file(&dir.join("long.txt")).await;
        assert_eq!((r.status, r.content_type), (200, "text/plain; charset=utf-8"));
        assert!(r.body.len() < TEXT_MOST && std::str::from_utf8(&r.body).is_ok(), "{}", r.body.len());
        assert_eq!(header(&r, CUT_HEADER), Some(long.len().to_string()));
        assert_eq!(header(&r, "Content-Security-Policy").as_deref(), Some(POLICY));
        // A picture named as text: a picture, whole.
        let png: Vec<u8> = b"\x89PNG\r\n\x1a\n".iter().copied().chain(std::iter::repeat_n(7, TEXT_MOST + 5)).collect();
        std::fs::write(dir.join("not.txt"), &png).unwrap();
        let r = file(&dir.join("not.txt")).await;
        assert_eq!((r.status, r.content_type, r.body.len()), (200, "image/png", png.len()));
        assert_eq!(header(&r, CUT_HEADER), None);
        // A short text: whole, nothing said of a cut.
        std::fs::write(dir.join("a.md"), "# a\n").unwrap();
        let r = file(&dir.join("a.md")).await;
        assert_eq!((r.status, r.body.as_slice(), header(&r, CUT_HEADER)), (200, &b"# a\n"[..], None));
        // A program: no preview; none there: 404.
        std::fs::write(dir.join("x.png"), b"MZ\x90\0").unwrap();
        assert_eq!(file(&dir.join("x.png")).await.status, 415);
        assert_eq!(file(&dir.join("gone")).await.status, 404);
        // A picture too big to read whole: not shown (nothing read of it but
        // its start; the file is sparse, nothing written but the start).
        let big = std::fs::File::create(dir.join("big.png")).unwrap();
        std::io::Write::write_all(&mut &big, b"\x89PNG\r\n\x1a\n").unwrap();
        big.set_len(SHOW_MOST + 1).unwrap();
        drop(big);
        let r = file(&dir.join("big.png")).await;
        assert_eq!(r.status, 415);
        assert!(String::from_utf8_lossy(&r.body).contains("larger than 50 MiB"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
