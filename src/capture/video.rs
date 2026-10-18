//! Window video capture (SPEC §5.1): the platform-independent part. Every
//! backend hands its CPU frames to a [`FrameSink`], which applies the FPS
//! cap, scales into packed BGRX frames for NDI and feeds the UI preview.
//!
//! Frames travel as `grafton_ndi::VideoFrame`s in a closed loop: the capture
//! thread fills a frame and sends it to the NDI thread, which returns the
//! frame the SDK has just released. This avoids a per-frame allocation and
//! any copy beyond the readback itself.
//!
//! Backends: Windows Graphics Capture (`video_wgc`); other platforms have a
//! stub that reports window capture as unavailable (D32).

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, TrySendError};
use grafton_ndi::{PixelFormat, VideoFrame};
use tracing::info;

use super::{pacer::Pacer, scale};

#[cfg(windows)]
pub use super::video_wgc::VideoCapture;
#[cfg(not(windows))]
pub use unsupported::VideoCapture;

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
            // Visible by default so the player sees what is captured (2026-10-17).
            show_border: true,
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
/// Upper bound for the preview image; the UI requests its displayed size.
const PREVIEW_MAX: (u32, u32) = (1920, 1080);
const PREVIEW_INTERVAL: Duration = Duration::from_millis(50);

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
    /// Size the UI displays the preview at, in physical pixels (0 = default).
    width: AtomicU32,
    height: AtomicU32,
    image: Mutex<Option<PreviewImage>>,
}

impl PreviewSlot {
    /// Sets the displayed size in physical pixels, so the preview is produced
    /// at screen resolution (capped at `PREVIEW_MAX`).
    pub fn set_display_size(&self, width: u32, height: u32) {
        self.width
            .store(width.clamp(64, PREVIEW_MAX.0), Ordering::Relaxed);
        self.height
            .store(height.clamp(36, PREVIEW_MAX.1), Ordering::Relaxed);
    }

    fn display_size(&self) -> (u32, u32) {
        match (
            self.width.load(Ordering::Relaxed),
            self.height.load(Ordering::Relaxed),
        ) {
            (0, _) | (_, 0) => (640, 360),
            size => size,
        }
    }

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

/// How long the capture may deliver no frames before the UI should suggest
/// borderless windowed mode (SPEC §5.1).
pub const NO_FRAMES_HINT_AFTER: Duration = Duration::from_secs(3);

/// Error type of the capture callbacks.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) type SinkError = Box<dyn std::error::Error + Send + Sync>;

/// A captured frame mapped into CPU memory: packed BGRA rows of `pitch`
/// bytes each.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) trait Readback {
    /// The pixel data and the row pitch in bytes.
    fn bgra(&mut self) -> (&[u8], usize);
}

