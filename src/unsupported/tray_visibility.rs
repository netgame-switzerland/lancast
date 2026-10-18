//! Stub of `tray_visibility` for platforms without a tray backend yet
//! (D32): there is no tray icon, so closing quits instead of hiding.

// Matched by the UI, but only `Hidden` is ever produced here.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPlacement {
    Shown,
    Overflow,
    Hidden,
}

pub fn icon_visible() -> bool {
    false
}

pub fn icon_placement() -> IconPlacement {
    IconPlacement::Hidden
}
