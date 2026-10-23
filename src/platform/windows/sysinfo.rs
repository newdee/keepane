//! The machine as the status line shows it, on Windows: CPU and memory in
//! use, the battery, the time since boot (Win32), the program a pane is
//! running (a toolhelp snapshot), and a directory with the home written `~`;
//! for the clock's animation, the cores, disks, network, temperature, CPU
//! power, battery draw and the system's own counts (see `counters`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::sysinfo::human_bytes;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

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

/// `pid` and every process under it, (pid, name without `.exe`), from the
/// snapshot (at most a second old); ConPTY's own conhost.exe left out. A
/// child is one listed after its parent (a parent's id can be reused).
pub fn tree_of(pid: u32) -> Vec<(u32, String)> {
    let procs = process_list();
    let name = |n: &str| n.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
    let Some(root) = procs.iter().position(|(p, _, _)| *p == pid) else { return Vec::new() };
    let mut out = vec![(pid, name(&procs[root].2))];
    let mut at = vec![(pid, root)];
    while let Some((parent, ip)) = at.pop() {
        for (i, (child, pp, n)) in procs.iter().enumerate() {
            if *pp == parent && i > ip && !n.eq_ignore_ascii_case("conhost.exe") {
                out.push((*child, name(n)));
                at.push((*child, i));
            }
        }
    }
    out
}

/// A process's CPU time so far (nanoseconds, user and kernel), the memory
/// it holds (working set, bytes) and when it started (Unix seconds); None
/// when it cannot be read.
pub fn usage_of(pid: u32) -> Option<(u64, u64, u64)> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let ft = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    let times = unsafe { GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user) } != 0;
    let mut mem: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let memory = unsafe { K32GetProcessMemoryInfo(h, &mut mem, mem.cb) } != 0;
    unsafe { CloseHandle(h) };
    // FILETIME counts 100 ns; as a moment, from 1601.
    let started = ft(created).saturating_sub(116_444_736_000_000_000) / 10_000_000;
    (times && memory).then(|| ((ft(kernel) + ft(user)) * 100, mem.WorkingSetSize as u64, started))
}

