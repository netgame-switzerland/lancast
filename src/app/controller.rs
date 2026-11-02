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
        audio::{self, AudioLevels, AudioMode, AudioSource},
        video::{PreviewSlot, VideoOptions},
    },
    distro,
    event_link::{self, EventLink},
    event_state,
    ndi_config::{self, Discovery},
    os_caps::OsCaps,
    session::{POLL_INTERVAL, Preview, Session, SessionConfig, SessionEvent, SessionStatus},
    tray::{TrayHandle, TrayState},
    windows_list::{self, WindowInfo},
};

const STATUS_INTERVAL: Duration = Duration::from_secs(1);
/// How often the discovery servers are checked again (D48, D49): LANcast's
/// own or the event's always, the one from NDI Access Manager while it is
/// not reachable.
const DISCOVERY_RECHECK: Duration = Duration::from_secs(15);
/// NDI connects to the discovery servers within a moment of the start.
pub const DISCOVERY_SETTLE: Duration = Duration::from_secs(5);

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
    /// Discovery server choice, applied at the next send (D23, D41).
    pub discovery: Discovery,
    /// NDI send groups (D39), applied at the next send.
    pub groups: String,
    /// Why sending is not allowed (event edition without a valid event
    /// configuration, D28); `None` = allowed.
    pub send_blocked: Option<String>,
    /// The event's discovery servers (D49): checked right before going
    /// live; if they are not reachable, the cast does not start.
    pub required_servers: Option<String>,
}

/// An event link waiting for confirmation.
#[derive(Debug, Clone)]
pub struct PendingLink {
    pub link: EventLink,
    /// The signature was checked against this event edition's key.
    pub verified: bool,
    /// The link names a `source` (D44): what loading from there gave.
    pub source: Option<LinkSource>,
}

/// The configuration of a link's `source` (D44), by the source's host.
#[derive(Debug, Clone)]
pub enum LinkSource {
    /// `link` is the configuration loaded from there; it wins over the
    /// link's own.
    Loaded(String),
    /// Loading failed (the reason): `link` is the link's own configuration.
    Failed(String, String),
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
    /// Discovery servers NDI uses (`""` = mDNS) while its runtime is loaded,
    /// i.e. during a send; `None` otherwise (D41).
    pub in_use: Option<String>,
    /// Discovery servers the current setting asks for (`""` = mDNS).
    pub wanted: String,
}

impl NetworkStatus {
    /// The setting differs from what the loaded NDI runtime uses. While
    /// sending it applies at the next send; otherwise unloading failed and
    /// only a restart applies it.
    pub fn setting_pending(&self) -> bool {
        self.in_use
            .as_ref()
            .is_some_and(|used| *used != self.wanted)
    }
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
    pub started: Instant,
    /// `(server, connected)` for each discovery server in use; empty for
    /// mDNS (D41).
    pub discovery: Vec<(String, bool)>,
}

impl Sending {
    /// NDI may still be connecting to the discovery servers.
    pub fn settling(&self) -> bool {
        self.started.elapsed() < DISCOVERY_SETTLE
    }

    /// The discovery servers, if none of them is connected once settled:
    /// with a discovery server NDI does not announce via mDNS, so the stream
    /// reaches nobody (D49).
    pub fn reaching_nobody(&self) -> Option<String> {
        reaching_nobody(&self.discovery, self.settling())
    }
}

/// Reachability of the discovery servers as NDI's connections show it while
/// sending: reachable when all are connected (like the check), not
/// reachable when the cast reaches nobody; `None` otherwise (still
/// connecting, only some connected, or mDNS).
fn live_reachability(discovery: &[(String, bool)], reaching_nobody: bool) -> Option<bool> {
    if !discovery.is_empty() && discovery.iter().all(|(_, ok)| *ok) {
        Some(true)
    } else if reaching_nobody {
        Some(false)
    } else {
        None
    }
}

/// Whether two comma-separated server lists name the same servers.
fn same_servers(a: &str, b: &str) -> bool {
    let list = |s: &str| {
        let mut list: Vec<String> = s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        list.sort();
        list
    };
    let a = list(a);
    !a.is_empty() && a == list(b)
}

