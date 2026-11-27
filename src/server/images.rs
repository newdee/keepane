//! The pictures panes were sent (kitty.rs), on disk by their hash, and in
//! memory only while they are being sent (docs/design/kitty-graphics.md
//! 4.2, 11): `<sessions-dir>/images/<sha256>.<ending>`. The same picture
//! shown twice is kept once. A picture is read from disk when a terminal
//! that does not have it is to be sent it, and let go of `kitty::UNSEEN`
//! after it was last taken; what is on disk stays for a server restart.
//!
//! A picture just received is written only once it has lasted `SETTLE`
//! with a pane still naming it (`settle`): the frames of an animation or
//! a video, each replaced by the next under the same number, never reach
//! the disk, and leave memory as soon as nothing names them.

use super::kitty::{Image, Received, UNSEEN};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// How old a picture no pane and no saved file names may get before it goes.
const ORPHAN_AGE: Duration = Duration::from_secs(3600);
/// How long a picture just received must last, still named, to be written.
pub const SETTLE: Duration = Duration::from_secs(2);

/// Mark a picture's file as just shown: the disk limit lets go of the
/// pictures not shown longest first.
fn touch(path: &Path) {
    let _ = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now()));
}

/// One picture held in memory.
struct Held {
    bytes: Arc<Vec<u8>>,
    seen: Instant,
    /// Not on disk yet: its file ending, and when it came.
    unwritten: Option<(&'static str, Instant)>,
}

#[derive(Default)]
pub struct Store {
    held: HashMap<String, Held>,
    /// Files known to be there (or not), so a frame does not ask the disk.
    known: HashMap<String, bool>,
}

/// The file a picture is kept in.
pub fn file_of(dir: &Path, hash: &str, ending: &str) -> PathBuf {
    dir.join(format!("{hash}.{ending}"))
}

/// The pictures the saved files under `dir` name (the saved sessions, their
/// previous saves, the killed ones put aside): each `"hash":"…"` in them.
/// Read as text, so a save of any shape (an older keepane's too) counts.
pub fn named_in_saves(dir: &Path) -> HashSet<String> {
    let mut named = HashSet::new();
    let mut dirs = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if depth < 2 && name != "images" {
                    dirs.push((path, depth + 1));
                }
                continue;
            }
            if !name.contains(".json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            // `"hash": "…"` (a pretty save) or `"hash":"…"`.
            for (at, _) in text.match_indices("\"hash\"") {
                let rest = text[at + "\"hash\"".len()..].trim_start();
                let Some(rest) = rest.strip_prefix(':') else { continue };
                let Some(rest) = rest.trim_start().strip_prefix('"') else { continue };
                if let Some(hash) = rest.get(..64)
                    && is_hash(hash)
                {
                    named.insert(hash.to_string());
                }
            }
        }
    }
    named
}

