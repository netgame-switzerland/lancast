//! System tray icon (SPEC §3.2) via `Shell_NotifyIconW`, on its own thread
//! with its own message loop (D18). Independent of the eframe event loop, so
//! state changes and notifications also work while the main window is hidden.

use std::{cell::RefCell, thread::JoinHandle};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use tracing::warn;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_WARNING,
                NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4,
                NOTIFYICONDATAW, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
                DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos,
                GetMessageW, GetSystemMetrics, HICON, IsIconic, IsWindowVisible, MF_GRAYED,
                MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
                RegisterWindowMessageW, SM_CXSMICON, SW_HIDE, SW_RESTORE, SW_SHOW,
                SetForegroundWindow, SetMenuDefaultItem, SetTimer, ShowWindow, TPM_NONOTIFY,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE, WM_APP,
                WM_CONTEXTMENU, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
            },
        },
    },
    core::{PCWSTR, w},
};

const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const WM_TRAY_UPDATE: u32 = WM_APP + 2;
const WM_TRAY_EXIT: u32 = WM_APP + 3;
const ICON_ID: u32 = 1;
const WATCH_TIMER: usize = 1;
/// `NIN_SELECT | NINF_KEY` from shellapi.h (missing in windows-rs).
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;

const MENU_SHOW: usize = 1;
const MENU_START: usize = 2;
const MENU_STOP: usize = 3;
const MENU_QUIT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Sending,
    /// Sending system audio (incl. Discord etc.): a distinct warning state.
    SendingSystemAudio,
    Error,
}

impl TrayState {
    /// Casting: a glowing red "REC" dot, top right.
    fn on_air(self) -> bool {
        matches!(self, Self::Sending | Self::SendingSystemAudio)
    }

    /// Attention (system audio, or stopped unexpectedly): an amber dot,
    /// bottom right.
    fn warning(self) -> bool {
        matches!(self, Self::SendingSystemAudio | Self::Error)
    }
}

/// Commands from the tray menu, delivered on the tray thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// The main window was shown (by left click or the menu).
    WindowShown,
    StartSending,
    StopSending,
    Quit,
}

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

/// Hides the main window when it was minimized and tray mode is on.
fn hide_if_minimized(inner: &mut Inner) {
    let Some(main) = inner.main_window else {
        return;
    };
    if !inner.minimize_to_tray {
        return;
    }
    // SAFETY: querying and hiding a (possibly stale) window.
    let hidden = unsafe {
        let main = hwnd(main);
        if IsIconic(main).as_bool() && IsWindowVisible(main).as_bool() {
            let _ = ShowWindow(main, SW_HIDE);
            true
        } else {
            false
        }
    };
    if hidden {
        tell_about_tray(inner);
    }
}

fn tell_about_tray(inner: &mut Inner) {
    if std::mem::replace(&mut inner.told_about_tray, true) {
        return;
    }
    let mut data = base_data(inner.hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    copy_wide(&mut data.szInfoTitle, "LANcast is still running");
    copy_wide(
        &mut data.szInfo,
        "It keeps running in the tray. Right-click the tray icon to stop or quit.",
    );
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

/// The tray icon for `state` as RGBA: the LANcast icon, plus a glowing red
/// REC dot (top right) while casting and an amber dot (bottom right) for
/// system audio or an error. Returns `(side, rgba)`.
pub fn tray_icon_rgba(size: usize, state: TrayState) -> (usize, Vec<u8>) {
    let (side, mut rgba) = crate::app_icon::icon_rgba(size);
    let s = side as f32;
    if state.warning() {
        let radius = s * 0.17;
        let ring = (s * 0.05).max(1.0);
        let c = s - radius - ring;
        draw_dot(&mut rgba, side, (c, c), radius, ring, AMBER_DOT, None);
    }
    if state.on_air() {
        let radius = s * 0.19;
        let ring = (s * 0.05).max(1.0);
        let glow = (s * 0.11).max(1.3);
        let inset = radius + ring + glow * 0.4;
        draw_dot(
            &mut rgba,
            side,
            (s - inset, inset),
            radius,
            ring,
            REC_DOT,
            Some(glow),
        );
    }
    (side, rgba)
}

/// REC dot: bright center, deeper red edge.
const REC_DOT: ([f32; 3], [f32; 3]) = ([255.0, 95.0, 80.0], [205.0, 20.0, 25.0]);
const AMBER_DOT: ([f32; 3], [f32; 3]) = ([255.0, 200.0, 60.0], [235.0, 150.0, 0.0]);
const RING: [f32; 3] = [18.0, 12.0, 14.0];
/// Halo around the REC dot; brighter than the dot's edge so it reads as light.
const GLOW: [f32; 3] = [255.0, 45.0, 35.0];

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

/// Paints an anti-aliased dot (radial gradient from the first to the second
/// color) with a dark contrast ring and an optional soft glow of width `glow`.
fn draw_dot(
    rgba: &mut [u8],
    side: usize,
    (cx, cy): (f32, f32),
    radius: f32,
    ring: f32,
    (center_color, edge_color): ([f32; 3], [f32; 3]),
    glow: Option<f32>,
) {
    let reach = radius + ring + glow.unwrap_or(0.0);
    for y in 0..side {
        for x in 0..side {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d > reach + 0.5 {
                continue;
            }
            // Layers from bottom to top: glow, ring, fill (straight alpha).
            let mut layer = [0.0f32; 4];
            if let Some(width) = glow {
                let t = ((d - radius - ring) / width).clamp(0.0, 1.0);
                let a = 0.65 * (1.0 - t).powi(2);
                layer = over([GLOW[0], GLOW[1], GLOW[2], a], layer);
            }
            let ring_cover = (radius + ring - d + 0.5).clamp(0.0, 1.0);
            layer = over([RING[0], RING[1], RING[2], ring_cover], layer);
            let fill_cover = (radius - d + 0.5).clamp(0.0, 1.0);
            let t = (d / radius).clamp(0.0, 1.0);
            let fill = [0, 1, 2].map(|c| center_color[c] + (edge_color[c] - center_color[c]) * t);
            layer = over([fill[0], fill[1], fill[2], fill_cover], layer);

            let px = &mut rgba[(y * side + x) * 4..][..4];
            let base = [
                f32::from(px[0]),
                f32::from(px[1]),
                f32::from(px[2]),
                f32::from(px[3]) / 255.0,
            ];
            let out = over(layer, base);
            for c in 0..3 {
                px[c] = out[c].round().clamp(0.0, 255.0) as u8;
            }
            px[3] = (out[3] * 255.0).round() as u8;
        }
    }
}

/// `top` over `bottom`, straight (non-premultiplied) RGBA with alpha 0..1.
fn over(top: [f32; 4], bottom: [f32; 4]) -> [f32; 4] {
    let alpha = top[3] + bottom[3] * (1.0 - top[3]);
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    let mix = |t: f32, b: f32| (t * top[3] + b * bottom[3] * (1.0 - top[3])) / alpha;
    [
        mix(top[0], bottom[0]),
        mix(top[1], bottom[1]),
        mix(top[2], bottom[2]),
        alpha,
    ]
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
