//! Window picker as an Alt-Tab-like carousel (D19): three tiles with live
//! DWM thumbnails, app icon and title; cycles left and right endlessly.

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

const VISIBLE: usize = 3;
const HEIGHT: f32 = 170.0;
const ARROW_WIDTH: f32 = 26.0;
const GAP: f32 = 8.0;
const TEXT_HEIGHT: f32 = 38.0;
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

pub struct Carousel {
    /// Alt-Tab-like order: most recently active first, kept stable.
    windows: Vec<WindowInfo>,
    /// Animated index of the centered tile (unbounded; wraps modulo len).
    position: f32,
    target: i64,
    last_frame: Option<Instant>,
    last_refresh: Option<Instant>,
    icons: HashMap<String, Option<TextureHandle>>,
    thumbnails: Option<Thumbnails>,
}

impl Carousel {
    /// `destination` is our own top-level window, for the DWM thumbnails.
    pub fn new(destination: Option<isize>) -> Self {
        Self {
            windows: Vec::new(),
            position: 0.0,
            target: 0,
            last_frame: None,
            last_refresh: None,
            icons: HashMap::new(),
            thumbnails: destination.map(Thumbnails::new),
        }
    }

    /// Removes all thumbnails (while sending or hidden).
    pub fn hide(&mut self) {
        if let Some(thumbnails) = &mut self.thumbnails {
            thumbnails.clear();
        }
        self.last_frame = None;
    }

    /// Re-reads the window list. Known windows keep their place; new ones
    /// (usually just started or activated) go to the front, centered windows
    /// stay centered.
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
            self.target = 0;
            return;
        }
        let centered = self.current().map(|w| w.hwnd);
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
        if let Some(index) = centered.and_then(|h| self.windows.iter().position(|w| w.hwnd == h)) {
            let shift = index as i64 - self.target.rem_euclid(self.len().max(1) as i64);
            self.target += shift;
            self.position += shift as f32;
        }
        if let Some(thumbnails) = &mut self.thumbnails {
            let windows = &self.windows;
            thumbnails.retain(|h| windows.iter().any(|w| w.hwnd == h));
        }
    }

    fn len(&self) -> usize {
        self.windows.len()
    }

    fn current(&self) -> Option<&WindowInfo> {
        let n = self.len() as i64;
        (n > 0).then(|| &self.windows[self.target.rem_euclid(n) as usize])
    }

    /// Draws the carousel; returns a window the user clicked.
    pub fn show(&mut self, ui: &mut egui::Ui, selected: Option<isize>) -> Option<WindowInfo> {
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
        let n = self.len();
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

        // Navigation: arrows, mouse wheel over the carousel.
        if cycling {
            let arrow = |ui: &mut egui::Ui, rect: Rect, text: &str| {
                ui.put(rect, egui::Button::new(RichText::new(text).size(18.0)))
                    .on_hover_text("More windows")
                    .clicked()
            };
            let left = Rect::from_min_size(area.min, vec2(ARROW_WIDTH, HEIGHT));
            let right = Rect::from_min_size(
                pos2(area.right() - ARROW_WIDTH, area.top()),
                vec2(ARROW_WIDTH, HEIGHT),
            );
            if arrow(ui, left, "<") {
                self.target -= 1;
            }
            if arrow(ui, right, ">") {
                self.target += 1;
            }
            let hovered = ui.rect_contains_pointer(area);
            let scroll = ctx.input(|i| i.smooth_scroll_delta.x + i.smooth_scroll_delta.y);
            if hovered && scroll.abs() > 20.0 {
                self.target += if scroll > 0.0 { -1 } else { 1 };
            }
        }

        // Animate towards the target.
        let now = Instant::now();
        let dt = self
            .last_frame
            .map_or(1.0, |t| now.duration_since(t).as_secs_f32());
        self.last_frame = Some(now);
        let target = self.target as f32;
        self.position += (target - self.position) * (dt * 14.0).min(1.0);
        if (target - self.position).abs() < 0.002 {
            self.position = target;
        } else {
            ctx.request_repaint();
        }

        // Tiles to draw: (slot, window index, x offset of the tile center
        // from the middle). The slot is stable while animating, for click ids.
        let slots: Vec<(i64, usize, f32)> = if cycling {
            let base = self.position.floor() as i64;
            (base - 2..=base + 3)
                .map(|i| {
                    (
                        i,
                        i.rem_euclid(n as i64) as usize,
                        (i as f32 - self.position) * step,
                    )
                })
                .collect()
        } else {
            (0..n)
                .map(|i| (i as i64, i, (i as f32 - (n as f32 - 1.0) / 2.0) * step))
                .collect()
        };

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
        let mut clicked = None;

        for (slot, index, offset) in slots {
            let center_x = inner.center().x + offset;
            let tile =
                Rect::from_center_size(pos2(center_x, area.center().y), vec2(tile_width, HEIGHT));
            let visible = tile.intersect(inner);
            if visible.width() <= 1.0 {
                continue;
            }
            let window = &self.windows[index];
            let is_selected = selected == Some(window.hwnd);
            let response = ui.interact(visible, ui.scope_id().with(("tile", slot)), Sense::click());
            let (fill, stroke) = if is_selected {
                (
                    ui.visuals().selection.bg_fill.gamma_multiply(0.35),
                    Stroke::new(2.0, ui.visuals().selection.stroke.color),
                )
            } else if response.hovered() {
                (
                    ui.visuals().widgets.hovered.weak_bg_fill,
                    Stroke::new(1.0, Color32::from_gray(110)),
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
            let has_thumbnail = match &mut self.thumbnails {
                Some(thumbnails) => {
                    let ok = thumbnails.source_size(window.hwnd).is_some();
                    if ok {
                        thumbnails.show(window.hwnd, px(thumb_area), px(inner));
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
            if !has_thumbnail && let Some(icon) = &icon {
                // No live thumbnail (e.g. minimized on some systems): big icon.
                let r = Rect::from_center_size(thumb_area.center(), vec2(48.0, 48.0));
                painter.image(icon.id(), r, uv(), Color32::WHITE);
            }
            let text_top = tile.bottom() - TEXT_HEIGHT;
            let mut text_left = tile.left() + 8.0;
            if let Some(icon) = &icon {
                let r = Rect::from_min_size(pos2(text_left, text_top + 2.0), vec2(16.0, 16.0));
                painter.image(icon.id(), r, uv(), Color32::WHITE);
                text_left += 22.0;
            }
            let text_width = tile.right() - 8.0 - text_left;
            let title = elide(&painter, &window.title, 13.0, text_width);
            painter.text(
                pos2(text_left, text_top + 10.0),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::proportional(13.0),
                ui.visuals().strong_text_color(),
            );
            if let Some(process) = &window.process_name {
                painter.text(
                    pos2(text_left, text_top + 27.0),
                    egui::Align2::LEFT_CENTER,
                    elide(&painter, process, 11.0, text_width),
                    egui::FontId::proportional(11.0),
                    ui.visuals().weak_text_color(),
                );
            }

            if response.clicked() {
                clicked = Some(window.clone());
                // Center the clicked tile.
                if cycling {
                    self.target = slot;
                }
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
