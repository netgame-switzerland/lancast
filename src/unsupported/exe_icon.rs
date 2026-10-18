//! Stub of `exe_icon` for platforms without a backend yet (D32): no icons,
//! so the carousel shows its placeholder.

pub fn exe_icon_rgba(_path: &str) -> Option<(usize, usize, Vec<u8>)> {
    None
}
