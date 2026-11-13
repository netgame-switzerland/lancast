//! Win32 backend of [`super`]: enumerates top-level windows and maps
//! HWND → PID → executable path.

use std::{ffi::c_void, path::Path};

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, RECT},
        Graphics::{
            Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
            Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect},
        },
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
        UI::HiDpi::GetDpiForSystem,
        UI::WindowsAndMessaging::{
            EnumWindows, FindWindowExW, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetWindow,
            GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
            IsWindow, IsWindowVisible, WS_CAPTION, WS_EX_TOOLWINDOW,
        },
    },
    core::{BOOL, HSTRING, PCWSTR, w},
};

use super::{WindowHandle, WindowInfo, WindowState};

/// Window class of the frame (title bar, border) that `ApplicationFrameHost.exe`
/// draws around a UWP app.
const FRAME_CLASS: &str = "ApplicationFrameWindow";
const FRAME_CLASS_W: PCWSTR = w!("ApplicationFrameWindow");
/// Window class of a UWP app's content window, owned by the app's process.
const CORE_CLASS: PCWSTR = w!("Windows.UI.Core.CoreWindow");

/// Visible, non-cloaked, unowned top-level windows that have a title, in
/// Z-order (topmost first), plus full-screen UWP frames (D56).
pub(super) fn list_windows() -> windows::core::Result<Vec<WindowInfo>> {
    let mut hwnds: Vec<HWND> = Vec::new();
    // SAFETY: the callback only runs during this call and receives a pointer to
    // `hwnds`, which outlives it and is not otherwise accessed meanwhile.
    unsafe { EnumWindows(Some(collect_hwnd), LPARAM(&raw mut hwnds as isize))? };
    // EnumWindows only returns windows of desktop apps (Windows 8+); the frame
    // of a full-screen UWP app is missing, so look for frames separately.
    let mut after = None;
    // SAFETY: plain queries; a frame destroyed meanwhile ends the loop early.
    while let Ok(frame) = unsafe { FindWindowExW(None, after, FRAME_CLASS_W, None) } {
        if !hwnds.contains(&frame) {
            hwnds.push(frame);
        }
        after = Some(frame);
    }
    Ok(hwnds.into_iter().filter_map(hwnd_info).collect())
}

pub(super) fn window_info(hwnd: WindowHandle) -> Option<WindowInfo> {
    hwnd_info(HWND(hwnd as *mut c_void))
}

pub(super) fn window_state(hwnd: WindowHandle) -> WindowState {
    let hwnd = HWND(hwnd as *mut c_void);
    // SAFETY: all calls only query state and accept stale handles.
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            WindowState::Gone
        } else if IsIconic(hwnd).as_bool() || is_cloaked(hwnd) {
            // A cloaked window delivers no frames either, e.g. a full-screen
            // UWP app in the background (D56).
            WindowState::Minimized
        } else {
            WindowState::Normal
        }
    }
}

pub(super) fn is_on_screen(x: f32, y: f32, width: f32) -> bool {
    // SAFETY: plain queries; the rectangle is a local.
    unsafe {
        let scale = GetDpiForSystem() as f32 / 96.0;
        // The title bar region must be reachable, not just a corner.
        let rect = RECT {
            left: ((x + 40.0) * scale) as i32,
            top: (y * scale) as i32,
            right: ((x + (width - 40.0).max(80.0)) * scale) as i32,
            bottom: ((y + 30.0) * scale) as i32,
        };
        !MonitorFromRect(&rect, MONITOR_DEFAULTTONULL).is_invalid()
    }
}

unsafe extern "system" fn collect_hwnd(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<HWND>` passed by `list_windows`.
    let hwnds = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    hwnds.push(hwnd);
    true.into()
}

fn hwnd_info(hwnd: HWND) -> Option<WindowInfo> {
    // SAFETY: all calls below only read window/process state; the HWND may be
    // stale, in which case the APIs fail and we skip the window.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
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

        // A UWP frame belongs to ApplicationFrameHost.exe; name, icon and
        // audio come from the app's process instead (D56).
        let app_pid = if class_name == FRAME_CLASS {
            uwp_app_pid(hwnd, pid, &title)
        } else {
            None
        };
        let cloaked = is_cloaked(hwnd);
        // The shell cloaks a full-screen UWP app whenever it is in the
        // background, i.e. while LANcast is in front: keep it in the list.
        if cloaked && !(app_pid.is_some() && is_full_screen_frame(hwnd)) {
            return None;
        }
        let pid = app_pid.unwrap_or(pid);

        let process_path = process_path(pid);
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &raw mut rect).ok()?;

        Some(WindowInfo {
            hwnd: hwnd.0 as isize,
            pid,
            title,
            process_name: process_path.as_deref().and_then(file_name),
            process_path,
            class_name,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
            minimized: IsIconic(hwnd).as_bool() || cloaked,
        })
    }
}

/// The process of the UWP app inside the frame `frame` (owned by
/// `frame_pid`, ApplicationFrameHost.exe). Windowed, the app's CoreWindow is
/// a child of the frame; full screen, it is a top-level window with the same
/// title. `None` if neither is found (e.g. the app is suspended).
fn uwp_app_pid(frame: HWND, frame_pid: u32, title: &str) -> Option<u32> {
    let title = HSTRING::from(title);
    // SAFETY: plain queries; stale handles make the calls fail.
    unsafe {
        let child = FindWindowExW(Some(frame), None, CORE_CLASS, None).ok();
        let candidates = child.into_iter().chain(std::iter::successors(
            FindWindowExW(None, None, CORE_CLASS, &title).ok(),
            |&after| FindWindowExW(None, Some(after), CORE_CLASS, &title).ok(),
        ));
        candidates
            .map(|core| {
                let mut pid = 0u32;
                GetWindowThreadProcessId(core, Some(&raw mut pid));
                pid
            })
            .find(|&pid| pid != 0 && pid != frame_pid)
    }
}

/// A UWP frame in full screen has no caption (a windowed or maximized one
/// has `WS_CAPTION`, also while suspended in the background).
fn is_full_screen_frame(frame: HWND) -> bool {
    // SAFETY: plain query; a stale handle yields 0.
    let style = unsafe { GetWindowLongPtrW(frame, GWL_STYLE) };
    style & WS_CAPTION.0 as isize == 0
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

fn file_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Executable path via `PROCESS_QUERY_LIMITED_INFORMATION`, which (unlike
/// `PROCESS_VM_READ`) is also granted for elevated processes.
fn process_path(pid: u32) -> Option<String> {
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
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn read_utf16(read: impl FnOnce(&mut [u16]) -> i32, capacity: usize) -> String {
    let mut buf = vec![0u16; capacity];
    let len = read(&mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..len])
}
