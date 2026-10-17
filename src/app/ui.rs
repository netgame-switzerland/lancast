//! The main window (SPEC §3.1). Renders [`AppState`] and sends commands to
//! the controller; holds no session state of its own.
//!
//! Layout: carousel at the top, the live preview with the audio meter
//! filling the space below it (scales with the window), and at the bottom
//! the source name, status, send buttons and the settings panel.

use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;
use eframe::egui::{
    self, Color32, ColorImage, RichText, Sense, TextureHandle, TextureOptions, ViewportCommand,
    WindowLevel, pos2, vec2,
};

use super::{
    carousel::Carousel,
    controller::{AppState, AudioCheck, AudioChoice, Command, NoticeLevel, Settings, SharedState},
    meter::Meter,
};
use crate::{
    capture::{
        audio_wasapi::{AudioLevels, AudioMode},
        video::{PreviewSlot, VideoOptions},
    },
    config::{Config, ConfigStore},
    ndi_config::Discovery,
    ndi_out::AUDIO_GAIN_STUDIO_MONITOR_DB,
    tray::{self, TrayHandle},
    windows_list::WindowInfo,
};

const MAX_SIZES: [(u32, u32); 4] = [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440)];
const AMBER: Color32 = Color32::from_rgb(235, 160, 0);
const RED: Color32 = Color32::from_rgb(215, 60, 50);
/// Width reserved for the meter next to the preview.
const METER_WIDTH: f32 = 58.0;
const MIN_PREVIEW_HEIGHT: f32 = 90.0;
/// Settings are written at most this often while being edited.
const SAVE_DELAY: Duration = Duration::from_millis(700);

pub struct LancastApp {
    state: SharedState,
    commands: Sender<Command>,
    tray: TrayHandle,
    hwnd: Option<isize>,
    machine_name: String,
    carousel: Carousel,
    settings: Settings,
    sent_settings: Option<Settings>,
    /// Closing (×) and minimizing hide the window to the tray (SPEC §3.2).
    to_tray: bool,
    always_on_top: bool,
    /// Discovery server setting as typed (checked when editing ends).
    discovery_server: String,
    ignore_system_discovery: bool,
    /// Persistent settings (`LANcast.toml`) and when they last changed.
    config: Config,
    store: ConfigStore,
    changed_at: Option<Instant>,
    /// Process key of the selected window, for remembering source names.
    selected_process: Option<String>,
    meter: Meter,
    preview: Option<TextureHandle>,
}

