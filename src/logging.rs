//! `tracing` setup: human-readable output to stdout and to `lancast.log`
//! next to the exe (the previous run's log is kept as `lancast.prev.log`).

use std::{
    fs::{self, File},
    path::PathBuf,
    sync::Mutex,
};

use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Installs the global subscriber. Returns the log file path, or `None` if
/// no log file could be created (stdout logging still works then).
pub fn init() -> Option<PathBuf> {
    let filter = EnvFilter::try_from_env("LANCAST_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let (file, path) = match open_log_file() {
        Ok((file, path)) => (Some(file), Some(path)),
        Err(_) => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(file.map(|file| fmt::layer().with_ansi(false).with_writer(Mutex::new(file))))
        .init();

    if path.is_none() {
        tracing::warn!("could not create a log file next to the exe; logging to stdout only");
    }
    path
}

fn open_log_file() -> std::io::Result<(File, PathBuf)> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or(std::io::ErrorKind::NotFound)?;
    let path = dir.join("lancast.log");
    if path.exists() {
        let _ = fs::rename(&path, dir.join("lancast.prev.log"));
    }
    Ok((File::create(&path)?, path))
}
