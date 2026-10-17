//! Window capture via Windows Graphics Capture (`windows-capture`), producing
//! packed BGRX frames for NDI (SPEC §5.1).
//!
//! Frames travel as `grafton_ndi::VideoFrame`s in a closed loop: the capture
//! thread fills a frame and sends it to the NDI thread, which returns the
//! frame the SDK has just released. This avoids a per-frame allocation and
//! any copy beyond the readback itself.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use grafton_ndi::{PixelFormat, VideoFrame};
use tracing::{info, warn};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

use super::{pacer::Pacer, scale};
use crate::os_caps::OsCaps;

type HandlerError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub struct VideoOptions {
    pub fps_cap: u32,
    /// Larger windows are downscaled to fit, keeping the aspect ratio.
    pub max_size: (u32, u32),
    pub show_border: bool,
    pub capture_cursor: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            fps_cap: 60,
            max_size: (1920, 1080),
            show_border: false,
            capture_cursor: false,
        }
    }
}

/// Counters for status display; written by the capture thread.
#[derive(Debug, Default)]
pub struct VideoStats {
    /// Frames read back and handed to the NDI thread.
    pub captured: AtomicU64,
    /// Frames skipped by the FPS cap.
    pub paced_out: AtomicU64,
    /// Frames skipped because the NDI thread was behind.
    pub backlogged: AtomicU64,
    pub readback_us: AtomicU64,
    pub source_width: AtomicU32,
    pub source_height: AtomicU32,
    pub width: AtomicU32,
    pub height: AtomicU32,
}

/// Preview size and rate limit for the UI miniview (SPEC §3.1).
const PREVIEW_MAX: (u32, u32) = (320, 180);
const PREVIEW_INTERVAL: Duration = Duration::from_millis(80);

/// A small RGBA image for the UI preview.
pub struct PreviewImage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

/// Latest preview image, produced by the capture thread only while
/// `enabled` (i.e. while the UI is visible).
#[derive(Default)]
pub struct PreviewSlot {
    pub enabled: AtomicBool,
    image: Mutex<Option<PreviewImage>>,
}

impl PreviewSlot {
    /// Takes the newest image, if one arrived since the last call.
    pub fn take(&self) -> Option<PreviewImage> {
        self.image.lock().ok()?.take()
    }

    fn put(&self, image: PreviewImage) {
        if let Ok(mut slot) = self.image.lock() {
            *slot = Some(image);
        }
    }
}

/// A running capture session. Stops when dropped.
pub struct VideoCapture {
    control: Option<CaptureControl<Handler, HandlerError>>,
    closed: Arc<AtomicBool>,
    pub stats: Arc<VideoStats>,
}

impl VideoCapture {
    /// Starts capturing `hwnd`. Options the OS cannot honour fall back to the
    /// OS default with a warning (SPEC §5.0).
    pub fn start(
        hwnd: isize,
        options: &VideoOptions,
        caps: &OsCaps,
        frames: Sender<VideoFrame>,
        recycled: Receiver<VideoFrame>,
        preview: Arc<PreviewSlot>,
    ) -> Result<Self> {
        if !caps.wgc {
            bail!("Windows Graphics Capture is not supported on this system");
        }

        let cursor = match (caps.cursor_control, options.capture_cursor) {
            (true, true) => CursorCaptureSettings::WithCursor,
            (true, false) => CursorCaptureSettings::WithoutCursor,
            (false, wanted) => {
                if !wanted {
                    warn!("this Windows version cannot hide the cursor in captures");
                }
                CursorCaptureSettings::Default
            }
        };
        let border = match (caps.border_control, options.show_border) {
            (true, true) => DrawBorderSettings::WithBorder,
            (true, false) => DrawBorderSettings::WithoutBorder,
            (false, wanted) => {
                if !wanted {
                    warn!("this Windows version always draws the yellow capture border");
                }
                DrawBorderSettings::Default
            }
        };
        let pacer = Pacer::new(options.fps_cap);
        // Only a coarse prefilter at twice the target rate; the exact cap is the
        // pacer's job (D8).
        let interval = if caps.min_update_interval {
            MinimumUpdateIntervalSettings::Custom(pacer.period() / 2)
        } else {
            MinimumUpdateIntervalSettings::Default
        };
        info!(?cursor, ?border, ?interval, "capture session settings");

        let stats = Arc::new(VideoStats::default());
        let closed = Arc::new(AtomicBool::new(false));
        let config = HandlerConfig {
            pacer,
            fps: options.fps_cap,
            max_size: options.max_size,
            frames,
            recycled,
            stats: stats.clone(),
            closed: closed.clone(),
            preview,
        };
        let settings = Settings::new(
            Window::from_raw_hwnd(hwnd as *mut _),
            cursor,
            border,
            SecondaryWindowSettings::Default,
            interval,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            config,
        );
        let control = Handler::start_free_threaded(settings)
            .map_err(|e| anyhow!("failed to start window capture: {e}"))?;

        Ok(Self {
            control: Some(control),
            closed,
            stats,
        })
    }