impl LancastApp {
    pub fn new(
        state: SharedState,
        commands: Sender<Command>,
        tray: TrayHandle,
        hwnd: Option<isize>,
        store: ConfigStore,
        config: Config,
    ) -> Self {
        let app = Self {
            state,
            commands,
            tray,
            hwnd,
            machine_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "THIS-PC".to_owned()),
            carousel: Carousel::new(hwnd),
            settings: Settings {
                source_name: String::new(),
                video: VideoOptions {
                    fps_cap: config.fps_cap,
                    max_size: (config.max_width, config.max_height),
                    show_border: config.show_border,
                    capture_cursor: config.capture_cursor,
                },
                audio_gain_db: config.audio_gain_db,
                discovery: Discovery::System,
            },
            sent_settings: None,
            to_tray: config.to_tray,
            always_on_top: config.always_on_top,
            discovery_server: config.discovery_server.clone(),
            ignore_system_discovery: config.ignore_system_discovery,
            config,
            store,
            changed_at: None,
            selected_process: None,
            meter: Meter::default(),
            preview: None,
        };
        if !app.discovery_server.is_empty() {
            app.send(Command::CheckDiscovery(app.discovery_server.clone()));
        }
        app
    }

    /// Copies the UI state (and the window geometry) into the persistent
    /// settings and saves them shortly after the last change.
    fn sync_config(&mut self, ctx: &egui::Context) {
        let mut config = self.config.clone();
        let (inner, outer, minimized, maximized) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.minimized, v.maximized)
        });
        if minimized != Some(true) {
            config.window_maximized = maximized == Some(true);
            // Keep the normal size and position while maximized.
            if maximized != Some(true)
                && let (Some(inner), Some(outer)) = (inner, outer)
            {
                config.window_width = inner.width().round();
                config.window_height = inner.height().round();
                config.window_x = Some(outer.min.x.round());
                config.window_y = Some(outer.min.y.round());
            }
        }
        config.to_tray = self.to_tray;
        config.always_on_top = self.always_on_top;
        config.fps_cap = self.settings.video.fps_cap;
        (config.max_width, config.max_height) = self.settings.video.max_size;
        config.show_border = self.settings.video.show_border;
        config.capture_cursor = self.settings.video.capture_cursor;
        config.audio_gain_db = self.settings.audio_gain_db;
        config.discovery_server = self.discovery_server.trim().to_owned();
        config.ignore_system_discovery = self.ignore_system_discovery;
        if config != self.config {
            self.config = config;
            self.changed_at = Some(Instant::now());
        }
        if self.changed_at.is_some_and(|t| t.elapsed() >= SAVE_DELAY) {
            self.store.save(&self.config);
            self.changed_at = None;
        }
    }

    /// Effective discovery choice: own server, else ignore, else system.
    fn discovery(&self) -> Discovery {
        let servers = self.discovery_server.trim();
        if !servers.is_empty() {
            Discovery::Servers(servers.to_owned())
        } else if self.ignore_system_discovery {
            Discovery::Mdns
        } else {
            Discovery::System
        }
    }

    fn select(&mut self, window: WindowInfo) {
        let key = process_key(&window);
        self.settings.source_name = key
            .as_ref()
            .and_then(|k| self.config.source_names.get(k).cloned())
            .unwrap_or_else(|| window.default_source_name());
        self.selected_process = key;
        self.send(Command::Select(window));
    }

    /// Starts sending and remembers the source name for this game.
    fn start(&mut self, choice: AudioChoice) {
        if let Some(key) = &self.selected_process {
            let name = self.settings.source_name.trim().to_owned();
            self.config.source_names.insert(key.clone(), name);
            self.store.save(&self.config);
        }
        self.send(Command::Start(choice));
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    fn hide_to_tray(&mut self, state: &AppState) {
        if let Some(slot) = preview_slot(state) {
            slot.enabled.store(false, Ordering::Relaxed);
        }
        self.carousel.hide();
        if let Some(hwnd) = self.hwnd {
            tray::hide_window(hwnd);
        }
        self.tray.hidden_to_tray();
    }
}

/// The preview being shown: the session's while sending, otherwise the
/// preview capture of the selected window.
fn preview_slot(state: &AppState) -> Option<&Arc<PreviewSlot>> {
    match (&state.sending, &state.preview) {
        (Some(sending), _) => Some(&sending.preview),
        (None, Some(preview)) => Some(&preview.preview),
        (None, None) => None,
    }
}

fn audio_levels(state: &AppState) -> Option<&AudioLevels> {
    match (&state.sending, &state.preview) {
        (Some(sending), _) => sending.levels.as_deref(),
        (None, Some(preview)) => preview.levels.as_deref(),
        (None, None) => None,
    }
}