/// See [`Sending::reaching_nobody`].
pub fn reaching_nobody(discovery: &[(String, bool)], settling: bool) -> Option<String> {
    let none_connected = !discovery.is_empty() && discovery.iter().all(|(_, ok)| !ok);
    (none_connected && !settling).then(|| {
        discovery
            .iter()
            .map(|(server, _)| server.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    })
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
    /// Between "Go live" and the start of the cast: checking the event's
    /// discovery server and starting NDI (D49).
    pub going_live: bool,
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
    /// The selected window was clicked again: no selection, no preview.
    Deselect,
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
    /// A message from the UI for the status line.
    Notice(NoticeLevel, String),
    /// Stop, then start LANcast anew (applies the discovery setting, D41).
    Restart,
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
    /// The cast reached nobody at the last status update (D49).
    reaching_nobody: bool,
    /// When the discovery servers were last checked (D48).
    own_checked: Instant,
    system_checked: Instant,
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
            reaching_nobody: false,
            own_checked: Instant::now(),
            system_checked: Instant::now(),
        }
    }

    /// Runs until `Quit`; stops the session cleanly before returning.
    pub fn run(mut self, commands: Receiver<Command>) {
        self.publish_tray();
        self.check_system_discovery(false);
        let mut restart = false;
        loop {
            select! {
                recv(commands) -> cmd => match cmd {
                    Ok(Command::Quit) | Err(_) => break,
                    Ok(Command::Restart) => {
                        restart = true;
                        break;
                    }
                    Ok(cmd) => self.handle(cmd),
                },
                default(POLL_INTERVAL) => {}
            }
            self.poll_session();
            self.poll_preview();
            self.recheck_discovery();
        }
        if let Some(session) = self.session.take() {
            session.stop();
        }
        self.stop_preview();
        if restart {
            spawn_successor();
        }
        info!("controller stopped");
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Select(window) => self.select(window),
            Command::Deselect => self.deselect(),
            Command::Settings(settings) => {
                let video_changed =
                    self.settings.as_ref().map(|s| &s.video) != Some(&settings.video);
                if self.settings.as_ref().map(|s| &s.discovery) != Some(&settings.discovery) {
                    self.state().network.wanted = ndi_config::resolve(&settings.discovery);
                }
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
            Command::CheckDiscovery(servers) => self.check_own_discovery(servers, false),
            Command::OpenLink(url) => match self.check_link(&url) {
                Ok(pending) => {
                    info!(link = ?pending.link, verified = pending.verified, "event link received");
                    self.offer_link(pending);
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
            Command::Notice(level, text) => self.set_notice(level, &text),
            Command::Restart | Command::Quit => {}
        }
        (self.wake)();
    }

    fn select(&mut self, window: WindowInfo) {
        info!(
            pid = window.pid,
            process = window.process_name.as_deref().unwrap_or("?"),
            "window selected"
        );
        let check = match audio::check_process_loopback(window.pid) {
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

    fn deselect(&mut self) {
        if self.session.is_some() {
            return;
        }
        info!("window deselected");
        self.stop_preview();
        let mut state = self.state();
        state.selected = None;
        state.audio_check = AudioCheck::Idle;
        state.notice = None;
        drop(state);
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
        // "Going live..." until the cast runs or fails (D49).
        self.state().going_live = true;
        (self.wake)();
        self.start_checked(window, settings, choice);
        self.state().going_live = false;
        self.publish_tray();
    }

    /// Checks the discovery servers as of going live (D48); the event's
    /// must be reachable (D49). Then starts the cast.
    fn start_checked(&mut self, window: WindowInfo, settings: Settings, choice: AudioChoice) {
        self.check_system_discovery(true);
        let required = settings
            .required_servers
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match required {
            Some(servers) => {
                self.own_checked = Instant::now();
                let reachable = ndi_config::check_reachable(servers)
                    .iter()
                    .all(|(_, ok)| *ok);
                let shown = {
                    let mut state = self.state();
                    let shown = state.network.own_servers == servers;
                    if shown {
                        state.network.own_reachable = Some(reachable);
                    }
                    shown
                };
                if !reachable {
                    warn!(%servers, "not going live: the event's discovery server is not reachable");
                    // Normally the "Sending is disabled" banner shows it,
                    // and it goes away once the server is back; a notice
                    // would stay and contradict it.
                    if !shown {
                        self.set_notice(
                            NoticeLevel::Error,
                            &format!(
                                "Not live: the event's discovery server {servers} is not reachable."
                            ),
                        );
                    }
                    return;
                }
            }
            None => {
                let own = self.state().network.own_servers.clone();
                if !own.is_empty() {
                    self.check_own_discovery(own, true);
                }
            }
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
            groups: settings.groups.clone(),
            video: settings.video.clone(),
            audio,
            audio_gain_db: settings.audio_gain_db,
        };
        // NDI reads its configuration when the session loads it.
        if let Err(err) = ndi_config::apply(&settings.discovery) {
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
                let in_use = ndi_config::in_use();
                info!(discovery = ?in_use, "NDI discovery in use");
                let discovery = ndi_config::connections(in_use.as_deref().unwrap_or_default());
                let mut state = self.state();
                state.network.in_use = in_use;
                state.sending = Some(Sending {
                    source_name: session.source_name().to_owned(),
                    audio_mode: session.audio_mode(),
                    audio_error,
                    levels: session.audio_levels(),
                    preview: session.preview.clone(),
                    status: SessionStatus::default(),
                    minimized: false,
                    frames_missing: false,
                    started: Instant::now(),
                    discovery,
                });
                state.notice = None;
                drop(state);
                self.session = Some(session);
                self.last_status = Instant::now();
                self.reaching_nobody = false;
            }
            Err(err) => {
                warn!("failed to start sending: {err:#}");
                self.release_ndi();
                self.set_notice(
                    NoticeLevel::Error,
                    &format!("Could not start sending: {err:#}"),
                );
                self.start_preview();
            }
        }
    }

    fn stop(&mut self, reason: &str, level: NoticeLevel) {
        let Some(session) = self.session.take() else {
            return;
        };
        info!("stopping: {reason}");
        session.stop();
        self.release_ndi();
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

    /// Unloads the NDI runtime after a session, so the next one reads the
    /// discovery setting again (D41).
    fn release_ndi(&self) {
        ndi_config::unload();
        self.state().network.in_use = ndi_config::in_use();
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
            let mut state = self.state();
            let servers = state.network.in_use.clone().unwrap_or_default();
            if let Some(sending) = state.sending.as_mut() {
                sending.status = status;
                sending.minimized = minimized;
                sending.frames_missing = frames_missing;
                let discovery = ndi_config::connections(&servers);
                if discovery != sending.discovery {
                    info!(?discovery, "NDI discovery server connections");
                    sending.discovery = discovery;
                }
            }
            let nobody = state.sending.as_ref().and_then(Sending::reaching_nobody);
            // While sending, NDI's own connection is the most current
            // reachability of the servers in use; the settings show the same
            // as the status line instead of waiting for the next check (D49).
            let reachable = state
                .sending
                .as_ref()
                .and_then(|s| live_reachability(&s.discovery, nobody.is_some()));
            if let Some(reachable) = reachable
                && same_servers(&state.network.own_servers, &servers)
            {
                state.network.own_reachable = Some(reachable);
            }
            drop(state);
            if nobody.is_some() != self.reaching_nobody {
                self.reaching_nobody = nobody.is_some();
                match &nobody {
                    Some(servers) => {
                        warn!(%servers, "not live: no discovery server connected");
                        self.tray.notify(
                            "LANcast is NOT live",
                            &format!("The discovery server {servers} is not reachable."),
                            true,
                        );
                    }
                    None => info!("live again: a discovery server is connected"),
                }
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
    /// it (D28), including a personal user id (D38).
    fn check_link(&self, url: &str) -> anyhow::Result<PendingLink> {
        let mut link = event_link::parse(url)?;
        if !distro::is_distro() {
            // Personal user ids are for event editions only, which can
            // check them.
            if link.user.take().is_some() {
                info!("ignoring the personal id of the link (not an event edition)");
            }
            return Ok(PendingLink {
                link,
                verified: false,
                source: None,
            });
        }
        let Some(signed) = &link.signed else {
            anyhow::bail!("this event edition only accepts signed event links");
        };
        let payload = distro::verify_signature_for_this_build(signed)?;
        if let Some(user) = &link.user {
            distro::verify_user_for_this_build(user, &payload.event_id)?;
        }
        Ok(PendingLink {
            link,
            verified: true,
            source: None,
        })
    }

    /// Hands a link to the UI for confirmation. A verified link with a
    /// `source` (D44) is first replaced by the configuration loaded from
    /// there, in the background, so the dialog shows what will be in force.
    fn offer_link(&self, pending: PendingLink) {
        let source = pending
            .link
            .signed
            .as_ref()
            .filter(|_| pending.verified)
            .and_then(|signed| distro::decode_payload(signed).ok())
            .and_then(|payload| payload.source);
        let Some(url) = source else {
            self.state().pending_link = Some(pending);
            return;
        };
        info!(%url, "event link names a source; loading the configuration from there");
        let (state, wake) = (self.state.clone(), self.wake.clone());
        let spawned = std::thread::Builder::new()
            .name("event-link-source".into())
            .spawn(move || {
                let pending = with_source(pending, &url);
                lock(&state).pending_link = Some(pending);
                wake();
            });
        if let Err(err) = spawned {
            warn!("cannot start the event source thread: {err}");
        }
    }

    /// Checks the discovery servers again every [`DISCOVERY_RECHECK`]:
    /// LANcast's own or the event's always, so a server that goes away
    /// blocks "Go live" (D49); the one from NDI Access Manager while it is
    /// not reachable (D48). The last result stays shown meanwhile.
    fn recheck_discovery(&mut self) {
        let (own_servers, system_down) = {
            let network = &self.state().network;
            (
                network.own_servers.clone(),
                network.system_reachable == Some(false),
            )
        };
        if !own_servers.is_empty() && self.own_checked.elapsed() >= DISCOVERY_RECHECK {
            self.check_own_discovery(own_servers, true);
        }
        if system_down && self.system_checked.elapsed() >= DISCOVERY_RECHECK {
            self.check_system_discovery(true);
        }
    }

    /// Checks the discovery server from NDI Access Manager in the background.
    /// A `recheck` (D48) logs only a change.
    fn check_system_discovery(&mut self, recheck: bool) {
        self.system_checked = Instant::now();
        let servers = ndi_config::system_discovery();
        self.state().network.system_servers = servers.clone();
        let Some(servers) = servers else { return };
        if !recheck {
            info!(%servers, "NDI Access Manager sets a discovery server; checking it");
        }
        let (state, wake) = (self.state.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let reachable = ndi_config::check_reachable(&servers)
                .iter()
                .all(|(_, ok)| *ok);
            let mut state = lock(&state);
            let before = state.network.system_reachable.replace(reachable);
            drop(state);
            if before != Some(reachable) {
                if reachable {
                    info!(%servers, "the configured NDI discovery server is reachable");
                } else {
                    warn!(%servers, "the configured NDI discovery server is not reachable");
                }
            }
            wake();
        });
    }

    /// Checks LANcast's own (or the event's) discovery server setting in the
    /// background. A first check shows "checking…"; a `recheck` (D48) keeps
    /// the last result until the new one is in.
    fn check_own_discovery(&mut self, servers: String, recheck: bool) {
        self.own_checked = Instant::now();
        let servers = servers.trim().to_owned();
        {
            let mut state = self.state();
            state.network.own_servers = servers.clone();
            if !recheck {
                state.network.own_reachable = None;
            }
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
            if state.network.own_servers != servers {
                return;
            }
            let before = state.network.own_reachable.replace(reachable);
            drop(state);
            if before != Some(reachable) {
                info!(%servers, reachable, "discovery server checked");
            }
            wake();
        });
    }

    fn publish_tray(&self) {
        let state = self.state();
        let (tray_state, tooltip) = match &state.sending {
            Some(s) if s.reaching_nobody().is_some() => (
                TrayState::Error,
                format!(
                    "LANcast – NOT live: discovery server {} not reachable",
                    s.reaching_nobody().unwrap_or_default()
                ),
            ),
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

/// Starts a new LANcast that waits for this one to exit (single instance).
fn spawn_successor() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            warn!("cannot restart: {err}");
            return;
        }
    };
    let result = std::process::Command::new(exe)
        .env(super::RESTART_ENV, std::process::id().to_string())
        .spawn();
    match result {
        Ok(_) => info!("restarting LANcast"),
        Err(err) => warn!("cannot restart: {err}"),
    }
}

fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, AppState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Loads the configuration of the link's `source` (D44) and puts it in
/// place of the link's own; the user confirms it in the dialog, so it need
/// not be newer than the link. The personal id stays if it belongs to the
/// loaded event (D38). If loading fails, the link stays as it is.
fn with_source(mut pending: PendingLink, url: &str) -> PendingLink {
    let host = distro::source_host(url).to_owned();
    let loaded = event_state::fetch_event_file(url)
        .and_then(|file| event_state::fetched_link_for_this_build(&file, None))
        .and_then(|link| link.ok_or_else(|| anyhow::anyhow!("no configuration")));
    match loaded {
        Ok(mut link) => {
            info!(%url, event = ?link.event, "event configuration loaded for the link");
            let event_id = link
                .signed
                .as_ref()
                .and_then(|signed| distro::decode_payload(signed).ok())
                .map(|payload| payload.event_id)
                .unwrap_or_default();
            if let Some(user) = pending.link.user.take() {
                match distro::verify_user_for_this_build(&user, &event_id) {
                    Ok(_) => link.user = Some(user),
                    Err(err) => warn!("dropping the link's personal id: {err:#}"),
                }
            }
            pending.link = link;
            pending.source = Some(LinkSource::Loaded(host));
        }
        Err(err) => {
            warn!(%url, "cannot load the event configuration for the link: {err:#}");
            pending.source = Some(LinkSource::Failed(host, format!("{err:#}")));
        }
    }
    pending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndi_connections_update_the_reachability() {
        let server = |name: &str, ok| (name.to_owned(), ok);
        assert_eq!(live_reachability(&[server("a", true)], false), Some(true));
        assert_eq!(live_reachability(&[server("a", false)], true), Some(false));
        // Still connecting, only some connected, or mDNS: unchanged.
        assert_eq!(live_reachability(&[server("a", false)], false), None);
        let some = [server("a", true), server("b", false)];
        assert_eq!(live_reachability(&some, false), None);
        assert_eq!(live_reachability(&[], false), None);

        assert!(same_servers("10.0.0.5, 10.0.0.6", "10.0.0.6,10.0.0.5"));
        assert!(!same_servers("10.0.0.5", "10.0.0.6"));
        assert!(!same_servers("", ""));
    }
}