    /// False once the window was closed or the capture thread ended.
    pub fn is_running(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
            && self.control.as_ref().is_some_and(|c| !c.is_finished())
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<()> {
        let Some(control) = self.control.take() else {
            return Ok(());
        };
        let result = if control.is_finished() {
            control.wait()
        } else {
            control.stop()
        };
        result.map_err(|e| anyhow!("window capture ended with an error: {e}"))
    }
}

impl Drop for VideoCapture {
    fn drop(&mut self) {
        if let Err(err) = self.stop_inner() {
            warn!(%err, "error while stopping capture");
        }
    }
}

struct HandlerConfig {
    pacer: Pacer,
    fps: u32,
    max_size: (u32, u32),
    frames: Sender<VideoFrame>,
    recycled: Receiver<VideoFrame>,
    stats: Arc<VideoStats>,
    closed: Arc<AtomicBool>,
    preview: Arc<PreviewSlot>,
}

struct Handler {
    config: HandlerConfig,
    /// A frame that could not be sent, kept for reuse.
    spare: Option<VideoFrame>,
    last_size: (u32, u32),
    last_preview: Option<Instant>,
}

impl Handler {
    /// Returns a frame of `size` from the recycled pool or a new one.
    fn frame_for(&mut self, size: (u32, u32)) -> Result<VideoFrame, HandlerError> {
        let matches = |f: &VideoFrame| (f.width() as u32, f.height() as u32) == size;
        if let Some(frame) = self.spare.take().filter(matches) {
            return Ok(frame);
        }
        // Frames of an outdated size are dropped here.
        while let Ok(frame) = self.config.recycled.try_recv() {
            if matches(&frame) {
                return Ok(frame);
            }
        }
        let frame = VideoFrame::builder()
            .resolution(size.0 as i32, size.1 as i32)
            .pixel_format(PixelFormat::BGRX)
            .frame_rate(self.config.fps as i32, 1)
            .aspect_ratio(size.0 as f32 / size.1 as f32)
            .build()?;
        Ok(frame)
    }
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = HandlerConfig;
    type Error = HandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            config: ctx.flags,
            spare: None,
            last_size: (0, 0),
            last_preview: None,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let now = Instant::now();
        let stats = self.config.stats.clone();
        // Drop surplus frames before the GPU → CPU readback, which is the cost.
        if !self.config.pacer.accept(now) {
            stats.paced_out.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        if self.config.frames.is_full() {
            stats.backlogged.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        let source = (frame.width(), frame.height());
        let size = scale::fit_size(source, self.config.max_size);
        if size != self.last_size {
            info!(
                source = format!("{}x{}", source.0, source.1),
                output = format!("{}x{}", size.0, size.1),
                "capture size changed"
            );
            self.last_size = size;
        }
        let mut out = self.frame_for(size)?;

        let mut buffer = frame.buffer()?;
        let pitch = buffer.row_pitch() as usize;
        scale::copy_or_scale(buffer.as_raw_buffer(), source, pitch, out.data_mut(), size);
        drop(buffer);

        let preview = &self.config.preview;
        if preview.enabled.load(Ordering::Relaxed)
            && self
                .last_preview
                .is_none_or(|t| t.elapsed() >= PREVIEW_INTERVAL)
        {
            self.last_preview = Some(now);
            let ((w, h), rgba) = scale::preview_rgba(out.data(), size, PREVIEW_MAX);
            preview.put(PreviewImage {
                size: [w as usize, h as usize],
                rgba,
            });
        }

        stats
            .readback_us
            .fetch_add(now.elapsed().as_micros() as u64, Ordering::Relaxed);
        stats.source_width.store(source.0, Ordering::Relaxed);
        stats.source_height.store(source.1, Ordering::Relaxed);
        stats.width.store(size.0, Ordering::Relaxed);
        stats.height.store(size.1, Ordering::Relaxed);

        match self.config.frames.try_send(out) {
            Ok(()) => {
                stats.captured.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(frame)) => {
                stats.backlogged.fetch_add(1, Ordering::Relaxed);
                self.spare = Some(frame);
            }
            Err(TrySendError::Disconnected(_)) => {
                info!("NDI sender gone; stopping capture");
                control.stop();
            }
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        info!("capture item closed (target window destroyed)");
        self.config.closed.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// How long the capture may deliver no frames before the UI should suggest
/// borderless windowed mode (SPEC §5.1).
pub const NO_FRAMES_HINT_AFTER: Duration = Duration::from_secs(3);
