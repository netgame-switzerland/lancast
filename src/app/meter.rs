//! OBS-style audio level meter (SPEC §3.1 item 6, D12): one vertical bar per
//! channel with green/yellow/red zones, peak with decay, RMS marker and peak
//! hold. Constants from OBS `frontend/components/VolumeMeter.cpp`.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};

use crate::capture::audio_wasapi::{AudioLevels, CHANNELS, SILENCE_DB};

const MIN_DB: f32 = -60.0;
const WARNING_DB: f32 = -20.0;
const ERROR_DB: f32 = -9.0;
/// Peak fall rate: 20 dB / 1.7 s.
const PEAK_DECAY_DB_PER_S: f32 = 11.76;
/// RMS reaches 99 % of a step in 300 ms.
const RMS_INTEGRATION_S: f32 = 0.3;
const PEAK_HOLD: Duration = Duration::from_secs(20);
/// Peaks above this count as "audio active".
const ACTIVE_DB: f32 = -60.0;

#[derive(Clone, Copy)]
struct Channel {
    peak: f32,
    rms: f32,
    hold: f32,
    hold_since: Instant,
}

pub struct Meter {
    channels: [Channel; CHANNELS],
    last_update: Option<Instant>,
    last_active: Option<Instant>,
}

impl Default for Meter {
    fn default() -> Self {
        let silent = Channel {
            peak: SILENCE_DB,
            rms: SILENCE_DB,
            hold: SILENCE_DB,
            hold_since: Instant::now(),
        };
        Self {
            channels: [silent; CHANNELS],
            last_update: None,
            last_active: None,
        }
    }
}

impl Meter {
    /// Pulls new levels and applies the ballistics; `None` lets the bars fall.
    pub fn update(&mut self, levels: Option<&AudioLevels>) {
        let now = Instant::now();
        let dt = self
            .last_update
            .map_or(0.0, |t| now.duration_since(t).as_secs_f32())
            .min(0.5);
        self.last_update = Some(now);

        for (index, ch) in self.channels.iter_mut().enumerate() {
            let (peak, rms) = match levels {
                Some(l) => (l.take_peak_db(index), l.rms_db(index)),
                None => (SILENCE_DB, SILENCE_DB),
            };
            ch.peak = if peak >= ch.peak {
                peak
            } else {
                (ch.peak - PEAK_DECAY_DB_PER_S * dt).max(peak)
            };
            let k = (dt / RMS_INTEGRATION_S * 0.99).min(1.0);
            ch.rms += (rms - ch.rms) * k;
            if peak >= ch.hold || now.duration_since(ch.hold_since) >= PEAK_HOLD {
                ch.hold = peak;
                ch.hold_since = now;
            }
            if peak > ACTIVE_DB {
                self.last_active = Some(now);
            }
        }
    }

    /// Non-silent audio arrived within the last second.
    pub fn active(&self) -> bool {
        self.last_active
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
    }

    /// Paints the bars with a dB scale; `height` in points.
    pub fn show(&self, ui: &mut egui::Ui, height: f32) {
        let bar_width = 12.0;
        let gap = 4.0;
        let scale_width = 26.0;
        let size = vec2(CHANNELS as f32 * (bar_width + gap) + scale_width, height);
        let (outer, _) = ui.allocate_exact_size(size, Sense::hover());
        let painter = ui.painter_at(outer);
        // Leave room for the scale labels at 0 and -60 dB.
        let rect = outer.shrink2(vec2(0.0, 6.0));
        let y_of = |db: f32| {
            let t = ((db - MIN_DB) / -MIN_DB).clamp(0.0, 1.0);
            rect.bottom() - t * rect.height()
        };

        for (index, ch) in self.channels.iter().enumerate() {
            let left = rect.left() + index as f32 * (bar_width + gap);
            let bar = Rect::from_min_max(
                pos2(left, rect.top()),
                pos2(left + bar_width, rect.bottom()),
            );
            // Dim zone backgrounds.
            for (from, to, color) in zones() {
                painter.rect_filled(
                    span(bar, y_of(from), y_of(to)),
                    0.0,
                    color.gamma_multiply(0.25),
                );
            }
            // Lit part up to the peak, in zone colors.
            for (from, to, color) in zones() {
                let top = to.min(ch.peak);
                if top > from {
                    painter.rect_filled(span(bar, y_of(from), y_of(top)), 0.0, color);
                }
            }
            // RMS marker and peak hold.
            if ch.rms > MIN_DB {
                let y = y_of(ch.rms);
                painter.line_segment(
                    [pos2(bar.left(), y), pos2(bar.right(), y)],
                    Stroke::new(2.0, Color32::WHITE),
                );
            }
            if ch.hold > MIN_DB {
                let y = y_of(ch.hold);
                painter.line_segment(
                    [pos2(bar.left(), y), pos2(bar.right(), y)],
                    Stroke::new(2.0, zone_color(ch.hold)),
                );
            }
        }

        // Scale.
        let text_x = rect.right() - scale_width + 4.0;
        for db in [0, -9, -20, -30, -40, -50, -60] {
            let y = y_of(db as f32);
            painter.text(
                Pos2::new(text_x, y),
                egui::Align2::LEFT_CENTER,
                db.to_string(),
                egui::FontId::proportional(9.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

fn zones() -> [(f32, f32, Color32); 3] {
    [
        (MIN_DB, WARNING_DB, Color32::from_rgb(60, 190, 80)),
        (WARNING_DB, ERROR_DB, Color32::from_rgb(230, 200, 40)),
        (ERROR_DB, 0.0, Color32::from_rgb(225, 60, 50)),
    ]
}

fn zone_color(db: f32) -> Color32 {
    zones()
        .into_iter()
        .find(|(_, to, _)| db <= *to)
        .map_or(Color32::from_rgb(225, 60, 50), |(_, _, c)| c)
}

/// The horizontal slice of `bar` between two y coordinates.
fn span(bar: Rect, y_bottom: f32, y_top: f32) -> Rect {
    Rect::from_min_max(pos2(bar.left(), y_top), pos2(bar.right(), y_bottom))
}
