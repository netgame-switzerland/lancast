//! lancast – send a game window as an NDI® source.
//!
//! M1/M2: command-line front end; the egui UI follows in M3.
//!
//! Usage: `lancast <0xHWND | PID> [--name NAME] [--fps N] [--max-size WxH] [--border] [--cursor]
//! [--system-audio | --no-audio] [--audio-gain-db DB]`
//! `--audio-gain-db 20` matches NDI Studio Monitor's level convention (D17).
//! Without arguments, prints the capturable windows.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use lancast::{
    capture::{
        audio_wasapi::{AudioCapture, AudioMode},
        video::{NO_FRAMES_HINT_AFTER, VideoCapture, VideoOptions},
    },
    logging,
    ndi_out::{NdiInputs, NdiOutput},
    os_caps::{self, OsCaps},
    windows_list::{self, WindowInfo, WindowState},
};
use tracing::{debug, error, info, warn};

/// Bounded queues between the threads (CLAUDE.md: drop rather than block).
/// Two frames in flight to NDI; the pool holds the frames NDI hands back.
const VIDEO_QUEUE: usize = 2;
const RECYCLE_POOL: usize = 4;
/// 500 ms of 10 ms audio chunks.
const AUDIO_QUEUE: usize = 50;
const STATUS_INTERVAL: Duration = Duration::from_secs(5);

struct Args {
    target: String,
    name: Option<String>,
    video: VideoOptions,
    audio: AudioChoice,
    audio_gain_db: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioChoice {
    /// Process loopback of the window's process tree, falling back to system audio.
    Game,
    /// Force system loopback (everything the default device plays).
    System,
    None,
}

fn main() {
    let log_path = logging::init();
    if let Err(err) = run(log_path) {
        error!("{err:#}");
        std::process::exit(1);
    }
}

fn run(log_path: Option<std::path::PathBuf>) -> Result<()> {
    info!(version = env!("CARGO_PKG_VERSION"), log = ?log_path, "lancast starting");
    let Some(args) = parse_args()? else {
        print_windows()?;
        return Ok(());
    };

    let caps = OsCaps::detect();
    info!("{caps}");
    if caps.build < os_caps::MIN_BUILD {
        warn!(
            "Windows build {} is older than the supported minimum {} (Windows 10 1903)",
            caps.build,
            os_caps::MIN_BUILD
        );
    }

    let window = windows_list::resolve_target(&args.target)?;
    let name = args.name.unwrap_or_else(|| default_source_name(&window));
    info!(
        hwnd = format!("0x{:x}", window.hwnd),
        pid = window.pid,
        process = window.process_name.as_deref().unwrap_or("?"),
        "target window"
    );
    // Window titles can contain private text (browser tabs, documents), and
    // users may share their log file, so titles are logged at debug only.
    debug!(title = %window.title, "target window title");

    let (frame_tx, frame_rx) = crossbeam_channel::bounded(VIDEO_QUEUE);
    let (recycle_tx, recycle_rx) = crossbeam_channel::bounded(RECYCLE_POOL);
    let (audio_tx, audio_rx) = crossbeam_channel::bounded(AUDIO_QUEUE);
    let ndi = NdiOutput::start(
        &name,
        args.audio_gain_db,
        NdiInputs {
            video: frame_rx,
            recycle: recycle_tx,
            audio: audio_rx,
        },
    )?;
    if args.audio == AudioChoice::Game && !caps.process_loopback_documented() {
        info!(
            "process loopback is documented from build {}; trying anyway",
            os_caps::PROCESS_LOOPBACK_BUILD
        );
    }
    let audio = match args.audio {
        AudioChoice::None => None,
        choice => {
            let pid = (choice == AudioChoice::Game).then_some(window.pid);
            match AudioCapture::start(pid, audio_tx) {
                Ok(audio) => {
                    if audio.mode == AudioMode::SystemLoopback && choice == AudioChoice::Game {
                        warn!("sending SYSTEM audio: other apps (Discord etc.) are audible too");
                    }
                    Some(audio)
                }
                Err(err) => {
                    warn!("no audio capture possible, sending video only: {err:#}");
                    None
                }
            }
        }
    };
    let capture = VideoCapture::start(window.hwnd, &args.video, &caps, frame_tx, recycle_rx)?;

    let interrupted = Arc::new(AtomicBool::new(false));
    ctrlc::set_handler({
        let interrupted = interrupted.clone();
        move || interrupted.store(true, Ordering::Relaxed)
    })
    .context("failed to install the Ctrl-C handler")?;
    info!(source = %ndi.source_name, "sending; press Ctrl-C to stop");

    let reason = supervise(&window, &capture, audio.as_ref(), &ndi, &interrupted);
    info!("stopping: {reason}");
    capture.stop()?;
    if let Some(audio) = audio {
        audio.stop();
    }
    ndi.stop();
    info!("stopped");
    Ok(())
}

/// Watches the session until it should end; returns the reason.
fn supervise(
    window: &WindowInfo,
    capture: &VideoCapture,
    audio: Option<&AudioCapture>,
    ndi: &NdiOutput,
    interrupted: &AtomicBool,
) -> &'static str {
    let mut state = WindowState::Normal;
    let mut last_status = Instant::now();
    let mut last_frame = Instant::now();
    let mut hinted = false;
    let (mut captured, mut sent) = (0u64, 0u64);

