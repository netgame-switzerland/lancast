//! Stub of `process_info` for platforms other than Windows (D32).

use std::net::SocketAddrV4;

/// Unknown here: callers then behave as before the DLL is loaded.
pub fn module_loaded(_name: &str) -> bool {
    false
}

pub fn own_tcp_connections() -> Vec<SocketAddrV4> {
    Vec::new()
}

pub fn wait_for_exit(_pid: u32, _timeout_ms: u32) {}
