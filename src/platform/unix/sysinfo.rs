//! The machine as the status line shows it: CPU and memory in use, the
//! battery, the time since boot (Linux: `/proc`, `/sys`); the program a pane
//! is running (its newest descendant, from `/proc/<pid>/stat`); a directory
//! with the home written `~`; for the clock's animation, the cores, disks,
//! network, temperature, CPU power, battery draw and the system's own counts
//! (see `counters`). macOS reads the same through `sysctl`, mach,
//! `proc_pidinfo` and IOKit (the disks); its context switches and threads
//! are those of the processes this user may look into; no temperature,
//! power or interrupts.

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
    /// The process list, at most a second old: ten panes are ten lookups
    /// a second, not ten reads of every process.
    procs: Option<(Instant, Vec<Proc>)>,
}

/// A process: its id, its parent's id, its name.
type Proc = (u32, u32, String);

static CACHE: std::sync::LazyLock<Mutex<Cache>> = std::sync::LazyLock::new(|| Mutex::new(Cache::default()));

/// Linux: `/proc` and `/sys`.
#[cfg(not(target_os = "macos"))]
mod readings {
    use std::path::Path;

    /// (idle, total) of a `/proc/stat` CPU line's numbers.
    fn ticks(numbers: &str) -> Option<(u64, u64)> {
        let v: Vec<u64> = numbers.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        // user nice system idle iowait irq softirq steal
        let idle = v.get(3)? + v.get(4).copied().unwrap_or(0);
        let total = v.iter().take(8).sum();
        Some((idle, total))
    }

