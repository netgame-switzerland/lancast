//! Controller thread: owns the send session and publishes [`AppState`] for
//! the UI and the tray. Runs independently of the UI, so stop-on-close and
//! notifications also work while the window is hidden (D18).

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, select};
use tracing::{debug, info, warn};

use crate::{
    capture::{
        audio_wasapi::{self, AudioLevels, AudioMode, AudioSource},
        video::{PreviewSlot, VideoOptions},
    },
    distro,
    event_link::{self, EventLink},
    ndi_config::{self, Discovery},
    os_caps::OsCaps,
    session::{POLL_INTERVAL, Preview, Session, SessionConfig, SessionEvent, SessionStatus},
    tray::{TrayHandle, TrayState},
    windows_list::{self, WindowInfo},
};

const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Result of the process-loopback pre-flight check (SPEC §3.1 item 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCheck {
    Idle,
    GameAudio,
    Unavailable(String),
}

/// How the user chose to send audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioChoice {
    Game,
    /// Explicitly confirmed after the warning.
    System,
    None,
}

/// User settings the UI edits and the controller uses on start.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub source_name: String,
    pub video: VideoOptions,
    pub audio_gain_db: f32,
    /// Discovery server choice, applied at the next send (D23).
    pub discovery: Discovery,
    /// Why sending is not allowed (event edition without a valid event
    /// configuration, D28); `None` = allowed.
    pub send_blocked: Option<String>,
}

/// An event link waiting for confirmation.
#[derive(Debug, Clone)]
pub struct PendingLink {
    pub link: EventLink,
    /// The signature was checked against this event edition's key.
    pub verified: bool,
}

/// Reachability of NDI discovery servers (SPEC §5.4, D14).
#[derive(Debug, Clone, Default)]
pub struct NetworkStatus {
    /// Discovery server(s) configured in NDI Access Manager.
    pub system_servers: Option<String>,
    /// `None` while checking.
    pub system_reachable: Option<bool>,
    /// LANcast's own discovery server setting that was last checked.
    pub own_servers: String,
    /// `None` while checking (or nothing set).
    pub own_reachable: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub level: NoticeLevel,
    pub text: String,
}

/// A running session as the UI sees it.
#[derive(Clone)]
pub struct Sending {
    pub source_name: String,
    pub audio_mode: Option<AudioMode>,
    /// Set when game audio was requested but failed after "Send".
    pub audio_error: Option<String>,
    pub levels: Option<Arc<AudioLevels>>,
    pub preview: Arc<PreviewSlot>,
    pub status: SessionStatus,
    pub minimized: bool,
    pub frames_missing: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub network: NetworkStatus,
    /// An event link waiting for the user's confirmation (D27, D28).
    pub pending_link: Option<PendingLink>,
    pub caps: OsCaps,
    pub selected: Option<WindowInfo>,
    pub audio_check: AudioCheck,
    /// Live preview of the selected window before sending.
    pub preview: Option<PreviewView>,
    pub sending: Option<Sending>,
    pub notice: Option<Notice>,
}

/// The preview capture as the UI sees it.
#[derive(Clone)]
pub struct PreviewView {
    pub preview: Arc<PreviewSlot>,
    pub levels: Option<Arc<AudioLevels>>,
}

pub enum Command {
    Select(WindowInfo),
    Settings(Settings),
    Start(AudioChoice),
    /// "Start sending" from the tray: game audio only, never system audio.
    TrayStart,
    Stop,
    SwitchToSystemAudio,
    /// Check whether these discovery servers are reachable (UI setting).
    CheckDiscovery(String),
    /// An event link (`lancast://…`) to confirm in the UI.
    OpenLink(String),
    /// The pending event link was applied or declined.
    DismissLink,
    Quit,
}

pub type SharedState = Arc<Mutex<AppState>>;
/// Wakes the UI (egui `request_repaint`); callable from any thread.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Controller {
    state: SharedState,
    tray: TrayHandle,
    /// Wakes the UI (egui `request_repaint`).
    wake: Wake,
    settings: Option<Settings>,
    session: Option<Session>,
    preview: Option<Preview>,
    last_status: Instant,
}

