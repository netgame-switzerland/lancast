//! Loads the event configuration from its `source` URL in the background
//! (D44): at startup and then at the interval the configuration sets (D45).
//! The UI checks and applies what arrives; network problems never block it.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use eframe::egui;
use tracing::warn;

use crate::{distro::EventFile, event_state};

/// What a load returned: the URL, and the file or why it failed.
pub type Loaded = (String, anyhow::Result<EventFile>);

pub struct SourceLoader {
    result: Arc<Mutex<Option<Loaded>>>,
    /// The URL being loaded.
    loading: Option<String>,
    /// The first load since LANcast started has been started.
    started: bool,
    /// The first load since LANcast started has not finished yet.
    first: bool,
    /// When to load again; `None` = not while running.
    next: Option<Instant>,
    ctx: egui::Context,
}

impl SourceLoader {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            result: Arc::new(Mutex::new(None)),
            loading: None,
            started: false,
            first: true,
            next: None,
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

    /// Whether the next load is due: the first one, or the scheduled one.
    pub fn due(&self) -> bool {
        self.loading.is_none()
            && (!self.started || self.next.is_some_and(|next| Instant::now() >= next))
    }

    /// When the next load is scheduled, for the UI's repaint timer.
    pub fn next(&self) -> Option<Instant> {
        self.next
    }

    /// Loads `url` on a thread; [`Self::take`] returns the result. The next
    /// load follows after `refresh`.
    pub fn start(&mut self, url: String, refresh: Option<Duration>) {
        self.started = true;
        self.loading = Some(url.clone());
        self.schedule(refresh);
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

    /// Loads again after `refresh` from now (`None` = not while running):
    /// after a load is started, or when the configuration in force changes
    /// its source or interval.
    pub fn schedule(&mut self, refresh: Option<Duration>) {
        self.next = refresh.map(|refresh| Instant::now() + refresh);
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

    /// No source: nothing to load or wait for until the configuration in
    /// force names one.
    pub fn none(&mut self) {
        self.started = true;
        self.first = false;
        self.next = None;
    }
}
