//! Update check and install in the background (D37). The UI reads
//! [`Updater::status`] every frame; network problems never block it.

use std::sync::{Arc, Mutex};

use eframe::egui;
use tracing::{info, warn};

use crate::update::{self, InstallMode, Update};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Not checked (turned off, or not available for this edition).
    Idle,
    Checking,
    UpToDate,
    /// The check failed (offline, no release yet, invalid manifest).
    CheckFailed(String),
    Available(Update),
    Downloading(Update),
    /// The setup runs; LANcast quits now.
    Installing,
    InstallFailed(Update, String),
}

pub struct Updater {
    status: Arc<Mutex<UpdateStatus>>,
    ctx: egui::Context,
    /// How this copy was installed (read once at startup).
    pub mode: InstallMode,
}

impl Updater {
    pub fn new(ctx: egui::Context) -> Self {
        // The setup of a previous update is no longer needed.
        let _ = std::fs::remove_dir_all(update::download_dir());
        Self {
            status: Arc::new(Mutex::new(UpdateStatus::Idle)),
            ctx,
            mode: update::install_mode(),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn busy(&self) -> bool {
        matches!(
            self.status(),
            UpdateStatus::Checking | UpdateStatus::Downloading(_) | UpdateStatus::Installing
        )
    }

    /// Checks `url` in the background.
    pub fn check(&self, url: &str) {
        if self.busy() || update::edition().is_none() {
            return;
        }
        let url = url.to_owned();
        self.spawn(UpdateStatus::Checking, move || match update::check(&url) {
            Ok(Some(update)) => {
                info!(version = update.version, "update available");
                UpdateStatus::Available(update)
            }
            Ok(None) => {
                info!(%url, "LANcast is up to date");
                UpdateStatus::UpToDate
            }
            Err(err) => {
                // Offline at a LAN party is normal: log it, start normally.
                info!(%url, "update check failed: {err:#}");
                UpdateStatus::CheckFailed(format!("{err:#}"))
            }
        });
    }

    /// Whether "Update now" can install `update` here.
    pub fn can_install(&self, update: &Update) -> bool {
        self.mode != InstallMode::Portable && update.setup.is_some() && update.manual.is_none()
    }

    /// Downloads and verifies the setup, then starts it. On success the
    /// status becomes `Installing` and the UI quits LANcast.
    pub fn install(&self, update: Update) {
        if self.busy() || !self.can_install(&update) {
            return;
        }
        let mode = self.mode;
        self.spawn(UpdateStatus::Downloading(update.clone()), move || {
            let result = update
                .setup
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no setup for this edition"))
                .and_then(|setup| update::download(&update, setup, &update::download_dir()))
                .and_then(|path| update::launch_setup(&path, mode));
            match result {
                Ok(()) => UpdateStatus::Installing,
                Err(err) => {
                    warn!("update failed: {err:#}");
                    UpdateStatus::InstallFailed(update, format!("{err:#}"))
                }
            }
        });
    }

    /// Runs `work` on a thread, showing `pending` until it returns.
    fn spawn(&self, pending: UpdateStatus, work: impl FnOnce() -> UpdateStatus + Send + 'static) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = pending;
        let status = self.status.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("update".into())
            .spawn(move || {
                let result = work();
                *status.lock().unwrap_or_else(|e| e.into_inner()) = result;
                ctx.request_repaint();
            });
        if let Err(err) = spawned {
            warn!("cannot start the update thread: {err}");
            *self.status.lock().unwrap_or_else(|e| e.into_inner()) =
                UpdateStatus::CheckFailed(err.to_string());
        }
    }
}
