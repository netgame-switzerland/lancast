//! M0 spike: capture one window via Windows Graphics Capture and log FPS,
//! resolution and the cost of the GPU → CPU readback.
//!
//! Usage: `cargo run --release --example wgc_window -- <0xHWND | PID> [seconds] [--border]`
//! (use `list_windows` to find a target; `seconds` defaults to 10; the yellow
//! capture border is off unless `--border` is given or the OS cannot turn it off).

use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use lancast::{capture::pacer::Pacer, windows_list};
use tracing::{info, warn};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

const FPS_CAP: u32 = 60;

/// Counters written by the capture thread and read by the reporting loop.
#[derive(Default)]
struct Stats {
    frames: AtomicU32,
    dropped: AtomicU32,
    width: AtomicU32,
    height: AtomicU32,
    readback_us: AtomicU64,
}

struct Capture {
    stats: Arc<Stats>,
    pacer: Pacer,
    /// Tightly packed BGRA copy of the latest frame, as M1 will hand it to NDI.
    pixels: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Capture {
    type Flags = Arc<Stats>;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            stats: ctx.flags,
            pacer: Pacer::new(FPS_CAP),
            pixels: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let start = Instant::now();
        // Drop surplus frames before the GPU → CPU readback, which is the cost.
        if !self.pacer.accept(start) {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let buffer = frame.buffer()?;
        let (width, height) = (buffer.width(), buffer.height());
        let packed = buffer.as_nopadding_buffer(&mut self.pixels);
        debug_assert_eq!(packed.len(), (width * height * 4) as usize);

        let stats = &self.stats;
        stats
            .readback_us
            .fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
        stats.width.store(width, Ordering::Relaxed);
        stats.height.store(height, Ordering::Relaxed);
        stats.frames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        info!("capture item closed (window destroyed)");
        Ok(())
    }
}

pub fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    let (flags, positional): (Vec<String>, Vec<String>) =
        std::env::args().skip(1).partition(|a| a.starts_with("--"));
    let show_border = flags.iter().any(|f| f == "--border");
    let mut args = positional.into_iter();
    let target = args
        .next()
        .context("usage: wgc_window <0xHWND | PID> [seconds] [--border]")?;
    let seconds: u64 = match args.next() {
        Some(s) => s.parse().context("seconds must be an integer")?,
        None => 10,
    };
    let window = windows_list::resolve_target(&target)?;
    info!(
        hwnd = format!("0x{:x}", window.hwnd),
        pid = window.pid,
        process = window.process_name.as_deref().unwrap_or("?"),
        "capturing"
    );
    if window.minimized {
        warn!("window is minimized; WGC delivers no frames until it is restored");
    }

    if !GraphicsCaptureApi::is_supported()? {
        bail!("Windows Graphics Capture is not supported on this system");
    }
    // windows-capture returns an error for non-default settings the OS lacks,
    // so fall back to `Default` explicitly (e.g. no border control on Windows 10).
    let cursor = if GraphicsCaptureApi::is_cursor_settings_supported()? {
        CursorCaptureSettings::WithoutCursor
    } else {
        CursorCaptureSettings::Default
    };
    let border = if GraphicsCaptureApi::is_border_settings_supported()? {
        if show_border {
            DrawBorderSettings::WithBorder
        } else {
            DrawBorderSettings::WithoutBorder
        }
    } else {
        warn!("border control not supported; the yellow capture border stays");
        DrawBorderSettings::Default
    };
    // Only a coarse prefilter at twice the target rate; the exact cap is the
    // Pacer's job (an interval of exactly 1/FPS_CAP yields 48 fps at 144 Hz).
    let interval = if GraphicsCaptureApi::is_minimum_update_interval_supported()? {
        MinimumUpdateIntervalSettings::Custom(Duration::from_secs(1) / (2 * FPS_CAP))
    } else {
        MinimumUpdateIntervalSettings::Default
    };
    info!(?cursor, ?border, ?interval, "session settings");

    let stats = Arc::new(Stats::default());
    let settings = Settings::new(
        Window::from_raw_hwnd(window.hwnd as *mut _),
        cursor,
        border,
        SecondaryWindowSettings::Default,
        interval,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        stats.clone(),
    );
    let control = Capture::start_free_threaded(settings)
        .map_err(|e| anyhow!("failed to start capture: {e}"))?;

    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed() < Duration::from_secs(seconds) && !control.is_finished() {
        std::thread::sleep(Duration::from_secs(1));
        let elapsed = last.elapsed().as_secs_f64();
        last = Instant::now();
        let frames = stats.frames.swap(0, Ordering::Relaxed);
        let dropped = stats.dropped.swap(0, Ordering::Relaxed);
        let readback_us = stats.readback_us.swap(0, Ordering::Relaxed);
        if frames == 0 {
            let minimized = windows_list::window_by_hwnd(window.hwnd).is_none_or(|w| w.minimized);
            info!(minimized, "no frames (static content or minimized window)");
            continue;
        }
        info!(
            fps = format!("{:.1}", frames as f64 / elapsed),
            dropped,
            resolution = format!(
                "{}x{}",
                stats.width.load(Ordering::Relaxed),
                stats.height.load(Ordering::Relaxed)
            ),
            readback_ms = format!("{:.2}", readback_us as f64 / frames as f64 / 1000.0),
            "capturing"
        );
    }

    if control.is_finished() {
        control.wait().map_err(|e| anyhow!("capture failed: {e}"))?;
    } else {
        control
            .stop()
            .map_err(|e| anyhow!("failed to stop capture: {e}"))?;
    }
    info!("done");
    Ok(())
}