    /// (idle, total) CPU time since boot, from `/proc/stat`'s first line.
    pub fn cpu_times() -> Option<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/stat").ok()?;
        ticks(text.lines().next()?.strip_prefix("cpu ")?)
    }

    /// Each core's (busy, total) time, from `/proc/stat`'s `cpuN` lines;
    /// the context switches (`ctxt`) and interrupts (`intr`'s total) since
    /// boot.
    pub fn stat() -> (Vec<(u64, u64)>, Option<u64>, Option<u64>) {
        let Ok(text) = std::fs::read_to_string("/proc/stat") else { return (Vec::new(), None, None) };
        let cores = text
            .lines()
            .filter_map(|l| {
                let rest = l.strip_prefix("cpu")?;
                let numbers = rest.trim_start_matches(|c: char| c.is_ascii_digit());
                (numbers.len() < rest.len()).then_some(numbers)
            })
            .filter_map(ticks)
            .map(|(idle, total)| (total.saturating_sub(idle), total))
            .collect();
        let first =
            |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.split_whitespace().next()?.parse::<u64>().ok());
        (cores, first("ctxt "), first("intr "))
    }

    /// Processes (`/proc`'s numbered directories) and threads (the
    /// scheduler's count in `/proc/loadavg`) running now.
    pub fn tasks() -> (Option<u64>, Option<u64>) {
        let processes = std::fs::read_dir("/proc").ok().map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())))
                .count() as u64
        });
        let threads = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|t| t.split_whitespace().nth(3)?.split_once('/')?.1.parse().ok());
        (processes, threads)
    }

    /// Microjoules the CPU packages have used, from RAPL under `dir`
    /// (`/sys/class/powercap`): each `intel-rapl:N` (AMD's too) is a
    /// package. Most systems let only root read it; then None.
    pub fn cpu_energy(dir: &Path) -> Option<u64> {
        let mut sum = None;
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(n) = name.strip_prefix("intel-rapl:") else { continue };
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let uj = std::fs::read_to_string(e.path().join("energy_uj")).ok()?.trim().parse::<u64>().ok()?;
            sum = Some(sum.unwrap_or(0u64).saturating_add(uj));
        }
        sum
    }

    /// Watts the batteries under `dir` (`/sys/class/power_supply`) give
    /// while discharging: `power_now` (µW), else `current_now` (µA) times
    /// `voltage_now` (µV). None on mains power or with no battery.
    pub fn battery_draw(dir: &Path) -> Option<f32> {
        let mut sum = None;
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
            if read("type").as_deref() != Some("Battery") || read("status").as_deref() != Some("Discharging") {
                continue;
            }
            let number = |f: &str| read(f).and_then(|s| s.parse::<f64>().ok()).map(f64::abs);
            let watts = match number("power_now") {
                Some(uw) => uw / 1e6,
                None => number("current_now").zip(number("voltage_now")).map(|(ua, uv)| ua * uv / 1e12)?,
            };
            sum = Some(sum.unwrap_or(0.0) + watts as f32);
        }
        sum
    }

    /// Bytes the disks have read and written, from `/proc/diskstats`
    /// (sectors there are 512 bytes, whatever the disk's own are). Only
    /// disks that are hardware (`/sys/block/<name>/device`): a partition,
    /// a loop or RAM device, device-mapper or RAID would count the same
    /// bytes again.
    pub fn disk() -> Option<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/diskstats").ok()?;
        let mut sum: Option<(u64, u64)> = None;
        for l in text.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (Some(name), Some(r), Some(w)) = (f.get(2), f.get(5), f.get(9)) else { continue };
            if !Path::new("/sys/block").join(name).join("device").exists() {
                continue;
            }
            let sectors = |s: &str| s.parse::<u64>().unwrap_or(0).saturating_mul(512);
            let (a, b) = sum.unwrap_or_default();
            sum = Some((a.saturating_add(sectors(r)), b.saturating_add(sectors(w))));
        }
        sum
    }

    /// Bytes, then packets, received and sent, from `/proc/net/dev`: the
    /// adapters that are hardware (`/sys/class/net/<name>/device`) when
    /// there are any (a bridge or a container's veth carries their bytes
    /// again), else every one but loopback.
    #[allow(clippy::type_complexity)]
    pub fn net() -> Option<((u64, u64), (u64, u64))> {
        let text = std::fs::read_to_string("/proc/net/dev").ok()?;
        // Name, then bytes in, packets in, bytes out, packets out.
        let rows: Vec<(&str, [u64; 4])> = text
            .lines()
            .skip(2)
            .filter_map(|l| {
                let (name, rest) = l.split_once(':')?;
                let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                Some((name.trim(), [*v.first()?, *v.get(1)?, *v.get(8)?, *v.get(9)?]))
            })
            .collect();
        let hardware = |n: &str| Path::new("/sys/class/net").join(n).join("device").exists();
        let any = rows.iter().any(|r| hardware(r.0));
        let picked: Vec<_> = rows.iter().filter(|r| r.0 != "lo" && (!any || hardware(r.0))).collect();
        (!picked.is_empty()).then(|| {
            let s = picked.iter().fold([0u64; 4], |s, r| std::array::from_fn(|i| s[i].saturating_add(r.1[i])));
            ((s[0], s[2]), (s[1], s[3]))
        })
    }

    /// The CPU's temperature, from the thermal zones and the hwmon sensors.
    pub fn temperature() -> Option<f32> {
        let read = |p: &Path| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string());
        let mut found = Vec::new();
        for e in std::fs::read_dir("/sys/class/thermal").into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with("thermal_zone")
                && let (Some(kind), Some(t)) = (read(&p.join("type")), read(&p.join("temp")))
                && let Ok(milli) = t.parse::<i64>()
            {
                found.push((kind, milli as f32 / 1000.0));
            }
        }
        // temp1 is the package's (coretemp) or Tctl (k10temp).
        for e in std::fs::read_dir("/sys/class/hwmon").into_iter().flatten().flatten() {
            let p = e.path();
            if let (Some(name), Some(t)) = (read(&p.join("name")), read(&p.join("temp1_input")))
                && let Ok(milli) = t.parse::<i64>()
            {
                found.push((name, milli as f32 / 1000.0));
            }
        }
        pick_temperature(&found)
    }

    /// The CPU's temperature among (sensor, °C): the first CPU sensor in
    /// `CPU`'s order; none when no sensor is a CPU's (a chipset's, a
    /// disk's or ACPI's `acpitz` is not it), or its figure is no
    /// temperature.
    pub fn pick_temperature(found: &[(String, f32)]) -> Option<f32> {
        const CPU: &[&str] = &[
            "x86_pkg_temp",
            "coretemp",
            "k10temp",
            "zenpower",
            "cpu_thermal",
            "cpu-thermal",
            "soc_thermal",
            "soc-thermal",
        ];
        CPU.iter().find_map(|kind| found.iter().find(|(k, t)| k == kind && *t > 0.0 && *t < 150.0).map(|(_, t)| *t))
    }

    /// (total, available) bytes of memory, from `/proc/meminfo`.
    pub fn memory() -> Option<(u64, u64)> {
        meminfo("MemTotal:", "MemAvailable:")
    }

    /// (total, free) bytes of swap, from `/proc/meminfo`.
    pub fn swap() -> Option<(u64, u64)> {
        meminfo("SwapTotal:", "SwapFree:")
    }

    fn meminfo(total: &str, free: &str) -> Option<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let field = |name: &str| -> Option<u64> {
            let l = text.lines().find(|l| l.starts_with(name))?;
            l.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
        };
        Some((field(total)?, field(free)?))
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

    /// A process's CPU time so far (nanoseconds), resident memory (bytes)
    /// and when it started (Unix seconds), from `/proc/<pid>/stat` and
    /// `statm`, and the boot time in `/proc/stat`.
    pub fn usage_of(pid: u32) -> Option<(u64, u64, u64)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // After "(comm)": state ppid ... utime is the 12th, stime the 13th,
        // starttime (ticks after boot) the 20th.
        let after = &stat[stat.rfind(')')? + 1..];
        let f: Vec<&str> = after.split_whitespace().collect();
        let ticks: u64 = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
        let per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
        static BOOT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let boot = *BOOT.get_or_init(|| {
            std::fs::read_to_string("/proc/stat")
                .ok()
                .and_then(|t| t.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok()))
                .unwrap_or(0)
        });
        let started = boot + f.get(19).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) / per_sec;
        Some((ticks * 1_000_000_000 / per_sec, pages * page, started))
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

    unsafe extern "C" {
        fn host_processor_info(host: u32, flavor: i32, count: *mut u32, info: *mut *mut i32, size: *mut u32) -> i32;
        fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
        static mach_task_self_: u32;
    }
    const PROCESSOR_CPU_LOAD_INFO: i32 = 2;

    /// Each core's (busy, total) ticks, and the context switches: no public
    /// interface counts them for the whole system, so they are the
    /// processes' own (`switches`). No interrupts.
    pub fn stat() -> (Vec<(u64, u64)>, Option<u64>, Option<u64>) {
        (cores(), switches(&task_infos()), None)
    }

    /// Processes (as many as `proc_listallpids` gives), and the threads of
    /// those this user may look into.
    pub fn tasks() -> (Option<u64>, Option<u64>) {
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let infos = task_infos();
        let threads = infos.iter().map(|(_, t)| t.pti_threadnum.max(0) as u64).sum::<u64>();
        ((n > 0).then_some(n as u64), (!infos.is_empty()).then_some(threads))
    }

    /// Every process this user may look into (others' are refused), with
    /// its task info (`proc_pidinfo`): its context switches and threads.
    /// One look serves `stat` and `tasks` (a `counters` asks both).
    fn task_infos() -> Vec<(libc::pid_t, libc::proc_taskinfo)> {
        type Infos = Vec<(libc::pid_t, libc::proc_taskinfo)>;
        static LAST: std::sync::Mutex<Option<(std::time::Instant, Infos)>> = std::sync::Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, infos)) = &*last
            && at.elapsed() < std::time::Duration::from_millis(100)
        {
            return infos.clone();
        }
        let infos = look_into_tasks();
        *last = Some((std::time::Instant::now(), infos.clone()));
        infos
    }

    fn look_into_tasks() -> Vec<(libc::pid_t, libc::proc_taskinfo)> {
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0 as libc::pid_t; n as usize + 64];
        let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        pids[..n.max(0) as usize]
            .iter()
            .filter_map(|&pid| {
                let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
                let got = unsafe {
                    libc::proc_pidinfo(
                        pid,
                        libc::PROC_PIDTASKINFO,
                        0,
                        (&mut info as *mut libc::proc_taskinfo).cast(),
                        size,
                    )
                };
                (got == size).then_some((pid, info))
            })
            .collect()
    }

    /// The context switches of the processes in `infos`, as a running total
    /// that never goes back: what each one did since the last reading
    /// (all it did, for one that is new), added up. A process that ends
    /// takes its count with it, so the plain sum would fall.
    fn switches(infos: &[(libc::pid_t, libc::proc_taskinfo)]) -> Option<u64> {
        static SEEN: std::sync::Mutex<Option<(std::collections::HashMap<libc::pid_t, u64>, u64)>> =
            std::sync::Mutex::new(None);
        if infos.is_empty() {
            return None;
        }
        let now: std::collections::HashMap<libc::pid_t, u64> =
            // (A 32-bit count, wrapping: read unsigned.)
            infos.iter().map(|(pid, t)| (*pid, u64::from(t.pti_csw as u32))).collect();
        Some(super::running_total(&mut SEEN.lock().unwrap_or_else(|e| e.into_inner()), now))
    }

    /// (total, free) bytes of swap, from `vm.swapusage`.
    pub fn swap() -> Option<(u64, u64)> {
        let s: libc::xsw_usage = sysctl("vm.swapusage")?;
        Some((s.xsu_total, s.xsu_avail))
    }

    /// The CPU's energy and the battery need IOKit or root: not read.
    pub fn cpu_energy(_: &std::path::Path) -> Option<u64> {
        None
    }

    pub fn battery_draw(_: &std::path::Path) -> Option<f32> {
        None
    }

    /// Each core's (busy, total) ticks, from mach (user, system, idle, nice).
    fn cores() -> Vec<(u64, u64)> {
        static HOST: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let host = *HOST.get_or_init(|| unsafe { mach_host_self() });
        let (mut count, mut info, mut size) = (0u32, std::ptr::null_mut::<i32>(), 0u32);
        if unsafe { host_processor_info(host, PROCESSOR_CPU_LOAD_INFO, &mut count, &mut info, &mut size) } != 0
            || info.is_null()
        {
            return Vec::new();
        }
        let ticks = unsafe { std::slice::from_raw_parts(info, size as usize) };
        let out = ticks
            .as_chunks::<CPU_STATE_MAX>()
            .0
            .iter()
            .take(count as usize)
            .map(|t| {
                let t: Vec<u64> = t.iter().map(|&x| u64::from(x as u32)).collect();
                (t[0] + t[1] + t[3], t.iter().sum())
            })
            .collect();
        unsafe { vm_deallocate(mach_task_self_, info as usize, size as usize * std::mem::size_of::<i32>()) };
        out
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const libc::c_char) -> *mut libc::c_void;
        fn IOServiceGetMatchingServices(main: u32, matching: *mut libc::c_void, found: *mut u32) -> i32;
        fn IOIteratorNext(iterator: u32) -> u32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const libc::c_void,
            allocator: *const libc::c_void,
            options: u32,
        ) -> *const libc::c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const libc::c_void,
            text: *const libc::c_char,
            encoding: u32,
        ) -> *const libc::c_void;
        fn CFDictionaryGetValue(dict: *const libc::c_void, key: *const libc::c_void) -> *const libc::c_void;
        fn CFNumberGetValue(number: *const libc::c_void, kind: isize, value: *mut libc::c_void) -> u8;
        fn CFGetTypeID(cf: *const libc::c_void) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFRelease(cf: *const libc::c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    const SINT64: isize = 4;

    /// A CoreFoundation string, released when dropped.
    struct CfString(*const libc::c_void);
    impl CfString {
        fn new(s: &str) -> Option<CfString> {
            let c = CString::new(s).ok()?;
            let p = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
            (!p.is_null()).then_some(CfString(p))
        }
    }
    impl Drop for CfString {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) };
        }
    }

    /// Bytes the disks have read and written, as `iostat` counts them: each
    /// block storage driver's `Statistics` (IOKit), which holds a disk's
    /// totals since it came; an APFS volume has none of its own, so its
    /// bytes count once, on its disk.
    pub fn disk() -> Option<(u64, u64)> {
        let (stats, read, written) =
            (CfString::new("Statistics")?, CfString::new("Bytes (Read)")?, CfString::new("Bytes (Write)")?);
        let matching = unsafe { IOServiceMatching(c"IOBlockStorageDriver".as_ptr()) };
        if matching.is_null() {
            return None;
        }
        let mut found = 0u32;
        // (It takes the matching dictionary's reference.)
        if unsafe { IOServiceGetMatchingServices(0, matching, &mut found) } != 0 {
            return None;
        }
        let number = |dict: *const libc::c_void, key: &CfString| -> Option<u64> {
            let v = unsafe { CFDictionaryGetValue(dict, key.0) };
            if v.is_null() || unsafe { CFGetTypeID(v) != CFNumberGetTypeID() } {
                return None;
            }
            let mut out = 0i64;
            (unsafe { CFNumberGetValue(v, SINT64, (&mut out as *mut i64).cast()) } != 0).then_some(out.max(0) as u64)
        };
        let mut sum: Option<(u64, u64)> = None;
        loop {
            let driver = unsafe { IOIteratorNext(found) };
            if driver == 0 {
                break;
            }
            let dict = unsafe { IORegistryEntryCreateCFProperty(driver, stats.0, std::ptr::null(), 0) };
            if !dict.is_null() {
                if unsafe { CFGetTypeID(dict) == CFDictionaryGetTypeID() } {
                    let (r, w) = sum.unwrap_or_default();
                    sum = Some((
                        r.saturating_add(number(dict, &read).unwrap_or(0)),
                        w.saturating_add(number(dict, &written).unwrap_or(0)),
                    ));
                }
                unsafe { CFRelease(dict) };
            }
            unsafe { IOObjectRelease(driver) };
        }
        unsafe { IOObjectRelease(found) };
        sum
    }

    /// Bytes, then packets, received and sent by the Ethernet and Wi-Fi
    /// adapters (a VPN's tunnel or a bridge carries their bytes again;
    /// loopback is not the network), from the routing socket's 64-bit
    /// counters.
    #[allow(clippy::type_complexity)]
    pub fn net() -> Option<((u64, u64), (u64, u64))> {
        const IFT_ETHER: u8 = 0x6;
        let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
        let mut len = 0usize;
        let mut sysctl = |buf: *mut libc::c_void, len: &mut usize| unsafe {
            libc::sysctl(mib.as_mut_ptr(), mib.len() as u32, buf, len, std::ptr::null_mut(), 0)
        };
        if sysctl(std::ptr::null_mut(), &mut len) != 0 {
            return None;
        }
        // Room for an adapter that comes meanwhile.
        len += 4096;
        let mut buf = vec![0u8; len];
        if sysctl(buf.as_mut_ptr().cast(), &mut len) != 0 {
            return None;
        }
        let (mut at, mut sum) = (0usize, None::<((u64, u64), (u64, u64))>);
        while at + std::mem::size_of::<libc::if_msghdr2>() <= len {
            let m: libc::if_msghdr2 = unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at).cast()) };
            let step = usize::from(m.ifm_msglen);
            if step == 0 {
                break;
            }
            let data = m.ifm_data;
            if i32::from(m.ifm_type) == libc::RTM_IFINFO2
                && m.ifm_flags & libc::IFF_LOOPBACK == 0
                && data.ifi_type == IFT_ETHER
            {
                let ((a, b), (pa, pb)) = sum.unwrap_or_default();
                sum = Some((
                    (a.saturating_add(data.ifi_ibytes), b.saturating_add(data.ifi_obytes)),
                    (pa.saturating_add(data.ifi_ipackets), pb.saturating_add(data.ifi_opackets)),
                ));
            }
            at += step;
        }
        sum
    }

    /// No public interface gives it.
    pub fn temperature() -> Option<f32> {
        None
    }

    /// Seconds since `kern.boottime`.
    pub fn uptime() -> Option<i64> {
        let boot: libc::timeval = sysctl("kern.boottime")?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
        Some(now - boot.tv_sec)
    }

    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    /// A process's CPU time so far (nanoseconds), resident memory (bytes)
    /// and when it started (Unix seconds), from `proc_pidinfo`; its times
    /// are in mach ticks.
    pub fn usage_of(pid: u32) -> Option<(u64, u64, u64)> {
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            )
        };
        if got != size {
            return None;
        }
        static BASE: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
        let (numer, denom) = *BASE.get_or_init(|| {
            let mut t = Timebase { numer: 1, denom: 1 };
            unsafe { mach_timebase_info(&mut t) };
            (u64::from(t.numer), u64::from(t.denom.max(1)))
        });
        let ticks = info.pti_total_user + info.pti_total_system;
        let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut bsd as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        let started = if got == size { bsd.pbi_start_tvsec } else { 0 };
        Some((ticks * numer / denom, info.pti_resident_size, started))
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

