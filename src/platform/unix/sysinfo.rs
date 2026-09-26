//! The machine as the status line shows it: CPU and memory in use, the
//! battery, the time since boot (Linux: `/proc`, `/sys`); the program a pane
//! is running (its newest descendant, from `/proc/<pid>/stat`); a directory
//! with the home written `~`. macOS reads the same through `sysctl`, mach
//! and `proc_pidinfo`.

use crate::sysinfo::human_bytes;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One reading of everything system-wide, taken at most once a second.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct System {
    /// "37%": busy since the last reading (the first one: since boot).
    pub cpu_percentage: String,
    pub ram_percentage: String,
    /// "6.2G", "812M".
    pub ram_used: String,
    /// "84%", or empty when there is no battery.
    pub battery_percentage: String,
    pub battery_charging: bool,
    /// Seconds since boot.
    pub uptime: i64,
}

#[derive(Default)]
struct Cache {
    at: Option<Instant>,
    system: System,
    /// (idle, total) jiffies at the last reading.
    cpu: Option<(u64, u64)>,
    programs: HashMap<u32, (Instant, String)>,
}

static CACHE: std::sync::LazyLock<Mutex<Cache>> = std::sync::LazyLock::new(|| Mutex::new(Cache::default()));

/// Linux: `/proc` and `/sys`.
#[cfg(not(target_os = "macos"))]
mod readings {
    /// (idle, total) CPU time since boot, from `/proc/stat`'s first line.
    pub fn cpu_times() -> Option<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/stat").ok()?;
        let line = text.lines().next()?.strip_prefix("cpu ")?;
        let v: Vec<u64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        // user nice system idle iowait irq softirq steal
        let idle = v.get(3)? + v.get(4).copied().unwrap_or(0);
        let total = v.iter().take(8).sum();
        Some((idle, total))
    }

    /// (total, available) bytes of memory, from `/proc/meminfo`.
    pub fn memory() -> Option<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let field = |name: &str| -> Option<u64> {
            let l = text.lines().find(|l| l.starts_with(name))?;
            l.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
        };
        Some((field("MemTotal:")?, field("MemAvailable:")?))
    }

    /// (percent, charging) of the first battery under `/sys/class/power_supply`.
    pub fn battery() -> Option<(u32, bool)> {
        for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let p = e.path();
            if std::fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "Battery") {
                let pct = std::fs::read_to_string(p.join("capacity")).ok()?.trim().parse().ok()?;
                let status = std::fs::read_to_string(p.join("status")).unwrap_or_default();
                return Some((pct, matches!(status.trim(), "Charging" | "Full")));
            }
        }
        None
    }

    /// Seconds since boot, from `/proc/uptime`.
    pub fn uptime() -> Option<i64> {
        let t = std::fs::read_to_string("/proc/uptime").ok()?;
        t.split_whitespace().next()?.parse::<f64>().ok().map(|s| s as i64)
    }

    /// (pid, parent, name) of every process, from `/proc/<pid>/stat`.
    pub fn processes() -> Vec<(u32, u32, String)> {
        let mut out = Vec::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
            // pid (comm) state ppid ...: comm may hold spaces and parentheses.
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
            let name = stat[open + 1..close].to_string();
            let ppid = stat[close + 1..].split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            out.push((pid, ppid, name));
        }
        out
    }
}

/// macOS: `sysctl`, the host's CPU ticks (mach), `proc_pidinfo`. The
/// battery needs IOKit and is left out: no figure, as on a desktop.
#[cfg(target_os = "macos")]
mod readings {
    use std::ffi::CString;

    unsafe extern "C" {
        fn mach_host_self() -> u32;
        fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    }
    const HOST_CPU_LOAD_INFO: i32 = 3;
    /// user, system, idle, nice.
    const CPU_STATE_MAX: usize = 4;

