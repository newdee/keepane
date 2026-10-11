//! What the status line can show about the machine and the pane's program
//! without running anything: CPU and memory in use, the battery, the time
//! since boot, the git branch of a directory, and the program a pane is
//! running right now. All of it is a system call or a small file read, so
//! the status line can ask every second. The machine and the processes are
//! the platform's (`platform::sysinfo`); the rest is here, with what the
//! clock's animation moves by: the machine's running totals (`counters`)
//! and the load two of them give (`load`).

use std::path::{Path, PathBuf};

pub use crate::platform::sysinfo::{System, counters, hostname, program_of, short_path, system, tree_of, usage_of};

/// What the machine has done since it started, as running totals, read as
/// often as the clock's animation asks (`counters`): two readings apart in
/// time give a `Load`. A figure the system cannot give is None (no cores:
/// none).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counters {
    /// (busy, total) time of each core, in the system's own ticks.
    pub cores: Vec<(u64, u64)>,
    /// (total, available) bytes of memory.
    pub memory: Option<(u64, u64)>,
    /// Bytes the disks have read and written.
    pub disk: Option<(u64, u64)>,
    /// Bytes the network has received and sent, loopback left out.
    pub net: Option<(u64, u64)>,
    /// The CPU's temperature in °C, from a real sensor only.
    pub temp: Option<f32>,
    /// Context switches and interrupts since boot.
    pub switches: Option<u64>,
    pub interrupts: Option<u64>,
    /// Processes and threads running now.
    pub processes: Option<u64>,
    pub threads: Option<u64>,
    /// (total, free) swap (the paging files on Windows), in the system's
    /// own unit: only the share is used.
    pub swap: Option<(u64, u64)>,
    /// Packets the network has received and sent (the adapters `net` counts).
    pub packets: Option<(u64, u64)>,
    /// Microjoules the CPU package has used (its own meter, RAPL).
    pub cpu_energy: Option<u64>,
    /// Watts the battery gives now, while the machine runs on it.
    pub battery_draw: Option<f32>,
}

/// What the machine is doing, between two `Counters`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Load {
    /// Each core's busy share, 0 to 1.
    pub cores: Vec<f32>,
    /// All the cores together, 0 to 1.
    pub cpu: f32,
    /// The share of memory in use, 0 to 1.
    pub memory: Option<f32>,
    /// Bytes a second: (read, written).
    pub disk: Option<(f64, f64)>,
    /// Bytes a second: (received, sent).
    pub net: Option<(f64, f64)>,
    pub temp: Option<f32>,
    /// A second: context switches, interrupts.
    pub switches: Option<f64>,
    pub interrupts: Option<f64>,
    pub processes: Option<u64>,
    pub threads: Option<u64>,
    /// The share of swap in use, 0 to 1; None with no swap at all.
    pub swap: Option<f32>,
    /// Packets a second: (received, sent).
    pub packets: Option<(f64, f64)>,
    /// Watts: the CPU package's, and the battery's while it runs the machine.
    pub cpu_power: Option<f32>,
    pub battery_draw: Option<f32>,
}

