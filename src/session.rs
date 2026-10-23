//! One send session: NDI output, window video capture and optional audio
//! capture, plus monitoring of the target window. Used by the CLI and the GUI
//! controller.

use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::Sender;
use tracing::{info, warn};

use crate::{
    capture::{
        audio::{AudioCapture, AudioChunk, AudioLevels, AudioMode, AudioSource},
        video::{NO_FRAMES_HINT_AFTER, PreviewSlot, VideoCapture, VideoOptions},
    },
    ndi_out::{NdiInputs, NdiOutput},
    os_caps::OsCaps,
    windows_list::{self, WindowInfo, WindowState},
};

/// Bounded queues between the threads (CLAUDE.md: drop rather than block).
/// Two frames in flight to NDI; the pool holds the frames NDI hands back.
const VIDEO_QUEUE: usize = 2;
const RECYCLE_POOL: usize = 4;
/// 500 ms of 10 ms audio chunks.
const AUDIO_QUEUE: usize = 50;

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub window: WindowInfo,
    /// NDI source name without the machine prefix.
    pub source_name: String,
    /// NDI send groups, comma-separated; empty = NDI's own setting (D39).
    pub groups: String,
    pub video: VideoOptions,
    /// `None` sends video only.
    pub audio: Option<AudioSource>,
    pub audio_gain_db: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    WindowClosed,
    Minimized,
    Restored,
    /// No frames for a while although the window is not minimized.
    NoFrames,
    FramesResumed,
}

/// Rates and sizes since the previous [`Session::status`] call.
#[derive(Debug, Clone, Default)]
pub struct SessionStatus {
    pub source_size: (u32, u32),
    pub output_size: (u32, u32),
    pub capture_fps: f64,
    pub send_fps: f64,
    pub readback_ms: f64,
    pub paced_out: u64,
    pub backlogged: u64,
    pub receivers: u32,
}

pub struct Session {
    hwnd: isize,
    ndi: NdiOutput,
    video: VideoCapture,
    audio: Option<AudioCapture>,
    /// Kept to (re)start audio capture during the session.
    audio_tx: Sender<AudioChunk>,
    pub preview: Arc<PreviewSlot>,
    window_state: WindowState,
    last_frame: Instant,
    frames_missing: bool,
    last_captured: u64,
    status_at: Instant,
    status_captured: u64,
    status_sent: u64,
}

impl Session {
    /// Starts sending. If audio was requested but cannot be captured, the
    /// session still runs (video only) and the audio error is returned.
    pub fn start(config: &SessionConfig, caps: &OsCaps) -> Result<(Self, Option<anyhow::Error>)> {
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(VIDEO_QUEUE);
        let (recycle_tx, recycle_rx) = crossbeam_channel::bounded(RECYCLE_POOL);
        let (audio_tx, audio_rx) = crossbeam_channel::bounded(AUDIO_QUEUE);
        let ndi = NdiOutput::start(
            &config.source_name,
            &config.groups,
            config.audio_gain_db,
            NdiInputs {
                video: frame_rx,
                recycle: recycle_tx,
                audio: audio_rx,
            },
        )?;

        let (audio, audio_error) = match config.audio {
            None => (None, None),
            Some(source) => match AudioCapture::start(source, Some(audio_tx.clone())) {
                Ok(audio) => (Some(audio), None),
                Err(err) => {
                    warn!("no audio capture, sending video only: {err:#}");
                    (None, Some(err))
                }
            },
        };

        let preview = Arc::new(PreviewSlot::default());
        let video = VideoCapture::start(
            config.window.hwnd,
            &config.video,
            caps,
            Some(frame_tx),
            recycle_rx,
            preview.clone(),
        )?;
        info!(source = %ndi.source_name, "session started");

        let now = Instant::now();
        Ok((
            Self {
                hwnd: config.window.hwnd,
                ndi,
                video,
                audio,
                audio_tx,
                preview,
                window_state: WindowState::Normal,
                last_frame: now,
                frames_missing: false,
                last_captured: 0,
                status_at: now,
                status_captured: 0,
                status_sent: 0,
            },
            audio_error,
        ))
    }

    /// Full NDI name, e.g. `MACHINE (war3)`.
    pub fn source_name(&self) -> &str {
        &self.ndi.source_name
    }

    pub fn audio_mode(&self) -> Option<AudioMode> {
        self.audio.as_ref().map(|a| a.mode)
    }

    pub fn audio_levels(&self) -> Option<Arc<AudioLevels>> {
        self.audio.as_ref().map(|a| a.levels.clone())
    }

    /// Replaces the audio capture (e.g. switching to system audio after the
    /// user agreed). On error the session continues without audio.
    pub fn set_audio(&mut self, source: AudioSource) -> Result<()> {
        if let Some(old) = self.audio.take() {
            old.stop();
        }
        self.audio = Some(AudioCapture::start(source, Some(self.audio_tx.clone()))?);
        Ok(())
    }

    /// Checks the target window and frame flow; call a few times per second.
    pub fn poll(&mut self) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        let state = windows_list::window_state(self.hwnd);
        if state == WindowState::Gone || !self.video.is_running() {
            return vec![SessionEvent::WindowClosed];
        }
        if state != self.window_state {
            events.push(match state {
                WindowState::Minimized => SessionEvent::Minimized,
                _ => SessionEvent::Restored,
            });
            self.window_state = state;
        }

