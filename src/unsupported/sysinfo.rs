//! System measurements for the diagnostics: not available on this platform
//! yet (D32, D57).

use std::net::Ipv4Addr;

use crate::{
    bandwidth::LinkInfo,
    diagnostics::{CpuUsage, NetUsage},
};

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
    pub fn sample(&mut self, _preferred: Option<&str>) -> Option<NetUsage> {
        None
    }
}

pub fn link_to(_target: Ipv4Addr) -> Option<LinkInfo> {
    None
}

pub fn fastest_link() -> Option<LinkInfo> {
    None
}