/// The per-frame work shared by all backends: FPS cap, backlog check,
/// scaling into recycled NDI frames, and the UI preview.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct FrameSink {
    pacer: Pacer,
    fps: u32,
    max_size: (u32, u32),
    /// `None` in preview-only mode (window selected, not sending).
    frames: Option<Sender<VideoFrame>>,
    recycled: Receiver<VideoFrame>,
    stats: Arc<VideoStats>,
    preview: Arc<PreviewSlot>,
    /// A frame that could not be sent, kept for reuse.
    spare: Option<VideoFrame>,
    last_size: (u32, u32),
    last_preview: Option<Instant>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl FrameSink {
    pub(super) fn new(
        pacer: Pacer,
        options: &VideoOptions,
        frames: Option<Sender<VideoFrame>>,
        recycled: Receiver<VideoFrame>,
        preview: Arc<PreviewSlot>,
    ) -> Self {
        Self {
            pacer,
            fps: options.fps_cap,
            max_size: options.max_size,
            frames,
            recycled,
            stats: Arc::new(VideoStats::default()),
            preview,
            spare: None,
            last_size: (0, 0),
            last_preview: None,
        }
    }

    pub(super) fn stats(&self) -> Arc<VideoStats> {
        self.stats.clone()
    }

    /// Whether a frame arriving at `now` should be read back at all. Surplus
    /// frames are dropped here, before the GPU → CPU readback, which is the
    /// cost.
    pub(super) fn wants_frame(&mut self, now: Instant) -> bool {
        if !self.pacer.accept(now) {
            self.stats.paced_out.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match &self.frames {
            Some(frames) if frames.is_full() => {
                self.stats.backlogged.fetch_add(1, Ordering::Relaxed);
                false
            }
            // Preview only: no readback at all while the UI is hidden.
            None => self.preview.enabled.load(Ordering::Relaxed),
            Some(_) => true,
        }
    }

    /// Processes a frame of `source` size that [`Self::wants_frame`]
    /// accepted. `readback` maps it into CPU memory; the mapping is released
    /// as soon as the pixels are copied. Returns `false` once the NDI sender
    /// is gone and capture should stop.
    pub(super) fn deliver<R: Readback>(
        &mut self,
        source: (u32, u32),
        now: Instant,
        readback: impl FnOnce() -> Result<R, SinkError>,
    ) -> Result<bool, SinkError> {
        let stats = self.stats.clone();
        let size = scale::fit_size(source, self.max_size);
        if size != self.last_size {
            info!(
                source = format!("{}x{}", source.0, source.1),
                output = format!("{}x{}", size.0, size.1),
                "capture size changed"
            );
            self.last_size = size;
        }
        let Some(frames) = self.frames.clone() else {
            // Preview only (not sending): straight from the readback to the
            // preview size, without an NDI-sized frame. The pacer already
            // limits this to the preview rate.
            let mut buffer = readback()?;
            let (data, pitch) = buffer.bgra();
            let preview = &self.preview;
            let ((w, h), rgba) = scale::preview_rgba(data, source, pitch, preview.display_size());
            drop(buffer);
            self.last_preview = Some(now);
            preview.put(PreviewImage {
                size: [w as usize, h as usize],
                rgba,
            });
            stats
                .readback_us
                .fetch_add(now.elapsed().as_micros() as u64, Ordering::Relaxed);
            stats.source_width.store(source.0, Ordering::Relaxed);
            stats.source_height.store(source.1, Ordering::Relaxed);
            stats.captured.fetch_add(1, Ordering::Relaxed);
            return Ok(true);
        };

        let mut out = self.frame_for(size)?;

        let mut buffer = readback()?;
        let (data, pitch) = buffer.bgra();
        scale::copy_or_scale(data, source, pitch, out.data_mut(), size);
        drop(buffer);

        let preview = &self.preview;
        if preview.enabled.load(Ordering::Relaxed)
            && self
                .last_preview
                .is_none_or(|t| t.elapsed() >= PREVIEW_INTERVAL)
        {
            self.last_preview = Some(now);
            let ((w, h), rgba) = scale::preview_rgba(
                out.data(),
                size,
                size.0 as usize * 4,
                preview.display_size(),
            );
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

        match frames.try_send(out) {
            Ok(()) => {
                stats.captured.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(frame)) => {
                stats.backlogged.fetch_add(1, Ordering::Relaxed);
                self.spare = Some(frame);
            }
            Err(TrySendError::Disconnected(_)) => {
                info!("NDI sender gone; stopping capture");
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Returns a frame of `size` from the recycled pool or a new one.
    fn frame_for(&mut self, size: (u32, u32)) -> Result<VideoFrame, SinkError> {
        let matches = |f: &VideoFrame| (f.width() as u32, f.height() as u32) == size;
        if let Some(frame) = self.spare.take().filter(matches) {
            return Ok(frame);
        }
        // Frames of an outdated size are dropped here.
        while let Ok(frame) = self.recycled.try_recv() {
            if matches(&frame) {
                return Ok(frame);
            }
        }
        let frame = VideoFrame::builder()
            .resolution(size.0 as i32, size.1 as i32)
            .pixel_format(PixelFormat::BGRX)
            .frame_rate(self.fps as i32, 1)
            .aspect_ratio(size.0 as f32 / size.1 as f32)
            .build()?;
        Ok(frame)
    }
}

/// Stub for platforms without a capture backend yet (D32): starting a
/// capture fails.
#[cfg(not(windows))]
mod unsupported {
    use std::sync::Arc;

    use anyhow::{Result, bail};
    use crossbeam_channel::{Receiver, Sender};
    use grafton_ndi::VideoFrame;

    use super::{PreviewSlot, VideoOptions, VideoStats};
    use crate::{os_caps::OsCaps, windows_list::WindowHandle};

    pub struct VideoCapture {
        pub stats: Arc<VideoStats>,
    }

    impl VideoCapture {
        pub fn start(
            _window: WindowHandle,
            _options: &VideoOptions,
            _caps: &OsCaps,
            _frames: Option<Sender<VideoFrame>>,
            _recycled: Receiver<VideoFrame>,
            _preview: Arc<PreviewSlot>,
        ) -> Result<Self> {
            bail!("window capture is not supported on this platform yet")
        }

        pub fn is_running(&self) -> bool {
            false
        }

        pub fn stop(self) -> Result<()> {
            Ok(())
        }
    }
}
