//! Game audio via WASAPI process loopback, with fallback to system loopback
//! (SPEC §5.2, D7). Produces a continuous stream of 10 ms interleaved stereo
//! f32 chunks at 48 kHz, plus peak/RMS levels for the meter.
//!
//! All COM/WASAPI calls go through the `wasapi` crate; this module needs no
//! `unsafe` of its own.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Sender, TrySendError};
use tracing::{debug, info, warn};
use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode,
    WaveFormat,
};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// 10 ms per chunk; about half a 60 fps video frame, as the NDI docs
/// recommend (D3).
pub const CHUNK_FRAMES: usize = SAMPLE_RATE as usize / 100;
const BYTES_PER_FRAME: usize = CHANNELS * 4;
/// Pad with silence when capture lags the wall clock by more than this
/// (WASAPI may deliver nothing while the target is silent).
const MAX_DEFICIT: Duration = Duration::from_millis(50);
/// Drop samples when capture runs ahead of the wall clock by more than this
/// (bursts after a stall, or device clock drift), bounding latency.
const MAX_SURPLUS: Duration = Duration::from_millis(100);
/// Levels below this count as silence for the "audio active" indicator.
pub const ACTIVITY_THRESHOLD_DB: f32 = -60.0;
pub const SILENCE_DB: f32 = -100.0;

/// When set, process loopback fails as on systems without it, so the
/// fallback and warning paths (Windows 10, D13) can be tested on Windows 11.
pub const SIMULATE_NO_PROCESS_LOOPBACK: &str = "LANCAST_SIMULATE_NO_PROCESS_LOOPBACK";

/// One 10 ms block of interleaved stereo samples.
pub struct AudioChunk {
    pub interleaved: Vec<f32>,
}

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    /// The process tree of `pid`. With `fallback_to_system`, a failed
    /// activation switches to system audio with only a log warning (CLI only;
    /// the GUI must ask first, SPEC §3.1).
    Process { pid: u32, fallback_to_system: bool },
    /// Everything the default output device plays.
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    /// Only the target process tree is captured.
    ProcessLoopback { pid: u32 },
    /// Everything the default output device plays (includes Discord etc.).
    SystemLoopback,
}

impl fmt::Display for AudioMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProcessLoopback { pid } => write!(f, "game only (process loopback, PID {pid})"),
            Self::SystemLoopback => write!(f, "system audio – includes Discord etc."),
        }
    }
}

/// Per-channel levels for the meter: the maximum sample peak since the last
/// [`AudioLevels::take_peak_db`] (so short peaks between UI frames are not
/// lost) and the RMS of the latest chunk. The UI applies the ballistics
/// (SPEC §5.2).
#[derive(Debug)]
pub struct AudioLevels {
    /// Linear peak amplitude as `f32` bits. Non-negative floats order like
    /// their bit patterns, so `fetch_max` on the bits is a float maximum.
    peak: [AtomicU32; CHANNELS],
    rms_db: [AtomicU32; CHANNELS],
    /// Chunks produced so far; lets readers detect a stalled capture.
    pub chunks: AtomicU64,
    /// Chunks dropped because the NDI thread was behind.
    pub dropped: AtomicU64,
    /// Silence inserted for gaps, in sample frames.
    pub padded_frames: AtomicU64,
    /// Samples discarded to bound latency, in sample frames.
    pub trimmed_frames: AtomicU64,
}

impl Default for AudioLevels {
    fn default() -> Self {
        let silent = || AtomicU32::new(SILENCE_DB.to_bits());
        Self {
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            rms_db: [silent(), silent()],
            chunks: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            padded_frames: AtomicU64::new(0),
            trimmed_frames: AtomicU64::new(0),
        }
    }
}

impl AudioLevels {
    /// Maximum peak since the previous call, in dBFS; resets it.
    pub fn take_peak_db(&self, channel: usize) -> f32 {
        to_db(f32::from_bits(
            self.peak[channel].swap(0, Ordering::Relaxed),
        ))
    }

    pub fn rms_db(&self, channel: usize) -> f32 {
        f32::from_bits(self.rms_db[channel].load(Ordering::Relaxed))
    }