        let captured = self.video.stats.captured.load(Ordering::Relaxed);
        if captured != self.last_captured {
            self.last_captured = captured;
            self.last_frame = Instant::now();
            if self.frames_missing {
                self.frames_missing = false;
                events.push(SessionEvent::FramesResumed);
            }
        } else if state == WindowState::Normal
            && !self.frames_missing
            && self.last_frame.elapsed() >= NO_FRAMES_HINT_AFTER
        {
            self.frames_missing = true;
            events.push(SessionEvent::NoFrames);
        }
        events
    }

    pub fn is_minimized(&self) -> bool {
        self.window_state == WindowState::Minimized
    }

    pub fn frames_missing(&self) -> bool {
        self.frames_missing
    }

    /// Rates since the previous call.
    pub fn status(&mut self) -> SessionStatus {
        let stats = &self.video.stats;
        let secs = self.status_at.elapsed().as_secs_f64().max(1e-3);
        self.status_at = Instant::now();
        let captured = stats.captured.load(Ordering::Relaxed);
        let sent = self.ndi.stats.video_sent.load(Ordering::Relaxed);
        let frames = captured - self.status_captured;
        let readback_us = stats.readback_us.swap(0, Ordering::Relaxed);
        let status = SessionStatus {
            source_size: (
                stats.source_width.load(Ordering::Relaxed),
                stats.source_height.load(Ordering::Relaxed),
            ),
            output_size: (
                stats.width.load(Ordering::Relaxed),
                stats.height.load(Ordering::Relaxed),
            ),
            capture_fps: frames as f64 / secs,
            send_fps: (sent - self.status_sent) as f64 / secs,
            readback_ms: readback_us as f64 / frames.max(1) as f64 / 1000.0,
            paced_out: stats.paced_out.swap(0, Ordering::Relaxed),
            backlogged: stats.backlogged.swap(0, Ordering::Relaxed),
            receivers: self.ndi.stats.connections.load(Ordering::Relaxed),
        };
        self.status_captured = captured;
        self.status_sent = sent;
        status
    }

    /// Audio counters for logging: (chunks sent, dropped, padded ms, trimmed ms).
    pub fn audio_counters(&self) -> (u64, u64, u64, u64) {
        let sent = self.ndi.stats.audio_sent.load(Ordering::Relaxed);
        match &self.audio {
            Some(audio) => {
                let l = &audio.levels;
                (
                    sent,
                    l.dropped.load(Ordering::Relaxed),
                    l.padded_frames.load(Ordering::Relaxed) / 48,
                    l.trimmed_frames.load(Ordering::Relaxed) / 48,
                )
            }
            None => (sent, 0, 0, 0),
        }
    }

    pub fn stop(self) {
        let Self {
            ndi, video, audio, ..
        } = self;
        if let Err(err) = video.stop() {
            warn!("{err:#}");
        }
        if let Some(audio) = audio {
            audio.stop();
        }
        ndi.stop();
        info!("session stopped");
    }
}

/// How often callers should call [`Session::poll`].
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Frame rate of the preview capture before sending; enough for the UI.
/// Matches the preview refresh interval (50 ms).
pub const PREVIEW_FPS: u32 = 20;

/// Capture without sending: the live preview and audio levels shown as soon
/// as a window is selected (SPEC §3.1). Video is read back only while the UI
/// shows the preview; audio only feeds the meter.
pub struct Preview {
    hwnd: isize,
    video: VideoCapture,
    audio: Option<AudioCapture>,
    pub preview: Arc<PreviewSlot>,
}

impl Preview {
    pub fn start(
        window: &WindowInfo,
        video: &VideoOptions,
        audio: Option<AudioSource>,
        caps: &OsCaps,
    ) -> Result<Self> {
        let preview = Arc::new(PreviewSlot::default());
        let options = VideoOptions {
            fps_cap: PREVIEW_FPS.min(video.fps_cap),
            ..video.clone()
        };
        // Nothing is sent, so nothing comes back for reuse.
        let (_, recycled) = crossbeam_channel::bounded(1);
        let video =
            VideoCapture::start(window.hwnd, &options, caps, None, recycled, preview.clone())?;
        let audio = audio.and_then(|source| match AudioCapture::start(source, None) {
            Ok(audio) => Some(audio),
            Err(err) => {
                warn!("no audio preview: {err:#}");
                None
            }
        });
        info!(pid = window.pid, "preview started");
        Ok(Self {
            hwnd: window.hwnd,
            video,
            audio,
            preview,
        })
    }

    pub fn levels(&self) -> Option<Arc<AudioLevels>> {
        self.audio.as_ref().map(|a| a.levels.clone())
    }

    /// False once the window is gone.
    pub fn is_running(&self) -> bool {
        windows_list::window_state(self.hwnd) != WindowState::Gone && self.video.is_running()
    }

    pub fn stop(self) {
        if let Err(err) = self.video.stop() {
            warn!("{err:#}");
        }
        if let Some(audio) = self.audio {
            audio.stop();
        }
    }
}
