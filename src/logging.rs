//! `tracing` setup: human-readable output to stdout and to
//! `logs\LANcast.log` in LANcast's data folder (`%LOCALAPPDATA%\LANcast\`, or
//! next to the exe in portable mode). The previous run's log is kept as
//! `LANcast.prev.log`.

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
        tracing::warn!("could not create the log file; logging to stdout only");
    }
    path
}

fn open_log_file() -> std::io::Result<(File, PathBuf)> {
    let dir = crate::config::data_dir().join("logs");
    fs::create_dir_all(&dir)?;
    let path = dir.join("LANcast.log");
    if path.exists() {
        let _ = fs::rename(&path, dir.join("LANcast.prev.log"));
    }
    Ok((File::create(&path)?, path))
}
