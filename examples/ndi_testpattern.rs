//! M0 spike: send color bars and a sine tone as an NDI source.
//!
//! The bottom strip flashes white for 100 ms once per second while the tone
//! plays only during that window, so the pattern doubles as a lip-sync check.
//!
//! Usage: `cargo run --release --example ndi_testpattern -- [source name] [seconds]`
//! (`seconds` = 0 or omitted runs until the process is killed).

use std::{
    f32::consts::TAU,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use grafton_ndi::{AudioFrame, NDI, PixelFormat, Sender, SenderOptions, VideoFrame};
use tracing::info;

const WIDTH: i32 = 1920;
const HEIGHT: i32 = 1080;
const FPS: i32 = 60;
const SAMPLE_RATE: i32 = 48_000;
const CHANNELS: i32 = 2;
/// 48 000 / 60: one audio frame of exactly one video frame's duration.
const SAMPLES_PER_FRAME: i32 = SAMPLE_RATE / FPS;
const TONE_HZ: f32 = 1_000.0;
/// -20 dBFS.
const TONE_AMPLITUDE: f32 = 0.1;
/// Frames per second during which the flash and the beep are active (100 ms).
const BEEP_FRAMES: u64 = 6;

/// 75 % color bars in BGRX byte order: white, yellow, cyan, green, magenta, red, blue.
const BARS: [[u8; 4]; 7] = [
    [191, 191, 191, 255],
    [0, 191, 191, 255],
    [191, 191, 0, 255],
    [0, 191, 0, 255],
    [191, 0, 191, 255],
    [0, 0, 191, 255],
    [191, 0, 0, 255],
];

fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    let mut args = std::env::args().skip(1);
    let name = args
        .next()
        .unwrap_or_else(|| "lancast test pattern".to_owned());
    let seconds: u64 = match args.next() {
        Some(s) => s
            .parse()
            .context("seconds must be a non-negative integer")?,
        None => 0,
    };

    info!(version = %NDI::version()?, "NDI runtime loaded");
    let ndi = NDI::new()?;
    // Video clocks the loop; audio is submitted from the same thread without its
    // own clocking, matching the planned single NDI sender thread (SPEC §4, §5.3).
    let options = SenderOptions::builder(&name)
        .clock_video(true)
        .clock_audio(false)
        .build();
    let mut sender = Sender::new(&ndi, &options)?;
    match sender.source() {
        Ok(source) => info!(source = %source, "NDI source created"),
        Err(err) => info!(%err, "NDI source created (name not yet available)"),
    }

    // Double buffering: one frame is owned by the SDK while we draw the other.
    let mut spare = Some(new_video_frame()?);
    let mut phase = 0.0f32;
    let mut audio = vec![0.0f32; (CHANNELS * SAMPLES_PER_FRAME) as usize];

    let start = Instant::now();
    let mut last_report = start;
    let mut frames_since_report = 0u32;

    for frame_index in 0u64.. {
        if seconds > 0 && start.elapsed() >= Duration::from_secs(seconds) {
            break;
        }
        let beep = frame_index % (FPS as u64) < BEEP_FRAMES;

        let mut frame = match spare.take() {
            Some(frame) => frame,
            None => new_video_frame()?,
        };
        draw_pattern(frame.data_mut(), frame_index, beep);
        spare = sender.send_video_async(frame);

        fill_tone(&mut audio, &mut phase, beep);
        let audio_frame = AudioFrame::builder()
            .sample_rate(SAMPLE_RATE)
            .channels(CHANNELS)
            .samples(SAMPLES_PER_FRAME)
            .data(audio.clone())
            .build()?;
        sender.send_audio(&audio_frame);

        frames_since_report += 1;
        let elapsed = last_report.elapsed();
        if elapsed >= Duration::from_secs(5) {
            let connections = sender.connection_count(Duration::ZERO)?;
            info!(
                fps = format!("{:.2}", frames_since_report as f64 / elapsed.as_secs_f64()),
                connections, "sending {WIDTH}x{HEIGHT}"
            );
            last_report = Instant::now();
            frames_since_report = 0;
        }
    }

    sender.finish_video_async();
    info!("done");
    Ok(())
}

fn new_video_frame() -> Result<VideoFrame> {
    Ok(VideoFrame::builder()
        .resolution(WIDTH, HEIGHT)
        .pixel_format(PixelFormat::BGRX)
        .frame_rate(FPS, 1)
        .build()?)
}

/// Draws the bars into the top 5/6 and a moving marker (or a white flash while
/// beeping) into the bottom strip.
fn draw_pattern(data: &mut [u8], frame_index: u64, beep: bool) {
    let width = WIDTH as usize;
    let row_bytes = width * 4;
    let strip_top = HEIGHT as usize * 5 / 6;
    let marker_width = width / 16;
    let marker_x = (frame_index as usize * 8) % (width - marker_width);

    for (y, row) in data.chunks_exact_mut(row_bytes).enumerate() {
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let color = if y < strip_top {
                BARS[x * BARS.len() / width]
            } else if beep {
                [255, 255, 255, 255]
            } else if (marker_x..marker_x + marker_width).contains(&x) {
                [191, 191, 191, 255]
            } else {
                [16, 16, 16, 255]
            };
            *pixel = color;
        }
    }
}

/// Fills planar stereo samples with the tone (or silence) and keeps the phase
/// continuous across frames.
fn fill_tone(planar: &mut [f32], phase: &mut f32, beep: bool) {
    let samples = SAMPLES_PER_FRAME as usize;
    let step = TAU * TONE_HZ / SAMPLE_RATE as f32;
    let (left, right) = planar.split_at_mut(samples);
    for (l, r) in left.iter_mut().zip(right.iter_mut()) {
        let value = if beep {
            phase.sin() * TONE_AMPLITUDE
        } else {
            0.0
        };
        *l = value;
        *r = value;
        *phase = (*phase + step) % TAU;
    }
}