    /// A `sysctl` value of type `T` by name.
    fn sysctl<T: Copy>(name: &str) -> Option<T> {
        let name = CString::new(name).ok()?;
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        let mut size = std::mem::size_of::<T>();
        let r =
            unsafe { libc::sysctlbyname(name.as_ptr(), value.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
        (r == 0 && size == std::mem::size_of::<T>()).then(|| unsafe { value.assume_init() })
    }

    /// (idle, total) CPU ticks since boot, over every core.
    pub fn cpu_times() -> Option<(u64, u64)> {
        // One send right for the process's life, not one per reading.
        static HOST: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let host = *HOST.get_or_init(|| unsafe { mach_host_self() });
        let mut ticks = [0i32; CPU_STATE_MAX];
        let mut count = CPU_STATE_MAX as u32;
        if unsafe { host_statistics(host, HOST_CPU_LOAD_INFO, ticks.as_mut_ptr(), &mut count) } != 0 {
            return None;
        }
        // Counters are unsigned 32-bit and wrap.
        let t: Vec<u64> = ticks.iter().map(|&x| u64::from(x as u32)).collect();
        Some((t[2], t.iter().sum()))
    }

    /// (total, available) bytes of memory: available is what is free, about
    /// to be, or file cache the system drops when asked (as `vm_stat` shows).
    pub fn memory() -> Option<(u64, u64)> {
        let total: u64 = sysctl("hw.memsize")?;
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
        let pages = |n: &str| sysctl::<u32>(n).map(u64::from);
        let avail = pages("vm.page_free_count")?
            + pages("vm.page_speculative_count").unwrap_or(0)
            + pages("vm.page_pageable_external_count").unwrap_or(0);
        Some((total, (avail * page).min(total)))
    }

    pub fn battery() -> Option<(u32, bool)> {
        None
    }

    /// Seconds since `kern.boottime`.
    pub fn uptime() -> Option<i64> {
        let boot: libc::timeval = sysctl("kern.boottime")?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
        Some(now - boot.tv_sec)
    }

    /// (pid, parent, name) of every process this user may read.
    pub fn processes() -> Vec<(u32, u32, String)> {
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        // Room for processes started meanwhile.
        let mut pids = vec![0 as libc::pid_t; n as usize + 64];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        let mut out = Vec::new();
        for &pid in &pids[..n.max(0) as usize] {
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let got = unsafe {
                libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size)
            };
            if got != size {
                continue;
            }
            let name = unsafe { std::ffi::CStr::from_ptr(info.pbi_comm.as_ptr()) }.to_string_lossy().into_owned();
            out.push((pid as u32, info.pbi_ppid, name));
        }
        out
    }
}

use readings::{battery, cpu_times, memory, processes};
/// The current reading, refreshed when the last one is over a second old.
pub fn system() -> System {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if c.at.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
        return c.system.clone();
    }
    c.at = Some(Instant::now());
    if let Some(sample) = cpu_times() {
        let (idle, total) = match c.cpu {
            Some((pi, pt)) => (sample.0.saturating_sub(pi), sample.1.saturating_sub(pt)),
            None => sample,
        };
        if let Some(busy) = (100 * total.saturating_sub(idle)).checked_div(total) {
            c.system.cpu_percentage = format!("{busy}%");
        }
        c.cpu = Some(sample);
    }
    if let Some((total, avail)) = memory()
        && total > 0
    {
        let used = total.saturating_sub(avail);
        c.system.ram_percentage = format!("{}%", used * 100 / total);
        c.system.ram_used = human_bytes(used);
    }
    match battery() {
        Some((pct, charging)) => {
            c.system.battery_percentage = format!("{pct}%");
            c.system.battery_charging = charging;
        }
        None => {
            c.system.battery_percentage.clear();
            c.system.battery_charging = false;
        }
    }
    c.system.uptime = readings::uptime().unwrap_or(0);

    c.system.clone()
}

/// This machine's name (`#H`), as `gethostname` gives it.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// `dir` with the home directory as `~`, the way prompts write it.
pub fn short_path(dir: &str) -> String {
    let Some(home) = dirs::home_dir() else { return dir.to_string() };
    let home = home.to_string_lossy();
    match dir.strip_prefix(home.as_ref()) {
        Some("") => "~".to_string(),
        Some(r) if r.starts_with('/') => format!("~{r}"),
        _ => dir.to_string(),
    }
}

/// The program a pane is running: its newest descendant (`cargo` under
/// `bash` during a build), the pane's own process when it has none.
pub fn program_of(pid: u32) -> String {
    {
        let c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, name)) = c.programs.get(&pid)
            && at.elapsed() < Duration::from_secs(1)
        {
            return name.clone();
        }
    }
    let procs = processes();
    let name = match procs.iter().find(|(p, _, _)| *p == pid) {
        None => String::new(),
        Some((_, _, own)) => {
            let mut current = own.clone();
            let mut at = pid;
            // The newest child: the highest pid (pids grow until they wrap).
            while let Some((child, _, name)) =
                procs.iter().filter(|(_, parent, _)| *parent == at).max_by_key(|(p, _, _)| *p)
            {
                current = name.clone();
                at = *child;
            }
            current
        }
    };
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    c.programs.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
    c.programs.insert(pid, (Instant::now(), name.clone()));
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_come_back_and_are_cached() {
        let a = system();
        assert!(a.cpu_percentage.ends_with('%'), "first reading: {a:?}");
        assert!(a.ram_percentage.ends_with('%'), "{a:?}");
        assert!(a.ram_used.ends_with('G') || a.ram_used.ends_with('M'), "{a:?}");
        assert!(a.uptime > 0);
        assert_eq!(a, system(), "within a second the same reading is handed back");
    }

    #[test]
    fn the_host_has_a_name() {
        let h = hostname();
        assert!(!h.is_empty() && !h.contains('\0'), "{h:?}");
        let uname = std::process::Command::new("uname").arg("-n").output().unwrap();
        assert_eq!(h, String::from_utf8_lossy(&uname.stdout).trim());
    }

    #[test]
    fn home_becomes_a_tilde() {
        let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
        assert_eq!(short_path(&home), "~");
        assert_eq!(short_path(&format!("{home}/src/x")), "~/src/x");
        assert_eq!(short_path("/etc"), "/etc");
        assert_eq!(short_path(&format!("{home}2/x")), format!("{home}2/x"), "a sibling directory is not home");
    }

    #[test]
    fn the_program_of_this_process_is_found() {
        let me = std::process::id();
        let name = program_of(me);
        assert!(!name.is_empty());
        assert_eq!(program_of(me), name, "cached for a second");
        assert_eq!(program_of(0x7fff_fff0), "", "no such process");
    }
}