/// Every process as (pid, parent pid, exe name), from one snapshot.
pub(super) fn processes() -> Vec<(u32, u32, String)> {
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

/// The machine's running totals for the clock's animation: each core
/// (`NtQuerySystemInformation`), memory, the disks, context switches,
/// interrupts, processes, threads, the paging files and the CPU package's
/// energy (PDH, see `PDH_COUNTERS`), the network adapters (`GetIfTable2`),
/// the battery's draw and the CPU's temperature when a sensor program gives
/// it (see `temperature`). Each a call or two; asked twice a second at
/// most, and only while a clock is drawn.
pub fn counters() -> crate::sysinfo::Counters {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let memory = (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then_some((m.ullTotalPhys, m.ullAvailPhys));
    let pdh = pdh();
    let first = |i: usize| pdh[i].map(|(v, _)| v);
    let (net, packets) = net().unzip();
    crate::sysinfo::Counters {
        cores: cores(),
        memory,
        disk: first(0).zip(first(1)),
        net,
        temp: temperature(),
        switches: first(2),
        interrupts: first(3),
        processes: first(4),
        threads: first(5),
        // A raw fraction: (used, total).
        swap: pdh[6].map(|(used, total)| (total, total.saturating_sub(used))),
        // Picowatt-hours (3.6 nJ each): its "Power" counter agrees.
        cpu_energy: first(7).map(|pwh| pwh.saturating_mul(36) / 10_000),
        packets,
        battery_draw: battery_draw(),
    }
}

/// The PDH counters `counters` reads, by their English names (a Windows in
/// another language finds them too). The raw value of a "/sec" counter is
/// its running total; context switches and interrupts are 32-bit totals
/// that wrap (a wrap is one reading of nothing done). The energy is the
/// CPU's own meter (RAPL), there where Windows reads one (it does on an
/// Intel Core i9-13900KF, for any user); elsewhere it is missing, as any of
/// them may be.
const PDH_COUNTERS: [&str; 8] = [
    r"\PhysicalDisk(_Total)\Disk Read Bytes/sec",
    r"\PhysicalDisk(_Total)\Disk Write Bytes/sec",
    r"\System\Context Switches/sec",
    r"\Processor(_Total)\Interrupts/sec",
    r"\System\Processes",
    r"\System\Threads",
    r"\Paging File(_Total)\% Usage",
    r"\Energy Meter(rapl_package0_pkg)\Energy",
];

/// A PDH query for `PDH_COUNTERS`, opened once; a counter PDH would not
/// add is None.
struct PdhQuery {
    query: windows_sys::Win32::System::Performance::PDH_HQUERY,
    counters: Vec<Option<windows_sys::Win32::System::Performance::PDH_HCOUNTER>>,
}
// The handles are PDH's, used under the lock only.
unsafe impl Send for PdhQuery {}

/// The raw (first, second) value of each of `PDH_COUNTERS`, None where it
/// is missing or not valid now.
fn pdh() -> [Option<(u64, u64)>; PDH_COUNTERS.len()] {
    use windows_sys::Win32::System::Performance::{
        PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_RAW_COUNTER, PdhAddEnglishCounterW, PdhCollectQueryData,
        PdhGetRawCounterValue, PdhOpenQueryW,
    };
    let mut out = [None; PDH_COUNTERS.len()];
    // None: not tried yet; Some(None): PDH said no, and is not asked again.
    static QUERY: Mutex<Option<Option<PdhQuery>>> = Mutex::new(None);
    let mut q = QUERY.lock().unwrap_or_else(|e| e.into_inner());
    let q = q.get_or_insert_with(|| {
        let mut query = std::ptr::null_mut();
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 {
            return None;
        }
        let counters = PDH_COUNTERS
            .iter()
            .map(|path| {
                let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
                let mut c = std::ptr::null_mut();
                (unsafe { PdhAddEnglishCounterW(query, wide.as_ptr(), 0, &mut c) } == 0).then_some(c)
            })
            .collect();
        Some(PdhQuery { query, counters })
    });
    let Some(q) = q.as_ref() else { return out };
    if unsafe { PdhCollectQueryData(q.query) } != 0 {
        return out;
    }
    for (slot, c) in out.iter_mut().zip(&q.counters) {
        let Some(c) = c else { continue };
        let mut v = PDH_RAW_COUNTER::default();
        let ok = unsafe { PdhGetRawCounterValue(*c, std::ptr::null_mut(), &mut v) } == 0
            && (v.CStatus == PDH_CSTATUS_VALID_DATA || v.CStatus == PDH_CSTATUS_NEW_DATA);
        *slot = ok.then_some((v.FirstValue.max(0) as u64, v.SecondValue.max(0) as u64));
    }
    out
}

/// Watts the battery gives while the machine runs on it; None on mains
/// power, with no battery, or when the battery does not say.
fn battery_draw() -> Option<f32> {
    use windows_sys::Win32::System::Power::{CallNtPowerInformation, SYSTEM_BATTERY_STATE, SystemBatteryState};
    let mut s = SYSTEM_BATTERY_STATE::default();
    let size = std::mem::size_of::<SYSTEM_BATTERY_STATE>() as u32;
    let status = unsafe { CallNtPowerInformation(SystemBatteryState, std::ptr::null(), 0, (&raw mut s).cast(), size) };
    // Rate: milliwatts, negative while discharging; 0x80000000 is unknown.
    let rate = s.Rate as i32;
    (status >= 0 && s.BatteryPresent && s.Discharging && !s.AcOnLine && rate < 0 && rate != i32::MIN)
        .then(|| -rate as f32 / 1000.0)
}

/// Each core's (busy, total) time in 100 ns ticks, those of this process's
/// processor group (all of them, up to 64 cores).
fn cores() -> Vec<(u64, u64)> {
    use windows_sys::Wdk::System::SystemInformation::{
        NtQuerySystemInformation, SystemProcessorPerformanceInformation,
    };
    use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION as Info;
    let mut buf = vec![Info::default(); 64];
    let mut got = 0u32;
    let size = (buf.len() * std::mem::size_of::<Info>()) as u32;
    let status = unsafe {
        NtQuerySystemInformation(SystemProcessorPerformanceInformation, buf.as_mut_ptr().cast(), size, &mut got)
    };
    if status < 0 {
        return Vec::new();
    }
    buf.truncate(got as usize / std::mem::size_of::<Info>());
    // Kernel time includes idle time.
    buf.iter()
        .map(|i| {
            let total = (i.KernelTime.max(0) as u64).saturating_add(i.UserTime.max(0) as u64);
            (total.saturating_sub(i.IdleTime.max(0) as u64), total)
        })
        .collect()
}

/// Bytes, then packets, received and sent by the network adapters that are
/// hardware: not loopback, not a virtual switch (Hyper-V's, WSL's, which
/// carry what a real adapter carries again), not a filter layered on an
/// adapter (the same bytes once more).
#[allow(clippy::type_complexity)]
fn net() -> Option<((u64, u64), (u64, u64))> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_TABLE2,
    };
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
        return None;
    }
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let mut sum: Option<((u64, u64), (u64, u64))> = None;
    for r in rows {
        // HardwareInterface is the first bit, FilterInterface the second.
        let flags = r.InterfaceAndOperStatusFlags._bitfield;
        if flags & 1 != 0 && flags & 2 == 0 && r.Type != IF_TYPE_SOFTWARE_LOOPBACK {
            let ((rx, tx), (prx, ptx)) = sum.unwrap_or_default();
            let (pin, pout) =
                (r.InUcastPkts.saturating_add(r.InNUcastPkts), r.OutUcastPkts.saturating_add(r.OutNUcastPkts));
            sum = Some((
                (rx.saturating_add(r.InOctets), tx.saturating_add(r.OutOctets)),
                (prx.saturating_add(pin), ptx.saturating_add(pout)),
            ));
        }
    }
    unsafe { FreeMibTable(table.cast()) };
    sum
}