/// The load between `before` and `now`, `secs` apart. A total that went
/// down (a disk or an adapter gone, a counter that wrapped) counts as
/// nothing done, never as a huge number; the cores, when their count
/// changed, as idle. A figure that is a level now (memory, processes,
/// swap, temperature, the battery) is `now`'s.
pub fn load(before: &Counters, now: &Counters, secs: f64) -> Load {
    let share = |(b0, t0): (u64, u64), (b1, t1): (u64, u64)| match t1.checked_sub(t0) {
        Some(t) if t > 0 => (b1.saturating_sub(b0) as f64 / t as f64).clamp(0.0, 1.0) as f32,
        _ => 0.0,
    };
    let same = before.cores.len() == now.cores.len();
    let cores: Vec<f32> = if same {
        before.cores.iter().zip(&now.cores).map(|(a, b)| share(*a, *b)).collect()
    } else {
        vec![0.0; now.cores.len()]
    };
    let sum =
        |c: &[(u64, u64)]| c.iter().fold((0u64, 0u64), |(b, t), (x, y)| (b.saturating_add(*x), t.saturating_add(*y)));
    let rate = |a: Option<(u64, u64)>, b: Option<(u64, u64)>| match (a, b) {
        (Some(a), Some(b)) if secs > 0.0 => {
            Some((b.0.saturating_sub(a.0) as f64 / secs, b.1.saturating_sub(a.1) as f64 / secs))
        }
        _ => None,
    };
    let rate1 = |a: Option<u64>, b: Option<u64>| rate(a.map(|a| (a, 0)), b.map(|b| (b, 0))).map(|r| r.0);
    let used = |m: Option<(u64, u64)>| {
        m.filter(|(t, _)| *t > 0).map(|(t, free)| (t.saturating_sub(free) as f64 / t as f64) as f32)
    };
    Load {
        cpu: if same { share(sum(&before.cores), sum(&now.cores)) } else { 0.0 },
        cores,
        memory: used(now.memory),
        disk: rate(before.disk, now.disk),
        net: rate(before.net, now.net),
        temp: now.temp,
        switches: rate1(before.switches, now.switches),
        interrupts: rate1(before.interrupts, now.interrupts),
        processes: now.processes,
        threads: now.threads,
        swap: used(now.swap),
        packets: rate(before.packets, now.packets),
        // Microjoules a second are microwatts.
        cpu_power: rate1(before.cpu_energy, now.cpu_energy).map(|uw| (uw / 1e6) as f32),
        battery_draw: now.battery_draw,
    }
}

/// The address this machine reaches the network from (IPv4), empty when it
/// has none. The system is asked which address a UDP socket "connected" to
/// a public address would use; connecting a UDP socket sends nothing. Kept
/// for ten seconds.
pub fn local_ip() -> String {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, ip)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(10)
    {
        return ip.clone();
    }
    let ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.0.2.1:9")?;
            s.local_addr()
        })
        .map(|a| a.ip())
        .ok()
        .filter(|ip| !ip.is_unspecified())
        .map(|ip| ip.to_string())
        .unwrap_or_default();
    *cache = Some((Instant::now(), ip.clone()));
    ip
}

/// Bytes as people write them: "812M", "6.2G".
pub fn human_bytes(b: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    const M: f64 = 1024.0 * 1024.0;
    let b = b as f64;
    if b >= G { format!("{:.1}G", b / G) } else { format!("{}M", (b / M).round() as u64) }
}

/// The branch of the git repository `dir` is in ("main"), a short commit
/// when the head is detached ("a1b2c3d"), or empty outside a repository.
/// Reads `.git/HEAD` (following a `.git` file's `gitdir:`, as worktrees
/// have); never runs git. A relative `dir` is no pane's directory (it would
/// be looked up from wherever this process runs): empty.
pub fn git_branch(dir: &str) -> String {
    if !Path::new(dir).is_absolute() {
        return String::new();
    }
    let mut at = Some(Path::new(dir));
    while let Some(d) = at {
        let dot = d.join(".git");
        if dot.is_dir() {
            return head_of(&dot);
        }
        if let Ok(text) = std::fs::read_to_string(&dot)
            && let Some(rest) = text.trim().strip_prefix("gitdir:")
        {
            let gitdir = PathBuf::from(rest.trim());
            let gitdir = if gitdir.is_absolute() { gitdir } else { d.join(gitdir) };
            return head_of(&gitdir);
        }
        at = d.parent();
    }
    String::new()
}