impl eframe::App for LancastApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();

        // × goes to the tray if enabled (minimize is handled by the tray
        // thread); otherwise × quits.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if self.to_tray && self.hwnd.is_some() {
                self.hide_to_tray(&state);
            } else {
                self.send(Command::Quit);
            }
            return;
        }

        self.settings.discovery = self.discovery();
        self.sync_config(&ctx);
        if self.sent_settings.as_ref() != Some(&self.settings) {
            self.sent_settings = Some(self.settings.clone());
            self.send(Command::Settings(self.settings.clone()));
        }

        match preview_slot(&state) {
            Some(slot) => {
                slot.enabled.store(true, Ordering::Relaxed);
                if let Some(image) = slot.take() {
                    let image = ColorImage::from_rgba_unmultiplied(image.size, &image.rgba);
                    match &mut self.preview {
                        Some(texture) => texture.set(image, TextureOptions::LINEAR),
                        None => {
                            self.preview =
                                Some(ctx.load_texture("preview", image, TextureOptions::LINEAR))
                        }
                    }
                }
            }
            None => self.preview = None,
        }
        self.meter.update(audio_levels(&state));

        egui::Panel::bottom("footer").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("NDI® is a registered trademark of Vizrt NDI AB.")
                        .small()
                        .weak(),
                );
                ui.hyperlink_to(RichText::new("ndi.video").small(), "https://ndi.video/");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(concat!("LANcast ", env!("CARGO_PKG_VERSION")))
                            .small()
                            .weak(),
                    );
                    let dir = crate::config::data_dir();
                    let mode = if crate::config::is_portable() {
                        "portable mode: LANcast.toml next to the exe"
                    } else {
                        "per user"
                    };
                    if ui
                        .link(RichText::new("Settings & logs folder").small())
                        .on_hover_text(format!("{} ({mode})", dir.display()))
                        .clicked()
                    {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                    }
                });
            });
        });

        egui::Panel::bottom("controls").show(ui, |ui| {
            ui.add_space(6.0);
            self.discovery_warning(ui, &state);
            self.source_name(ui, &state);
            ui.add_space(4.0);
            self.audio_and_status(ui, &state);
            ui.add_space(6.0);
            self.send_controls(ui, &state);
            ui.add_space(8.0);
            self.settings_panel(ui, &state);
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if state.sending.is_some() {
                ui.horizontal(|ui| {
                    lock_icon(ui, AMBER);
                    ui.label(
                        RichText::new(
                            "Window selection is locked while casting – press Stop to choose another window",
                        )
                        .strong()
                        .size(15.0)
                        .color(AMBER),
                    );
                });
            } else {
                ui.label(
                    RichText::new("Choose the window to cast")
                        .strong()
                        .size(15.0),
                );
            }
            ui.add_space(4.0);
            let selected = state.selected.as_ref().map(|w| w.hwnd);
            let locked = state.sending.is_some();
            if let Some(window) = self.carousel.show(ui, selected, locked) {
                self.select(window);
            }
            ui.add_space(8.0);
            self.preview_and_meter(ui, &state);
        });

        // Animate the meter and preview; otherwise the controller wakes us.
        let live = state.sending.is_some() || state.preview.is_some();
        let interval = if live { 33 } else { 1000 };
        ctx.request_repaint_after(std::time::Duration::from_millis(interval));
    }
}

