//! Test receiver: finds an NDI source and reports what arrives (resolution,
//! video fps, audio rate and level) once per second. Lets us verify senders
//! without NDI Studio Monitor.
//!
//! With `--av` it also measures lip sync against a flash + beep source
//! (`ndi_testpattern`, or `av_sync_source` captured by lancast): for every
//! flash it reports how much later (positive) or earlier the beep starts,
//! both by NDI timecode and by arrival time at this receiver.
//!
//! Usage: `cargo run --release --example ndi_probe -- [name substring] [seconds] [--av]`
//! (defaults: first source found, 10 s).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use grafton_ndi::{
    Finder, FinderOptions, LineStrideOrSize, NDI, Receiver, ReceiverOptions, VideoFrameRef,
};
use tracing::info;

/// Sampled luma above this counts as "flash".
const FLASH_LUMA: f32 = 128.0;
/// Audio sample magnitude above this counts as "beep".
const BEEP_LEVEL: f32 = 0.02;
/// A beep or flash only counts as an onset after this much quiet/dark time.
const REARM: Duration = Duration::from_millis(300);
/// Flash and beep onsets further apart than this are not paired.
const MAX_PAIR_GAP_MS: f64 = 400.0;

fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    let (flags, positional): (Vec<String>, Vec<String>) =
        std::env::args().skip(1).partition(|a| a.starts_with("--"));
    let measure_av = flags.iter().any(|f| f == "--av");
    let mut args = positional.into_iter();
    let filter = args.next().unwrap_or_default();
    let seconds: u64 = match args.next() {
        Some(s) => s.parse().context("seconds must be an integer")?,
        None => 10,
    };

    let ndi = NDI::new()?;
    let finder = Finder::new(
        &ndi,
        &FinderOptions::builder().show_local_sources(true).build(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let source = loop {
        let sources = finder.find_sources(Duration::from_secs(1))?;
        if let Some(source) = sources.iter().find(|s| s.to_string().contains(&filter)) {
            break source.clone();
        }
        if Instant::now() >= deadline {
            let names: Vec<String> = sources.iter().map(|s| s.to_string()).collect();
            bail!("no source matching {filter:?} found; visible: {names:?}");
        }
    };
    info!(%source, "connecting");

    let receiver = Receiver::new(
        &ndi,
        &ReceiverOptions::builder(source)
            .name("lancast probe")
            .build(),
    )?;
    let start = Instant::now();
    let mut window = Instant::now();
    let (mut video, mut audio_frames, mut audio_samples, mut peak) = (0u32, 0u32, 0u64, 0f32);
    let mut last_video = None;
    let mut last_audio = None;
    let mut sync = SyncMeter::default();

    while start.elapsed() < Duration::from_secs(seconds) {
        if let Some(frame) = receiver.video().try_capture_ref(Duration::from_millis(2))? {
            let arrival = Instant::now();
            video += 1;
            last_video = Some((frame.width(), frame.height(), frame.pixel_format()));
            if measure_av && let Some(luma) = center_luma(&frame) {
                sync.on_video(luma > FLASH_LUMA, frame.timecode(), arrival);
            }
        }
        while let Some(frame) = receiver.audio().try_capture_ref(Duration::ZERO)? {
            let arrival = Instant::now();
            audio_frames += 1;
            audio_samples += frame.num_samples() as u64;
            last_audio = Some((frame.sample_rate(), frame.num_channels()));
            if let Some(left) = frame.channel_data(0) {
                peak = left.iter().fold(peak, |p, s| p.max(s.abs()));
                if measure_av {
                    sync.on_audio(left, frame.sample_rate(), frame.timecode(), arrival);
                }
            }
        }

        if window.elapsed() >= Duration::from_secs(1) {
            let secs = window.elapsed().as_secs_f64();
            window = Instant::now();
            info!(
                video = ?last_video,
                video_fps = format!("{:.1}", video as f64 / secs),
                audio = ?last_audio,
                audio_frames,
                audio_rate = format!("{:.0}", audio_samples as f64 / secs),
                audio_peak_dbfs = format!("{:.1}", 20.0 * peak.max(1e-10).log10()),
                "received"
            );
            (video, audio_frames, audio_samples, peak) = (0, 0, 0, 0.0);
        }
    }
    if measure_av {
        sync.report();
    }
    Ok(())
}

/// BT.601 luma of a BGRA/BGRX pixel in the bottom strip (where
/// `ndi_testpattern` flashes; `av_sync_source` flashes everywhere).
fn center_luma(frame: &VideoFrameRef<'_>) -> Option<f32> {
    let data = frame.data()?;
    let LineStrideOrSize::LineStrideBytes(stride) = frame.line_stride_or_size();
    let (x, y) = (
        frame.width() as usize / 2,
        frame.height() as usize * 11 / 12,
    );
    let px = data.get(y * stride as usize + x * 4..)?.get(..3)?;
    Some(0.114 * f32::from(px[0]) + 0.587 * f32::from(px[1]) + 0.299 * f32::from(px[2]))
}

/// An onset in both clocks: NDI timecode (100 ns units) and local arrival.
#[derive(Clone, Copy)]
struct Onset {
    timecode: i64,
    arrival: Instant,
}

#[derive(Default)]
struct SyncMeter {
    video_on: bool,
    video_off_since: Option<Instant>,
    audio_quiet_since: Option<i64>,
    last_flash: Option<Onset>,
    last_beep: Option<Onset>,
    /// (timecode offset, arrival offset) in ms; positive = audio late.
    pairs: Vec<(f64, f64)>,
}

impl SyncMeter {
    fn on_video(&mut self, bright: bool, timecode: i64, arrival: Instant) {
        if bright && !self.video_on {
            let rearmed = self
                .video_off_since
                .is_none_or(|since| arrival.duration_since(since) >= REARM);
            if rearmed {
                self.last_flash = Some(Onset { timecode, arrival });
                self.try_pair();
            }
        }
        if !bright && self.video_on {
            self.video_off_since = Some(arrival);
        }
        self.video_on = bright;
    }

    fn on_audio(&mut self, samples: &[f32], sample_rate: i32, timecode: i64, arrival: Instant) {
        let rearm_ticks = REARM.as_nanos() as i64 / 100;
        let tick_per_sample = 10_000_000.0 / f64::from(sample_rate);
        let frame_ticks = (samples.len() as f64 * tick_per_sample) as i64;
        for (i, s) in samples.iter().enumerate() {
            let tc = timecode + (i as f64 * tick_per_sample) as i64;
            if s.abs() < BEEP_LEVEL {
                continue;
            }
            let quiet = self
                .audio_quiet_since
                .is_none_or(|since| tc - since >= rearm_ticks);
            if quiet {
                // The frame arrived when its last sample was due; shift back.
                let back = Duration::from_nanos(((frame_ticks - (tc - timecode)) * 100) as u64);
                self.last_beep = Some(Onset {
                    timecode: tc,
                    arrival: arrival.checked_sub(back).unwrap_or(arrival),
                });
                self.try_pair();
            }
            self.audio_quiet_since = Some(tc);
        }
    }

    fn try_pair(&mut self) {
        let (Some(flash), Some(beep)) = (self.last_flash, self.last_beep) else {
            return;
        };
        let by_timecode = (beep.timecode - flash.timecode) as f64 / 10_000.0;
        if by_timecode.abs() > MAX_PAIR_GAP_MS {
            return;
        }
        let by_arrival = if beep.arrival >= flash.arrival {
            beep.arrival.duration_since(flash.arrival).as_secs_f64() * 1000.0
        } else {
            -(flash.arrival.duration_since(beep.arrival).as_secs_f64() * 1000.0)
        };
        info!(
            timecode_ms = format!("{by_timecode:+.1}"),
            arrival_ms = format!("{by_arrival:+.1}"),
            "A/V offset (positive = audio late)"
        );
        self.pairs.push((by_timecode, by_arrival));
        self.last_flash = None;
        self.last_beep = None;
    }

    fn report(&self) {
        if self.pairs.is_empty() {
            info!("no flash/beep pairs detected");
            return;
        }
        let stats = |values: Vec<f64>| {
            let n = values.len() as f64;
            let mean = values.iter().sum::<f64>() / n;
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            format!("mean {mean:+.1} ms, min {min:+.1}, max {max:+.1}")
        };
        info!(
            pairs = self.pairs.len(),
            by_timecode = stats(self.pairs.iter().map(|p| p.0).collect()),
            by_arrival = stats(self.pairs.iter().map(|p| p.1).collect()),
            "A/V sync summary (positive = audio late)"
        );
    }
}
