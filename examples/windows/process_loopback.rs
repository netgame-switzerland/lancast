//! M0 spike: record a process tree's audio via WASAPI process loopback into a
//! 48 kHz stereo float WAV file. Falls back to system loopback if process
//! loopback cannot be activated (or with `--system`).
//!
//! Usage: `cargo run --release --example process_loopback -- <PID | --system> [seconds] [out.wav]`
//! (defaults: 10 s, `loopback.wav`). Use `list_windows` to find the PID.

use std::{
    fs::File,
    io::{BufWriter, Write},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use tracing::{info, warn};
use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BYTES_PER_FRAME: usize = CHANNELS as usize * 4;
/// Wall-clock deficit after which missing audio is filled with silence. WASAPI
/// delivers no packets while the target renders nothing, but the NDI stream
/// needs continuous audio (SPEC §5.2).
const GAP_THRESHOLD: Duration = Duration::from_millis(50);

pub fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    let mut args = std::env::args().skip(1);
    let target = args
        .next()
        .context("usage: process_loopback <PID | --system> [seconds] [out.wav]")?;
    let seconds: u64 = match args.next() {
        Some(s) => s.parse().context("seconds must be an integer")?,
        None => 10,
    };
    let out_path = args.next().unwrap_or_else(|| "loopback.wav".to_owned());
    let pid = match target.as_str() {
        "--system" => None,
        pid => Some(pid.parse::<u32>().context("PID must be a decimal number")?),
    };

    wasapi::initialize_mta()
        .ok()
        .map_err(|e| anyhow!("CoInitializeEx failed: {e}"))?;

    // GetMixFormat/IsFormatSupported are not implemented for process loopback,
    // so request the format explicitly and let the engine convert (autoconvert).
    let format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        SAMPLE_RATE as usize,
        CHANNELS.into(),
        None,
    );
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };

    let activation = Instant::now();
    let mut client = match pid {
        Some(pid) => match AudioClient::new_application_loopback_client(pid, true) {
            Ok(client) => {
                info!(pid, "process loopback activated (include process tree)");
                client
            }
            Err(err) => {
                warn!(%err, "process loopback failed; falling back to system loopback (includes Discord etc.)");
                system_loopback_client()?
            }
        },
        None => system_loopback_client()?,
    };
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| anyhow!("IAudioClient::Initialize failed: {e}"))?;
    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    info!(
        elapsed_ms = activation.elapsed().as_millis(),
        "audio client ready"
    );

    let mut samples: Vec<f32> =
        Vec::with_capacity(SAMPLE_RATE as usize * CHANNELS as usize * seconds as usize);
    let mut packet = vec![0u8; SAMPLE_RATE as usize * BYTES_PER_FRAME]; // 1 s, ample for one packet
    let (mut packets, mut silent_packets, mut gap_frames, mut peak) = (0u64, 0u64, 0u64, 0f32);

    client.start_stream()?;
    let start = Instant::now();
    let duration = Duration::from_secs(seconds);
    while start.elapsed() < duration {
        // A timeout is normal: the target may simply be silent.
        let _ = event.wait_for_event(20);

        while capture.get_next_packet_size()?.unwrap_or(0) > 0 {
            let (frames, info) = capture.read_from_device(&mut packet)?;
            let bytes = &packet[..frames as usize * BYTES_PER_FRAME];
            packets += 1;
            if info.flags.silent {
                // The buffer content is undefined when AUDCLNT_BUFFERFLAGS_SILENT is set.
                silent_packets += 1;
                samples.extend(std::iter::repeat_n(
                    0.0,
                    frames as usize * CHANNELS as usize,
                ));
            } else {
                for chunk in bytes.as_chunks::<4>().0 {
                    let sample = f32::from_le_bytes(*chunk);
                    peak = peak.max(sample.abs());
                    samples.push(sample);
                }
            }
        }

        // Pad with silence when the stream lags the wall clock (no packets
        // while the target renders nothing).
        let expected_frames = (start.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize;
        let have_frames = samples.len() / CHANNELS as usize;
        let threshold = (GAP_THRESHOLD.as_secs_f64() * SAMPLE_RATE as f64) as usize;
        if expected_frames > have_frames + threshold {
            let missing = expected_frames - have_frames;
            gap_frames += missing as u64;
            samples.extend(std::iter::repeat_n(0.0, missing * CHANNELS as usize));
        }
    }
    client.stop_stream()?;

    let frames = samples.len() / CHANNELS as usize;
    info!(
        packets,
        silent_packets,
        gap_fill_ms = gap_frames * 1000 / SAMPLE_RATE as u64,
        peak_dbfs = format!("{:.1}", 20.0 * peak.max(1e-10).log10()),
        recorded_s = format!("{:.2}", frames as f64 / SAMPLE_RATE as f64),
        "capture finished"
    );
    write_wav_f32(&out_path, &samples)?;
    info!(path = %out_path, "WAV written");
    Ok(())
}

/// Loopback on the default render endpoint: everything the system plays.
fn system_loopback_client() -> Result<AudioClient> {
    let device = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
    info!(device = %device.get_friendlyname()?, "using system loopback");
    Ok(device.get_iaudioclient()?)
}

/// Writes interleaved samples as a WAVE_FORMAT_IEEE_FLOAT file.
fn write_wav_f32(path: &str, samples: &[f32]) -> Result<()> {
    let data_len = u32::try_from(samples.len() * 4).context("recording too long for WAV")?;
    let block_align = CHANNELS * 4;
    let mut w =
        BufWriter::new(File::create(path).with_context(|| format!("cannot create {path}"))?);
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
    w.write_all(&CHANNELS.to_le_bytes())?;
    w.write_all(&SAMPLE_RATE.to_le_bytes())?;
    w.write_all(&(SAMPLE_RATE * u32::from(block_align)).to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&32u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        w.write_all(&sample.to_le_bytes())?;
    }
    w.flush()?;
    Ok(())
}
