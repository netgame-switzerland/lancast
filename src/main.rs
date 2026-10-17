//! LANcast – send a game window and its audio as an NDI® source.
//!
//! Without arguments LANcast opens its window (SPEC §3). With arguments it
//! runs as a command-line tool; see `LANcast --help`.

// Release builds are GUI applications without a console window; the CLI
// attaches to the console it was started from.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The binary is deliberately named "LANcast".
#![allow(non_snake_case)]

use lancast::{app, cli, logging, ndi_config};
use tracing::{error, info};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // An event link (lancast://…) from the browser opens the GUI (D27).
    let link = match args.as_slice() {
        [arg] if lancast::event_link::is_link(arg) => Some(arg.clone()),
        _ => None,
    };
    let gui = args.is_empty() || link.is_some();
    if !gui {
        attach_parent_console();
    }
    let log_path = logging::init();
    // SAFETY: still single-threaded here; no thread has been spawned yet.
    unsafe { ndi_config::init_before_threads() };
    info!(version = env!("CARGO_PKG_VERSION"), log = ?log_path, gui, "LANcast starting");

    let result = if gui { app::run(link) } else { cli::run(args) };
    if let Err(err) = result {
        error!("{err:#}");
        std::process::exit(1);
    }
}

/// Lets a GUI-subsystem exe print to the console it was started from, unless
/// its output is already redirected.
fn attach_parent_console() {
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    // SAFETY: plain Win32 calls without pointers.
    unsafe {
        let redirected =
            GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
        if !redirected {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
