//! The diagnostics window while sending (D57): numbers on the left, the
//! gaps between sent frames as a bar graph on the right. Laid out to be read
//! from a screenshot; values that point at a problem are amber. A window of
//! its own, because the main window has no room to spare once the settings
//! or banners are open.

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, pos2, vec2};

use super::{controller::Sending, theme::Palette};
use crate::{
    capture::timeline::GapStats,
    diagnostics::{Diagnostics, GRAPH_SPAN},
};

const GRAPH_MIN_WIDTH: f32 = 220.0;
const GRAPH_MIN_HEIGHT: f32 = 120.0;
/// Below this width the graph goes under the numbers.
const SIDE_BY_SIDE_WIDTH: f32 = 820.0;

/// Shows the diagnostics window for this frame; returns `true` when the
/// user closed it.
pub fn window(ctx: &egui::Context, sending: &Sending) -> bool {
    let id = egui::ViewportId::from_hash_of("lancast-diagnostics");
    let builder = egui::ViewportBuilder::default()
        .with_title("LANcast diagnostics")
        .with_inner_size([1000.0, 250.0])
        .with_min_inner_size([520.0, 200.0])
        .with_icon(icon());
    ctx.show_viewport_immediate(id, builder, |ui, _class| {
        egui::CentralPanel::default().show(ui, |ui| show(ui, sending));
        ui.input(|i| i.viewport().close_requested())
    })
}

fn icon() -> Arc<egui::IconData> {
    let (side, rgba) = crate::app_icon::icon_rgba(32);
    Arc::new(egui::IconData {
        rgba,
        width: side as u32,
        height: side as u32,
    })
}

/// Numbers beside the graph if the window is wide enough, else above it.
fn show(ui: &mut egui::Ui, sending: &Sending) {
    let Some(d) = &sending.diagnostics else {
        ui.centered_and_justified(|ui| ui.label(RichText::new("Measuring…").weak()));
        return;
    };
    let available = ui.available_size();
    if available.x >= SIDE_BY_SIDE_WIDTH {
        ui.horizontal_top(|ui| {
            let numbers = ui.vertical(|ui| numbers(ui, sending, d));
            ui.add_space(12.0);
            let width = (available.x - numbers.response.rect.width() - 12.0).max(GRAPH_MIN_WIDTH);
            graph(ui, d, vec2(width, available.y.max(GRAPH_MIN_HEIGHT)));
        });
    } else {
        egui::ScrollArea::vertical().show(ui, |ui| {
            numbers(ui, sending, d);
            ui.add_space(8.0);
            let height = ui.available_height().max(GRAPH_MIN_HEIGHT);
            graph(ui, d, vec2(ui.available_width(), height));
        });
    }
}

fn gaps(stats: &GapStats) -> String {
    if stats.count == 0 {
        return "–".to_owned();
    }
    format!(
        "{:.1} / {:.1} / {:.1} ms  ±{:.1}",
        stats.min_ms, stats.avg_ms, stats.max_ms, stats.jitter_ms
    )
}

fn numbers(ui: &mut egui::Ui, sending: &Sending, d: &Diagnostics) {
    let amber = Palette::of(ui).amber;
    let s = &sending.status;
    let period_ms = d.period().as_secs_f64() * 1000.0;
    let row = |ui: &mut egui::Ui, label: &str, value: String, warn: bool| {
        ui.label(RichText::new(label).weak());
        let text = RichText::new(value).monospace();
        ui.label(if warn { text.color(amber) } else { text });
        ui.end_row();
    };
    ui.label(RichText::new(format!("Diagnostics · LANcast {}", crate::VERSION)).strong());
    egui::Grid::new("diagnostics")
        .num_columns(2)
        .spacing(vec2(12.0, 1.0))
        .show(ui, |ui| {
            // Even motion needs a whole number of refreshes per frame; else
            // the frames mix e.g. 2 and 3 refreshes (60 fps on 144 Hz, D59).
            let (display, uneven) = match d.refresh_hz {
                Some(hz) => {
                    let ratio = f64::from(hz) / f64::from(d.fps_cap.max(1));
                    let uneven = ratio > 1.0 && (ratio - ratio.round()).abs() > 0.02;
                    let text = format!(
                        "{hz} Hz · cap {} fps · {ratio:.2} refreshes/frame{}",
                        d.fps_cap,
                        if uneven { " (uneven)" } else { "" }
                    );
                    (text, uneven)
                }
                None => (format!("unknown · cap {} fps", d.fps_cap), false),
            };
            row(ui, "Display", display, uneven);
            row(
                ui,
                "Frames/s",
                format!(
                    "captured {:.1} → picked {:.1} → NDI {:.1}",
                    s.arrived_fps, s.capture_fps, s.send_fps
                ),
                false,
            );
            row(
                ui,
                "Gaps sent",
                format!("{}  · {} hitches", gaps(&d.sent), d.sent.hitches),
                d.sent.hitches > 0,
            );
            row(ui, "Gaps captured", gaps(&d.arrived), false);
            row(
                ui,
                "Skipped/s",
                format!(
                    "{} over the cap · {} dropped (NDI busy)",
                    s.paced_out, s.backlogged
                ),
                s.backlogged > 0,
            );
            row(
                ui,
                "GPU readback",
                format!("avg {:.1} · max {:.1} ms", s.readback_ms, s.readback_max_ms),
                s.readback_max_ms > period_ms,
            );
            row(
                ui,
                "NDI send",
                format!(
                    "blocked max {:.1} ms · {} receiver{}",
                    s.send_block_max_ms,
                    s.receivers,
                    if s.receivers == 1 { "" } else { "s" }
                ),
                false,
            );
            match d.cpu {
                Some(cpu) => row(
                    ui,
                    "CPU",
                    format!("LANcast {:.0} % · system {:.0} %", cpu.process, cpu.system),
                    cpu.system >= 90.0,
                ),
                None => row(ui, "CPU", "–".to_owned(), false),
            }
            match &d.net {
                Some(net) => {
                    ui.label(RichText::new("Network").weak());
                    let warn = net.wifi || net.link_mbps < 1000 || net.out_errors > 0;
                    let text = RichText::new(format!(
                        "{}{} · link {} · out {:.0} Mbit/s · {} errors",
                        net.name,
                        if net.wifi { " (Wi-Fi)" } else { "" },
                        link(net.link_mbps),
                        net.out_mbps,
                        net.out_errors
                    ))
                    .monospace();
                    ui.label(if warn { text.color(amber) } else { text })
                        .on_hover_text(&net.description);
                    ui.end_row();
                }
                None => row(ui, "Network", "–".to_owned(), false),
            }
        });
}

