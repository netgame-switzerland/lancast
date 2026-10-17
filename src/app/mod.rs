//! The GUI application (SPEC §3): eframe main window, tray icon and the
//! controller thread that owns the send session (D18).

mod carousel;
mod controller;
mod meter;
mod ui;

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow};
use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tracing::{info, warn};

use crate::{
    config::{ConfigStore, MIN_WINDOW_SIZE},
    os_caps::OsCaps,
    tray::{self, TrayCommand},
};
use controller::{AppState, AudioCheck, Command, Controller, NetworkStatus, Wake};

/// Runs the GUI until the user quits. Exits the process. `link` is an event
/// link from the command line (`lancast://…`, D27).
pub fn run(link: Option<String>) -> Result<()> {
    // One LANcast window per user: a second start shows the first one and
    // hands over the event link.
    if !tray::claim_single_instance(link.as_deref()) {
        info!("LANcast is already running; showing its window");
        return Ok(());
    }
    // Event links on websites need the lancast: scheme (per user, no admin).
    crate::url_scheme::ensure_registered();

    let caps = OsCaps::detect();
    info!("{caps}");
    let (store, config) = ConfigStore::load();
    info!(path = %store.path().display(), "settings file");

    let state = Arc::new(Mutex::new(AppState {
        network: NetworkStatus::default(),
        pending_link: None,
        caps,
        selected: None,
        audio_check: AudioCheck::Idle,
        preview: None,
        sending: None,
        notice: None,
    }));
    let (commands_tx, commands_rx) = crossbeam_channel::unbounded();
    // The egui context exists only once eframe has started.
    let egui_ctx: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let wake: Wake = {
        let egui_ctx = egui_ctx.clone();
        Arc::new(move || {
            if let Some(ctx) = egui_ctx.get() {
                ctx.request_repaint();
            }
        })
    };

    let (tray, _tray_thread) = tray::start({
        let commands = commands_tx.clone();
        let wake = wake.clone();
        move |command| match command {
            TrayCommand::WindowShown => wake(),
            TrayCommand::StartSending => {
                let _ = commands.send(Command::TrayStart);
            }
            TrayCommand::StopSending => {
                let _ = commands.send(Command::Stop);
            }
            TrayCommand::Quit => {
                let _ = commands.send(Command::Quit);
            }
            TrayCommand::OpenLink(url) => {
                let _ = commands.send(Command::OpenLink(url));
                wake();
            }
        }
    })?;
    if let Some(url) = link {
        let _ = commands_tx.send(Command::OpenLink(url));
    }
    tray.set_minimize_to_tray(config.to_tray);

    std::thread::Builder::new()
        .name("controller".into())
        .spawn({
            let state = state.clone();
            let tray = tray.clone();
            move || {
                Controller::new(state, tray.clone(), wake).run(commands_rx);
                // Quit: the session is stopped; remove the tray icon and leave.
                // eframe does not return while its window is hidden, so exit here.
                tray.exit();
                std::thread::sleep(std::time::Duration::from_millis(100));
                info!("LANcast exiting");
                std::process::exit(0);
            }
        })?;

    let (side, rgba) = crate::app_icon::icon_rgba(64);
    let icon = egui::IconData {
        rgba,
        width: side as u32,
        height: side as u32,
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("LANcast")
        .with_inner_size([config.window_width, config.window_height])
        .with_min_inner_size([MIN_WINDOW_SIZE.0, MIN_WINDOW_SIZE.1])
        .with_maximized(config.window_maximized)
        .with_icon(Arc::new(icon));
    // Restore the position only if it is still on a connected monitor.
    if let (Some(x), Some(y)) = (config.window_x, config.window_y)
        && crate::windows_list::is_on_screen(x, y, config.window_width)
    {
        viewport = viewport.with_position([x, y]);
    }
    if config.always_on_top {
        viewport = viewport.with_always_on_top();
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = eframe::run_native(
        "LANcast",
        options,
        Box::new(move |cc| {
            let _ = egui_ctx.set(cc.egui_ctx.clone());
            let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get()),
                _ => {
                    warn!("no Win32 window handle; closing to the tray is unavailable");
                    None
                }
            };
            if let Some(hwnd) = hwnd {
                tray.set_main_window(hwnd);
            }
            Ok(Box::new(ui::LancastApp::new(
                state,
                commands_tx.clone(),
                tray,
                hwnd,
                store,
                config,
            )))
        }),
    );
    result.map_err(|e| anyhow!("the window failed: {e}"))
}
