//! Windows backend of [`super`]: the tray icon via `Shell_NotifyIconW`, on
//! its own thread with its own message loop (D18). Independent of the
//! eframe event loop, so state changes and notifications also work while
//! the main window is hidden.

use std::{cell::RefCell, thread::JoinHandle};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use tracing::{debug, warn};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM,
        },
        System::{
            DataExchange::COPYDATASTRUCT, LibraryLoader::GetModuleHandleW, Threading::CreateMutexW,
        },
        UI::{
            Shell::{
                NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_WARNING,
                NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_SELECT,
                NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AllowSetForegroundWindow, AppendMenuW, CreateIcon, CreatePopupMenu,
                CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
                DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics,
                GetWindowThreadProcessId, HICON, IsIconic, IsWindowVisible, MF_GRAYED,
                MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
                RegisterWindowMessageW, SM_CXSMICON, SMTO_ABORTIFHUNG, SW_HIDE, SW_RESTORE,
                SW_SHOW, SendMessageTimeoutW, SetForegroundWindow, SetMenuDefaultItem, SetTimer,
                ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
                WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU, WM_COPYDATA, WM_DESTROY, WM_TIMER,
                WNDCLASSW, WS_OVERLAPPED,
            },
        },
    },
    core::{PCWSTR, w},
};

use super::{TrayCommand, TrayState, tray_icon_rgba};

const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const WM_TRAY_UPDATE: u32 = WM_APP + 2;
const WM_TRAY_EXIT: u32 = WM_APP + 3;
/// Sent by a second LANcast instance: show the main window.
const WM_TRAY_ACTIVATE: u32 = WM_APP + 4;
const ICON_ID: u32 = 1;
const WATCH_TIMER: usize = 1;
/// `NIN_SELECT | NINF_KEY` from shellapi.h (missing in windows-rs).
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;

const MENU_SHOW: usize = 1;
const MENU_START: usize = 2;
const MENU_STOP: usize = 3;
const MENU_QUIT: usize = 4;

/// Marks our WM_COPYDATA messages (event links).
const COPYDATA_LINK: usize = 0x4C41_4E43; // "LANC"
const MAX_LINK_BYTES: u32 = 8192;

enum TrayUpdate {
    State {
        state: TrayState,
        tooltip: String,
        can_start: bool,
        sending: bool,
    },
    Notify {
        title: String,
        text: String,
        warning: bool,
    },
    MainWindow(isize),
    ShowMainWindow,
    MinimizeToTray(bool),
    HiddenToTray,
}

/// Handle to the tray thread; cheap to clone and usable from any thread.
#[derive(Clone)]
pub struct TrayHandle {
    hwnd: isize,
    tx: Sender<TrayUpdate>,
}

impl TrayHandle {
    pub fn set_state(&self, state: TrayState, tooltip: &str, can_start: bool, sending: bool) {
        self.send(TrayUpdate::State {
            state,
            tooltip: tooltip.to_owned(),
            can_start,
            sending,
        });
    }

    /// Shows a Windows notification (a toast on Windows 10/11).
    pub fn notify(&self, title: &str, text: &str, warning: bool) {
        self.send(TrayUpdate::Notify {
            title: title.to_owned(),
            text: text.to_owned(),
            warning,
        });
    }

    /// Tells the tray which window to show and hide.
    pub fn set_main_window(&self, hwnd: isize) {
        self.send(TrayUpdate::MainWindow(hwnd));
    }

    /// Whether minimizing the main window hides it to the tray. eframe does
    /// not run the UI for a minimized window, so the tray thread watches it.
    pub fn set_minimize_to_tray(&self, enabled: bool) {
        self.send(TrayUpdate::MinimizeToTray(enabled));
    }

    /// Reports that the main window was hidden to the tray; the first time,
    /// a notification explains where LANcast went.
    pub fn hidden_to_tray(&self) {
        self.send(TrayUpdate::HiddenToTray);
    }

    pub fn show_main_window(&self) {
        self.send(TrayUpdate::ShowMainWindow);
    }

