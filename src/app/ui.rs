//! The main window (SPEC §3.1). Renders [`AppState`] and sends commands to
//! the controller; holds no session state of its own.

use std::sync::atomic::Ordering;

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
    capture::{audio_wasapi::AudioMode, video::VideoOptions},
    ndi_out::AUDIO_GAIN_STUDIO_MONITOR_DB,
    tray::{self, TrayHandle},
    windows_list::WindowInfo,
};

const PREVIEW_SIZE: egui::Vec2 = egui::vec2(320.0, 180.0);
const MAX_SIZES: [(u32, u32); 4] = [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440)];
const AMBER: Color32 = Color32::from_rgb(235, 160, 0);
const RED: Color32 = Color32::from_rgb(215, 60, 50);

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
    meter: Meter,
    preview: Option<TextureHandle>,
}

impl LancastApp {
    pub fn new(
        state: SharedState,
        commands: Sender<Command>,
        tray: TrayHandle,
        hwnd: Option<isize>,
    ) -> Self {
        Self {
            state,
            commands,
            tray,
            hwnd,
            machine_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "THIS-PC".to_owned()),
            carousel: Carousel::new(hwnd),
            settings: Settings {
                source_name: String::new(),
                video: VideoOptions::default(),
                audio_gain_db: 0.0,
            },
            sent_settings: None,
            to_tray: true,
            always_on_top: false,
            meter: Meter::default(),
            preview: None,
        }
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    fn hide_to_tray(&mut self, state: &AppState) {
        if let Some(sending) = &state.sending {
            sending.preview.enabled.store(false, Ordering::Relaxed);
        }
        self.carousel.hide();
        if let Some(hwnd) = self.hwnd {
            tray::hide_window(hwnd);
        }
        self.tray.hidden_to_tray();
    }
}

impl eframe::App for LancastApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();

        // × and minimize go to the tray if enabled; otherwise × quits.
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.to_tray && self.hwnd.is_some() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.hide_to_tray(&state);
                return;
            }
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.send(Command::Quit);
            return;
        }

        if self.sent_settings.as_ref() != Some(&self.settings) {
            self.sent_settings = Some(self.settings.clone());
            self.send(Command::Settings(self.settings.clone()));
        }

        if let Some(sending) = &state.sending {
            sending.preview.enabled.store(true, Ordering::Relaxed);
            if let Some(image) = sending.preview.take() {
                let image = ColorImage::from_rgba_unmultiplied(image.size, &image.rgba);
                match &mut self.preview {
                    Some(texture) => texture.set(image, TextureOptions::LINEAR),
                    None => {
                        self.preview =
                            Some(ctx.load_texture("preview", image, TextureOptions::LINEAR))
                    }
                }
            }
        } else {
            self.preview = None;
        }
        self.meter
            .update(state.sending.as_ref().and_then(|s| s.levels.as_deref()));

        egui::Panel::bottom("footer").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("NDI® is a registered trademark of Vizrt NDI AB.")
                        .small()
                        .weak(),
                );
                ui.hyperlink_to(RichText::new("ndi.video").small(), "https://ndi.video/");
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if state.sending.is_some() {
                // While sending: what is on air.
                self.carousel.hide();
                self.sending_header(ui, &state);
                ui.add_space(6.0);
                self.preview_and_meter(ui, &state);
            } else {
                // Before sending: pick the window.
                ui.label(RichText::new("Choose the window of your game:").strong());
                ui.add_space(4.0);
                let selected = state.selected.as_ref().map(|w| w.hwnd);
                if let Some(window) = self.carousel.show(ui, selected) {
                    self.settings.source_name = window.default_source_name();
                    self.send(Command::Select(window));
                }
                ui.add_space(6.0);
                self.source_name(ui, &state);
            }
            ui.add_space(6.0);
            self.audio_and_status(ui, &state);
            ui.add_space(8.0);
            self.send_controls(ui, &state);
            ui.add_space(8.0);
            self.options(ui, &state);
        });

        // Animate the meter while sending; otherwise the controller wakes us.
        let interval = if state.sending.is_some() { 33 } else { 1000 };
        ctx.request_repaint_after(std::time::Duration::from_millis(interval));
    }
}

