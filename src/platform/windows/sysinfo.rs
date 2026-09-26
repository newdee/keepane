//! The machine as the status line shows it, on Windows: CPU and memory in
//! use, the battery, the time since boot (Win32), the program a pane is
//! running (a toolhelp snapshot), and a directory with the home written `~`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::sysinfo::human_bytes;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;

/// One reading of everything system-wide, taken at most once a second
/// (every window's status context asks, several times a render).
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

struct CpuSample {
    idle: u64,
    total: u64,
}

struct Cache {
    at: Option<Instant>,
    system: System,
    cpu: Option<CpuSample>,
    /// The program of a pane, by the pane's process id, with when it was
    /// looked up.
    programs: HashMap<u32, (Instant, String)>,
    /// The process list, at most a second old: ten panes are ten lookups
    /// a second, not ten snapshots.
    procs: Option<(Instant, Vec<Proc>)>,
}

/// A process: its id, its parent's id, its executable's name.
type Proc = (u32, u32, String);

static CACHE: std::sync::LazyLock<Mutex<Cache>> = std::sync::LazyLock::new(|| {
    Mutex::new(Cache { at: None, system: System::new(), cpu: None, programs: HashMap::new(), procs: None })
});

impl System {
    const fn new() -> System {
        System {
            cpu_percentage: String::new(),
            ram_percentage: String::new(),
            ram_used: String::new(),
            battery_percentage: String::new(),
            battery_charging: false,
            uptime: 0,
        }
    }
}

fn filetime(ft: &FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// The current reading, refreshed when the last one is over a second old.
pub fn system() -> System {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if c.at.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
        return c.system.clone();
    }
    c.at = Some(Instant::now());
    // CPU: the share of time not idle since the previous sample.
    let (mut idle, mut kernel, mut user) = (
        FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
        FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
        FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
    );
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0 {
        // Kernel time includes idle time.
        let sample = CpuSample { idle: filetime(&idle), total: filetime(&kernel) + filetime(&user) };
        // The share busy since the last sample; for the very first reading
        // there is none, and the counters (which run from boot) give the
        // average since boot instead of a blank.
        let (total, idle) = match &c.cpu {
            Some(prev) => (sample.total.saturating_sub(prev.total), sample.idle.saturating_sub(prev.idle)),
            None => (sample.total, sample.idle),
        };
        if let Some(busy) = (100 * total.saturating_sub(idle)).checked_div(total) {
            c.system.cpu_percentage = format!("{busy}%");
        }
        c.cpu = Some(sample);
    }
    // Memory.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
        c.system.ram_percentage = format!("{}%", m.dwMemoryLoad);
        c.system.ram_used = human_bytes(m.ullTotalPhys.saturating_sub(m.ullAvailPhys));
    }
    // Battery: 255 is "unknown", which a desktop reports.
    let mut p: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { GetSystemPowerStatus(&mut p) } != 0 && p.BatteryLifePercent != 255 && p.BatteryFlag & 128 == 0 {
        c.system.battery_percentage = format!("{}%", p.BatteryLifePercent);
        c.system.battery_charging = p.BatteryFlag & 8 != 0 || p.ACLineStatus == 1;
    } else {
        c.system.battery_percentage.clear();
        c.system.battery_charging = false;
    }
    c.system.uptime = (unsafe { GetTickCount64() } / 1000) as i64;
    c.system.clone()
}

/// The long form of a path that has 8.3 short names in it
/// (`C:\Users\RUNNER~1` → `C:\Users\runneradmin`); the path as it was when
/// it has none or cannot be resolved (gone, no access).
fn long_path(p: &str) -> String {
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    if !p.contains('~') {
        return p.to_string();
    }
    let wide: Vec<u16> = p.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetLongPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return p.to_string();
    }
    String::from_utf16_lossy(&buf[..n])
}

/// This machine's name (`#H`).
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// `dir` with the home directory as `~`, the way prompts write it. Either
/// side may be written with 8.3 short names; both are compared long.
pub fn short_path(dir: &str) -> String {
    let Some(home) = std::env::var_os("USERPROFILE") else { return dir.to_string() };
    let home = long_path(&home.to_string_lossy());
    let long = long_path(dir);
    let dir = long.as_str();
    let rest = dir.strip_prefix(home.as_str()).or_else(|| {
        // Case does not matter on Windows.
        (dir.len() >= home.len() && dir[..home.len()].eq_ignore_ascii_case(&home)).then(|| &dir[home.len()..])
    });
    match rest {
        Some("") => "~".to_string(),
        Some(r) if r.starts_with(['\\', '/']) => format!("~{r}"),
        _ => dir.to_string(),
    }
}