    fn store(&self, interleaved: &[f32]) {
        for channel in 0..CHANNELS {
            let (mut peak, mut sum) = (0f32, 0f32);
            for &s in interleaved.iter().skip(channel).step_by(CHANNELS) {
                peak = peak.max(s.abs());
                sum += s * s;
            }
            let rms = (sum / (interleaved.len() / CHANNELS) as f32).sqrt();
            // NaN would break the bit-order trick; treat it as silence.
            let peak = if peak.is_nan() { 0.0 } else { peak };
            self.peak[channel].fetch_max(peak.to_bits(), Ordering::Relaxed);
            self.rms_db[channel].store(to_db(rms).to_bits(), Ordering::Relaxed);
        }
    }
}

fn to_db(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        SILENCE_DB
    } else {
        (20.0 * amplitude.log10()).max(SILENCE_DB)
    }
}

/// A running audio capture thread. Stops when dropped.
pub struct AudioCapture {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    pub mode: AudioMode,
    pub levels: Arc<AudioLevels>,
}

impl AudioCapture {
    /// Starts capturing `source` on a new thread. Without `chunks` only the
    /// levels are measured (preview before sending).
    pub fn start(source: AudioSource, chunks: Option<Sender<AudioChunk>>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let levels = Arc::new(AudioLevels::default());
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<AudioMode>>(1);

        let thread = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn({
                let stop = stop.clone();
                let levels = levels.clone();
                move || {
                    // The wasapi crate does not initialize COM itself.
                    if let Err(err) = wasapi::initialize_mta().ok() {
                        let _ = ready_tx.send(Err(anyhow!("CoInitializeEx failed: {err}")));
                        return;
                    }
                    let stream = match open_stream(source) {
                        Ok(stream) => stream,
                        Err(err) => {
                            let _ = ready_tx.send(Err(err));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(stream.mode));
                    if let Err(err) = run(&stream, &chunks, &levels, &stop) {
                        warn!("audio capture ended: {err:#}");
                    }
                    if let Err(err) = stream.client.stop_stream() {
                        debug!(%err, "failed to stop the audio stream");
                    }
                }
            })
            .context("failed to spawn the audio thread")?;

        let mode = ready_rx
            .recv()
            .map_err(|_| anyhow!("audio thread exited during start-up"))??;
        info!(%mode, "audio capture started");
        Ok(Self {
            thread: Some(thread),
            stop,
            mode,
            levels,
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
            warn!("audio thread panicked");
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

struct Stream {
    client: AudioClient,
    capture: AudioCaptureClient,
    event: Handle,
    mode: AudioMode,
}

/// Checks whether process loopback can be activated for `pid` (the GUI's
/// pre-flight check, SPEC §3.1). Opens and releases a client on a short-lived
/// MTA thread; takes a few milliseconds.
pub fn check_process_loopback(pid: u32) -> Result<()> {
    std::thread::Builder::new()
        .name("audio-check".into())
        .spawn(move || {
            wasapi::initialize_mta()
                .ok()
                .map_err(|e| anyhow!("CoInitializeEx failed: {e}"))?;
            let stream = open_client(Some(pid))?;
            let _ = stream.client.stop_stream();
            Ok(())
        })
        .context("failed to spawn the audio check thread")?
        .join()
        .map_err(|_| anyhow!("audio check thread panicked"))?
}

fn open_stream(source: AudioSource) -> Result<Stream> {
    match source {
        AudioSource::Process {
            pid,
            fallback_to_system,
        } => match open_client(Some(pid)) {
            Ok(stream) => Ok(stream),
            Err(err) if fallback_to_system => {
                warn!(
                    "process loopback for PID {pid} failed ({err:#}); \
                     falling back to system audio, which includes Discord etc."
                );
                open_client(None).context("system loopback failed as well")
            }
            Err(err) => Err(err.context(format!("process loopback for PID {pid} failed"))),
        },
        AudioSource::System => open_client(None),
    }
}

/// Process loopback for `pid` (including its child processes), or loopback
/// of the default render device for `None`.
fn open_client(pid: Option<u32>) -> Result<Stream> {
    if pid.is_some() && std::env::var_os(SIMULATE_NO_PROCESS_LOOPBACK).is_some() {
        return Err(anyhow!(
            "process loopback disabled by {SIMULATE_NO_PROCESS_LOOPBACK} (test mode)"
        ));
    }
    let (mut client, mode) = match pid {
        Some(pid) => (
            AudioClient::new_application_loopback_client(pid, true)
                .map_err(|e| anyhow!("activation failed: {e}"))?,
            AudioMode::ProcessLoopback { pid },
        ),
        None => {
            let device = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
            if let Ok(name) = device.get_friendlyname() {
                info!(device = %name, "system loopback device");
            }
            (device.get_iaudioclient()?, AudioMode::SystemLoopback)
        }
    };
    // GetMixFormat is not implemented for process loopback, so request the
    // format explicitly and let the engine convert (D7).
    let format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        SAMPLE_RATE as usize,
        CHANNELS,
        None,
    );
    let stream_mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };
    client
        .initialize_client(&format, &Direction::Capture, &stream_mode)
        .map_err(|e| anyhow!("IAudioClient::Initialize failed: {e}"))?;
    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    client.start_stream()?;
    Ok(Stream {
        client,
        capture,
        event,
        mode,
    })
}

fn run(
    stream: &Stream,
    chunks: &Option<Sender<AudioChunk>>,
    levels: &AudioLevels,
    stop: &AtomicBool,
) -> Result<()> {
    // Room for one second, far more than one WASAPI packet.
    let mut packet = vec![0u8; SAMPLE_RATE as usize * BYTES_PER_FRAME];
    let mut pending: Vec<f32> = Vec::with_capacity(CHUNK_FRAMES * CHANNELS * 4);
    let start = Instant::now();
    // Sample frames emitted plus pending, compared against the wall clock.
    let mut produced: u64 = 0;
    let deficit_limit = frames_for(MAX_DEFICIT);
    let surplus_limit = frames_for(MAX_SURPLUS);

    while !stop.load(Ordering::Relaxed) {
        // A timeout is normal: the target may simply be silent.
        let _ = stream.event.wait_for_event(20);

        while stream.capture.get_next_packet_size()?.unwrap_or(0) > 0 {
            let (frames, info) = stream.capture.read_from_device(&mut packet)?;
            let samples = frames as usize * CHANNELS;
            if info.flags.silent {
                // The buffer content is undefined for silent packets.
                pending.resize(pending.len() + samples, 0.0);
            } else {
                let bytes = &packet[..frames as usize * BYTES_PER_FRAME];
                pending.extend(
                    bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b)),
                );
            }
            produced += u64::from(frames);
        }

        let expected = (start.elapsed().as_secs_f64() * f64::from(SAMPLE_RATE)) as u64;
        if expected > produced + deficit_limit {
            let missing = expected - produced;
            pending.resize(pending.len() + missing as usize * CHANNELS, 0.0);
            produced += missing;
            levels.padded_frames.fetch_add(missing, Ordering::Relaxed);
        } else if produced > expected + surplus_limit {
            let excess = ((produced - expected) as usize).min(pending.len() / CHANNELS);
            pending.drain(..excess * CHANNELS);
            produced -= excess as u64;
            levels
                .trimmed_frames
                .fetch_add(excess as u64, Ordering::Relaxed);
        }

        while pending.len() >= CHUNK_FRAMES * CHANNELS {
            let interleaved: Vec<f32> = pending.drain(..CHUNK_FRAMES * CHANNELS).collect();
            levels.store(&interleaved);
            levels.chunks.fetch_add(1, Ordering::Relaxed);
            let Some(chunks) = chunks else { continue };
            match chunks.try_send(AudioChunk { interleaved }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    levels.dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => return Ok(()),
            }
        }
    }
    Ok(())
}

fn frames_for(duration: Duration) -> u64 {
    (duration.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_of_full_scale_square_and_silence() {
        let levels = AudioLevels::default();
        let mut block = vec![0.0f32; CHUNK_FRAMES * CHANNELS];
        for (i, s) in block.iter_mut().enumerate() {
            if i % CHANNELS == 0 {
                *s = if i % 4 == 0 { 1.0 } else { -1.0 };
            }
        }
        levels.store(&block);
        assert!(levels.rms_db(0).abs() < 1e-3);
        assert_eq!(levels.rms_db(1), SILENCE_DB);
        // A later, quieter chunk does not lower the held peak before it is taken.
        levels.store(&vec![0.0; CHUNK_FRAMES * CHANNELS]);
        assert!(levels.take_peak_db(0).abs() < 1e-3);
        assert_eq!(levels.take_peak_db(0), SILENCE_DB);
        assert_eq!(levels.take_peak_db(1), SILENCE_DB);
    }
}
