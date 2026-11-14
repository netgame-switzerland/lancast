//! System measurements for the diagnostics: not available on this platform
//! yet (D32, D57).

use crate::diagnostics::{CpuUsage, NetUsage};

pub fn refresh_rate(_hwnd: isize) -> Option<u32> {
    None
}

#[derive(Default)]
pub struct CpuMeter;

impl CpuMeter {
    pub fn sample(&mut self) -> Option<CpuUsage> {
        None
    }
}

#[derive(Default)]
pub struct NetMeter;

impl NetMeter {
    pub fn sample(&mut self) -> Option<NetUsage> {
        None
    }
}