impl LancastApp {
    fn sending_header(&mut self, ui: &mut egui::Ui, state: &AppState) {
        if let (Some(window), Some(sending)) = (&state.selected, &state.sending) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Sending:").strong());
                ui.label(window_label(window));
            });
            ui.label(RichText::new(format!("NDI name: {}", sending.source_name)).weak());
        }
    }

    fn source_name(&mut self, ui: &mut egui::Ui, _state: &AppState) {
        ui.horizontal(|ui| {
            ui.label("Source name:");
            ui.add(egui::TextEdit::singleline(&mut self.settings.source_name).desired_width(180.0));
            let name = format!("{} ({})", self.machine_name, self.settings.source_name);
            ui.label(RichText::new(format!("NDI name: {name}")).weak())
                .on_hover_text("NDI adds the computer name in front of the source name.");
        });
    }

    fn preview_and_meter(&mut self, ui: &mut egui::Ui, state: &AppState) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(PREVIEW_SIZE, Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, Color32::from_gray(16));
            let message = match (&state.sending, &self.preview) {
                (None, _) => Some("Preview appears while sending"),
                (Some(s), _) if s.minimized => Some("Window minimized – last frame stays on air"),
                (Some(s), _) if s.frames_missing => Some(
                    "No picture. Is the game in exclusive fullscreen?\nPlease use borderless windowed mode.",
                ),
                (Some(_), None) => Some("Waiting for the first frame…"),
                (Some(_), Some(_)) => None,
            };
            if let Some(texture) = &self.preview {
                let [w, h] = texture.size();
                let scale = (rect.width() / w as f32).min(rect.height() / h as f32);
                let size = vec2(w as f32 * scale, h as f32 * scale);
                let image_rect = egui::Rect::from_center_size(rect.center(), size);
                painter.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            if let Some(message) = message {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    message,
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(200),
                );
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                self.meter.show(ui, PREVIEW_SIZE.y - 18.0);
                ui.label(RichText::new("  L   R").small().weak());
            });
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
                    "{}×{} · {:.1} fps · {} receiver{}",
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
                    ui.label("Audio: game only (checked)");
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
                        self.send(Command::Start(AudioChoice::System));
                    }
                    if ui.add_enabled(ready, big("Send without audio")).clicked() {
                        self.send(Command::Start(AudioChoice::None));
                    }
                }
                _ => {
                    if ui.add_enabled(ready, big("Send")).clicked() {
                        self.send(Command::Start(AudioChoice::Game));
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

    fn options(&mut self, ui: &mut egui::Ui, state: &AppState) {
        let editable = state.sending.is_none();
        let caps = state.caps;
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut self.to_tray, "Close and minimize to tray")
                .on_hover_text(
                    "× and minimize hide LANcast to the tray; sending continues.\n\
                     Quit from the tray icon's menu.",
                )
                .changed()
            {
                // Minimizing is detected by the tray thread (eframe does not
                // run the UI for a minimized window).
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
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::WindowLevel(if self.always_on_top {
                        WindowLevel::AlwaysOnTop
                    } else {
                        WindowLevel::Normal
                    }));
            }
        });
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal(|ui| {
                ui.add_enabled(
                    caps.border_control,
                    egui::Checkbox::new(
                        &mut self.settings.video.show_border,
                        "Show capture border",
                    ),
                )
                .on_disabled_hover_text("This Windows version always shows the yellow border.");
                ui.add_enabled(
                    caps.cursor_control,
                    egui::Checkbox::new(&mut self.settings.video.capture_cursor, "Capture cursor"),
                )
                .on_disabled_hover_text("This Windows version cannot hide the cursor.");
            });
            egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                egui::Grid::new("advanced").num_columns(2).show(ui, |ui| {
                    ui.label("Max. frame rate:");
                    ui.add(
                        egui::DragValue::new(&mut self.settings.video.fps_cap)
                            .range(10..=120)
                            .suffix(" fps"),
                    );
                    ui.end_row();

                    ui.label("Max. resolution:");
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
                    ui.end_row();

                    ui.label("Receiver audio level:");
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
                    ui.end_row();
                });
            });
        });
    }
}

fn window_label(window: &WindowInfo) -> String {
    let process = window.process_name.as_deref().unwrap_or("?");
    let mut title: String = window.title.chars().take(48).collect();
    if window.title.chars().count() > 48 {
        title.push('…');
    }
    format!("{title}  ({process})")
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
