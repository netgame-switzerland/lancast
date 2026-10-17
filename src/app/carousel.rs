//! Window picker as an Alt-Tab-like carousel (D19, D20, D22): four tiles
//! with live DWM thumbnails, app icon and title. It scrolls continuously with
//! the mouse wheel or touchpad, can be dragged ("pulled") with mouse or finger
//! with momentum, and then settles softly on a tile boundary. While casting it
//! is locked: no interaction, other tiles dimmed, the cast window marked LIVE.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Color32, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2,
};

use crate::{
    exe_icon,
    win_thumbnail::Thumbnails,
    windows_list::{self, WindowInfo},
};

const VISIBLE: usize = 4;
const HEIGHT: f32 = 170.0;
const ARROW_WIDTH: f32 = 26.0;
const GAP: f32 = 8.0;
const TEXT_HEIGHT: f32 = 38.0;
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// Tiles per point of wheel/touchpad scrolling, relative to one tile width.
const WHEEL_FACTOR: f32 = 1.5;
/// Momentum decay per second after a drag is released.
const FRICTION: f32 = 4.0;
/// Below this speed (tiles/s) momentum ends and the carousel settles.
const MIN_VELOCITY: f32 = 0.15;
/// Settle only after the wheel has been quiet this long.
const SETTLE_DELAY: Duration = Duration::from_millis(150);
/// Settling speed (higher = faster).
const SETTLE_RATE: f32 = 9.0;
/// Thumbnail opacity of the other windows while casting.
const LOCKED_OPACITY: u8 = 70;
const LIVE_RED: Color32 = Color32::from_rgb(225, 45, 40);

pub struct Carousel {
    /// Alt-Tab-like order: most recently active first, kept stable.
    windows: Vec<WindowInfo>,
    /// Continuous index of the leftmost visible tile (unbounded; wraps modulo
    /// len). Most recent window on the left, like Alt-Tab.
    position: f32,
    /// Momentum in tiles per second.
    velocity: f32,
    /// Where to settle; `None` settles on the nearest tile boundary.
    snap: Option<f32>,
    last_input: Option<Instant>,
    last_frame: Option<Instant>,
    last_refresh: Option<Instant>,
    /// Whether the previous frame was locked (to scroll the cast window into
    /// view once when locking).
    was_locked: bool,
    icons: HashMap<String, Option<TextureHandle>>,
    thumbnails: Option<Thumbnails>,
}

impl Carousel {
    /// `destination` is our own top-level window, for the DWM thumbnails.
    pub fn new(destination: Option<isize>) -> Self {
        Self {
            windows: Vec::new(),
            position: 0.0,
            velocity: 0.0,
            snap: None,
            last_input: None,
            last_frame: None,
            last_refresh: None,
            was_locked: false,
            icons: HashMap::new(),
            thumbnails: destination.map(Thumbnails::new),
        }
    }

    /// Removes all thumbnails (while hidden).
    pub fn hide(&mut self) {
        if let Some(thumbnails) = &mut self.thumbnails {
            thumbnails.clear();
        }
        self.last_frame = None;
    }

