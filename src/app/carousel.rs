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

use super::theme::Palette;
use crate::{
    config::LastCast,
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

/// What the badge on the cast window says (D49).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveBadge {
    /// Receivers can find the stream.
    Live,
    /// Just started; NDI is still connecting to the discovery server.
    Connecting,
    /// Sending, but no discovery server is connected: nobody finds it.
    NotLive,
}

impl LiveBadge {
    /// Badge text, badge fill, text color and the tile's border color.
    fn style(self) -> (&'static str, Color32, Color32, Color32) {
        match self {
            Self::Live => ("LIVE", LIVE_RED, Color32::WHITE, LIVE_RED),
            Self::Connecting => (
                "LIVE",
                Color32::from_gray(105),
                Color32::WHITE,
                Color32::from_gray(140),
            ),
            Self::NotLive => (
                "NOT LIVE",
                Color32::from_gray(55),
                Color32::from_rgb(255, 120, 110),
                Color32::from_gray(140),
            ),
        }
    }
}

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
    /// Search text (D53). The first matching window is scrolled into view
    /// and outlined; all windows stay in the carousel. Cleared while casting.
    query: String,
    /// The match shown (outlined), chosen with the arrow keys; the first
    /// match when it no longer matches.
    current_match: Option<isize>,
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
            query: String::new(),
            current_match: None,
        }
    }

    /// The open window of the program cast last (D54): the one with the same
    /// title, else the most recently used one of that program. Scrolls it
    /// into view.
    pub fn find_last_cast(&mut self, last: &LastCast) -> Option<WindowInfo> {
        let of_program = |w: &&WindowInfo| {
            w.process_name
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case(&last.process))
        };
        let window = self
            .windows
            .iter()
            .filter(of_program)
            .find(|w| w.title == last.title)
            .or_else(|| self.windows.iter().find(of_program))
            .cloned()?;
        self.scroll_into_view(Some(window.hwnd));
        Some(window)
    }

    /// All windows the search matches, in carousel order (most recently used
    /// first); empty without a search.
    fn search_matches(&self) -> Vec<&WindowInfo> {
        if self.query.trim().is_empty() {
            return Vec::new();
        }
        self.windows
            .iter()
            .filter(|w| matches(w, &self.query))
            .collect()
    }

    /// The match shown and its index among all matches.
    fn search_match_at(&self) -> Option<(usize, &WindowInfo)> {
        let found = self.search_matches();
        let index = found
            .iter()
            .position(|w| Some(w.hwnd) == self.current_match)
            .unwrap_or(0);
        found.get(index).map(|&w| (index, w))
    }

    /// The match shown (outlined); `None` without a match.
    fn search_match(&self) -> Option<&WindowInfo> {
        self.search_match_at().map(|(_, w)| w)
    }

    /// Shows the next (`step` 1) or previous (-1) match, wrapping around.
    fn step_match(&mut self, step: i64) {
        let found = self.search_matches();
        if found.is_empty() {
            return;
        }
        let index = self.search_match_at().map_or(0, |(i, _)| i) as i64;
        let next = found[(index + step).rem_euclid(found.len() as i64) as usize].hwnd;
        self.current_match = Some(next);
        self.scroll_into_view(Some(next));
    }

    /// The search field (D53). Typing while no other field has the keyboard
    /// starts a new search in it, and the carousel scrolls to the first
    /// matching window. Down / Up go to the next / previous match, Enter
    /// picks the one shown, Esc clears the search. Returns the picked window.
    /// Disabled and empty while casting.
    pub fn search_field(&mut self, ui: &mut egui::Ui, locked: bool) -> Option<WindowInfo> {
        let id = egui::Id::unique("window_search");
        let before = self.query.clone();
        if locked {
            self.query.clear();
        } else {
            self.type_to_search(ui.ctx(), id);
            // Before the field, which would otherwise move its cursor.
            if ui.ctx().memory(|m| m.has_focus(id)) {
                let (down, up) = ui.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    )
                });
                if down {
                    self.step_match(1);
                }
                if up {
                    self.step_match(-1);
                }
            }
        }
        let count = self.search_matches().len();
        let shown = self.search_match_at().map(|(i, _)| i);
        let no_match = !self.query.trim().is_empty() && count == 0;
        let mut field = egui::TextEdit::singleline(&mut self.query)
            .id(id)
            .hint_text("Search: just type")
            .desired_width(220.0);
        if no_match {
            field = field.text_color(Palette::of(ui).red);
        }
        let response = ui.add_enabled(!locked, field).on_hover_text(
            "Type anywhere to jump to the first window whose title or program \
             contains the words. Down / Up go to the next / previous match, \
             Enter picks it, Esc clears the search.",
        );
        if let Some(index) = shown
            && count > 1
        {
            // Right to left: shows as "2 of 5 ^ v [field]".
            if step_button(ui, false)
                .on_hover_text("Next match (Down key)")
                .clicked()
            {
                self.step_match(1);
            }
            if step_button(ui, true)
                .on_hover_text("Previous match (Up key)")
                .clicked()
            {
                self.step_match(-1);
            }
            ui.label(RichText::new(format!("{} of {count}", index + 1)).weak());
        }
        let mut picked = None;
        if !locked {
            let (enter, escape) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if escape {
                self.query.clear();
                response.surrender_focus();
            } else if enter && response.lost_focus() {
                picked = self.search_match().cloned();
            }
        }
        if self.query != before {
            // A new search starts at its first match.
            self.current_match = self.search_matches().first().map(|w| w.hwnd);
            if let Some(hwnd) = self.current_match {
                self.scroll_into_view(Some(hwnd));
            }
        }
        picked
    }

    /// Text typed while no widget has the keyboard replaces the search and
    /// focuses the field, with the cursor at the end.
    fn type_to_search(&mut self, ctx: &egui::Context, id: egui::Id) {
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let typed = ctx.input_mut(|i| {
            let mut typed = String::new();
            i.events.retain(|event| match event {
                egui::Event::Text(text) => {
                    typed.push_str(text);
                    false
                }
                _ => true,
            });
            typed
        });
        let typed = typed.trim_start();
        if typed.is_empty() {
            return;
        }
        self.query = typed.to_owned();
        ctx.memory_mut(|m| m.request_focus(id));
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let end = egui::text::CCursor::new(self.query.chars().count());
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ctx, id);
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

    /// Draws the carousel; returns a window the user clicked. With `cast`
    /// (while casting) it ignores all input, dims the other windows and marks
    /// the selected one with the badge.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        selected: Option<isize>,
        cast: Option<LiveBadge>,
    ) -> Option<WindowInfo> {
        let locked = cast.is_some();
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
        let found = self.search_match().map(|w| w.hwnd).filter(|_| !locked);

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
            let badge = cast.filter(|_| live).map(LiveBadge::style);
            let dark = ui.visuals().dark_mode;
            let (fill, stroke) = if let Some((_, _, _, border)) = badge {
                (border.gamma_multiply(0.18), Stroke::new(2.5, border))
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
                (gray(dark, 22, 228), Stroke::new(1.0, gray(dark, 40, 210)))
            } else {
                (
                    ui.visuals().faint_bg_color,
                    Stroke::new(1.0, gray(dark, 60, 180)),
                )
            };
            painter.rect(tile, 6.0, fill, stroke, egui::StrokeKind::Inside);
            if found == Some(window.hwnd) {
                painter.rect_stroke(
                    tile,
                    6.0,
                    Stroke::new(3.0, Palette::of(ui).teal),
                    egui::StrokeKind::Inside,
                );
            }

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
                // Darkens the icon on the dark theme, fades it on the light one.
                if dark {
                    Color32::from_gray(90)
                } else {
                    Color32::from_white_alpha(110)
                }
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
            if let Some((text, badge_fill, text_color, _)) = badge {
                // "LIVE" / "NOT LIVE" badge on the cast window (D49).
                let galley = painter.layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::proportional(11.0),
                    text_color,
                );
                let width = (galley.size().x + 14.0).max(40.0);
                let badge = Rect::from_min_size(
                    pos2(tile.right() - 8.0 - width, text_top + 2.0),
                    vec2(width, 17.0),
                );
                painter.rect_filled(badge, 3.0, badge_fill);
                painter.galley(badge.center() - galley.size() / 2.0, galley, text_color);
                text_right = badge.left() - 6.0;
            }
            let (title_color, sub_color) = if dimmed {
                (gray(dark, 95, 120), gray(dark, 70, 140))
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

/// Whether every word of `query` occurs in the window title or the exe name,
/// ignoring case. An empty query matches every window.
fn matches(window: &WindowInfo, query: &str) -> bool {
    let haystack = format!(
        "{} {}",
        window.title,
        window.process_name.as_deref().unwrap_or_default()
    )
    .to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

/// A gray for the dark or the light theme.
fn gray(dark: bool, on_dark: u8, on_light: u8) -> Color32 {
    Color32::from_gray(if dark { on_dark } else { on_light })
}

/// A small button with a triangle pointing up or down, drawn because egui's
/// default font has no arrows (D51).
fn step_button(ui: &mut egui::Ui, up: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
    }
    let c = rect.center();
    let (tip, base) = if up { (-4.0, 3.0) } else { (4.0, -3.0) };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            pos2(c.x, c.y + tip),
            pos2(c.x + 5.0, c.y + base),
            pos2(c.x - 5.0, c.y + base),
        ],
        visuals.fg_stroke.color,
        Stroke::NONE,
    ));
    response
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

