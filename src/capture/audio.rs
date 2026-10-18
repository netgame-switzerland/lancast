//! Game audio capture (SPEC §5.2): the platform-independent part. Every
//! backend produces a continuous stream of 10 ms interleaved stereo f32
//! chunks at 48 kHz, plus peak/RMS levels for the meter.
//!
//! Backends: WASAPI process loopback on Windows (`audio_wasapi`); other
//! platforms have a stub that reports audio capture as unavailable (D32).

use std::{
    fmt,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

#[cfg(windows)]
pub use super::audio_wasapi::{AudioCapture, check_process_loopback};
#[cfg(not(windows))]
pub use unsupported::{AudioCapture, check_process_loopback};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// 10 ms per chunk; about half a 60 fps video frame, as the NDI docs
/// recommend (D3).
pub const CHUNK_FRAMES: usize = SAMPLE_RATE as usize / 100;
/// Levels below this count as silence for the "audio active" indicator.
pub const ACTIVITY_THRESHOLD_DB: f32 = -60.0;
pub const SILENCE_DB: f32 = -100.0;

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

    /// Records the levels of one chunk; called by the backends.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn store(&self, interleaved: &[f32]) {
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

/// Stub for platforms without an audio backend yet (D32): every capture
/// fails, so sessions run video only.
#[cfg(not(windows))]
mod unsupported {
    use std::sync::Arc;

    use anyhow::{Result, bail};
    use crossbeam_channel::Sender;

    use super::{AudioChunk, AudioLevels, AudioMode, AudioSource};

    pub struct AudioCapture {
        pub mode: AudioMode,
        pub levels: Arc<AudioLevels>,
    }

    impl AudioCapture {
        pub fn start(_source: AudioSource, _chunks: Option<Sender<AudioChunk>>) -> Result<Self> {
            bail!("audio capture is not supported on this platform yet")
        }

        pub fn stop(self) {}
    }

    pub fn check_process_loopback(_pid: u32) -> Result<()> {
        bail!("game audio capture is not supported on this platform yet")
    }
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