fn head_of(gitdir: &Path) -> String {
    let Ok(head) = std::fs::read_to_string(gitdir.join("HEAD")) else { return String::new() };
    let head = head.trim();
    match head.strip_prefix("ref: ") {
        Some(r) => r.strip_prefix("refs/heads/").unwrap_or(r).to_string(),
        None => head.chars().take(7).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two readings give each core's share, all of them together, memory
    /// in use and bytes a second; a total that went down is nothing done,
    /// cores that changed in number are idle, a figure missing from either
    /// reading is missing.
    #[test]
    fn two_readings_give_the_load() {
        let a = Counters {
            cores: vec![(10, 100), (50, 100)],
            memory: Some((1000, 600)),
            disk: Some((1_000, 2_000)),
            net: Some((5_000, 5_000)),
            temp: Some(50.0),
            switches: Some(100),
            interrupts: Some(5_000),
            processes: Some(300),
            threads: Some(4_000),
            swap: Some((1_000, 1_000)),
            packets: Some((10, 20)),
            cpu_energy: Some(1_000_000),
            battery_draw: None,
        };
        let b = Counters {
            cores: vec![(60, 200), (150, 200)],
            memory: Some((1000, 250)),
            disk: Some((3_000, 2_000)),
            net: Some((4_000, 9_000)),
            temp: Some(61.5),
            switches: Some(20_100),
            interrupts: Some(4_000),
            processes: Some(312),
            threads: Some(4_300),
            swap: Some((1_000, 900)),
            packets: Some((30, 60)),
            cpu_energy: Some(41_000_000),
            battery_draw: Some(19.5),
        };
        let l = load(&a, &b, 2.0);
        assert_eq!(l.cores, vec![0.5, 1.0]);
        assert_eq!(l.cpu, 0.75);
        assert_eq!(l.memory, Some(0.75));
        assert_eq!(l.disk, Some((1_000.0, 0.0)));
        assert_eq!(l.net, Some((0.0, 2_000.0)), "received went down: nothing, not a wrap");
        assert_eq!(l.temp, Some(61.5));
        assert_eq!((l.switches, l.interrupts), (Some(10_000.0), Some(0.0)), "interrupts wrapped: nothing");
        assert_eq!((l.processes, l.threads, l.swap), (Some(312), Some(4_300), Some(0.1)));
        assert_eq!(l.packets, Some((10.0, 20.0)));
        assert_eq!((l.cpu_power, l.battery_draw), (Some(20.0), Some(19.5)), "40 J in 2 s: 20 W");
        assert_eq!(load(&a, &Counters { swap: Some((0, 0)), ..b.clone() }, 2.0).swap, None, "no swap at all");
        let fewer = Counters { cores: vec![(1, 1)], disk: None, cpu_energy: None, ..b.clone() };
        let l = load(&a, &fewer, 2.0);
        assert_eq!((l.cores, l.cpu, l.disk, l.cpu_power), (vec![0.0], 0.0, None, None));
        // One reading against itself: every figure there, at nothing done.
        let l = load(&b, &b, 1.0);
        assert_eq!((l.cpu, l.disk, l.net, l.memory), (0.0, Some((0.0, 0.0)), Some((0.0, 0.0)), Some(0.75)));
        assert_eq!(load(&a, &b, 0.0).disk, None, "no time between them: no rate");
        assert_eq!(load(&Counters::default(), &Counters::default(), 1.0), Load::default());
    }

    #[test]
    fn the_branch_is_read_from_the_head_file() {
        let dir = std::env::temp_dir().join(format!("keepane-git-{}", std::process::id()));
        let repo = dir.join("repo");
        let sub = repo.join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_branch(&sub.to_string_lossy()), "feature/x", "found from a subdirectory");
        std::fs::write(repo.join(".git").join("HEAD"), "a1b2c3d4e5f6a7b8\n").unwrap();
        assert_eq!(git_branch(&repo.to_string_lossy()), "a1b2c3d", "detached: the short commit");
        // A worktree: .git is a file pointing at the real git dir.
        let wt = dir.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let gd = dir.join("gitdir");
        std::fs::create_dir_all(&gd).unwrap();
        std::fs::write(gd.join("HEAD"), "ref: refs/heads/wt-branch\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gd.display())).unwrap();
        assert_eq!(git_branch(&wt.to_string_lossy()), "wt-branch");
        assert_eq!(git_branch(&dir.to_string_lossy()), "", "no repository above");
        let nowhere = if cfg!(windows) { r"Z:\no\such\place" } else { "/no/such/place" };
        assert_eq!(git_branch(nowhere), "");
        assert_eq!(git_branch("src"), "", "relative: not looked up from this process's directory");
        assert_eq!(git_branch(""), "");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
