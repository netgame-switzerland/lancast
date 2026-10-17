//! NDI sender thread (SPEC §5.3). It owns the `grafton_ndi::Sender`, takes
//! video frames and audio chunks from the capture threads and returns released
//! video frames for reuse.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Receiver, Sender as ChannelSender, select};
use grafton_ndi::{AudioFrame, AudioLayout, NDI, Sender, SenderOptions, VideoFrame};
use tracing::{debug, info, warn};

use crate::capture::audio_wasapi::{AudioChunk, CHANNELS, CHUNK_FRAMES, SAMPLE_RATE};

/// NDI defines float 1.0 as +4 dBu (SMPTE reference) and digital full scale
/// as 10.0, i.e. 20 dB of headroom (SDK docs §21.2). NDI Studio Monitor
/// follows this; OBS + DistroAV passes the floats through unchanged
/// (1.0 = full scale). Sending WASAPI samples 1:1 is right for OBS and 20 dB
/// quiet in Studio Monitor; this gain makes Studio Monitor correct instead
/// (D17).
pub const AUDIO_GAIN_STUDIO_MONITOR_DB: f32 = 20.0;

/// How often the receiver count is refreshed.
const CONNECTION_POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub struct NdiStats {
    pub video_sent: AtomicU64,
    pub audio_sent: AtomicU64,
    pub connections: AtomicU32,
}

/// The channels the sender thread works from.
pub struct NdiInputs {
    pub video: Receiver<VideoFrame>,
    /// Video frames released by the SDK, back to the capture thread.
    pub recycle: ChannelSender<VideoFrame>,
    /// Audio chunks; a disconnected channel simply means "no audio".
    pub audio: Receiver<AudioChunk>,
}

/// The running sender thread. Stops when dropped.
pub struct NdiOutput {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    /// Full network name, e.g. `MACHINE (war3)`.
    pub source_name: String,
    pub stats: Arc<NdiStats>,
}

impl NdiOutput {
    /// Creates the NDI source `name` and starts sending from `inputs`.
    /// `audio_gain_db` is applied to all audio (see [`AUDIO_GAIN_STUDIO_MONITOR_DB`]).
    pub fn start(name: &str, audio_gain_db: f32, inputs: NdiInputs) -> Result<Self> {
        let audio_gain = 10f32.powf(audio_gain_db / 20.0);
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(NdiStats::default());
        // The sender is created on its own thread; it reports success or the
        // creation error back before streaming starts.
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<String>>(1);

        let thread = std::thread::Builder::new()
            .name("ndi-send".into())
            .spawn({
                let name = name.to_owned();
                let stop = stop.clone();
                let stats = stats.clone();
                move || match create_sender(&name) {
                    Ok((sender, source)) => {
                        let _ = ready_tx.send(Ok(source));
                        run(sender, inputs, audio_gain, &stop, &stats);
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
            })
            .context("failed to spawn NDI thread")?;

        let source_name = ready_rx
            .recv()
            .map_err(|_| anyhow!("NDI thread exited during start-up"))??;
        info!(source = %source_name, "NDI source created");
        Ok(Self {
            thread: Some(thread),
            stop,
            source_name,
            stats,
        })
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            warn!("NDI thread panicked");
        }
    }
}

impl Drop for NdiOutput {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// The sender keeps the NDI runtime alive for its own lifetime.
fn create_sender(name: &str) -> Result<(Sender, String)> {
    let ndi = NDI::new().context("failed to initialize the NDI runtime")?;
    // Video clocks the stream; audio (M2) is submitted from this same thread
    // and must not be clocked as well (D3).
    let options = SenderOptions::builder(name)
        .clock_video(true)
        .clock_audio(false)
        .build();
    let sender = Sender::new(&ndi, &options).context("failed to create the NDI sender")?;
    let source = sender
        .source()
        .map(|s| s.to_string())
        .unwrap_or_else(|_| name.to_owned());
    Ok((sender, source))
}

fn run(
    mut sender: Sender,
    inputs: NdiInputs,
    audio_gain: f32,
    stop: &AtomicBool,
    stats: &NdiStats,
) {
    let NdiInputs {
        video,
        recycle,
        mut audio,
    } = inputs;
    let mut last_poll = Instant::now() - CONNECTION_POLL;
    while !stop.load(Ordering::Relaxed) {
        select! {
            recv(video) -> msg => match msg {
                Ok(frame) => {
                    // With clock_video this blocks until the frame's slot.
                    if let Some(released) = sender.send_video_async(frame) {
                        // A full pool just means the capture side has enough.
                        let _ = recycle.try_send(released);
                    }
                    stats.video_sent.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    debug!("video channel closed");
                    break;
                }
            },
            recv(audio) -> msg => match msg {
                Ok(chunk) => send_audio(&sender, chunk, audio_gain, stats),
                Err(_) => {
                    debug!("audio channel closed; continuing without audio");
                    audio = crossbeam_channel::never();
                }
            },
            default(Duration::from_millis(100)) => {}
        }
        // Audio that queued up while a clocked video send was blocking.
        while let Ok(chunk) = audio.try_recv() {
            send_audio(&sender, chunk, audio_gain, stats);
        }

        if last_poll.elapsed() >= CONNECTION_POLL {
            last_poll = Instant::now();
            match sender.connection_count(Duration::ZERO) {
                Ok(count) => {
                    let previous = stats.connections.swap(count, Ordering::Relaxed);
                    if previous != count {
                        info!(receivers = count, "NDI receivers changed");
                    }
                }
                Err(err) => warn!(%err, "failed to query NDI connections"),
            }
        }
    }
    // Wait until the SDK has released the last async frame before dropping it.
    sender.finish_video_async();
}

/// Sends one 10 ms chunk. NDI wants planar float; the frame builder converts
/// from WASAPI's interleaved layout.
fn send_audio(sender: &Sender, mut chunk: AudioChunk, gain: f32, stats: &NdiStats) {
    if gain != 1.0 {
        chunk.interleaved.iter_mut().for_each(|s| *s *= gain);
    }
    let frame = AudioFrame::builder()
        .sample_rate(SAMPLE_RATE as i32)
        .channels(CHANNELS as i32)
        .samples(CHUNK_FRAMES as i32)
        .layout(AudioLayout::Interleaved)
        .data(chunk.interleaved)
        .build();
    match frame {
        Ok(frame) => {
            sender.send_audio(&frame);
            stats.audio_sent.fetch_add(1, Ordering::Relaxed);
        }
        Err(err) => warn!(%err, "invalid audio frame"),
    }
}
