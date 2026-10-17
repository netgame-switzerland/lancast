//! Enumerates capturable top-level windows and maps HWND → PID → process name.
//!
//! We use our own list instead of the WGC system picker because the picker
//! does not reveal the HWND and therefore not the PID that process-loopback
//! audio needs (SPEC §3).

use std::{ffi::c_void, path::Path};

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, RECT},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetWindow, GetWindowLongPtrW,
            GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
            WS_EX_TOOLWINDOW,
        },
    },
    core::BOOL,
};

/// A capturable top-level window.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// Raw HWND value. Stored as an integer so the struct is `Send`.
    pub hwnd: isize,
    pub pid: u32,
    pub title: String,
    /// Executable file name, e.g. `game.exe`. `None` if the process cannot be
    /// queried (e.g. protected processes).
    pub process_name: Option<String>,
    pub class_name: String,
    pub width: i32,
    pub height: i32,
    pub minimized: bool,
}

impl WindowInfo {
    pub fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut c_void)
    }
}

/// Returns visible, non-cloaked, unowned top-level windows that have a title,
/// in Z-order (topmost first).
pub fn list_windows() -> windows::core::Result<Vec<WindowInfo>> {
    let mut hwnds: Vec<HWND> = Vec::new();
    // SAFETY: the callback only runs during this call and receives a pointer to
    // `hwnds`, which outlives it and is not otherwise accessed meanwhile.
    unsafe { EnumWindows(Some(collect_hwnd), LPARAM(&raw mut hwnds as isize))? };
    Ok(hwnds.into_iter().filter_map(window_info).collect())
}

/// Looks up a single window by its raw HWND value, applying the same filters
/// as [`list_windows`].
pub fn window_by_hwnd(hwnd: isize) -> Option<WindowInfo> {
    window_info(HWND(hwnd as *mut c_void))
}

unsafe extern "system" fn collect_hwnd(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<HWND>` passed by `list_windows`.
    let hwnds = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    hwnds.push(hwnd);
    true.into()
}

fn window_info(hwnd: HWND) -> Option<WindowInfo> {
    // SAFETY: all calls below only read window/process state; the HWND may be
    // stale, in which case the APIs fail and we skip the window.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || is_cloaked(hwnd) {
            return None;
        }
        // Owned windows are dialogs/popups; tool windows are not in Alt-Tab.
        if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
            return None;
        }
        if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW.0 as isize != 0 {
            return None;
        }

        let title = read_utf16(|buf| GetWindowTextW(hwnd, buf), 512);
        if title.is_empty() {
            return None;
        }
        let class_name = read_utf16(|buf| GetClassNameW(hwnd, buf), 256);

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
        if pid == 0 {
            return None;
        }

        let mut rect = RECT::default();
        GetWindowRect(hwnd, &raw mut rect).ok()?;

        Some(WindowInfo {
            hwnd: hwnd.0 as isize,
            pid,
            title,
            process_name: process_name(pid),
            class_name,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            minimized: IsIconic(hwnd).as_bool(),
        })
    }
}

/// Cloaked windows are invisible despite `WS_VISIBLE`, e.g. UWP frames that
/// are suspended or windows on another virtual desktop.
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: `cloaked` is a valid DWORD out-buffer of the size passed.
    let result = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

/// Executable name via `PROCESS_QUERY_LIMITED_INFORMATION`, which (unlike
/// `PROCESS_VM_READ`) is also granted for elevated processes.
fn process_name(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed before returning; the buffer and its size
    // are passed consistently.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &raw mut len,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}

fn read_utf16(read: impl FnOnce(&mut [u16]) -> i32, capacity: usize) -> String {
    let mut buf = vec![0u16; capacity];
    let len = read(&mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..len])
}
