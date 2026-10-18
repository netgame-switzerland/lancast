//! Enumerates capturable top-level windows and maps window → PID → process
//! name.
//!
//! We use our own list instead of the WGC system picker because the picker
//! does not reveal the HWND and therefore not the PID that process-loopback
//! audio needs (SPEC §3).
//!
//! Backends: Win32 (`win32`); other platforms have a stub with an empty
//! window list (D32).

#[cfg(windows)]
mod win32;
#[cfg(windows)]
use win32 as backend;

/// A native top-level window handle: the raw HWND value on Windows. Stored
/// as an integer so it is `Send`.
pub type WindowHandle = isize;

/// A capturable top-level window.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// Native window handle (see [`WindowHandle`]).
    pub hwnd: WindowHandle,
    pub pid: u32,
    pub title: String,
    /// Executable file name, e.g. `game.exe`. `None` if the process cannot be
    /// queried (e.g. protected processes).
    pub process_name: Option<String>,
    /// Full path of the executable, for its icon.
    pub process_path: Option<String>,
    pub class_name: String,
    pub width: i32,
    pub height: i32,
    pub minimized: bool,
}

impl WindowInfo {
    /// Proposed NDI source name: the process name without `.exe`. NDI
    /// prepends the machine name itself (D4).
    pub fn default_source_name(&self) -> String {
        self.process_name
            .as_deref()
            .map(|name| name.strip_suffix(".exe").unwrap_or(name).to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "LANcast".to_owned())
    }
}

/// Returns visible, non-cloaked, unowned top-level windows that have a title,
/// in Z-order (topmost first).
pub fn list_windows() -> anyhow::Result<Vec<WindowInfo>> {
    let filter = process_filter();
    Ok(backend::list_windows()?
        .into_iter()
        .filter(|w| matches_filter(&filter, w))
        .collect())
}

/// Env var restricting the window list to some programs, e.g.
/// `LANCAST_WINDOW_FILTER=RocketLeague.exe,obs64.exe` (case-insensitive exe
/// names). For demos and screenshots without private windows.
pub const WINDOW_FILTER: &str = "LANCAST_WINDOW_FILTER";

fn process_filter() -> Vec<String> {
    std::env::var(WINDOW_FILTER)
        .map(|value| {
            value
                .split(',')
                .map(|name| name.trim().to_lowercase())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn matches_filter(filter: &[String], window: &WindowInfo) -> bool {
    filter.is_empty()
        || window
            .process_name
            .as_ref()
            .is_some_and(|name| filter.contains(&name.to_lowercase()))
}

/// Looks up a single window by its raw handle, applying the same visibility
/// rules as [`list_windows`].
pub fn window_by_hwnd(hwnd: WindowHandle) -> Option<WindowInfo> {
    backend::window_info(hwnd)
}

/// Resolves a CLI target: `0x…` (HWND) or a decimal PID (that process's
/// topmost capturable window).
pub fn resolve_target(target: &str) -> anyhow::Result<WindowInfo> {
    use anyhow::Context;
    if let Some(hex) = target.strip_prefix("0x") {
        let hwnd = isize::from_str_radix(hex, 16).context("invalid HWND")?;
        return window_by_hwnd(hwnd)
            .with_context(|| format!("no capturable window with HWND {target}"));
    }
    let pid: u32 = target.parse().context("expected 0xHWND or a decimal PID")?;
    list_windows()?
        .into_iter()
        .find(|w| w.pid == pid)
        .with_context(|| format!("no capturable window for PID {pid}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    /// The window no longer exists.
    Gone,
}

pub fn window_state(hwnd: WindowHandle) -> WindowState {
    backend::window_state(hwnd)
}

/// Whether the top part of a window at `(x, y)` with `width` (logical
/// units at the system DPI) lies on a connected monitor; used before
/// restoring a saved window position.
pub fn is_on_screen(x: f32, y: f32, width: f32) -> bool {
    backend::is_on_screen(x, y, width)
}

/// Stub for platforms without a window list backend yet (D32): no windows.
#[cfg(not(windows))]
mod backend {
    use super::{WindowHandle, WindowInfo, WindowState};

    pub fn list_windows() -> anyhow::Result<Vec<WindowInfo>> {
        Ok(Vec::new())
    }

    pub fn window_info(_hwnd: WindowHandle) -> Option<WindowInfo> {
        None
    }

    pub fn window_state(_hwnd: WindowHandle) -> WindowState {
        WindowState::Gone
    }

    /// Unknown: let the window manager place the window as saved.
    pub fn is_on_screen(_x: f32, _y: f32, _width: f32) -> bool {
        true
    }
}