impl LancastApp {
    /// The live preview and the meter, centered, filling the remaining space.
    fn preview_and_meter(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let available = ui.available_size();
        if state.selected.is_none() && state.sending.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Select a window above to see its live preview.").weak());
            });
            return;
        }
        let aspect = self.preview.as_ref().map_or(16.0 / 9.0, |t| {
            let [w, h] = t.size();
            w as f32 / h.max(1) as f32
        });
        let max_width = (available.x - METER_WIDTH - 12.0).max(80.0);
        let height = (available.y - 4.0)
            .min(max_width / aspect)
            .max(MIN_PREVIEW_HEIGHT);
        let width = (height * aspect).min(max_width);
        let total = width + 12.0 + METER_WIDTH;

        ui.horizontal(|ui| {
            ui.add_space(((available.x - total) / 2.0).max(0.0));
            let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            // Produce the preview at the size it is shown, in physical pixels.
            if let Some(slot) = preview_slot(state) {
                let ppp = ui.ctx().pixels_per_point();
                slot.set_display_size(
                    (rect.width() * ppp).round() as u32,
                    (rect.height() * ppp).round() as u32,
                );
            }
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, Color32::from_gray(16));
            if let Some(texture) = &self.preview {
                let [w, h] = texture.size();
                let scale = (rect.width() / w as f32).min(rect.height() / h as f32);
                let image_rect =
                    egui::Rect::from_center_size(rect.center(), vec2(w as f32 * scale, h as f32 * scale));
                painter.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            let message = match &state.sending {
                Some(s) if s.minimized => Some("Window minimized – last frame stays on air"),
                Some(s) if s.frames_missing => Some(
                    "No picture. Is the game in exclusive fullscreen?\nPlease use borderless windowed mode.",
                ),
                _ if self.preview.is_none() => Some("Waiting for the picture…"),
                _ => None,
            };
            if let Some(message) = message {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    message,
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(200),
                );
            }
            ui.add_space(12.0);
            ui.vertical(|ui| {
                self.meter.show(ui, height - 18.0);
                ui.label(RichText::new("  L   R").small().weak());
            });
        });
    }

    fn source_name(&mut self, ui: &mut egui::Ui, state: &AppState) {
        ui.horizontal(|ui| {
            ui.label("Source name:");
            match &state.sending {
                Some(sending) => {
                    ui.label(RichText::new(&sending.source_name).strong());
                }
                None => {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.source_name)
                            .desired_width(180.0),
                    );
                    let name = format!("{} ({})", self.machine_name, self.settings.source_name);
                    ui.label(RichText::new(format!("NDI name: {name}")).weak())
                        .on_hover_text("NDI adds the computer name in front of the source name.");
                }
            }
        });
    }

    fn audio_and_status(&mut self, ui: &mut egui::Ui, state: &AppState) {
        match &state.sending {
            Some(sending) => {
                match (sending.audio_mode, &sending.audio_error) {
                    (Some(AudioMode::SystemLoopback), _) => banner(
                        ui,
                        AMBER,
                        "WARNING: Sending ALL system audio – everyone hears your Discord, music and notifications.",
                    ),
                    (None, Some(error)) => {
                        banner(
                            ui,
                            RED,
                            &format!(
                                "WARNING: Game audio failed – sending WITHOUT audio.\n{error}"
                            ),
                        );
                        if ui
                            .button("Switch to system audio (includes Discord etc.)")
                            .clicked()
                        {
                            self.send(Command::SwitchToSystemAudio);
                        }
                    }
                    (Some(AudioMode::ProcessLoopback { .. }), _) => {
                        ui.horizontal(|ui| {
                            ui.label("Audio: game only");
                            activity_dot(ui, self.meter.active());
                        });
                    }
                    (None, None) => {
                        ui.label("Audio: off");
                    }
                }
                let s = &sending.status;
                let mut line = format!(
                    "Sending {}×{} · {:.1} fps · {} receiver{}",
                    s.output_size.0,
                    s.output_size.1,
                    s.send_fps,
                    s.receivers,
                    if s.receivers == 1 { "" } else { "s" }
                );
                if s.source_size != s.output_size && s.source_size.0 > 0 {
                    line.push_str(&format!(
                        " · scaled from {}×{}",
                        s.source_size.0, s.source_size.1
                    ));
                }
                ui.label(line);
            }
            None => match (&state.selected, &state.audio_check) {
                (Some(_), AudioCheck::GameAudio) => {
                    ui.horizontal(|ui| {
                        ui.label("Audio: game only (checked)");
                        activity_dot(ui, self.meter.active());
                    });
                }
                (Some(_), AudioCheck::Unavailable(reason)) => {
                    banner(
                        ui,
                        AMBER,
                        "WARNING: Game-only audio is not available on this system.\nLANcast would send ALL system audio – everyone will hear your Discord, music and notifications.",
                    );
                    ui.label(RichText::new(reason).small().weak());
                }
                _ => {
                    ui.label(RichText::new("Select the window of your game.").weak());
                }
            },
        }

        if let Some(notice) = &state.notice {
            let color = match notice.level {
                NoticeLevel::Info => ui.visuals().text_color(),
                NoticeLevel::Warning => AMBER,
                NoticeLevel::Error => RED,
            };
            ui.colored_label(color, &notice.text);
        }
    }

    fn send_controls(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let big = |text: &str| {
            egui::Button::new(RichText::new(text).size(16.0)).min_size(vec2(160.0, 34.0))
        };
        ui.horizontal(|ui| {
            if state.sending.is_some() {
                if ui.add(big("Stop")).clicked() {
                    self.send(Command::Stop);
                }
                return;
            }
            let ready = state.selected.is_some()
                && !self.settings.source_name.trim().is_empty()
                && state.caps.wgc;
            match &state.audio_check {
                AudioCheck::Unavailable(_) if state.selected.is_some() => {
                    // No plain "Send" that silently uses system audio (SPEC §3.1).
                    if ui
                        .add_enabled(ready, big("Send with system audio"))
                        .clicked()
                    {
                        self.start(AudioChoice::System);
                    }
                    if ui.add_enabled(ready, big("Send without audio")).clicked() {
                        self.start(AudioChoice::None);
                    }
                }
                _ => {
                    if ui.add_enabled(ready, big("Send")).clicked() {
                        self.start(AudioChoice::Game);
                    }
                }
            }
            if !state.caps.wgc {
                ui.colored_label(
                    RED,
                    "Window capture is not supported on this Windows version.",
                );
            }
        });
    }

    /// The settings, grouped in a framed panel.
    fn settings_panel(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let editable = state.sending.is_none();
        let caps = state.caps;
        egui::Frame::group(ui.style())
            .corner_radius(6.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Settings").strong().size(14.0));
                ui.add_space(4.0);
                egui::Grid::new("settings")
                    .num_columns(3)
                    .spacing([18.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Window").weak());
                        if ui
                            .checkbox(&mut self.to_tray, "Close and minimize to tray")
                            .on_hover_text(
                                "× and minimize hide LANcast to the tray; sending continues.\n\
                                 Quit from the tray icon's menu.",
                            )
                            .changed()
                        {
                            // Minimizing is detected by the tray thread
                            // (eframe does not run the UI while minimized).
                            self.tray.set_minimize_to_tray(self.to_tray);
                        }
                        if ui
                            .checkbox(&mut self.always_on_top, "Always on top")
                            .on_hover_text(
                                "Keeps LANcast visible over your game to check the setup.\n\
                                 Works with borderless windowed games, not exclusive fullscreen.",
                            )
                            .changed()
                        {
                            ui.ctx().send_viewport_cmd(ViewportCommand::WindowLevel(
                                if self.always_on_top {
                                    WindowLevel::AlwaysOnTop
                                } else {
                                    WindowLevel::Normal
                                },
                            ));
                        }
                        ui.end_row();

                        ui.label(RichText::new("Capture").weak());
                        ui.add_enabled(
                            editable && caps.border_control,
                            egui::Checkbox::new(
                                &mut self.settings.video.show_border,
                                "Show capture border",
                            ),
                        )
                        .on_disabled_hover_text(
                            "Fixed while sending, or always shown on this Windows version.",
                        );
                        ui.add_enabled(
                            editable && caps.cursor_control,
                            egui::Checkbox::new(
                                &mut self.settings.video.capture_cursor,
                                "Capture cursor",
                            ),
                        )
                        .on_disabled_hover_text(
                            "Fixed while sending, or not supported on this Windows version.",
                        );
                        ui.end_row();

                        ui.label(RichText::new("Video").weak());
                        ui.add_enabled_ui(editable, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Max. frame rate");
                                ui.add(
                                    egui::DragValue::new(&mut self.settings.video.fps_cap)
                                        .range(10..=120)
                                        .suffix(" fps"),
                                );
                            });
                        });
                        ui.add_enabled_ui(editable, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Max. resolution");
                                let (w, h) = self.settings.video.max_size;
                                egui::ComboBox::from_id_salt("max_size")
                                    .selected_text(format!("{w}×{h}"))
                                    .show_ui(ui, |ui| {
                                        for size in MAX_SIZES {
                                            ui.selectable_value(
                                                &mut self.settings.video.max_size,
                                                size,
                                                format!("{}×{}", size.0, size.1),
                                            );
                                        }
                                    });
                            });
                        });
                        ui.end_row();

                        ui.label(RichText::new("Audio").weak());
                        ui.add_enabled_ui(editable, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Receiver level");
                                let unity = self.settings.audio_gain_db == 0.0;
                                egui::ComboBox::from_id_salt("audio_level")
                                    .selected_text(if unity {
                                        "Unity (OBS / DistroAV)"
                                    } else {
                                        "NDI standard +20 dB (Studio Monitor)"
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.settings.audio_gain_db,
                                            0.0,
                                            "Unity (OBS / DistroAV)",
                                        );
                                        ui.selectable_value(
                                            &mut self.settings.audio_gain_db,
                                            AUDIO_GAIN_STUDIO_MONITOR_DB,
                                            "NDI standard +20 dB (Studio Monitor)",
                                        );
                                    });
                            });
                        });
                        ui.end_row();

                        ui.label(RichText::new("Network").weak());
                        self.network_settings(ui, state);
                        ui.end_row();
                    });
            });
    }

    /// Discovery server setting with its reachability, and the system
    /// setting from NDI Access Manager (SPEC §5.4).
    fn network_settings(&mut self, ui: &mut egui::Ui, state: &AppState) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("Discovery server");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.discovery_server)
                        .hint_text("automatic (mDNS)")
                        .desired_width(160.0),
                );
                if response.lost_focus() {
                    let servers = self.discovery_server.trim().to_owned();
                    if servers != state.network.own_servers {
                        self.send(Command::CheckDiscovery(servers));
                    }
                }
                let network = &state.network;
                if !self.discovery_server.trim().is_empty()
                    && network.own_servers == self.discovery_server.trim()
                {
                    match network.own_reachable {
                        Some(true) => ui.colored_label(Color32::from_rgb(60, 190, 80), "reachable"),
                        Some(false) => ui.colored_label(RED, "NOT reachable"),
                        None => ui.weak("checking…"),
                    };
                }
                ui.label(RichText::new("Optional. Applies at the next Send.").small().weak())
                    .on_hover_text(
                        "Leave empty for automatic discovery (mDNS), which works when all PCs \
                         share one network.\nWith a discovery server, receivers must use the same server.",
                    );
            });
            if let Some(system) = &state.network.system_servers {
                ui.horizontal(|ui| {
                    let status = match state.network.system_reachable {
                        Some(true) => "reachable",
                        Some(false) => "NOT reachable",
                        None => "checking…",
                    };
                    ui.label(
                        RichText::new(format!("NDI Access Manager sets {system} ({status})."))
                            .small()
                            .weak(),
                    );
                    ui.add_enabled(
                        self.discovery_server.trim().is_empty(),
                        egui::Checkbox::new(&mut self.ignore_system_discovery, "Ignore it for LANcast"),
                    )
                    .on_hover_text(
                        "Use automatic discovery (mDNS) instead. The system setting is not changed.",
                    );
                });
            }
        });
    }

    /// Warns when the discovery server from NDI Access Manager is in use but
    /// unreachable: receivers would not find the stream (D14).
    fn discovery_warning(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let network = &state.network;
        let (Some(system), Some(false)) = (&network.system_servers, network.system_reachable)
        else {
            return;
        };
        if self.ignore_system_discovery || !self.discovery_server.trim().is_empty() {
            return;
        }
        banner(
            ui,
            AMBER,
            &format!(
                "WARNING: NDI Access Manager sets the discovery server {system}, which is not \
                 reachable. Receivers will not find your stream."
            ),
        );
        if ui
            .button("Ignore it for LANcast (use automatic discovery)")
            .clicked()
        {
            self.ignore_system_discovery = true;
        }
        ui.add_space(4.0);
    }
}

