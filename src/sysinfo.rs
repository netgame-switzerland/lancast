//! System measurements for the diagnostics (D57): the refresh rate of the
//! game's monitor, CPU load, and the busiest network adapter. Read-only OS
//! queries; nothing touches the game.

use std::{collections::HashMap, time::Instant};

use windows::{
    Win32::{
        Foundation::{FILETIME, HWND, NO_ERROR},
        Graphics::Gdi::{
            DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW,
            MONITOR_DEFAULTTONULL, MONITORINFO, MONITORINFOEXW, MonitorFromWindow,
        },
        NetworkManagement::{
            IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2},
            Ndis::IfOperStatusUp,
        },
        System::Threading::{GetCurrentProcess, GetProcessTimes, GetSystemTimes},
    },
    core::PCWSTR,
};

use crate::diagnostics::{CpuUsage, NetUsage};

/// The refresh rate (Hz, rounded by Windows) of the monitor showing most of
/// `hwnd`, or `None` if it cannot be read.
pub fn refresh_rate(hwnd: isize) -> Option<u32> {
    // SAFETY: MonitorFromWindow accepts any HWND value (an invalid one gives
    // a null monitor with MONITOR_DEFAULTTONULL).
    let monitor = unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONULL) };
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` is a MONITORINFOEXW with cbSize set accordingly, which
    // GetMonitorInfoW may fill as such.
    if !unsafe { GetMonitorInfoW(monitor, &raw mut info.monitorInfo) }.as_bool() {
        return None;
    }
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: szDevice is the NUL-terminated device name from
    // GetMonitorInfoW; `mode` is a DEVMODEW with dmSize set.
    let ok = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(info.szDevice.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &raw mut mode,
        )
    };
    // 0 and 1 mean "the hardware default" (no actual rate).
    (ok.as_bool() && mode.dmDisplayFrequency > 1).then_some(mode.dmDisplayFrequency)
}

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// CPU load of LANcast and of the whole system between two samples.
pub struct CpuMeter {
    /// (process busy, system busy, system total), in 100 ns ticks.
    last: Option<(u64, u64, u64)>,
    /// Wall clock of the last sample, for the process share.
    at: Instant,
    cpus: f64,
}

impl Default for CpuMeter {
    fn default() -> Self {
        Self {
            last: None,
            at: Instant::now(),
            cpus: std::thread::available_parallelism().map_or(1, |n| n.get()) as f64,
        }
    }
}

impl CpuMeter {
    /// The load since the previous call; `None` on the first call.
    pub fn sample(&mut self) -> Option<CpuUsage> {
        let (mut idle, mut kernel, mut user) = Default::default();
        let (mut created, mut exited, mut p_kernel, mut p_user) = Default::default();
        // SAFETY: all pointers are valid FILETIME out-parameters;
        // GetCurrentProcess returns a pseudo handle that needs no closing.
        let read = unsafe {
            GetSystemTimes(
                Some(&raw mut idle),
                Some(&raw mut kernel),
                Some(&raw mut user),
            )
            .and_then(|()| {
                GetProcessTimes(
                    GetCurrentProcess(),
                    &raw mut created,
                    &raw mut exited,
                    &raw mut p_kernel,
                    &raw mut p_user,
                )
            })
        };
        read.ok()?;
        let now = Instant::now();
        // System kernel time includes idle time.
        let total = ticks(kernel) + ticks(user);
        let current = (ticks(p_kernel) + ticks(p_user), total - ticks(idle), total);
        let wall = now.duration_since(self.at).as_secs_f64();
        let usage = self.last.map(|(process, busy, total)| {
            let process_s = (current.0 - process) as f64 / 1e7;
            let total_delta = (current.2 - total).max(1) as f64;
            CpuUsage {
                // Like Task Manager: a share of all logical processors.
                process: (100.0 * process_s / (wall.max(1e-3) * self.cpus)) as f32,
                system: (100.0 * (current.1 - busy) as f64 / total_delta) as f32,
            }
        });
        self.last = Some(current);
        self.at = now;
        usage
    }
}

/// Outgoing traffic per physical network adapter between two samples; the
/// busiest one is the one carrying the stream.
#[derive(Default)]
pub struct NetMeter {
    /// Interface index → (sent bytes, send errors + discards).
    last: HashMap<u32, (u64, u64)>,
    at: Option<Instant>,
}

/// HardwareInterface and FilterInterface in MIB_IF_ROW2's flag bits.
const HARDWARE_INTERFACE: u8 = 1 << 0;
const FILTER_INTERFACE: u8 = 1 << 1;
/// IF_TYPE_IEEE80211.
const IF_TYPE_WIFI: u32 = 71;

impl NetMeter {
    /// The busiest connected physical adapter since the previous call;
    /// `None` on the first call or if there is none.
    pub fn sample(&mut self) -> Option<NetUsage> {
        let now = Instant::now();
        let rows = adapters();
        let secs = self.at.map(|at| now.duration_since(at).as_secs_f64());
        self.at = Some(now);
        let mut best: Option<(u64, NetUsage)> = None;
        let mut last = HashMap::new();
        for row in &rows {
            let errors = row.OutErrors + row.OutDiscards;
            last.insert(row.InterfaceIndex, (row.OutOctets, errors));
            let (Some(secs), Some(&(octets, errors_before))) =
                (secs, self.last.get(&row.InterfaceIndex))
            else {
                continue;
            };
            let sent = row.OutOctets.saturating_sub(octets);
            if best.as_ref().is_some_and(|(b, _)| *b >= sent) {
                continue;
            }
            let usage = NetUsage {
                name: wide(&row.Alias),
                description: wide(&row.Description),
                wifi: row.Type == IF_TYPE_WIFI,
                link_mbps: row.TransmitLinkSpeed / 1_000_000,
                out_mbps: sent as f64 * 8.0 / 1e6 / secs.max(1e-3),
                out_errors: errors.saturating_sub(errors_before),
            };
            best = Some((sent, usage));
        }
        self.last = last;
        best.map(|(_, usage)| usage)
    }
}

/// Connected physical adapters (no filter, virtual or loopback interfaces).
fn adapters() -> Vec<MIB_IF_ROW2> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: `table` is a valid out-pointer; on success it points to a
    // table allocated by the system, freed below with FreeMibTable.
    if unsafe { GetIfTable2(&raw mut table) } != NO_ERROR || table.is_null() {
        return Vec::new();
    }
    // SAFETY: on success the table holds `NumEntries` rows after the count;
    // the rows are copied out before the table is freed.
    unsafe {
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let picked = rows
            .iter()
            .filter(|r| {
                let flags = r.InterfaceAndOperStatusFlags._bitfield;
                flags & HARDWARE_INTERFACE != 0
                    && flags & FILTER_INTERFACE == 0
                    && r.OperStatus == IfOperStatusUp
            })
            .copied()
            .collect();
        FreeMibTable(table.cast());
        picked
    }
}

fn wide(text: &[u16]) -> String {
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}