#[cfg(test)]
mod tests {
    use super::*;

    fn window(hwnd: isize, title: &str, exe: Option<&str>) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid: 1,
            title: title.to_owned(),
            process_name: exe.map(str::to_owned),
            process_path: None,
            class_name: String::new(),
            width: 800,
            height: 600,
            minimized: false,
        }
    }

    #[test]
    fn last_cast_prefers_the_same_title_then_the_most_recent() {
        let mut carousel = Carousel::new(None);
        carousel.windows = vec![
            window(1, "Notes", Some("notepad.exe")),
            window(2, "Song A - Spotify", Some("Spotify.exe")),
            window(3, "Spotify Premium", Some("Spotify.exe")),
        ];
        let last = |title: &str| LastCast {
            process: "spotify.exe".to_owned(),
            title: title.to_owned(),
        };
        let found = |c: &mut Carousel, title| c.find_last_cast(&last(title)).map(|w| w.hwnd);
        assert_eq!(found(&mut carousel, "Spotify Premium"), Some(3));
        assert_eq!(found(&mut carousel, "Another song"), Some(2));
        carousel.windows.retain(|w| w.hwnd == 1);
        assert_eq!(found(&mut carousel, "Spotify Premium"), None);
    }

    #[test]
    fn search_matches_all_words_in_title_or_exe() {
        let w = window(1, "Rocket League (64-bit, DX11)", Some("RocketLeague.exe"));
        assert!(matches(&w, "rocket"));
        assert!(matches(&w, "LEAGUE dx11"));
        assert!(matches(&w, "rocketleague.exe"));
        assert!(!matches(&w, "rocket chrome"));
        assert!(!matches(&window(2, "Untitled", None), "notepad"));
    }

    #[test]
    fn search_finds_the_most_recent_match_and_scrolls_to_it() {
        let mut carousel = Carousel::new(None);
        carousel.windows = (1..=8)
            .map(|i| window(i, &format!("Window {i}"), Some("app.exe")))
            .collect();
        carousel.windows[6].title = "Counter-Strike 2".to_owned();
        carousel.windows[7].title = "CS2 wiki - Chrome".to_owned();
        assert!(carousel.search_match().is_none());
        carousel.query = "cs2".to_owned();
        assert_eq!(carousel.search_match().unwrap().hwnd, 8);
        carousel.query = "nothing".to_owned();
        assert!(carousel.search_match().is_none());
        carousel.query = "counter".to_owned();
        let hwnd = carousel.search_match().unwrap().hwnd;
        assert_eq!(hwnd, 7);
        carousel.scroll_into_view(Some(hwnd));
        let left = carousel.snap.unwrap() as i64;
        let offset = (6 - left).rem_euclid(8);
        assert!(offset < VISIBLE as i64, "index 6 visible from {left}");
    }

    #[test]
    fn arrow_keys_step_through_matches_and_wrap() {
        let mut carousel = Carousel::new(None);
        carousel.windows = vec![
            window(1, "Chrome - news", Some("chrome.exe")),
            window(2, "Notes", Some("notepad.exe")),
            window(3, "Chrome - mail", Some("chrome.exe")),
            window(4, "Chrome - wiki", Some("chrome.exe")),
        ];
        carousel.query = "chrome".to_owned();
        let shown = |c: &Carousel| c.search_match_at().map(|(i, w)| (i, w.hwnd));
        assert_eq!(shown(&carousel), Some((0, 1)));
        carousel.step_match(1);
        assert_eq!(shown(&carousel), Some((1, 3)));
        carousel.step_match(1);
        carousel.step_match(1);
        assert_eq!(shown(&carousel), Some((0, 1)), "wraps to the first");
        carousel.step_match(-1);
        assert_eq!(shown(&carousel), Some((2, 4)), "wraps to the last");
        // The shown window closes: back to the first match.
        carousel.windows.retain(|w| w.hwnd != 4);
        assert_eq!(shown(&carousel), Some((0, 1)));
    }
}
