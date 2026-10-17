//! Test "game" for lip-sync measurements: a window that flashes white for
//! 100 ms once per second and plays a 1 kHz beep at the same moment through
//! the default output device. Capture it with lancast and measure with
//! `ndi_probe -- <name> <seconds> --av`.
//!
//! Usage: `cargo run --release --example av_sync_source -- [seconds] [--silent]`
//! (default 60 s). `--silent` only flashes, for audio-isolation tests.
//! Prints its PID for `lancast <PID>`.

use std::{
    f64::consts::TAU,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
            PAINTSTRUCT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, MSG,
            PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassW, TranslateMessage,
            WINDOW_EX_STYLE, WM_DESTROY, WM_PAINT, WM_QUIT, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            WS_VISIBLE,
        },
    },
    core::w,
};

const FLASH: Duration = Duration::from_millis(100);
const PERIOD: Duration = Duration::from_secs(1);
const SAMPLE_RATE: u32 = 48_000;
const TONE_HZ: f64 = 1_000.0;

/// Shared time base for picture and sound.
static START: OnceLock<Instant> = OnceLock::new();

fn flash_at(t: Instant) -> bool {
    let since = t.saturating_duration_since(*START.get().expect("start set"));
    since.as_nanos() % PERIOD.as_nanos() < FLASH.as_nanos()
}

fn main() -> Result<()> {
    let (flags, positional): (Vec<String>, Vec<String>) =
        std::env::args().skip(1).partition(|a| a.starts_with("--"));
    let silent = flags.iter().any(|f| f == "--silent");
    let seconds: u64 = positional
        .first()
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(60);
    START.set(Instant::now()).ok();
    println!(
        "av_sync_source PID {} running for {seconds}s",
        std::process::id()
    );

    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let audio = scope.spawn(|| if silent { Ok(()) } else { play_beeps(&stop) });
        let result = run_window(Duration::from_secs(seconds));
        stop.store(true, Ordering::Relaxed);
        let audio = audio.join().map_err(|_| anyhow!("audio thread panicked"))?;
        result.and(audio)
    })
}

/// Fills the render buffer so that each sample's *playback* time decides
/// whether it is part of a beep: a sample written now plays after the
/// samples already queued (`padding`).
fn play_beeps(stop: &AtomicBool) -> Result<()> {
    wasapi::initialize_mta().ok()?;
    let device = DeviceEnumerator::new()?.get_default_device(&Direction::Render)?;
    let mut client = device.get_iaudioclient()?;
    let format = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE as usize, 2, None);
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };
    client.initialize_client(&format, &Direction::Render, &mode)?;
    let event = client.set_get_eventhandle()?;
    let render = client.get_audiorenderclient()?;
    client.start_stream()?;

    let mut phase = 0.0f64;
    let mut bytes = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let _ = event.wait_for_event(100);
        let frames = client.get_available_space_in_frames()? as usize;
        if frames == 0 {
            continue;
        }
        let padding = client.get_current_padding()?;
        let first =
            Instant::now() + Duration::from_secs_f64(f64::from(padding) / f64::from(SAMPLE_RATE));
        bytes.clear();
        for i in 0..frames {
            let t = first + Duration::from_secs_f64(i as f64 / f64::from(SAMPLE_RATE));
            let value = if flash_at(t) {
                (phase.sin() * 0.1) as f32
            } else {
                0.0
            };
            phase = (phase + TAU * TONE_HZ / f64::from(SAMPLE_RATE)) % TAU;
            bytes.extend_from_slice(&value.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        render.write_to_device(frames, &bytes, None)?;
    }
    client.stop_stream()?;
    Ok(())
}

fn run_window(duration: Duration) -> Result<()> {
    // SAFETY: standard Win32 window creation and message loop on this thread;
    // all pointers passed are to locals that outlive the calls.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: w!("lancast_av_sync_source"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(anyhow!("RegisterClassW failed"));
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("lancast_av_sync_source"),
            w!("LANcast A/V sync source"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            360,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let end = Instant::now() + duration;
        let mut shown = None;
        let mut msg = MSG::default();
        while Instant::now() < end {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let flash = flash_at(Instant::now());
            if shown != Some(flash) {
                shown = Some(flash);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by the system on the window thread with valid arguments.
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                // COLORREF is 0x00BBGGRR.
                let color = if flash_at(Instant::now()) {
                    0x00FF_FFFF
                } else {
                    0x0020_2020
                };
                let brush = CreateSolidBrush(COLORREF(color));
                FillRect(hdc, &rect, brush);
                let _ = DeleteObject(brush.into());
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
