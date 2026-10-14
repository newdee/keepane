use super::*;

#[test]
fn crc_is_the_zip_one() {
    let crc = |b: &[u8]| {
        let mut c = Crc::default();
        c.update(b);
        c.value()
    };
    assert_eq!(crc(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc(b""), 0);
    // A piece at a time, the same.
    let mut c = Crc::default();
    c.update(b"1234");
    c.update(b"56789");
    assert_eq!(c.value(), 0xCBF4_3926);
}

#[test]
fn times_for_how_long() {
    let d = |s| Some(Duration::from_secs(s));
    assert_eq!(parse_for("0"), Ok(None));
    assert_eq!(parse_for("90s"), Ok(d(90)));
    assert_eq!(parse_for("30m"), Ok(d(1800)));
    assert_eq!(parse_for("2h"), Ok(d(7200)));
    assert_eq!(parse_for("7d"), Ok(d(7 * 86400)));
    assert_eq!(parse_for("365d"), Ok(Some(LONGEST)));
    for bad in ["", "h", "2", "2x", "-1h", "1.5h", "0s", "0h", "2 h", "99999999999999999999d"] {
        assert!(parse_for(bad).is_err(), "{bad}");
    }
    assert!(parse_for("366d").unwrap_err().contains("longer than 365d"));
    assert_eq!(for_text(DEFAULT_FOR), "1d");
    assert_eq!(for_text(Duration::from_secs(7200)), "2h");
    assert_eq!(for_text(Duration::from_secs(90)), "90s");
    assert_eq!(for_text(Duration::from_secs(1800)), "30m");
}

#[test]
fn tokens_are_web_keys() {
    assert!(valid_token(&crate::web::new_key().unwrap()));
    assert!(!valid_token("short"));
    assert!(!valid_token("aaaaaaaaaaaaaaaaaaaa/a"));
    assert!(!valid_token("aaaaaaaaaaaaaaaaaaaa..a"));
}

#[test]
fn ranges_as_browsers_ask() {
    let s = |h: &str, len| span(Some(h), len);
    assert_eq!(span(None, 10), Span::Whole);
    assert_eq!(s("bytes=0-", 10), Span::Part(0, 9));
    assert_eq!(s("bytes=4-", 10), Span::Part(4, 9));
    assert_eq!(s("bytes=2-5", 10), Span::Part(2, 5));
    assert_eq!(s("bytes=2-500", 10), Span::Part(2, 9), "past the end: to the end");
    assert_eq!(s("bytes=-3", 10), Span::Part(7, 9));
    assert_eq!(s("bytes=-30", 10), Span::Part(0, 9));
    assert_eq!(s("bytes=10-", 10), Span::Outside);
    assert_eq!(s("bytes=-0", 10), Span::Outside);
    assert_eq!(s("bytes=0-", 0), Span::Outside);
    assert_eq!(s("bytes=-5", 0), Span::Outside);
    // Not understood: all of it.
    for whole in ["bytes=5-2", "bytes=0-1,4-5", "items=0-1", "bytes=x-", "bytes=-", "bytes=+1-2", "bytes=1"] {
        assert_eq!(s(whole, 10), Span::Whole, "{whole}");
    }
}

#[test]
fn download_names_cannot_break_the_header() {
    let d = disposition("报告 \"v2\".pdf");
    assert_eq!(d, "attachment; filename=\"__ _v2_.pdf\"; filename*=UTF-8''%E6%8A%A5%E5%91%8A%20%22v2%22.pdf");
    let d = disposition("a\r\nSet-Cookie: x");
    assert!(!d.contains('\r') && !d.contains('\n'), "{d}");
    assert_eq!(disposition("plain.txt"), "attachment; filename=\"plain.txt\"; filename*=UTF-8''plain.txt");
}

#[test]
fn dos_times() {
    use chrono::TimeZone;
    assert_eq!(dos_time(SystemTime::UNIX_EPOCH), (0, (1 << 5) | 1), "before 1980: its first day");
    let t: SystemTime = chrono::Local.with_ymd_and_hms(2024, 5, 6, 7, 8, 11).unwrap().into();
    assert_eq!(dos_time(t), ((7 << 11) | (8 << 5) | 5, (44 << 9) | (5 << 5) | 6));
}

/// An entry as a reader finds it.
#[derive(Debug)]
struct Found {
    name: String,
    dir: bool,
    data: Vec<u8>,
}

fn u16_at(z: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(z[i..i + 2].try_into().unwrap())
}
fn u32_at(z: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(z[i..i + 4].try_into().unwrap())
}
fn u64_at(z: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(z[i..i + 8].try_into().unwrap())
}

/// A zip read as a reader that trusts nothing would: from the end record
/// (and the ZIP64 one) to the central directory, each entry's local header,
/// its data and data descriptor; every CRC, size and offset checked, and
/// the entries back to back with the central directory right after them.
fn read_zip(z: &[u8], want64: bool) -> Vec<Found> {
    let eocd = z.len() - 22;
    assert_eq!(u32_at(z, eocd), 0x0605_4b50, "end record last, no comment");
    let (mut count, mut cd_size, mut cd_at) =
        (u16_at(z, eocd + 10) as u64, u32_at(z, eocd + 12) as u64, u32_at(z, eocd + 16) as u64);
    let has64 = eocd >= 20 && u32_at(z, eocd - 20) == 0x0706_4b50;
    assert_eq!(has64, want64);
    if has64 {
        let at = u64_at(z, eocd - 20 + 8) as usize;
        assert_eq!(u32_at(z, at), 0x0606_4b50);
        assert_eq!(u64_at(z, at + 4), 44);
        count = u64_at(z, at + 32);
        cd_size = u64_at(z, at + 40);
        cd_at = u64_at(z, at + 48);
        assert_eq!(at as u64, cd_at + cd_size, "ZIP64 end right after the central directory");
    } else {
        assert_eq!(eocd as u64, cd_at + cd_size);
    }
    let mut out = Vec::new();
    let mut i = cd_at as usize;
    let mut next_local = 0u64;
    for _ in 0..count {
        assert_eq!(u32_at(z, i), 0x0201_4b50);
        let flags = u16_at(z, i + 8);
        assert_eq!(u16_at(z, i + 10), 0, "stored");
        let crc = u32_at(z, i + 16);
        let (mut size, size2) = (u32_at(z, i + 20) as u64, u32_at(z, i + 24) as u64);
        assert_eq!(size, size2, "stored: as big packed as not");
        let (n, x, k) = (u16_at(z, i + 28) as usize, u16_at(z, i + 30) as usize, u16_at(z, i + 32) as usize);
        let attrs = u32_at(z, i + 38);
        let mut offset = u32_at(z, i + 42) as u64;
        let name = String::from_utf8(z[i + 46..i + 46 + n].to_vec()).unwrap();
        let mut e = i + 46 + n;
        let extra_end = e + x;
        while e < extra_end {
            let (id, len) = (u16_at(z, e), u16_at(z, e + 2) as usize);
            if id == 1 {
                let mut f = e + 4;
                if size == 0xFFFF_FFFF {
                    size = u64_at(z, f);
                    assert_eq!(u64_at(z, f + 8), size);
                    f += 16;
                }
                if offset == 0xFFFF_FFFF {
                    offset = u64_at(z, f);
                }
            }
            e += 4 + len;
        }
        assert!(want64 || x == 0, "{name}: ZIP64 where none is needed");
        assert_eq!(flags & 0x0800, 0x0800, "UTF-8 names");
        assert_eq!(offset, next_local, "{name}: right after the one before");
        // The local header.
        let l = offset as usize;
        assert_eq!(u32_at(z, l), 0x0403_4b50);
        assert_eq!(u16_at(z, l + 6), flags);
        let (ln, lx) = (u16_at(z, l + 26) as usize, u16_at(z, l + 28) as usize);
        assert_eq!(&z[l + 30..l + 30 + ln], name.as_bytes());
        let local64 = lx > 0;
        let data_at = l + 30 + ln + lx;
        let data = z[data_at..data_at + size as usize].to_vec();
        let mut c = Crc::default();
        c.update(&data);
        assert_eq!(c.value(), crc, "{name}: CRC");
        let dir = name.ends_with('/');
        assert_eq!(attrs & 0x10 != 0, dir);
        let mut end = data_at + size as usize;
        if flags & 0x0008 != 0 {
            assert!(!dir);
            assert_eq!(u32_at(z, end), 0x0807_4b50);
            assert_eq!(u32_at(z, end + 4), crc);
            if local64 {
                assert_eq!((u64_at(z, end + 8), u64_at(z, end + 16)), (size, size));
                end += 24;
            } else {
                assert_eq!((u32_at(z, end + 8) as u64, u32_at(z, end + 12) as u64), (size, size));
                end += 16;
            }
        } else {
            assert!(dir && size == 0);
        }
        next_local = end as u64;
        out.push(Found { name, dir, data });
        i += 46 + n + x + k;
    }
    assert_eq!(next_local, cd_at, "the central directory right after the entries");
    assert_eq!(i as u64, cd_at + cd_size);
    out
}

fn tree(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("keepane-share-{tag}-{}", std::process::id())).join("proj");
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    std::fs::write(root.join("a.txt"), "hello").unwrap();
    std::fs::write(root.join("empty.txt"), "").unwrap();
    std::fs::write(root.join("中文.txt"), "你好").unwrap();
    let big: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    std::fs::write(root.join("sub/b.bin"), &big).unwrap();
    std::fs::create_dir_all(root.join("zz")).unwrap();
    std::fs::write(root.join("zz/c.txt"), "last").unwrap();
    root
}