pub use readings::usage_of;
use readings::{battery, cpu_times, memory, processes};

/// Counts kept per process (`now`, by pid) as one running total that never
/// goes back: to the total in `seen`, what each process did since then
/// (all it did, for one that is new; nothing, for one whose count went
/// back: its pid taken by another, or its count wrapped); the first time,
/// what they all did so far. A process that ends takes its count with it,
/// so their plain sum would fall. `seen` keeps this reading for the next.
#[cfg(any(target_os = "macos", test))]
fn running_total(seen: &mut Option<(HashMap<i32, u64>, u64)>, now: HashMap<i32, u64>) -> u64 {
    let total = match seen.take() {
        None => now.values().sum(),
        Some((before, total)) => {
            total + now.iter().map(|(pid, c)| c.saturating_sub(before.get(pid).copied().unwrap_or(0))).sum::<u64>()
        }
    };
    *seen = Some((now, total));
    total
}

/// The machine's running totals for the clock's animation: each core,
/// memory, the disks, the network, the CPU's temperature, context switches
/// and interrupts, processes and threads, swap, the CPU's energy and the
/// battery's draw, as far as the system gives them. Asked twice a second
/// at most, and only while a clock is drawn.
pub fn counters() -> crate::sysinfo::Counters {
    use std::path::Path;
    let (cores, switches, interrupts) = readings::stat();
    let (processes, threads) = readings::tasks();
    let (net, packets) = readings::net().unzip();
    crate::sysinfo::Counters {
        cores,
        memory: memory(),
        disk: readings::disk(),
        net,
        temp: readings::temperature(),
        switches,
        interrupts,
        processes,
        threads,
        swap: readings::swap(),
        packets,
        cpu_energy: readings::cpu_energy(Path::new("/sys/class/powercap")),
        battery_draw: readings::battery_draw(Path::new("/sys/class/power_supply")),
    }
}

