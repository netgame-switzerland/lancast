//! lancast – send a game window and its audio as an NDI® source.
//!
//! Without arguments lancast opens its window (SPEC §3). With arguments it
//! runs as a command-line tool; see `lancast --help`.

// Release builds are GUI applications without a console window; the CLI
// attaches to the console it was started from.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use lancast::{app, cli, logging};
use tracing::{error, info};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let gui = args.is_empty();
    if !gui {
        attach_parent_console();
    }
    let log_path = logging::init();
    info!(version = env!("CARGO_PKG_VERSION"), log = ?log_path, gui, "lancast starting");

    let result = if gui { app::run() } else { cli::run(args) };
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