    loop {
        std::thread::sleep(Duration::from_millis(250));
        if interrupted.load(Ordering::Relaxed) {
            return "interrupted";
        }
        let current = windows_list::window_state(window.hwnd);
        if current == WindowState::Gone || !capture.is_running() {
            warn!("the target window was closed");
            return "target window closed";
        }
        if current != state {
            match current {
                WindowState::Minimized => {
                    info!("target window minimized; the last frame stays on air")
                }
                _ => info!("target window restored"),
            }
            state = current;
        }

        let stats = &capture.stats;
        let now_captured = stats.captured.load(Ordering::Relaxed);
        if now_captured != captured {
            last_frame = Instant::now();
            hinted = false;
        } else if state == WindowState::Normal
            && !hinted
            && last_frame.elapsed() >= NO_FRAMES_HINT_AFTER
        {
            warn!(
                "no frames for {}s: static content, or a game in exclusive fullscreen. \
                 Please use borderless windowed mode.",
                NO_FRAMES_HINT_AFTER.as_secs()
            );
            hinted = true;
        }

        if last_status.elapsed() >= STATUS_INTERVAL {
            let secs = last_status.elapsed().as_secs_f64();
            last_status = Instant::now();
            let now_sent = ndi.stats.video_sent.load(Ordering::Relaxed);
            let frames = now_captured - captured;
            let readback_us = stats.readback_us.swap(0, Ordering::Relaxed);
            info!(
                source = format!(
                    "{}x{}",
                    stats.source_width.load(Ordering::Relaxed),
                    stats.source_height.load(Ordering::Relaxed)
                ),
                output = format!(
                    "{}x{}",
                    stats.width.load(Ordering::Relaxed),
                    stats.height.load(Ordering::Relaxed)
                ),
                capture_fps = format!("{:.1}", frames as f64 / secs),
                send_fps = format!("{:.1}", (now_sent - sent) as f64 / secs),
                readback_ms = format!("{:.2}", readback_us as f64 / frames.max(1) as f64 / 1000.0),
                paced_out = stats.paced_out.swap(0, Ordering::Relaxed),
                backlogged = stats.backlogged.swap(0, Ordering::Relaxed),
                receivers = ndi.stats.connections.load(Ordering::Relaxed),
                "status"
            );
            if let Some(audio) = audio {
                let levels = &audio.levels;
                info!(
                    mode = %audio.mode,
                    peak_db = format!("{:.1}/{:.1}", levels.peak_db(0), levels.peak_db(1)),
                    rms_db = format!("{:.1}/{:.1}", levels.rms_db(0), levels.rms_db(1)),
                    chunks_sent = ndi.stats.audio_sent.load(Ordering::Relaxed),
                    dropped = levels.dropped.load(Ordering::Relaxed),
                    padded_ms = levels.padded_frames.load(Ordering::Relaxed) / 48,
                    trimmed_ms = levels.trimmed_frames.load(Ordering::Relaxed) / 48,
                    "audio"
                );
            }
            captured = now_captured;
            sent = now_sent;
        }
    }
}

/// The process name without `.exe`. NDI prepends the machine name itself (D4).
fn default_source_name(window: &WindowInfo) -> String {
    window
        .process_name
        .as_deref()
        .map(|name| name.strip_suffix(".exe").unwrap_or(name).to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "lancast".to_owned())
}

const USAGE: &str = "usage: lancast <0xHWND | PID> [--name NAME] [--fps N] [--max-size WxH] [--border] [--cursor] [--system-audio | --no-audio] [--audio-gain-db DB]";

/// Returns `None` when no target was given.
fn parse_args() -> Result<Option<Args>> {
    let mut args = std::env::args().skip(1);
    let mut target = None;
    let mut name = None;
    let mut video = VideoOptions::default();
    let mut audio = AudioChoice::Game;
    let mut audio_gain_db = 0.0f32;

    while let Some(arg) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .with_context(|| format!("{flag} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--name" => name = Some(value("--name")?),
            "--fps" => {
                video.fps_cap = value("--fps")?.parse().context("--fps must be a number")?;
                if !(1..=240).contains(&video.fps_cap) {
                    bail!("--fps must be between 1 and 240");
                }
            }
            "--max-size" => {
                let size = value("--max-size")?;
                let (w, h) = size
                    .split_once('x')
                    .context("--max-size must look like 1920x1080")?;
                video.max_size = (
                    w.parse().context("invalid --max-size width")?,
                    h.parse().context("invalid --max-size height")?,
                );
                if video.max_size.0 < 2 || video.max_size.1 < 2 {
                    bail!("--max-size is too small");
                }
            }
            "--border" => video.show_border = true,
            "--cursor" => video.capture_cursor = true,
            "--system-audio" => audio = AudioChoice::System,
            "--no-audio" => audio = AudioChoice::None,
            "--audio-gain-db" => {
                audio_gain_db = value("--audio-gain-db")?
                    .parse()
                    .context("--audio-gain-db must be a number, e.g. 20")?;
                if !(-40.0..=40.0).contains(&audio_gain_db) {
                    bail!("--audio-gain-db must be between -40 and 40");
                }
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            flag if flag.starts_with("--") => bail!("unknown option {flag}\n{USAGE}"),
            _ if target.is_none() => target = Some(arg),
            _ => bail!("unexpected argument {arg}\n{USAGE}"),
        }
    }
    Ok(target.map(|target| Args {
        target,
        name,
        video,
        audio,
        audio_gain_db,
    }))
}

fn print_windows() -> Result<()> {
    println!("{USAGE}\n\nCapturable windows:");
    for w in windows_list::list_windows()? {
        println!(
            "  0x{:<10x} {:>7}  {:<24} {}",
            w.hwnd,
            w.pid,
            w.process_name.as_deref().unwrap_or("?"),
            w.title
        );
    }
    Ok(())
}