/// Where the temperature thread is.
struct TempState {
    /// When the clock last asked; the thread stops 5 s after.
    asked: Option<Instant>,
    running: bool,
    /// The last reading, and when it was taken.
    value: Option<(Instant, f32)>,
}

static TEMP: Mutex<TempState> = Mutex::new(TempState { asked: None, running: false, value: None });

/// The CPU's temperature, from LibreHardwareMonitor (or OpenHardwareMonitor)
/// while one runs. Windows has no real sensor to read without a driver: its
/// ACPI thermal zone is a fixed figure on many machines (27.9 °C on one we
/// looked at), so it is not used. WMI can take a while to answer, so a
/// thread of its own asks every 2 s, while the clock asked within 5 s; the
/// server never waits for it.
fn temperature() -> Option<f32> {
    let mut t = TEMP.lock().unwrap_or_else(|e| e.into_inner());
    t.asked = Some(Instant::now());
    if !t.running {
        t.running = std::thread::Builder::new().name("keepane-temperature".into()).spawn(temperature_thread).is_ok();
    }
    t.value.filter(|(at, _)| at.elapsed() < Duration::from_secs(10)).map(|(_, v)| v)
}

fn temperature_thread() {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    loop {
        let v = ["root\\LibreHardwareMonitor", "root\\OpenHardwareMonitor"].iter().find_map(|ns| {
            let rows =
                wmi_rows(ns, "SELECT Name, Identifier, Value FROM Sensor WHERE SensorType='Temperature'").ok()?;
            pick_temperature(&rows)
        });
        let mut t = TEMP.lock().unwrap_or_else(|e| e.into_inner());
        t.value = v.map(|v| (Instant::now(), v));
        if t.asked.is_none_or(|a| a.elapsed() > Duration::from_secs(5)) {
            t.running = false;
            break;
        }
        drop(t);
        std::thread::sleep(Duration::from_secs(2));
    }
    if com {
        unsafe { CoUninitialize() };
    }
}