#[tokio::test]
async fn a_folder_is_a_zip_a_reader_takes() {
    let root = tree("zip");
    // A link inside: left out (where this machine lets one be made).
    #[cfg(unix)]
    let links = u64::from(std::os::unix::fs::symlink(root.join("a.txt"), root.join("link.txt")).is_ok());
    #[cfg(windows)]
    let links = u64::from(std::os::windows::fs::symlink_file(root.join("a.txt"), root.join("link.txt")).is_ok());
    for all64 in [false, true] {
        let mut z = Vec::new();
        let (files, left_out) = write_zip(&mut z, &root, "proj", all64).await.unwrap();
        assert_eq!((files, left_out), (5, links));
        let found = read_zip(&z, all64);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "proj/",
                "proj/a.txt",
                "proj/empty.txt",
                "proj/sub/",
                "proj/zz/",
                "proj/中文.txt",
                "proj/sub/b.bin",
                "proj/sub/deeper/",
                "proj/zz/c.txt"
            ]
        );
        for f in found.iter().filter(|f| !f.dir) {
            let on_disk = std::fs::read(root.join(f.name.strip_prefix("proj/").unwrap())).unwrap();
            assert_eq!(f.data, on_disk, "{}", f.name);
        }
        // The same folder, the same bytes (times included).
        let mut again = Vec::new();
        write_zip(&mut again, &root, "proj", all64).await.unwrap();
        assert_eq!(again, z);
    }
    let _ = std::fs::remove_dir_all(root.parent().unwrap());
}

