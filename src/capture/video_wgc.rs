//! Window capture via Windows Graphics Capture (`windows-capture`): the
//! Windows backend of [`super::video`] (SPEC §5.1).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use anyhow::{Result, anyhow, bail};
use crossbeam_channel::{Receiver, Sender};
use grafton_ndi::VideoFrame;
use tracing::{info, warn};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiHandler},
    frame::{Frame, FrameBuffer},
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

use super::{
    pacer::Pacer,
    video::{FrameSink, PreviewSlot, Readback, SinkError, VideoOptions, VideoStats},
};
use crate::{os_caps::OsCaps, windows_list::WindowHandle};

/// A running capture session. Stops when dropped.
pub struct VideoCapture {
    control: Option<CaptureControl<Handler, SinkError>>,
    closed: Arc<AtomicBool>,
    pub stats: Arc<VideoStats>,
}

impl VideoCapture {
    /// Starts capturing `hwnd`. Options the OS cannot honour fall back to the
    /// OS default with a warning (SPEC §5.0).
    pub fn start(
        hwnd: WindowHandle,
        options: &VideoOptions,
        caps: &OsCaps,
        frames: Option<Sender<VideoFrame>>,
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

        let sink = FrameSink::new(pacer, options, frames, recycled, preview);
        let stats = sink.stats();
        let closed = Arc::new(AtomicBool::new(false));
        let config = HandlerConfig {
            sink,
            closed: closed.clone(),
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
    sink: FrameSink,
    closed: Arc<AtomicBool>,
}

struct Handler {
    config: HandlerConfig,
}

impl Readback for FrameBuffer<'_> {
    fn bgra(&mut self) -> (&[u8], usize) {
        let pitch = self.row_pitch() as usize;
        (self.as_raw_buffer(), pitch)
    }
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = HandlerConfig;
    type Error = SinkError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self { config: ctx.flags })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let now = Instant::now();
        let sink = &mut self.config.sink;
        if !sink.wants_frame(now) {
            return Ok(());
        }
        let source = (frame.width(), frame.height());
        if !sink.deliver(source, now, || Ok(frame.buffer()?))? {
            control.stop();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        info!("capture item closed (target window destroyed)");
        self.config.closed.store(true, Ordering::Relaxed);
        Ok(())
    }
}
