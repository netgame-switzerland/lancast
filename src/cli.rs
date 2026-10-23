//! Command-line mode (`LANcast <0xHWND | PID> [options]`), mainly for testing
//! and troubleshooting. The GUI is the default without arguments.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tracing::{debug, info, warn};

use crate::{
    capture::{audio::AudioSource, video::VideoOptions},
    os_caps::OsCaps,
    session::{POLL_INTERVAL, Session, SessionConfig, SessionEvent},
    windows_list,
};

const STATUS_INTERVAL: Duration = Duration::from_secs(5);

pub const USAGE: &str = "usage: LANcast [<0xHWND | PID> [--name NAME] [--groups G1,G2] [--fps N] [--max-size WxH] [--no-border] [--cursor] [--system-audio | --no-audio] [--audio-gain-db DB]]
  Without arguments, LANcast opens its window. `LANcast --list` prints the capturable windows.
  --audio-gain-db 20 matches NDI Studio Monitor's level convention (D17).
  --groups sets the NDI send groups (default Public,lancast; \"\" = NDI's own setting, D39).";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioChoice {
    /// Process loopback of the window's process tree, falling back to system audio.
    Game,
    /// Force system loopback (everything the default device plays).
    System,
    None,
}

struct Args {
    target: String,
    name: Option<String>,
    groups: String,
    video: VideoOptions,
    audio: AudioChoice,
    audio_gain_db: f32,
}

/// Runs the CLI with the arguments after the program name.
pub fn run(args: Vec<String>) -> Result<()> {
    let Some(args) = parse_args(args)? else {
        print_windows()?;
        return Ok(());
    };

    let caps = OsCaps::detect();
    info!("{caps}");
    #[cfg(windows)]
    if caps.build < crate::os_caps::MIN_BUILD {
        warn!(
            "Windows build {} is older than the supported minimum {} (Windows 10 1903)",
            caps.build,
            crate::os_caps::MIN_BUILD
        );
    }

    let window = windows_list::resolve_target(&args.target)?;
    info!(
        hwnd = format!("0x{:x}", window.hwnd),
        pid = window.pid,
        process = window.process_name.as_deref().unwrap_or("?"),
        "target window"
    );
    // Window titles can contain private text (browser tabs, documents), and
    // users may share their log file, so titles are logged at debug only.
    debug!(title = %window.title, "target window title");

    let audio = match args.audio {
        AudioChoice::Game => Some(AudioSource::Process {
            pid: window.pid,
            fallback_to_system: true,
        }),
        AudioChoice::System => Some(AudioSource::System),
        AudioChoice::None => None,
    };
    let config = SessionConfig {
        source_name: args.name.unwrap_or_else(|| window.default_source_name()),
        groups: args.groups,
        window,
        video: args.video,
        audio,
        audio_gain_db: args.audio_gain_db,
    };
    let (mut session, _audio_error) = Session::start(&config, &caps)?;
    if args.audio == AudioChoice::Game
        && session.audio_mode() == Some(crate::capture::audio::AudioMode::SystemLoopback)
    {
        warn!("sending SYSTEM audio: other apps (Discord etc.) are audible too");
    }

    let interrupted = Arc::new(AtomicBool::new(false));
    ctrlc::set_handler({
        let interrupted = interrupted.clone();
        move || interrupted.store(true, Ordering::Relaxed)
    })
    .context("failed to install the Ctrl-C handler")?;
    info!(source = %session.source_name(), "sending; press Ctrl-C to stop");

    let reason = supervise(&mut session, &interrupted);
    info!("stopping: {reason}");
    session.stop();
    info!("stopped");
    Ok(())
}

fn supervise(session: &mut Session, interrupted: &AtomicBool) -> &'static str {
    let mut last_status = Instant::now();
    loop {
        std::thread::sleep(POLL_INTERVAL);
        if interrupted.load(Ordering::Relaxed) {
            return "interrupted";
        }
        for event in session.poll() {
            match event {
                SessionEvent::WindowClosed => {
                    warn!("the target window was closed");
                    return "target window closed";
                }
                SessionEvent::Minimized => {
                    info!("target window minimized; the last frame stays on air")
                }
                SessionEvent::Restored => info!("target window restored"),
                SessionEvent::NoFrames => warn!(
                    "no frames: static content, or a game in exclusive fullscreen. \
                     Please use borderless windowed mode."
                ),
                SessionEvent::FramesResumed => info!("frames arriving again"),
            }
        }

        if last_status.elapsed() >= STATUS_INTERVAL {
            last_status = Instant::now();
            let s = session.status();
            info!(
                source = format!("{}x{}", s.source_size.0, s.source_size.1),
                output = format!("{}x{}", s.output_size.0, s.output_size.1),
                capture_fps = format!("{:.1}", s.capture_fps),
                send_fps = format!("{:.1}", s.send_fps),
                readback_ms = format!("{:.2}", s.readback_ms),
                paced_out = s.paced_out,
                backlogged = s.backlogged,
                receivers = s.receivers,
                "status"
            );
            if let (Some(mode), Some(levels)) = (session.audio_mode(), session.audio_levels()) {
                let (sent, dropped, padded_ms, trimmed_ms) = session.audio_counters();
                info!(
                    %mode,
                    peak_db = format!("{:.1}/{:.1}", levels.take_peak_db(0), levels.take_peak_db(1)),
                    rms_db = format!("{:.1}/{:.1}", levels.rms_db(0), levels.rms_db(1)),
                    chunks_sent = sent,
                    dropped,
                    padded_ms,
                    trimmed_ms,
                    "audio"
                );
            }
        }
    }
}

/// Returns `None` for `--list`.
fn parse_args(args: Vec<String>) -> Result<Option<Args>> {
    let mut args = args.into_iter();
    let mut target = None;
    let mut name = None;
    let mut groups = crate::ndi_out::DEFAULT_GROUPS.to_owned();
    let mut video = VideoOptions::default();
    let mut audio = AudioChoice::Game;
    let mut audio_gain_db = 0.0f32;

    while let Some(arg) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .with_context(|| format!("{flag} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--list" => return Ok(None),
            "--name" => name = Some(value("--name")?),
            "--groups" => groups = crate::event_link::groups(&value("--groups")?)?,
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
            "--no-border" => video.show_border = false,
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
    let target = target.with_context(|| format!("no target window given\n{USAGE}"))?;
    Ok(Some(Args {
        target,
        name,
        groups,
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