#[tokio::test]
async fn an_empty_folder_is_a_zip_of_one_entry() {
    let dir = std::env::temp_dir().join(format!("keepane-share-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut z = Vec::new();
    assert_eq!(write_zip(&mut z, &dir, "e", false).await.unwrap(), (0, 0));
    let found = read_zip(&z, false);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "e/");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 65535 entries or more: the count in the ZIP64 end; fewer, not.
#[test]
fn many_entries_take_a_zip64_end() {
    for (n, want64) in [(0xFFFEu32, false), (0xFFFF, true)] {
        let mut z = Zip::new(false);
        let mut bytes = Vec::new();
        for i in 0..n {
            bytes.extend(z.folder(&format!("{i}/"), (0, 0)));
        }
        bytes.extend(z.finish());
        assert_eq!(read_zip(&bytes, want64).len(), n as usize);
    }
}

/// What does not fit 32 bits: the ZIP64 forms, decided by the size, the
/// offset and the count, each on its own.
#[test]
fn big_sizes_and_offsets_take_zip64() {
    let mut z = Zip::new(false);
    let h = z.begin("big", (0, 0), MAX32);
    assert_eq!(u16_at(&h, 4), 45, "version 4.5");
    assert_eq!((u32_at(&h, 18), u32_at(&h, 22)), (u32::MAX, u32::MAX));
    assert_eq!(u16_at(&h, 28), 20, "the ZIP64 extra");
    let d = z.end(7, MAX32);
    assert_eq!(d.len(), 24, "64-bit sizes in the descriptor");
    // An entry after 4 GiB: its offset in ZIP64, its sizes not.
    let h = z.begin("small", (0, 0), 3);
    assert_eq!(u16_at(&h, 28), 0);
    assert_eq!(z.end(0, 3).len(), 16);
    let end = z.finish();
    // The second entry in the central directory: offset 0xFFFFFFFF, an
    // 8-byte extra; then the ZIP64 end (the directory starts past 4 GiB).
    let second = 46 + 3 + 20;
    assert_eq!(u32_at(&end, second + 42), u32::MAX);
    assert_eq!(u16_at(&end, second + 30), 12);
    assert_eq!(u32_at(&end, end.len() - 22 - 20), 0x0706_4b50);
}