/// The CPU's temperature among a sensor program's (name, identifier, °C):
/// the package's (Intel's "CPU Package", AMD's "Core (Tctl/Tdie)"), else
/// the hottest under a CPU, else none (a disk's or a GPU's is not the
/// machine's).
fn pick_temperature(rows: &[(String, String, f64)]) -> Option<f32> {
    let cpu: Vec<&(String, String, f64)> =
        rows.iter().filter(|(_, id, v)| id.contains("cpu") && v.is_finite() && *v > 0.0).collect();
    let package = cpu.iter().find(|(name, _, _)| name == "CPU Package" || name.starts_with("Core (Tctl"));
    package.map(|r| r.2).or_else(|| cpu.iter().map(|r| r.2).reduce(f64::max)).map(|v| v as f32)
}

/// (Name, Identifier, Value) of each row a WMI query gives.
fn wmi_rows(namespace: &str, query: &str) -> windows::core::Result<Vec<(String, String, f64)>> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, CoCreateInstance, CoSetProxyBlanket, CoTaskMemFree, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
        RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    use windows::Win32::System::Variant::{VARIANT, VariantClear, VariantToDouble, VariantToStringAlloc};
    use windows::Win32::System::Wmi::{
        IWbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE, WbemLocator,
    };
    use windows::core::{BSTR, PCWSTR, w};
    unsafe {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let services = locator.ConnectServer(
            &BSTR::from(namespace),
            &BSTR::new(),
            &BSTR::new(),
            &BSTR::new(),
            0,
            &BSTR::new(),
            None,
        )?;
        // RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE.
        CoSetProxyBlanket(
            &services,
            10,
            0,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )?;
        let rows = services.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(query),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?;
        let mut out = Vec::new();
        loop {
            let mut row = [None];
            let mut n = 0;
            let _ = rows.Next(WBEM_INFINITE, &mut row, &mut n);
            let Some(obj) = row[0].take().filter(|_| n == 1) else { break };
            let text = |name: PCWSTR| -> String {
                let mut v = VARIANT::default();
                if obj.Get(name, 0, &mut v, None, None).is_err() {
                    return String::new();
                }
                let s = match VariantToStringAlloc(&v) {
                    Ok(p) => {
                        let s = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.0 as _));
                        s
                    }
                    Err(_) => String::new(),
                };
                let _ = VariantClear(&mut v);
                s
            };
            let mut v = VARIANT::default();
            let value = match obj.Get(w!("Value"), 0, &mut v, None, None) {
                Ok(()) => VariantToDouble(&v).unwrap_or(f64::NAN),
                Err(_) => f64::NAN,
            };
            let _ = VariantClear(&mut v);
            out.push((text(w!("Name")), text(w!("Identifier")), value));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The running totals come back, and grow: the cores (as many as the
    /// machine has), memory, the disks, the network.
    #[test]
    fn the_counters_come_back_and_grow() {
        let a = counters();
        let n = std::thread::available_parallelism().map_or(1, |n| n.get()).min(64);
        assert_eq!(a.cores.len(), n, "{a:?}");
        assert!(a.cores.iter().all(|(busy, total)| busy <= total && *total > 0), "{a:?}");
        assert!(a.memory.is_some_and(|(t, av)| t > 0 && av <= t), "{a:?}");
        assert!(a.disk.is_some(), "PDH gives the disks: {a:?}");
        // Make the disk and the cores do something.
        let f = std::env::temp_dir().join(format!("keepane-counters-{}", std::process::id()));
        std::fs::write(&f, vec![7u8; 4 << 20]).unwrap();
        let _ = std::fs::File::open(&f).and_then(|f| f.sync_all());
        let _ = std::fs::remove_file(&f);
        let spin = Instant::now();
        while spin.elapsed() < Duration::from_millis(200) {
            std::hint::black_box(spin.elapsed());
        }
        let b = counters();
        assert!(a.cores.iter().zip(&b.cores).all(|(x, y)| y.1 >= x.1 && y.0 >= x.0), "never backwards");
        let (da, db) = (a.disk.unwrap(), b.disk.unwrap());
        assert!(db.0 >= da.0 && db.1 >= da.1, "{da:?} {db:?}");
        for (x, y) in [(a.net, b.net), (a.packets, b.packets)] {
            if let (Some(x), Some(y)) = (x, y) {
                assert!(y.0 >= x.0 && y.1 >= x.1, "{x:?} {y:?}");
            }
        }
        assert_eq!(a.net.is_some(), a.packets.is_some(), "the same adapters: {a:?}");
        let l = crate::sysinfo::load(&a, &b, 0.2);
        assert!(l.cpu > 0.0 && l.cpu <= 1.0, "a core was busy: {l:?}");
        // What every Windows has: context switches (the spin above made
        // some), interrupts, processes and threads, the paging files.
        assert!(l.switches.is_some_and(|s| s > 0.0), "{l:?}");
        assert!(l.interrupts.is_some(), "{l:?}");
        assert!(b.processes.is_some_and(|p| p > 10) && b.threads >= b.processes, "{b:?}");
        // The energy meter where the CPU has one (this machine's does; a
        // virtual machine's often not): a CPU package awake draws 1 W to
        // 1000 W (a figure in the wrong unit is thousands of times off).
        if l.cpu_power.is_some() {
            let spin = Instant::now();
            while spin.elapsed() < Duration::from_secs(1) {
                std::hint::black_box(spin.elapsed());
            }
            let c = counters();
            let w = crate::sysinfo::load(&b, &c, spin.elapsed().as_secs_f64()).cpu_power.unwrap();
            assert!((1.0..1000.0).contains(&w), "{w} W");
        } else {
            eprintln!("no CPU energy meter here");
        }
        assert_eq!(b.battery_draw.is_some(), battery_draw().is_some());
    }

    /// A sensor program's rows: the package wins, else the hottest CPU
    /// sensor; a GPU or a disk is never the CPU.
    #[test]
    fn the_cpu_temperature_is_picked_from_the_sensors() {
        let row = |n: &str, id: &str, v: f64| (n.to_string(), id.to_string(), v);
        let intel = [
            row("CPU Core #1", "/intelcpu/0/temperature/0", 61.0),
            row("CPU Package", "/intelcpu/0/temperature/4", 64.0),
            row("CPU Core #2", "/intelcpu/0/temperature/1", 70.0),
            row("GPU Core", "/gpu-nvidia/0/temperature/0", 80.0),
        ];
        assert_eq!(pick_temperature(&intel), Some(64.0));
        let amd =
            [row("Core (Tctl/Tdie)", "/amdcpu/0/temperature/2", 55.5), row("CCD1", "/amdcpu/0/temperature/3", 58.0)];
        assert_eq!(pick_temperature(&amd), Some(55.5));
        let cores =
            [row("Core #1", "/intelcpu/0/temperature/0", 50.0), row("Core #2", "/intelcpu/0/temperature/1", 52.0)];
        assert_eq!(pick_temperature(&cores), Some(52.0), "no package: the hottest core");
        let none =
            [row("GPU Core", "/gpu-nvidia/0/temperature/0", 80.0), row("Temperature", "/hdd/0/temperature/0", 35.0)];
        assert_eq!(pick_temperature(&none), None);
        assert_eq!(pick_temperature(&[row("CPU Package", "/intelcpu/0/temperature/4", f64::NAN)]), None);
    }

    /// WMI answers through the same code the sensor program is read with
    /// (a namespace every Windows has); a namespace that is not there is
    /// an error, not a hang or a panic.
    #[test]
    fn wmi_is_read_and_a_missing_namespace_is_none() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let rows = wmi_rows("root\\cimv2", "SELECT Name, DeviceID, NumberOfCores FROM Win32_Processor").unwrap();
        assert!(!rows.is_empty() && !rows[0].0.is_empty(), "{rows:?}");
        assert!(wmi_rows("root\\NoSuchNamespaceForKeepane", "SELECT Name FROM Sensor").is_err());
    }

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
