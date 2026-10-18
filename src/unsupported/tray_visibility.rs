//! Stub of `tray_visibility` for platforms without a tray backend yet
//! (D32): there is no tray icon, so closing quits instead of hiding.

pub fn icon_visible() -> bool {
    false
}