/// `pid` and every process under it, (pid, name).
pub fn tree_of(pid: u32) -> Vec<(u32, String)> {
    let procs = process_list();
    let Some((_, _, own)) = procs.iter().find(|(p, _, _)| *p == pid) else { return Vec::new() };
    let mut out = vec![(pid, own.clone())];
    let mut at = vec![pid];
    while let Some(parent) = at.pop() {
        for (child, pp, name) in &procs {
            if *pp == parent && *child != parent && !out.iter().any(|(p, _)| p == child) {
                out.push((*child, name.clone()));
                at.push(*child);
            }
        }
    }
    out
}

/// `processes()`, at most a second old.
fn process_list() -> Vec<Proc> {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, procs)) = &c.procs
        && at.elapsed() < Duration::from_secs(1)
    {
        return procs.clone();
    }
    let procs = processes();
    c.procs = Some((Instant::now(), procs.clone()));
    procs
}

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
    let procs = process_list();
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

    /// The running totals come back and never go backwards; the cores are
    /// as many as the machine has.
    #[test]
    fn the_counters_come_back_and_grow() {
        let a = counters();
        assert_eq!(a.cores.len(), std::thread::available_parallelism().map_or(1, |n| n.get()), "{a:?}");
        assert!(a.cores.iter().all(|(busy, total)| busy <= total), "{a:?}");
        assert!(a.memory.is_some_and(|(t, av)| t > 0 && av <= t), "{a:?}");
        let spin = Instant::now();
        while spin.elapsed() < Duration::from_millis(200) {
            std::hint::black_box(spin.elapsed());
        }
        let b = counters();
        assert!(a.cores.iter().zip(&b.cores).all(|(x, y)| y.1 >= x.1 && y.0 >= x.0), "never backwards");
        for (x, y) in [(a.disk, b.disk), (a.net, b.net), (a.packets, b.packets)] {
            if let (Some(x), Some(y)) = (x, y) {
                assert!(y.0 >= x.0 && y.1 >= x.1, "{x:?} {y:?}");
            }
        }
        assert_eq!(a.net.is_some(), a.packets.is_some(), "the same adapters: {a:?}");
        let l = crate::sysinfo::load(&a, &b, 0.2);
        assert!(l.cpu > 0.0 && l.cpu <= 1.0, "a core was busy: {l:?}");
        assert!(b.processes.is_some_and(|p| p > 0), "{b:?}");
        assert!(b.swap.is_some_and(|(t, f)| f <= t), "{b:?}");
        // The context switches (the spin above made some) and the threads,
        // everywhere; the interrupts on Linux. macOS reads the disks too
        // (Linux's /proc/diskstats can have no hardware disk in a container).
        assert!(l.switches.is_some_and(|s| s > 0.0), "{l:?}");
        assert!(b.threads.is_some_and(|t| t >= 1), "{b:?}");
        if cfg!(target_os = "macos") {
            assert!(a.disk.is_some() && b.disk.is_some(), "IOKit's disk statistics: {b:?}");
        } else {
            assert!(l.interrupts.is_some(), "{l:?}");
        }
    }

    /// The processes' counts (macOS's context switches) as one total: what
    /// each did since the last reading; a process that ends, a new one, a
    /// pid taken again, a count that wrapped.
    #[test]
    fn a_running_total_of_processes_never_goes_back() {
        let map = |v: &[(i32, u64)]| v.iter().copied().collect::<HashMap<i32, u64>>();
        let mut seen = None;
        assert_eq!(running_total(&mut seen, map(&[(1, 100), (2, 50)])), 150, "the first time: all so far");
        assert_eq!(running_total(&mut seen, map(&[(1, 130), (2, 60)])), 190, "+30 +10");
        assert_eq!(running_total(&mut seen, map(&[(1, 131)])), 191, "2 ended: +1, not -60");
        assert_eq!(running_total(&mut seen, map(&[(1, 131), (3, 7)])), 198, "3 is new: all of its 7");
        assert_eq!(
            running_total(&mut seen, map(&[(1, 5), (3, 9)])),
            200,
            "1 went back (a new process, or wrapped): +0, +2"
        );
        assert_eq!(running_total(&mut seen, map(&[(1, 8), (3, 9)])), 203, "and goes on from 5");
        assert_eq!(running_total(&mut seen, map(&[])), 203, "none at all");
        assert_eq!(running_total(&mut seen, map(&[(4, 1)])), 204);
    }

    /// macOS: the processes' switches add up to a total that never goes
    /// back, though a process that ends takes its own count with it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_switches_never_go_back_when_a_process_ends() {
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let a = readings::stat().1.expect("switches");
        child.kill().unwrap();
        child.wait().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let b = readings::stat().1.expect("switches");
        assert!(b >= a, "{a} then {b}");
        let (_, threads) = readings::tasks();
        assert!(threads.is_some_and(|t| t >= 1), "{threads:?}");
    }

    /// The CPU's energy from RAPL: each package's total, its parts (and
    /// files a user may not read) not counted twice; the batteries' draw
    /// while discharging, in watts, from power or from current and voltage.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_cpu_energy_and_the_battery_draw_are_read() {
        use readings::{battery_draw, cpu_energy};
        let dir = std::env::temp_dir().join(format!("keepane-power-{}", std::process::id()));
        let file = |rel: &str, text: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        let cap = dir.join("powercap");
        assert_eq!(cpu_energy(&cap), None, "no such directory");
        file("powercap/intel-rapl:0/energy_uj", "1000000\n");
        file("powercap/intel-rapl:0:0/energy_uj", "999\n");
        file("powercap/intel-rapl:1/energy_uj", "500\n");
        file("powercap/intel-rapl/enabled", "1\n");
        assert_eq!(cpu_energy(&cap), Some(1_000_500), "two packages; a package's core part not again");
        file("powercap/intel-rapl:1/energy_uj", "denied");
        assert_eq!(cpu_energy(&cap), None, "a package unreadable: no figure, not a part of one");
        let ps = dir.join("power_supply");
        file("power_supply/AC/type", "Mains\n");
        file("power_supply/AC/online", "1\n");
        assert_eq!(battery_draw(&ps), None, "no battery");
        file("power_supply/BAT0/type", "Battery\n");
        file("power_supply/BAT0/status", "Charging\n");
        file("power_supply/BAT0/power_now", "20000000\n");
        assert_eq!(battery_draw(&ps), None, "charging: the battery runs nothing");
        file("power_supply/BAT0/status", "Discharging\n");
        assert_eq!(battery_draw(&ps), Some(20.0));
        file("power_supply/BAT1/type", "Battery\n");
        file("power_supply/BAT1/status", "Discharging\n");
        file("power_supply/BAT1/current_now", "-500000\n");
        file("power_supply/BAT1/voltage_now", "11000000\n");
        assert_eq!(battery_draw(&ps), Some(25.5), "20 W, and 0.5 A at 11 V");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The CPU's sensor, by kind: the package's first; none when no sensor
    /// is a CPU's, or its figure is no temperature.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_cpu_temperature_is_picked_from_the_sensors() {
        use readings::pick_temperature;
        let s = |k: &str, t: f32| (k.to_string(), t);
        assert_eq!(pick_temperature(&[s("acpitz", 27.8), s("coretemp", 61.0), s("x86_pkg_temp", 64.0)]), Some(64.0));
        assert_eq!(pick_temperature(&[s("nvme", 40.0), s("k10temp", 55.5)]), Some(55.5));
        assert_eq!(pick_temperature(&[s("acpitz", 27.8), s("pch_cannonlake", 50.0)]), None, "no CPU sensor");
        assert_eq!(
            pick_temperature(&[s("coretemp", -273.0), s("cpu_thermal", 48.2)]),
            Some(48.2),
            "a broken one skipped"
        );
        assert_eq!(pick_temperature(&[]), None);
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