impl Controller {
    pub fn new(state: SharedState, tray: TrayHandle, wake: Wake) -> Self {
        Self {
            state,
            tray,
            wake,
            settings: None,
            session: None,
            preview: None,
            last_status: Instant::now(),
        }
    }

    /// Runs until `Quit`; stops the session cleanly before returning.
    pub fn run(mut self, commands: Receiver<Command>) {
        self.publish_tray();
        self.check_system_discovery();
        loop {
            select! {
                recv(commands) -> cmd => match cmd {
                    Ok(Command::Quit) | Err(_) => break,
                    Ok(cmd) => self.handle(cmd),
                },
                default(POLL_INTERVAL) => {}
            }
            self.poll_session();
            self.poll_preview();
        }
        if let Some(session) = self.session.take() {
            session.stop();
        }
        self.stop_preview();
        info!("controller stopped");
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Select(window) => self.select(window),
            Command::Settings(settings) => {
                let video_changed =
                    self.settings.as_ref().map(|s| &s.video) != Some(&settings.video);
                self.settings = Some(settings);
                self.publish_tray();
                // Border and cursor show in the preview right away.
                if video_changed && self.preview.is_some() {
                    self.start_preview();
                }
            }
            Command::Start(choice) => self.start(choice),
            Command::TrayStart => {
                let check = self.state().audio_check.clone();
                if check == AudioCheck::GameAudio {
                    self.start(AudioChoice::Game);
                } else {
                    // Never send system audio without the explicit choice in
                    // the window (SPEC §3.1 item 6).
                    self.set_notice(
                        NoticeLevel::Warning,
                        "Choose the audio option in the window before sending.",
                    );
                    self.tray.show_main_window();
                }
            }
            Command::Stop => self.stop("stopped by the user", NoticeLevel::Info),
            Command::SwitchToSystemAudio => self.switch_to_system_audio(),
            Command::CheckDiscovery(servers) => self.check_own_discovery(servers),
            Command::OpenLink(url) => match self.check_link(&url) {
                Ok(pending) => {
                    info!(link = ?pending.link, verified = pending.verified, "event link received");
                    self.state().pending_link = Some(pending);
                }
                Err(err) => {
                    warn!("event link rejected: {err:#}");
                    self.set_notice(
                        NoticeLevel::Warning,
                        &format!("Event link rejected: {err:#}"),
                    );
                }
            },
            Command::DismissLink => self.state().pending_link = None,
            Command::Quit => {}
        }
        (self.wake)();
    }

    fn select(&mut self, window: WindowInfo) {
        info!(
            pid = window.pid,
            process = window.process_name.as_deref().unwrap_or("?"),
            "window selected"
        );
        let check = match audio_wasapi::check_process_loopback(window.pid) {
            Ok(()) => AudioCheck::GameAudio,
            Err(err) => {
                warn!("game-only audio unavailable: {err:#}");
                AudioCheck::Unavailable(format!("{err:#}"))
            }
        };
        let mut state = self.state();
        state.selected = Some(window);
        state.audio_check = check;
        state.notice = None;
        drop(state);
        self.start_preview();
        self.publish_tray();
    }

    /// (Re)starts the preview of the selected window, unless sending.
    fn start_preview(&mut self) {
        self.stop_preview();
        if self.session.is_some() {
            return;
        }
        let state = self.state();
        let (Some(window), caps) = (state.selected.clone(), state.caps) else {
            return;
        };
        if windows_list::window_state(window.hwnd) == windows_list::WindowState::Gone {
            drop(state);
            self.state().selected = None;
            return;
        }
        // Meter only for game-only audio; system audio needs consent first.
        let audio = (state.audio_check == AudioCheck::GameAudio).then_some(AudioSource::Process {
            pid: window.pid,
            fallback_to_system: false,
        });
        drop(state);
        let video = self
            .settings
            .as_ref()
            .map(|s| s.video.clone())
            .unwrap_or_default();
        match Preview::start(&window, &video, audio, &caps) {
            Ok(preview) => {
                self.state().preview = Some(PreviewView {
                    preview: preview.preview.clone(),
                    levels: preview.levels(),
                });
                self.preview = Some(preview);
            }
            Err(err) => warn!("preview failed: {err:#}"),
        }
    }

    fn stop_preview(&mut self) {
        if let Some(preview) = self.preview.take() {
            preview.stop();
        }
        self.state().preview = None;
    }

    fn poll_preview(&mut self) {
        if self.preview.as_ref().is_some_and(|p| !p.is_running()) {
            info!("selected window closed");
            self.stop_preview();
            self.state().selected = None;
            (self.wake)();
        }
    }

    fn start(&mut self, choice: AudioChoice) {
        if self.session.is_some() {
            return;
        }
        let (Some(window), Some(settings)) = (self.state().selected.clone(), self.settings.clone())
        else {
            return;
        };
        if let Some(reason) = &settings.send_blocked {
            self.set_notice(NoticeLevel::Error, reason);
            return;
        }
        let caps = self.state().caps;
        let audio = match choice {
            AudioChoice::Game => Some(AudioSource::Process {
                pid: window.pid,
                fallback_to_system: false,
            }),
            AudioChoice::System => Some(AudioSource::System),
            AudioChoice::None => None,
        };
        let config = SessionConfig {
            window,
            source_name: settings.source_name.clone(),
            video: settings.video.clone(),
            audio,
            audio_gain_db: settings.audio_gain_db,
        };
        // NDI reads its configuration when the session initializes it.
        if let Err(err) = ndi_config::write_effective(&settings.discovery) {
            warn!("failed to apply the discovery setting: {err:#}");
        }
        // One capture of the window at a time: the session replaces the preview.
        self.stop_preview();
        match Session::start(&config, &caps) {
            Ok((session, audio_error)) => {
                let audio_error = audio_error.map(|e| format!("{e:#}"));
                if audio_error.is_some() {
                    self.tray.notify(
                        "LANcast: game audio failed",
                        "Sending without audio. Open LANcast to switch to system audio.",
                        true,
                    );
                } else if session.audio_mode() == Some(AudioMode::SystemLoopback) {
                    self.tray.notify(
                        "LANcast is sending ALL system audio",
                        "Everyone hears your Discord, music and notifications.",
                        true,
                    );
                }
                let mut state = self.state();
                state.sending = Some(Sending {
                    source_name: session.source_name().to_owned(),
                    audio_mode: session.audio_mode(),
                    audio_error,
                    levels: session.audio_levels(),
                    preview: session.preview.clone(),
                    status: SessionStatus::default(),
                    minimized: false,
                    frames_missing: false,
                });
                state.notice = None;
                drop(state);
                self.session = Some(session);
                self.last_status = Instant::now();
            }
            Err(err) => {
                warn!("failed to start sending: {err:#}");
                self.set_notice(
                    NoticeLevel::Error,
                    &format!("Could not start sending: {err:#}"),
                );
                self.start_preview();
            }
        }
        self.publish_tray();
    }

    fn stop(&mut self, reason: &str, level: NoticeLevel) {
        let Some(session) = self.session.take() else {
            return;
        };
        info!("stopping: {reason}");
        session.stop();
        let mut state = self.state();
        state.sending = None;
        state.notice = Some(Notice {
            level,
            text: format!("Sending stopped: {reason}."),
        });
        drop(state);
        // Back to the preview if the window still exists.
        self.start_preview();
        self.publish_tray();
    }

    fn switch_to_system_audio(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match session.set_audio(AudioSource::System) {
            Ok(()) => {
                let (mode, levels) = (session.audio_mode(), session.audio_levels());
                let mut state = self.state();
                if let Some(sending) = state.sending.as_mut() {
                    sending.audio_mode = mode;
                    sending.levels = levels;
                    sending.audio_error = None;
                }
                drop(state);
                self.tray.notify(
                    "LANcast is sending ALL system audio",
                    "Everyone hears your Discord, music and notifications.",
                    true,
                );
            }
            Err(err) => self.set_notice(
                NoticeLevel::Error,
                &format!("System audio failed too: {err:#}"),
            ),
        }
        self.publish_tray();
    }

    fn poll_session(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut changed = false;
        for event in session.poll() {
            changed = true;
            match event {
                SessionEvent::WindowClosed => {
                    self.tray.notify(
                        "LANcast stopped sending",
                        "The game window was closed.",
                        true,
                    );
                    self.stop("the game window was closed", NoticeLevel::Warning);
                    (self.wake)();
                    return;
                }
                other => debug!(?other, "session event"),
            }
        }
        if self.last_status.elapsed() >= STATUS_INTERVAL {
            self.last_status = Instant::now();
            let status = session.status();
            let (minimized, frames_missing) = (session.is_minimized(), session.frames_missing());
            if let Some(sending) = self.state().sending.as_mut() {
                sending.status = status;
                sending.minimized = minimized;
                sending.frames_missing = frames_missing;
            }
            self.publish_tray();
            changed = true;
        }
        if changed {
            (self.wake)();
        }
    }

    fn set_notice(&self, level: NoticeLevel, text: &str) {
        self.state().notice = Some(Notice {
            level,
            text: text.to_owned(),
        });
    }

    /// Parses an event link; an event edition only accepts links signed for
    /// it (D28).
    fn check_link(&self, url: &str) -> anyhow::Result<PendingLink> {
        let link = event_link::parse(url)?;
        if !distro::is_distro() {
            return Ok(PendingLink {
                link,
                verified: false,
            });
        }
        let Some(signed) = &link.signed else {
            anyhow::bail!("this event edition only accepts signed event links");
        };
        distro::verify_signature_for_this_build(signed)?;
        Ok(PendingLink {
            link,
            verified: true,
        })
    }

    /// Checks the discovery server from NDI Access Manager in the background.
    fn check_system_discovery(&self) {
        let servers = ndi_config::system_discovery();
        self.state().network.system_servers = servers.clone();
        let Some(servers) = servers else { return };
        info!(%servers, "NDI Access Manager sets a discovery server; checking it");
        let (state, wake) = (self.state.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let reachable = ndi_config::check_reachable(&servers)
                .iter()
                .all(|(_, ok)| *ok);
            if !reachable {
                warn!(%servers, "the configured NDI discovery server is not reachable");
            }
            lock(&state).network.system_reachable = Some(reachable);
            wake();
        });
    }

    /// Checks LANcast's own discovery server setting in the background.
    fn check_own_discovery(&self, servers: String) {
        let servers = servers.trim().to_owned();
        {
            let mut state = self.state();
            state.network.own_servers = servers.clone();
            state.network.own_reachable = None;
        }
        if servers.is_empty() {
            return;
        }
        let (state, wake) = (self.state.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let reachable = ndi_config::check_reachable(&servers)
                .iter()
                .all(|(_, ok)| *ok);
            let mut state = lock(&state);
            // Ignore results for a setting the user has changed meanwhile.
            if state.network.own_servers == servers {
                state.network.own_reachable = Some(reachable);
            }
            drop(state);
            wake();
        });
    }

    fn publish_tray(&self) {
        let state = self.state();
        let (tray_state, tooltip) = match &state.sending {
            Some(s) => {
                let audio = match s.audio_mode {
                    Some(AudioMode::SystemLoopback) => " – SYSTEM audio",
                    Some(AudioMode::ProcessLoopback { .. }) => "",
                    None => " – no audio",
                };
                let (w, h) = s.status.output_size;
                let tooltip = format!(
                    "LANcast – sending {}{audio}\n{w}×{h} {:.0} fps, {} receivers",
                    s.source_name, s.status.send_fps, s.status.receivers
                );
                let tray_state = if s.audio_mode == Some(AudioMode::SystemLoopback) {
                    TrayState::SendingSystemAudio
                } else {
                    TrayState::Sending
                };
                (tray_state, tooltip)
            }
            None if state.notice.as_ref().is_some_and(|n| {
                n.level == NoticeLevel::Error || n.level == NoticeLevel::Warning
            }) =>
            {
                (
                    TrayState::Error,
                    "LANcast – stopped (see window)".to_owned(),
                )
            }
            None => (TrayState::Idle, "LANcast – idle".to_owned()),
        };
        let can_start = state.selected.is_some()
            && state.audio_check == AudioCheck::GameAudio
            && self
                .settings
                .as_ref()
                .is_some_and(|s| s.send_blocked.is_none());
        let sending = state.sending.is_some();
        drop(state);
        self.tray
            .set_state(tray_state, &tooltip, can_start, sending);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, AppState> {
        lock(&self.state)
    }
}

fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, AppState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}