    /// Removes the icon and ends the tray thread.
    pub fn exit(&self) {
        // SAFETY: posting to a window owned by the tray thread; harmless if
        // the window no longer exists.
        let _ = unsafe { PostMessageW(Some(hwnd(self.hwnd)), WM_TRAY_EXIT, WPARAM(0), LPARAM(0)) };
    }

    fn send(&self, update: TrayUpdate) {
        if self.tx.send(update).is_ok() {
            // SAFETY: as above.
            let _ = unsafe {
                PostMessageW(Some(hwnd(self.hwnd)), WM_TRAY_UPDATE, WPARAM(0), LPARAM(0))
            };
        }
    }
}

/// Starts the tray thread. `on_command` runs on that thread.
pub fn start(
    on_command: impl Fn(TrayCommand) + Send + 'static,
) -> Result<(TrayHandle, JoinHandle<()>)> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<isize>>(1);
    let thread = std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || run(rx, Box::new(on_command), &ready_tx))
        .context("failed to spawn the tray thread")?;
    let hwnd = ready_rx
        .recv()
        .map_err(|_| anyhow!("tray thread exited during start-up"))??;
    Ok((TrayHandle { hwnd, tx }, thread))
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

/// State owned by the tray thread, reached from the window procedure.
struct Inner {
    hwnd: HWND,
    rx: Receiver<TrayUpdate>,
    on_command: Box<dyn Fn(TrayCommand) + Send>,
    icons: [HICON; 4],
    state: TrayState,
    tooltip: String,
    can_start: bool,
    sending: bool,
    main_window: Option<isize>,
    minimize_to_tray: bool,
    /// The minimized main window was already handled (hidden or left on
    /// the taskbar) and has not been restored since.
    minimize_handled: bool,
    told_about_tray: bool,
    taskbar_created: u32,
}

thread_local! {
    static INNER: RefCell<Option<Inner>> = const { RefCell::new(None) };
}