/// The program a pane is running: its newest descendant process (`cargo`
/// under `pwsh` during a build), the pane's own process when it has none.
/// One process snapshot per pane per second.
pub fn program_of(pid: u32) -> String {
    {
        let c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, name)) = c.programs.get(&pid)
            && at.elapsed() < Duration::from_secs(1)
        {
            return name.clone();
        }
    }
    let name = newest_descendant(pid).unwrap_or_default();
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    c.programs.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
    c.programs.insert(pid, (Instant::now(), name.clone()));
    name
}

/// Every process as (pid, parent pid, exe name), from one snapshot.
fn processes() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return out;
    }
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while more {
        let end = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
        let name = String::from_utf16_lossy(&e.szExeFile[..end]);
        out.push((e.th32ProcessID, e.th32ParentProcessID, name));
        more = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    unsafe { CloseHandle(snap) };
    out
}

/// The name (without `.exe`) of the deepest, latest-listed descendant of
/// `pid`, or of `pid` itself. Snapshot order is creation order in practice,
/// so the last descendant found is the newest.
fn process_list() -> Vec<(u32, u32, String)> {
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

fn newest_descendant(pid: u32) -> Option<String> {
    let procs = process_list();
    let mut current = procs.iter().find(|(p, _, _)| *p == pid)?.2.clone();
    // Walk down: among the children of the current process take the last
    // listed; conhost.exe is ConPTY's own helper, not a program of ours.
    let mut at = pid;
    while let Some((child, _, name)) =
        procs.iter().rfind(|(_, parent, name)| *parent == at && !name.eq_ignore_ascii_case("conhost.exe"))
    {
        // A parent id can be reused by a later process: only go down to a
        // child listed after its parent.
        let ip = procs.iter().position(|(p, _, _)| *p == at);
        let ic = procs.iter().position(|(p, _, _)| *p == *child);
        if ic <= ip {
            break;
        }
        current = name.clone();
        at = *child;
    }
    Some(current.trim_end_matches(".exe").trim_end_matches(".EXE").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_come_back_and_are_cached() {
        let a = system();
        // The very first reading already has a CPU figure (since boot).
        assert!(a.cpu_percentage.ends_with('%'), "first reading: {a:?}");
        assert!(a.ram_percentage.ends_with('%'), "{a:?}");
        assert!(a.ram_used.ends_with('G') || a.ram_used.ends_with('M'), "{a:?}");
        assert!(a.uptime > 0);
        let b = system();
        assert_eq!(a, b, "within a second the same reading is handed back");
        std::thread::sleep(Duration::from_millis(1100));
        let c = system();
        assert!(c.cpu_percentage.ends_with('%'), "a second sample gives a CPU figure: {c:?}");
        let n: u32 = c.cpu_percentage.trim_end_matches('%').parse().unwrap();
        assert!(n <= 100);
        assert_eq!(human_bytes(812 * 1024 * 1024), "812M");
        assert_eq!(human_bytes(6_657_199_308), "6.2G");
    }

    #[test]
    fn home_becomes_a_tilde() {
        let home = std::env::var("USERPROFILE").unwrap();
        assert_eq!(short_path(&home), "~");
        assert_eq!(short_path(&format!("{home}\\src\\x")), "~\\src\\x");
        assert_eq!(short_path(&format!("{}\\src", home.to_uppercase())), "~\\src", "case does not matter");
        assert_eq!(short_path(r"C:\Windows"), r"C:\Windows");
        assert_eq!(short_path(&format!("{home}2\\x")), format!("{home}2\\x"), "a sibling directory is not home");
        // Home written with 8.3 short names (a CI runner's TEMP is
        // C:\Users\RUNNER~1\...): still home. Only where the volume keeps
        // short names; many do not, and then there is nothing to test.
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
        let wide: Vec<u16> = home.encode_utf16().chain(std::iter::once(0)).collect();
        let mut buf = vec![0u16; 1024];
        let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let short = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        if n > 0 && short.contains('~') {
            assert_eq!(short_path(&short), "~", "{short}");
            assert_eq!(short_path(&format!("{short}\\AppData")), "~\\AppData", "{short}");
        } else {
            eprintln!("no 8.3 name for {home}; skipped");
        }
        assert_eq!(long_path(r"Z:\NOSUCH~1\x"), r"Z:\NOSUCH~1\x", "unresolvable: as it was");
    }

    #[test]
    fn the_program_of_this_process_is_found() {
        let me = std::process::id();
        let name = program_of(me);
        // The test binary itself, or a child it has running (cargo's
        // test harness has none): never empty.
        assert!(!name.is_empty());
        assert!(!name.ends_with(".exe"));
        assert_eq!(program_of(me), name, "cached for a second");
        assert_eq!(program_of(0xFFFF_FFF0), "", "no such process");
    }
}