/// Lowercase exe name, the key for remembered source names.
fn process_key(window: &WindowInfo) -> Option<String> {
    window.process_name.as_ref().map(|name| name.to_lowercase())
}

fn banner(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(egui::Stroke::new(1.5, color))
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).strong().color(color));
        });
}

fn activity_dot(ui: &mut egui::Ui, active: bool) {
    let (rect, response) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
    let color = if active {
        Color32::from_rgb(60, 190, 80)
    } else {
        Color32::from_gray(90)
    };
    ui.painter().circle_filled(rect.center(), 5.0, color);
    response.on_hover_text(if active {
        "Audio is arriving"
    } else {
        "Silence (is the game muted or in the background?)"
    });
}

/// A small padlock, drawn (egui's default fonts have no lock glyph).
fn lock_icon(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
    let painter = ui.painter();
    let body = egui::Rect::from_min_max(
        pos2(rect.left() + 1.0, rect.top() + 7.0),
        pos2(rect.right() - 1.0, rect.bottom() - 1.0),
    );
    painter.rect_filled(body, 2.0, color);
    let shackle = egui::Rect::from_min_max(
        pos2(rect.left() + 3.5, rect.top() + 1.0),
        pos2(rect.right() - 3.5, rect.top() + 11.0),
    );
    painter.rect_stroke(
        shackle,
        4.0,
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Inside,
    );
}