fn run(
    rx: Receiver<TrayUpdate>,
    on_command: Box<dyn Fn(TrayCommand) + Send>,
    ready: &Sender<Result<isize>>,
) {
    // SAFETY: Win32 window creation and message loop on this thread; all
    // pointers are to locals that outlive the calls.
    unsafe {
        let created = (|| -> Result<HWND> {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: w!("lancast_tray"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(anyhow!("RegisterClassW failed"));
            }
            // A hidden top-level window (not message-only): the context menu
            // needs a window that can become foreground.
            Ok(CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("lancast_tray"),
                w!("LANcast tray"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )?)
        })();
        let window = match created {
            Ok(window) => window,
            Err(err) => {
                let _ = ready.send(Err(err));
                return;
            }
        };

        let size = GetSystemMetrics(SM_CXSMICON).max(16);
        let icons = [
            TrayState::Idle,
            TrayState::Sending,
            TrayState::SendingSystemAudio,
            TrayState::Error,
        ]
        .map(|state| make_icon(size, state).unwrap_or_default());
        let inner = Inner {
            hwnd: window,
            rx,
            on_command,
            icons,
            state: TrayState::Idle,
            tooltip: "LANcast – idle".to_owned(),
            can_start: false,
            sending: false,
            main_window: None,
            minimize_to_tray: true,
            minimize_handled: false,
            told_about_tray: false,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        };
        add_icon(&inner);
        SetTimer(Some(window), WATCH_TIMER, 250, None);
        INNER.with(|cell| *cell.borrow_mut() = Some(inner));
        let _ = ready.send(Ok(window.0 as isize));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }

        INNER.with(|cell| {
            if let Some(inner) = cell.borrow_mut().take() {
                let _ = Shell_NotifyIconW(NIM_DELETE, &base_data(inner.hwnd));
                for icon in inner.icons {
                    if !icon.is_invalid() {
                        let _ = DestroyIcon(icon);
                    }
                }
            }
        });
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by the system on the tray thread with valid arguments.
    unsafe {
        match msg {
            WM_TRAY_CALLBACK => {
                // NOTIFYICON_VERSION_4: the event is in the low word of lParam.
                match (lparam.0 & 0xFFFF) as u32 {
                    NIN_SELECT | NIN_KEYSELECT => toggle_main_window(),
                    // A click on one of our notifications: also works when
                    // Windows hides the icon itself.
                    NIN_BALLOONUSERCLICK => with_inner(|inner| {
                        if let Some(main) = inner.main_window {
                            show_window(main);
                            (inner.on_command)(TrayCommand::WindowShown);
                        }
                    }),
                    WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TRAY_UPDATE => {
                with_inner(apply_updates);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == WATCH_TIMER => {
                with_inner(hide_if_minimized);
                LRESULT(0)
            }
            WM_COPYDATA => {
                // An event link from a second LANcast instance.
                let data = lparam.0 as *const COPYDATASTRUCT;
                // SAFETY: for WM_COPYDATA the system guarantees a valid
                // COPYDATASTRUCT (and data) for the duration of the call.
                let link = (!data.is_null())
                    .then(|| &*data)
                    .filter(|d| d.dwData == COPYDATA_LINK && d.cbData <= MAX_LINK_BYTES)
                    .filter(|d| !d.lpData.is_null() && d.cbData % 2 == 0)
                    .map(|d| {
                        let units = std::slice::from_raw_parts(
                            d.lpData as *const u16,
                            d.cbData as usize / 2,
                        );
                        String::from_utf16_lossy(units)
                    });
                if let Some(link) = link {
                    with_inner(|inner| {
                        if let Some(main) = inner.main_window {
                            show_window(main);
                        }
                        (inner.on_command)(TrayCommand::OpenLink(link));
                    });
                    return LRESULT(1);
                }
                LRESULT(0)
            }
            WM_TRAY_ACTIVATE => {
                with_inner(|inner| {
                    if let Some(main) = inner.main_window {
                        show_window(main);
                        (inner.on_command)(TrayCommand::WindowShown);
                    }
                });
                LRESULT(0)
            }
            WM_TRAY_EXIT => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => {
                let recreate = INNER.with(|cell| {
                    cell.try_borrow()
                        .ok()
                        .and_then(|i| i.as_ref().map(|i| i.taskbar_created == msg && msg != 0))
                        .unwrap_or(false)
                });
                if recreate {
                    // Explorer restarted: the icon must be added again.
                    with_inner(|inner| add_icon(inner));
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
    }
}

/// Runs `f` on the tray state unless it is already borrowed (re-entrant
/// message during a modal loop), in which case the call is skipped.
fn with_inner(f: impl FnOnce(&mut Inner)) {
    INNER.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(inner) = guard.as_mut()
        {
            f(inner);
        }
    });
}

fn apply_updates(inner: &mut Inner) {
    while let Ok(update) = inner.rx.try_recv() {
        match update {
            TrayUpdate::State {
                state,
                tooltip,
                can_start,
                sending,
            } => {
                let changed = state != inner.state || tooltip != inner.tooltip;
                inner.state = state;
                inner.tooltip = tooltip;
                inner.can_start = can_start;
                inner.sending = sending;
                if changed {
                    let mut data = base_data(inner.hwnd);
                    data.uFlags = NIF_ICON | NIF_TIP | NIF_SHOWTIP;
                    data.hIcon = icon_for(inner);
                    copy_wide(&mut data.szTip, &inner.tooltip);
                    // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
                    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
                }
            }
            TrayUpdate::Notify {
                title,
                text,
                warning,
            } => {
                let mut data = base_data(inner.hwnd);
                data.uFlags = NIF_INFO;
                data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
                copy_wide(&mut data.szInfoTitle, &title);
                copy_wide(&mut data.szInfo, &text);
                // SAFETY: as above.
                let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
            }
            TrayUpdate::MainWindow(hwnd) => inner.main_window = Some(hwnd),
            TrayUpdate::MinimizeToTray(enabled) => inner.minimize_to_tray = enabled,
            TrayUpdate::HiddenToTray => tell_about_tray(inner),
            TrayUpdate::ShowMainWindow => {
                if let Some(main) = inner.main_window {
                    show_window(main);
                    (inner.on_command)(TrayCommand::WindowShown);
                }
            }
        }
    }
}

/// Hides the main window when it was minimized and tray mode is on. If
/// Windows hides the tray icon, minimizing stays normal, so the window
/// cannot vanish (D29).
fn hide_if_minimized(inner: &mut Inner) {
    let Some(main) = inner.main_window else {
        return;
    };
    if !inner.minimize_to_tray {
        return;
    }
    // SAFETY: only queries a (possibly stale) window.
    let minimized = unsafe {
        let main = hwnd(main);
        IsIconic(main).as_bool() && IsWindowVisible(main).as_bool()
    };
    if !minimized {
        inner.minimize_handled = false;
        return;
    }
    if std::mem::replace(&mut inner.minimize_handled, true) {
        return;
    }
    if crate::tray_visibility::icon_visible() {
        hide_window(main);
        tell_about_tray(inner);
    } else {
        debug!("tray icon hidden by Windows; minimized to the taskbar");
    }
}

fn tell_about_tray(inner: &mut Inner) {
    if std::mem::replace(&mut inner.told_about_tray, true) {
        return;
    }
    show_info(
        inner,
        "LANcast is still running in the tray",
        "It keeps running in the tray. Click here, the tray icon or start LANcast again to \
         reopen it. Right-click the tray icon to stop or quit.",
    );
}

fn show_info(inner: &Inner, title: &str, text: &str) {
    let mut data = base_data(inner.hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    copy_wide(&mut data.szInfoTitle, title);
    copy_wide(&mut data.szInfo, text);
    // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
}

fn icon_for(inner: &Inner) -> HICON {
    let index = match inner.state {
        TrayState::Idle => 0,
        TrayState::Sending => 1,
        TrayState::SendingSystemAudio => 2,
        TrayState::Error => 3,
    };
    inner.icons[index]
}

fn base_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    }
}

fn add_icon(inner: &Inner) {
    let mut data = base_data(inner.hwnd);
    data.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY_CALLBACK;
    data.hIcon = icon_for(inner);
    copy_wide(&mut data.szTip, &inner.tooltip);
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    // SAFETY: `data` is a fully initialized NOTIFYICONDATAW.
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            warn!("failed to add the tray icon");
        }
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
}

fn toggle_main_window() {
    let mut shown = false;
    with_inner(|inner| {
        let Some(main) = inner.main_window else {
            return;
        };
        // SAFETY: only queries the visibility of a (possibly stale) window.
        let visible = unsafe { IsWindowVisible(hwnd(main)).as_bool() };
        if visible {
            // SAFETY: hiding a window; harmless if the handle is stale.
            let _ = unsafe { ShowWindow(hwnd(main), SW_HIDE) };
        } else {
            show_window(main);
            shown = true;
        }
        if shown {
            (inner.on_command)(TrayCommand::WindowShown);
        }
    });
}

fn show_window(main: isize) {
    // SAFETY: showing and focusing a window; harmless if the handle is stale.
    unsafe {
        let main = hwnd(main);
        let _ = ShowWindow(main, SW_SHOW);
        if IsIconic(main).as_bool() {
            let _ = ShowWindow(main, SW_RESTORE);
        }
        let _ = SetForegroundWindow(main);
    }
}

fn show_menu(window: HWND) {
    // Copy what the menu needs and release the state: TrackPopupMenu runs a
    // modal loop that re-enters the window procedure.
    let mut flags = None;
    with_inner(|inner| flags = Some((inner.can_start && !inner.sending, inner.sending)));
    let Some((can_start, can_stop)) = flags else {
        return;
    };

    // SAFETY: standard popup-menu sequence on the tray thread; the menu is
    // destroyed before returning.
    let choice = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let enabled = |on: bool| if on { MF_STRING } else { MF_STRING | MF_GRAYED };
        let _ = AppendMenuW(menu, MF_STRING, MENU_SHOW, w!("Show LANcast"));
        let _ = AppendMenuW(menu, enabled(can_start), MENU_START, w!("Start sending"));
        let _ = AppendMenuW(menu, enabled(can_stop), MENU_STOP, w!("Stop sending"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));
        let _ = SetMenuDefaultItem(menu, MENU_SHOW as u32, 0);

        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(window);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            None,
            window,
            None,
        );
        let _ = DestroyMenu(menu);
        choice.0 as usize
    };

    match choice {
        MENU_SHOW => with_inner(|inner| {
            if let Some(main) = inner.main_window {
                show_window(main);
                (inner.on_command)(TrayCommand::WindowShown);
            }
        }),
        MENU_START => with_inner(|inner| (inner.on_command)(TrayCommand::StartSending)),
        MENU_STOP => with_inner(|inner| (inner.on_command)(TrayCommand::StopSending)),
        MENU_QUIT => with_inner(|inner| (inner.on_command)(TrayCommand::Quit)),
        _ => {}
    }
}