/// Write a picture's file whole under another name, then rename it: a
/// reader never finds half a file.
fn write(dir: &Path, hash: &str, ending: &str, bytes: &[u8]) {
    let path = file_of(dir, hash, ending);
    if path.exists() {
        return;
    }
    let written = std::fs::create_dir_all(dir).and_then(|()| {
        let tmp = path.with_extension(format!("{ending}.part-{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    });
    if let Err(e) = written {
        log::warn!("picture {hash}: cannot keep it in {}: {e}", dir.display());
    }
}
/// A hash is 64 hex digits; anything else never names a file.
fn is_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Store {
    /// Keep a picture just received: in memory, as just seen; on disk once
    /// it has lasted (`settle`), unless it is there already.
    pub fn put(&mut self, dir: &Path, r: Received) {
        if !is_hash(&r.hash) {
            return;
        }
        let path = file_of(dir, &r.hash, r.format.ext());
        let now = Instant::now();
        let unwritten = if path.exists() {
            touch(&path);
            None
        } else {
            Some((r.format.ext(), now))
        };
        self.held.insert(r.hash, Held { bytes: Arc::new(r.bytes), seen: now, unwritten });
    }

    /// Every second: what no pane names leaves memory at once (never
    /// written, if it was not yet); what is still named after `SETTLE` is
    /// written; what was not taken for `UNSEEN` leaves memory (on disk it
    /// stays). What was known of the disk is asked again.
    pub fn settle(&mut self, dir: &Path, named: &HashSet<String>, now: Instant) {
        self.held
            .retain(|hash, h| named.contains(hash) && (h.unwritten.is_some() || now.duration_since(h.seen) < UNSEEN));
        for (hash, h) in &mut self.held {
            if let Some((ending, since)) = h.unwritten
                && now.duration_since(since) >= SETTLE
            {
                write(dir, hash, ending, &h.bytes);
                h.unwritten = None;
            }
        }
        self.known.clear();
    }

    /// Every picture `named` not written yet, written now: before a saved
    /// file names them (a restore reads them from disk).
    pub fn flush(&mut self, dir: &Path, named: &HashSet<String>) {
        for (hash, h) in &mut self.held {
            if let Some((ending, _)) = h.unwritten
                && named.contains(hash)
            {
                write(dir, hash, ending, &h.bytes);
                h.unwritten = None;
            }
        }
    }

    /// Whether a picture can be shown: in memory, or its file there.
    pub fn has(&mut self, dir: &Path, image: &Image) -> bool {
        if self.held.contains_key(&image.hash) {
            return true;
        }
        if !is_hash(&image.hash) {
            return false;
        }
        *self.known.entry(image.hash.clone()).or_insert_with(|| file_of(dir, &image.hash, image.format.ext()).is_file())
    }

    /// A picture's bytes, read from disk when not in memory; seen now.
    pub fn get(&mut self, dir: &Path, image: &Image) -> Option<Arc<Vec<u8>>> {
        let now = Instant::now();
        if let Some(h) = self.held.get_mut(&image.hash) {
            h.seen = now;
            return Some(Arc::clone(&h.bytes));
        }
        if !is_hash(&image.hash) {
            return None;
        }
        let path = file_of(dir, &image.hash, image.format.ext());
        match std::fs::read(&path) {
            Ok(bytes) => {
                touch(&path);
                let bytes = Arc::new(bytes);
                self.known.insert(image.hash.clone(), true);
                self.held.insert(image.hash.clone(), Held { bytes: Arc::clone(&bytes), seen: now, unwritten: None });
                Some(bytes)
            }
            Err(_) => {
                self.known.insert(image.hash.clone(), false);
                None
            }
        }
    }

    /// Clean the folder: a picture no pane and no saved file names goes
    /// once it is `ORPHAN_AGE` old (a pane being saved for the first time
    /// keeps what it was just sent); then, past `limit` bytes, the pictures
    /// not shown longest go, named or not (their cells show a label). Half
    /// written files left by a crash go too. How many files went.
    pub fn sweep_disk(&mut self, dir: &Path, named: &HashSet<String>, limit: u64, now: SystemTime) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        let mut kept: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut gone = 0;
        let old = |t: SystemTime| now.duration_since(t).is_ok_and(|age| age >= ORPHAN_AGE);
        for e in rd.flatten() {
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let when = meta.modified().unwrap_or(now);
            let hash = name.split('.').next().unwrap_or("").to_string();
            if !is_hash(&hash) {
                continue; // not one of ours
            }
            let part = name.contains(".part-");
            if part || !named.contains(&hash) {
                // Half written, or named by nothing: gone once old enough.
                if old(when) {
                    if std::fs::remove_file(&path).is_ok() {
                        gone += 1;
                        if !part {
                            self.forget(&hash);
                        }
                    }
                    continue;
                }
                if part {
                    continue;
                }
            }
            kept.push((when, meta.len(), path));
        }
        let mut total: u64 = kept.iter().map(|k| k.1).sum();
        kept.sort_by_key(|k| k.0);
        for (_, len, path) in kept {
            if total <= limit {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                gone += 1;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.forget(name.split('.').next().unwrap_or(""));
            }
        }
        gone
    }

    fn forget(&mut self, hash: &str) {
        self.held.remove(hash);
        self.known.remove(hash);
    }

    /// How many pictures are in memory, and their bytes.
    #[cfg(test)]
    pub fn held(&self) -> (usize, usize) {
        (self.held.len(), self.held.values().map(|h| h.bytes.len()).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::kitty::Format;

    fn received(bytes: &[u8]) -> Received {
        use sha2::Digest as _;
        let hash: String = sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        Received { hash, format: Format::Png, bytes: bytes.to_vec() }
    }

    fn image_of(r: &Received) -> Image {
        Image { hash: r.hash.clone(), format: r.format, width: 1, height: 1, cells: None }
    }

    #[test]
    fn a_picture_is_kept_once_and_read_back_after_it_was_let_go() {
        let dir = std::env::temp_dir().join(format!("kp-images-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        let r = received(b"\x89PNG one picture");
        let image = image_of(&r);
        let named = HashSet::from([r.hash.clone()]);
        store.put(&dir, received(b"\x89PNG one picture"));
        store.put(&dir, r);
        assert!(!dir.exists(), "not on disk until it has lasted");
        assert!(store.has(&dir, &image), "shown from memory meanwhile");
        let now = Instant::now();
        store.settle(&dir, &named, now + SETTLE);
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(files.len(), 1, "the same picture twice is one file: {files:?}");
        assert_eq!(store.held().0, 1);
        // Not taken for longer than UNSEEN: out of memory, still on disk.
        store.settle(&dir, &named, now + SETTLE + UNSEEN);
        assert_eq!(store.held(), (0, 0));
        assert!(store.has(&dir, &image));
        assert_eq!(store.get(&dir, &image).as_deref().map(|v| v.as_slice()), Some(&b"\x89PNG one picture"[..]));
        assert_eq!(store.held().0, 1, "read back when drawn again");
        // A file gone (cleared by hand): not shown, said so.
        store.settle(&dir, &named, Instant::now() + UNSEEN);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!store.has(&dir, &image));
        assert!(store.get(&dir, &image).is_none());
    }

    #[test]
    fn the_folder_keeps_what_is_named_and_what_is_new_and_fits_its_limit() {
        let dir = std::env::temp_dir().join(format!("kp-images-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        let (a, b, c) = (received(&[1; 100]), received(&[2; 100]), received(&[3; 100]));
        let (ha, hb, hc) = (a.hash.clone(), b.hash.clone(), c.hash.clone());
        let all = HashSet::from([ha.clone(), hb.clone(), hc.clone()]);
        for r in [a, b, c] {
            store.put(&dir, r);
        }
        store.flush(&dir, &all);
        std::fs::write(dir.join(format!("{ha}.png.part-1")), b"half").unwrap();
        std::fs::write(dir.join("notes.txt"), b"not ours").unwrap();
        let now = SystemTime::now();
        // Only a is named; nothing is old yet: everything stays.
        let named = HashSet::from([ha.clone()]);
        assert_eq!(store.sweep_disk(&dir, &named, 1 << 30, now), 0);
        // An hour on: b and c (named by nothing) and the half file go; a stays.
        let later = now + ORPHAN_AGE + Duration::from_secs(1);
        assert_eq!(store.sweep_disk(&dir, &named, 1 << 30, later), 3);
        let left: HashSet<String> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, HashSet::from([format!("{ha}.png"), "notes.txt".into()]), "{hb} {hc}");
        // Past the limit, even a named one goes (its cells show a label).
        assert_eq!(store.sweep_disk(&dir, &named, 10, later), 1);
        assert!(!dir.join(format!("{ha}.png")).exists() && dir.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_frames_of_an_animation_never_reach_the_disk() {
        let dir = std::env::temp_dir().join(format!("kp-images-frames-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = Store::default();
        let now = Instant::now();
        // Thirty frames under one number: each replaces the last in the pane,
        // which names only the newest.
        let mut last = String::new();
        for i in 0..30u8 {
            let r = received(&[i; 1000]);
            last = r.hash.clone();
            store.put(&dir, r);
            store.settle(&dir, &HashSet::from([last.clone()]), now);
        }
        assert_eq!(store.held().0, 1, "only the frame still named is in memory");
        assert!(!dir.exists(), "no frame written yet");
        store.settle(&dir, &HashSet::from([last.clone()]), Instant::now() + SETTLE);
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(files.len(), 1, "the last frame, once it lasted: {files:?}");
        // The server going: a picture still named and not written yet is.
        let r = received(b"just now");
        let hash = r.hash.clone();
        store.put(&dir, r);
        store.flush(&dir, &HashSet::from([hash.clone()]));
        assert!(dir.join(format!("{hash}.png")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_name_their_pictures_however_they_are_written() {
        let dir = std::env::temp_dir().join(format!("kp-images-saves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("killed")).unwrap();
        let (h1, h2, h3) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        std::fs::write(dir.join("work-1.json"), format!("{{\n  \"hash\": \"{h1}\",\n  \"x\": 1\n}}")).unwrap();
        std::fs::write(dir.join("work-1.json.prev"), format!("{{\"hash\":\"{h2}\"}}")).unwrap();
        std::fs::write(dir.join("killed").join("k.json"), format!("{{\"hash\" : \"{h3}\"}}")).unwrap();
        std::fs::create_dir_all(dir.join("images")).unwrap();
        std::fs::write(dir.join("images").join("x.json"), format!("{{\"hash\":\"{}\"}}", "d".repeat(64))).unwrap();
        let named = named_in_saves(&dir);
        assert_eq!(named, HashSet::from([h1, h2, h3]), "not what lies in images/");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_that_is_no_hash_never_reaches_the_disk() {
        let dir = std::env::temp_dir().join(format!("kp-images-bad-{}", std::process::id()));
        let mut store = Store::default();
        let mut r = received(b"x");
        r.hash = "../../escape".into();
        let image = image_of(&r);
        store.put(&dir, r);
        assert!(!dir.exists());
        assert!(!store.has(&dir, &image) && store.get(&dir, &image).is_none());
    }
}