    /// Re-reads the window list. Known windows keep their place; new ones
    /// (usually just started or activated) go to the front, and the leftmost
    /// visible window stays where it is.
    fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        let own_pid = std::process::id();
        let fresh: Vec<WindowInfo> = windows_list::list_windows()
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.pid != own_pid)
            .collect();
        if self.windows.is_empty() {
            self.windows = fresh;
            self.position = 0.0;
            self.snap = None;
            return;
        }
        let n_old = self.windows.len() as i64;
        let anchor = self.windows[(self.position.round() as i64).rem_euclid(n_old) as usize].hwnd;
        let new: Vec<WindowInfo> = fresh
            .iter()
            .filter(|f| !self.windows.iter().any(|w| w.hwnd == f.hwnd))
            .cloned()
            .collect();
        let mut kept: Vec<WindowInfo> = self
            .windows
            .iter()
            .filter_map(|w| fresh.iter().find(|f| f.hwnd == w.hwnd).cloned())
            .collect();
        let mut windows = new;
        windows.append(&mut kept);
        self.windows = windows;
        if let Some(index) = self.windows.iter().position(|w| w.hwnd == anchor) {
            let n = self.windows.len() as i64;
            let shift = (index as i64 - (self.position.round() as i64).rem_euclid(n)) as f32;
            self.position += shift;
            self.snap = self.snap.map(|s| s + shift);
        }
        if let Some(thumbnails) = &mut self.thumbnails {
            let windows = &self.windows;
            thumbnails.retain(|h| windows.iter().any(|w| w.hwnd == h));
        }
    }

    /// Draws the carousel; returns a window the user clicked. With `locked`
    /// (while casting) it ignores all input, dims the other windows and marks
    /// the selected one as LIVE.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        selected: Option<isize>,
        locked: bool,
    ) -> Option<WindowInfo> {
        if self
            .last_refresh
            .is_none_or(|t| t.elapsed() >= REFRESH_INTERVAL)
        {
            self.refresh();
        }
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(REFRESH_INTERVAL);

        let width = ui.available_width();
        let (area, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
        let n = self.windows.len();
        if n == 0 {
            ui.painter().text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                "No windows found. Start your game, it appears here.",
                egui::FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
            return None;
        }

        let cycling = n > VISIBLE;
        let inner = Rect::from_min_max(
            pos2(area.left() + ARROW_WIDTH + GAP, area.top()),
            pos2(area.right() - ARROW_WIDTH - GAP, area.bottom()),
        );
        let tile_width = (inner.width() - GAP * (VISIBLE as f32 - 1.0)) / VISIBLE as f32;
        let step = tile_width + GAP;
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(0.0, |t| now.duration_since(t).as_secs_f32())
            .min(0.1);
        self.last_frame = Some(now);

        // When casting starts, scroll the cast window into view once.
        if locked && !self.was_locked && cycling {
            self.scroll_into_view(selected);
        }
        self.was_locked = locked;

        // One interaction for the whole strip: drag to pull, click to pick.
        let sense = if locked {
            Sense::hover()
        } else {
            Sense::click_and_drag()
        };
        let response = ui.interact(inner, ui.scope_id().with("carousel"), sense);
        if cycling {
            let left = Rect::from_min_size(area.min, vec2(ARROW_WIDTH, HEIGHT));
            let right = Rect::from_min_size(
                pos2(area.right() - ARROW_WIDTH, area.top()),
                vec2(ARROW_WIDTH, HEIGHT),
            );
            if arrow(ui, left, "<", !locked) {
                self.snap = Some(self.position.round() - 1.0);
                self.velocity = 0.0;
            }
            if arrow(ui, right, ">", !locked) {
                self.snap = Some(self.position.round() + 1.0);
                self.velocity = 0.0;
            }

            if !locked {
                // Mouse wheel / touchpad: continuous scrolling.
                if ui.rect_contains_pointer(area) {
                    let delta = ctx.input(|i| i.smooth_scroll_delta);
                    let amount = delta.x + delta.y;
                    if amount != 0.0 {
                        self.position -= amount * WHEEL_FACTOR / step;
                        self.velocity = 0.0;
                        self.snap = None;
                        self.last_input = Some(now);
                    }
                }
                // Dragging with mouse or finger, with momentum on release.
                if response.dragged() {
                    self.position -= response.drag_delta().x / step;
                    self.velocity = 0.0;
                    self.snap = None;
                    self.last_input = Some(now);
                }
                if response.drag_stopped() {
                    self.velocity = -ctx.input(|i| i.pointer.velocity().x) / step;
                }
            }
        }

        // Physics: momentum, then settle on a tile boundary.
        let mut moving = false;
        if !response.dragged() {
            if self.velocity.abs() > MIN_VELOCITY {
                self.position += self.velocity * dt;
                self.velocity *= (-FRICTION * dt).exp();
                moving = true;
            } else {
                self.velocity = 0.0;
                let quiet = self.last_input.is_none_or(|t| t.elapsed() >= SETTLE_DELAY);
                if quiet || self.snap.is_some() {
                    let target = self.snap.unwrap_or_else(|| self.position.round());
                    let diff = target - self.position;
                    if diff.abs() < 0.001 {
                        self.position = target;
                        self.snap = None;
                    } else {
                        self.position += diff * (dt * SETTLE_RATE).min(1.0);
                        moving = true;
                    }
                } else {
                    moving = true; // wait for the wheel to go quiet
                }
            }
        }
        if moving || response.dragged() {
            ctx.request_repaint();
        }

        // Tiles to draw: (window index, tile center x).
        let slots: Vec<(usize, f32)> = if cycling {
            let first_center = inner.left() + tile_width / 2.0;
            let base = self.position.floor() as i64;
            (base - 1..=base + VISIBLE as i64)
                .map(|i| {
                    (
                        i.rem_euclid(n as i64) as usize,
                        first_center + (i as f32 - self.position) * step,
                    )
                })
                .collect()
        } else {
            (0..n)
                .map(|i| {
                    (
                        i,
                        inner.center().x + (i as f32 - (n as f32 - 1.0) / 2.0) * step,
                    )
                })
                .collect()
        };
        let tile_at = |center_x: f32| {
            Rect::from_center_size(pos2(center_x, area.center().y), vec2(tile_width, HEIGHT))
        };

        // Which tile a click hit.
        let mut clicked = None;
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(&(index, _)) = slots
                .iter()
                .find(|&&(_, x)| tile_at(x).intersect(inner).contains(pointer))
        {
            clicked = Some(self.windows[index].clone());
        }

        let ppp = ctx.pixels_per_point();
        let px = |r: Rect| {
            [
                (r.left() * ppp).round() as i32,
                (r.top() * ppp).round() as i32,
                (r.right() * ppp).round() as i32,
                (r.bottom() * ppp).round() as i32,
            ]
        };
        let painter = ui.painter_at(inner);
        let mut shown = Vec::new();

        for (index, center_x) in slots {
            let tile = tile_at(center_x);
            if tile.intersect(inner).width() <= 1.0 {
                continue;
            }
            let window = &self.windows[index];
            let is_selected = selected == Some(window.hwnd);
            let live = locked && is_selected;
            let dimmed = locked && !is_selected;
            let hovered =
                !locked && !response.dragged() && ui.rect_contains_pointer(tile.intersect(inner));
            let (fill, stroke) = if live {
                (LIVE_RED.gamma_multiply(0.18), Stroke::new(2.5, LIVE_RED))
            } else if is_selected {
                (
                    ui.visuals().selection.bg_fill.gamma_multiply(0.35),
                    Stroke::new(2.0, ui.visuals().selection.stroke.color),
                )
            } else if hovered {
                (
                    ui.visuals().widgets.hovered.weak_bg_fill,
                    Stroke::new(1.0, Color32::from_gray(110)),
                )
            } else if dimmed {
                (
                    Color32::from_gray(22),
                    Stroke::new(1.0, Color32::from_gray(40)),
                )
            } else {
                (
                    ui.visuals().faint_bg_color,
                    Stroke::new(1.0, Color32::from_gray(60)),
                )
            };
            painter.rect(tile, 6.0, fill, stroke, egui::StrokeKind::Inside);

            let thumb_area = Rect::from_min_max(
                tile.min + vec2(8.0, 8.0),
                pos2(tile.right() - 8.0, tile.bottom() - TEXT_HEIGHT - 4.0),
            );
            let opacity = if dimmed { LOCKED_OPACITY } else { 255 };
            let has_thumbnail = match &mut self.thumbnails {
                Some(thumbnails) => {
                    let ok = thumbnails.source_size(window.hwnd).is_some();
                    if ok {
                        thumbnails.show(window.hwnd, px(thumb_area), px(inner), opacity);
                        shown.push(window.hwnd);
                    }
                    ok
                }
                None => false,
            };

            // Icon and title below the thumbnail.
            let icon = window.process_path.as_ref().and_then(|path| {
                self.icons
                    .entry(path.clone())
                    .or_insert_with(|| {
                        exe_icon::exe_icon_rgba(path).map(|(w, h, rgba)| {
                            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                            ctx.load_texture(format!("icon:{path}"), image, TextureOptions::LINEAR)
                        })
                    })
                    .clone()
            });
            let tint = if dimmed {
                Color32::from_gray(90)
            } else {
                Color32::WHITE
            };
            if !has_thumbnail && let Some(icon) = &icon {
                // No live thumbnail available: show the icon large.
                let r = Rect::from_center_size(thumb_area.center(), vec2(48.0, 48.0));
                painter.image(icon.id(), r, uv(), tint);
            }
            let text_top = tile.bottom() - TEXT_HEIGHT;
            let mut text_left = tile.left() + 8.0;
            if let Some(icon) = &icon {
                let r = Rect::from_min_size(pos2(text_left, text_top + 2.0), vec2(16.0, 16.0));
                painter.image(icon.id(), r, uv(), tint);
                text_left += 22.0;
            }
            let mut text_right = tile.right() - 8.0;
            if live {
                // "LIVE" badge on the cast window.
                let badge = Rect::from_min_size(
                    pos2(tile.right() - 8.0 - 40.0, text_top + 2.0),
                    vec2(40.0, 17.0),
                );
                painter.rect_filled(badge, 3.0, LIVE_RED);
                painter.text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    "LIVE",
                    egui::FontId::proportional(11.0),
                    Color32::WHITE,
                );
                text_right = badge.left() - 6.0;
            }
            let (title_color, sub_color) = if dimmed {
                (Color32::from_gray(95), Color32::from_gray(70))
            } else {
                (
                    ui.visuals().strong_text_color(),
                    ui.visuals().weak_text_color(),
                )
            };
            let text_width = text_right - text_left;
            painter.text(
                pos2(text_left, text_top + 10.0),
                egui::Align2::LEFT_CENTER,
                elide(&painter, &window.title, 13.0, text_width),
                egui::FontId::proportional(13.0),
                title_color,
            );
            if let Some(process) = &window.process_name {
                painter.text(
                    pos2(text_left, text_top + 27.0),
                    egui::Align2::LEFT_CENTER,
                    elide(&painter, process, 11.0, tile.right() - 8.0 - text_left),
                    egui::FontId::proportional(11.0),
                    sub_color,
                );
            }
        }

        if let Some(thumbnails) = &mut self.thumbnails {
            for window in &self.windows {
                if !shown.contains(&window.hwnd) {
                    thumbnails.hide(window.hwnd);
                }
            }
        }
        clicked
    }

    /// Scrolls so that `hwnd` is among the visible tiles (shortest way).
    fn scroll_into_view(&mut self, hwnd: Option<isize>) {
        let n = self.windows.len() as i64;
        let Some(index) = hwnd.and_then(|h| self.windows.iter().position(|w| w.hwnd == h)) else {
            return;
        };
        let left = self.position.round() as i64;
        let offset = (index as i64 - left).rem_euclid(n);
        if offset < VISIBLE as i64 {
            return; // already visible
        }
        // Either bring it in from the right (as the last tile) or the left.
        let from_right = offset - (VISIBLE as i64 - 1);
        let from_left = n - offset;
        self.snap = Some(if from_right <= from_left {
            (left + from_right) as f32
        } else {
            (left - from_left) as f32
        });
        self.velocity = 0.0;
    }
}

fn arrow(ui: &mut egui::Ui, rect: Rect, text: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| {
        ui.put(rect, egui::Button::new(RichText::new(text).size(18.0)))
            .on_hover_text("More windows (or scroll, or drag)")
            .on_disabled_hover_text("Locked while casting")
            .clicked()
    })
    .inner
}

fn uv() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// Shortens `text` with "…" to fit `width` points.
fn elide(painter: &egui::Painter, text: &str, size: f32, width: f32) -> String {
    let font = egui::FontId::proportional(size);
    let fits = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x
            <= width
    };
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if fits(&candidate) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}
