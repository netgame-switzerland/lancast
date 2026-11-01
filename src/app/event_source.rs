//! Loads the event configuration from its `source` URL in the background
//! (D44): at startup and every few minutes. The UI checks and applies what
//! arrives; network problems never block it.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use eframe::egui;
use tracing::warn;

use crate::{distro::EventFile, event_state};

/// How often the configuration is loaded again while LANcast runs.
pub const REFRESH: Duration = Duration::from_secs(5 * 60);

/// What a load returned: the URL, and the file or why it failed.
pub type Loaded = (String, anyhow::Result<EventFile>);

pub struct SourceLoader {
    result: Arc<Mutex<Option<Loaded>>>,
    /// The URL being loaded.
    loading: Option<String>,
    /// The first load since LANcast started has not finished yet.
    first: bool,
    last: Option<Instant>,
    ctx: egui::Context,
}

impl SourceLoader {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            result: Arc::new(Mutex::new(None)),
            loading: None,
            first: true,
            last: None,
            ctx,
        }
    }

    /// The host of the first load since the start, while it runs.
    pub fn loading_first(&self) -> Option<&str> {
        self.loading
            .as_deref()
            .filter(|_| self.first)
            .map(crate::distro::source_host)
    }

    /// Whether the next load is due: none yet, or [`REFRESH`] since the last.
    pub fn due(&self) -> bool {
        self.loading.is_none() && self.last.is_none_or(|t| t.elapsed() >= REFRESH)
    }

    /// Loads `url` on a thread; [`Self::take`] returns the result.
    pub fn start(&mut self, url: String) {
        self.loading = Some(url.clone());
        self.last = Some(Instant::now());
        let result = self.result.clone();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("event-source".into())
            .spawn(move || {
                let file = event_state::fetch_event_file(&url);
                *result.lock().unwrap_or_else(|e| e.into_inner()) = Some((url, file));
                ctx.request_repaint();
            });
        if let Err(err) = spawned {
            warn!("cannot start the event source thread: {err}");
            self.loading = None;
            self.first = false;
        }
    }

    /// The result of the load started last, once it is there.
    pub fn take(&mut self) -> Option<Loaded> {
        let loaded = self
            .result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        self.loading = None;
        self.first = false;
        Some(loaded)
    }

    /// No source (any more): nothing to wait for. Checked again after
    /// [`REFRESH`].
    pub fn none(&mut self) {
        self.first = false;
        self.last = Some(Instant::now());
    }
}
