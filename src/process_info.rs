//! Facts about this process from the OS: loaded DLLs, own TCP connections,
//! and waiting for another process to exit (D41).

use std::net::{Ipv4Addr, SocketAddrV4};

use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR, WAIT_FAILED},
        NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCP_STATE_ESTAB, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
            TCP_TABLE_OWNER_PID_CONNECTIONS,
        },
        Networking::WinSock::AF_INET,
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        },
    },
    core::HSTRING,
};

/// Whether this process has loaded the DLL `name` (e.g. a delay-loaded one).
pub fn module_loaded(name: &str) -> bool {
    // SAFETY: GetModuleHandleW only reads the NUL-terminated name; the
    // returned handle is not used (and needs no release).
    unsafe { GetModuleHandleW(&HSTRING::from(name)).is_ok() }
}

/// Remote addresses of this process's established IPv4 TCP connections.
pub fn own_tcp_connections() -> Vec<SocketAddrV4> {
    let pid = std::process::id();
    let mut size = 0u32;
    // Retry if the table grows between the size query and the read.
    for _ in 0..3 {
        // u32 elements keep the buffer aligned for MIB_TCPTABLE_OWNER_PID.
        let mut buffer = vec![0u32; (size as usize).div_ceil(4).max(1)];
        // SAFETY: `buffer` holds `size` writable bytes (rounded up), and
        // `size` is a valid out-pointer.
        let result = unsafe {
            GetExtendedTcpTable(
                Some(buffer.as_mut_ptr().cast()),
                &raw mut size,
                false,
                u32::from(AF_INET.0),
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER.0 {
            continue;
        }
        if result != NO_ERROR.0 {
            return Vec::new();
        }
        let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
        // SAFETY: on success the buffer holds a MIB_TCPTABLE_OWNER_PID with
        // `dwNumEntries` rows following the count, all inside `buffer`.
        let rows: &[MIB_TCPROW_OWNER_PID] = unsafe {
            std::slice::from_raw_parts((*table).table.as_ptr(), (*table).dwNumEntries as usize)
        };
        return rows
            .iter()
            .filter(|row| row.dwOwningPid == pid && row.dwState == MIB_TCP_STATE_ESTAB.0 as u32)
            .map(|row| {
                // Address and port are in network byte order; the port is
                // in the low 16 bits.
                let ip = Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes());
                let port = u16::from_be(row.dwRemotePort as u16);
                SocketAddrV4::new(ip, port)
            })
            .collect();
    }
    Vec::new()
}

/// Waits up to `timeout_ms` for the process `pid` to exit. Returns at once
/// if it does not exist (any more).
pub fn wait_for_exit(pid: u32, timeout_ms: u32) {
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else {
            return;
        };
        if WaitForSingleObject(process, timeout_ms) == WAIT_FAILED {
            tracing::warn!(pid, "waiting for the previous LANcast failed");
        }
        let _ = CloseHandle(process);
    }
}