fn make_icon(size: i32, state: TrayState) -> Result<HICON> {
    let (side, mut rgba) = tray_icon_rgba(size as usize, state);
    // CreateIcon wants BGRA with straight alpha.
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    // Monochrome AND mask, rows padded to 16 bits; all zero = use alpha.
    let mask = vec![0u8; side.div_ceil(16) * 2 * side];
    let side = side as i32;
    // SAFETY: both buffers match the given size and bit depths.
    let icon = unsafe { CreateIcon(None, side, side, 1, 32, mask.as_ptr(), rgba.as_ptr())? };
    Ok(icon)
}

/// Claims the single-instance mutex. Returns `false` if another LANcast GUI
/// already runs; it is then asked to show its window and, if given, to open
/// the event `link` (SPEC §6, D27).
pub fn claim_single_instance(link: Option<&str>) -> bool {
    // SAFETY: plain Win32 calls; the mutex handle is intentionally kept open
    // for the lifetime of the process.
    unsafe {
        let Ok(mutex) = CreateMutexW(None, false, w!("Local\\LANcast.SingleInstance")) else {
            return true; // cannot tell; do not block startup
        };
        if GetLastError() != ERROR_ALREADY_EXISTS {
            // The handle stays open until the process exits.
            return true;
        }
        let _ = CloseHandle(mutex);
        // The other instance may still be starting: wait for its tray window.
        for _ in 0..20 {
            if let Ok(other) = FindWindowW(w!("lancast_tray"), PCWSTR::null())
                && !other.is_invalid()
            {
                let mut pid = 0u32;
                GetWindowThreadProcessId(other, Some(&raw mut pid));
                // Let the other process take the foreground.
                let _ = AllowSetForegroundWindow(pid);
                match link {
                    Some(link) => {
                        let units: Vec<u16> = link.encode_utf16().collect();
                        let data = COPYDATASTRUCT {
                            dwData: COPYDATA_LINK,
                            cbData: (units.len() * 2) as u32,
                            lpData: units.as_ptr() as *mut _,
                        };
                        // WM_COPYDATA must be sent (not posted); `units`
                        // lives until the call returns.
                        SendMessageTimeoutW(
                            other,
                            WM_COPYDATA,
                            WPARAM(0),
                            LPARAM(&raw const data as isize),
                            SMTO_ABORTIFHUNG,
                            3000,
                            None,
                        );
                    }
                    None => {
                        let _ = PostMessageW(Some(other), WM_TRAY_ACTIVATE, WPARAM(0), LPARAM(0));
                    }
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }
}

/// Hides a top-level window (the main window when closed to the tray).
pub fn hide_window(window: isize) {
    // SAFETY: hiding a window; harmless if the handle is stale.
    let _ = unsafe { ShowWindow(hwnd(window), SW_HIDE) };
}

/// Copies `text` into a fixed, NUL-terminated UTF-16 buffer, truncating.
fn copy_wide<const N: usize>(buffer: &mut [u16; N], text: &str) {
    let mut len = 0;
    for (slot, unit) in buffer.iter_mut().take(N - 1).zip(text.encode_utf16()) {
        *slot = unit;
        len += 1;
    }
    buffer[len] = 0;
}
