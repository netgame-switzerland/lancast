//! Test receiver: finds an NDI source and reports what arrives (resolution,
//! video fps, audio rate and level) once per second. Lets us verify senders
//! without NDI Studio Monitor.
//!
//! Usage: `cargo run --release --example ndi_probe -- [name substring] [seconds]`
//! (defaults: first source found, 10 s).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use grafton_ndi::{Finder, FinderOptions, NDI, Receiver, ReceiverOptions};
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    let mut args = std::env::args().skip(1);
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

    while start.elapsed() < Duration::from_secs(seconds) {
        if let Some(frame) = receiver.video().try_capture_ref(Duration::from_millis(5))? {
            video += 1;
            last_video = Some((frame.width(), frame.height(), frame.pixel_format()));
        }
        while let Some(frame) = receiver.audio().try_capture_ref(Duration::ZERO)? {
            audio_frames += 1;
            audio_samples += frame.num_samples() as u64;
            last_audio = Some((frame.sample_rate(), frame.num_channels()));
            if let Some(samples) = frame.data() {
                peak = samples.iter().fold(peak, |p, s| p.max(s.abs()));
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
    Ok(())
}
