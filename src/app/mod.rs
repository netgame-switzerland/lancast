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
    os_caps::OsCaps,
    tray::{self, TrayCommand},
};
use controller::{AppState, AudioCheck, Command, Controller};

/// Runs the GUI until the user quits. Exits the process.
pub fn run() -> Result<()> {
    let caps = OsCaps::detect();
    info!("{caps}");

    let state = Arc::new(Mutex::new(AppState {
        caps,
        selected: None,
        audio_check: AudioCheck::Idle,
        sending: None,
        notice: None,
    }));
    let (commands_tx, commands_rx) = crossbeam_channel::unbounded();
    // The egui context exists only once eframe has started.
    let egui_ctx: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let wake = {
        let egui_ctx = egui_ctx.clone();
        move || {
            if let Some(ctx) = egui_ctx.get() {
                ctx.request_repaint();
            }
        }
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
        }
    })?;

    std::thread::Builder::new()
        .name("controller".into())
        .spawn({
            let state = state.clone();
            let tray = tray.clone();
            move || {
                Controller::new(state, tray.clone(), Box::new(wake)).run(commands_rx);
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LANcast")
            .with_inner_size([620.0, 600.0])
            .with_min_inner_size([520.0, 480.0])
            .with_icon(Arc::new(icon)),
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
            )))
        }),
    );
    result.map_err(|e| anyhow!("the window failed: {e}"))
}