fn link(mbps: u64) -> String {
    if mbps >= 1000 && mbps.is_multiple_of(1000) {
        format!("{} Gbit/s", mbps / 1000)
    } else if mbps >= 1000 {
        format!("{:.1} Gbit/s", mbps as f64 / 1000.0)
    } else {
        format!("{mbps} Mbit/s")
    }
}

/// The gaps between sent frames over the last [`GRAPH_SPAN`], one bar per
/// frame, with the cap's period as a line. Gaps over 1.5 periods are amber:
/// there the receiver shows a frame twice.
fn graph(ui: &mut egui::Ui, d: &Diagnostics, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let weak = visuals.weak_text_color();
    let text = visuals.text_color();
    let amber = Palette::of(ui).amber;
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);

    let title_font = egui::FontId::proportional(11.0);
    painter.text(
        rect.left_top() + vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        format!("Gaps between sent frames, last {} s", GRAPH_SPAN.as_secs()),
        title_font.clone(),
        weak,
    );
    let plot = egui::Rect::from_min_max(
        rect.left_top() + vec2(6.0, 20.0),
        rect.right_bottom() - vec2(6.0, 4.0),
    );

    let period_ms = (d.period().as_secs_f64() * 1000.0) as f32;
    let peak = d.recent_ms.iter().copied().fold(0.0, f32::max);
    // At least 3 periods high, so an even stream sits at a third.
    let top = (period_ms * 3.0).max(peak * 1.1).min(250.0);
    let y = |ms: f32| plot.bottom() - (ms.min(top) / top) * plot.height();

    // One slot per expected frame over the span, so the bars keep their
    // width when frames are missing.
    let slots = (d.fps_cap as f32 * GRAPH_SPAN.as_secs_f32())
        .max(d.recent_ms.len() as f32)
        .max(1.0);
    let slot = plot.width() / slots;
    let bar = (slot - 1.0).max(1.0);
    let start = plot.right() - d.recent_ms.len() as f32 * slot;
    for (i, &ms) in d.recent_ms.iter().enumerate() {
        let x = start + i as f32 * slot;
        let color = if ms > period_ms * 1.5 {
            amber
        } else {
            text.gamma_multiply(0.55)
        };
        painter.rect_filled(
            egui::Rect::from_min_max(pos2(x, y(ms)), pos2(x + bar, plot.bottom())),
            0.0,
            color,
        );
    }

    let line_y = y(period_ms);
    painter.line_segment(
        [pos2(plot.left(), line_y), pos2(plot.right(), line_y)],
        Stroke::new(1.0, Palette::of(ui).accent),
    );
    painter.text(
        pos2(plot.left() + 2.0, line_y - 2.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{period_ms:.1} ms = {} fps", d.fps_cap),
        title_font.clone(),
        weak,
    );
    painter.text(
        pos2(plot.right(), plot.top()),
        egui::Align2::RIGHT_TOP,
        format!("{top:.0} ms"),
        title_font,
        weak,
    );
    if d.recent_ms.is_empty() {
        painter.text(
            plot.center(),
            egui::Align2::CENTER_CENTER,
            "no frames",
            egui::FontId::proportional(12.0),
            Color32::from_gray(140),
        );
    }
}
