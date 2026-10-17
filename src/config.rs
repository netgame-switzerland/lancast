//! Settings file `LANcast.toml` and the data folder (SPEC §4.6, D23).
//!
//! Following Windows conventions, per-user, machine-specific data lives in
//! `%LOCALAPPDATA%\LANcast\` (settings, NDI config, logs); nothing is written
//! into the program folder. **Portable mode:** if a `LANcast.toml` exists next
//! to the exe, that folder is used instead (e.g. on a USB stick).
//!
//! All settings are optional; a missing or invalid file falls back to
//! defaults (an invalid file is kept as `LANcast.toml.bak`).

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

const FILE_NAME: &str = "LANcast.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// × and minimize hide LANcast to the tray.
    pub to_tray: bool,
    pub always_on_top: bool,
    pub fps_cap: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub show_border: bool,
    pub capture_cursor: bool,
    /// Gain for the receiver's audio level convention (D17): 0 for OBS /
    /// DistroAV, 20 for NDI Studio Monitor.
    pub audio_gain_db: f32,
    /// NDI discovery server(s) for LANcast, e.g. `192.168.1.10` or
    /// `192.168.1.10:5959`; empty = automatic (mDNS, or the system setting).
    pub discovery_server: String,
    /// Ignore the discovery server configured in NDI Access Manager (D14).
    pub ignore_system_discovery: bool,
    /// Last source name per process (lowercase exe name).
    pub source_names: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            to_tray: true,
            always_on_top: false,
            fps_cap: 60,
            max_width: 1920,
            max_height: 1080,
            show_border: true,
            capture_cursor: false,
            audio_gain_db: 0.0,
            discovery_server: String::new(),
            ignore_system_discovery: false,
            source_names: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Keeps values in the ranges the UI offers.
    fn sanitized(mut self) -> Self {
        self.fps_cap = self.fps_cap.clamp(10, 120);
        self.max_width = self.max_width.clamp(320, 3840);
        self.max_height = self.max_height.clamp(180, 2160);
        if !self.audio_gain_db.is_finite() {
            self.audio_gain_db = 0.0;
        }
        self.audio_gain_db = self.audio_gain_db.clamp(-40.0, 40.0);
        self.discovery_server = self.discovery_server.trim().to_owned();
        self
    }
}

/// Where the settings live, and what was last written there.
pub struct ConfigStore {
    path: PathBuf,
    saved: Option<Config>,
}

impl ConfigStore {
    /// Loads the settings; never fails (defaults on any problem).
    pub fn load() -> (Self, Config) {
        let path = choose_path();
        let config = match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(config) => {
                    info!(path = %path.display(), "settings loaded");
                    Some(config.sanitized())
                }
                Err(err) => {
                    warn!(path = %path.display(), "invalid settings file, using defaults: {err}");
                    let _ = fs::rename(&path, path.with_extension("toml.bak"));
                    None
                }
            },
            Err(_) => None,
        };
        let saved = config.clone();
        (Self { path, saved }, config.unwrap_or_default())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes the settings if they changed since the last save.
    pub fn save(&mut self, config: &Config) {
        if self.saved.as_ref() == Some(config) {
            return;
        }
        let text = match toml::to_string_pretty(config) {
            Ok(text) => text,
            Err(err) => {
                warn!("failed to serialize settings: {err}");
                return;
            }
        };
        let header =
            "# LANcast settings. Written by LANcast; edit while LANcast is not running.\n\n";
        let result = self
            .path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| {
                // Write a temporary file first so a crash cannot leave half a file.
                let tmp = self.path.with_extension("toml.tmp");
                fs::write(&tmp, format!("{header}{text}"))?;
                fs::rename(&tmp, &self.path)
            });
        match result {
            Ok(()) => self.saved = Some(config.clone()),
            Err(err) => warn!(path = %self.path.display(), "failed to save settings: {err}"),
        }
    }
}

fn choose_path() -> PathBuf {
    data_dir().join(FILE_NAME)
}

/// LANcast's data folder: next to the exe in portable mode (a `LANcast.toml`
/// exists there), otherwise `%LOCALAPPDATA%\LANcast\`.
pub fn data_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(dir) = &exe_dir
        && dir.join(FILE_NAME).is_file()
    {
        return dir.clone();
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("LANcast"))
        .or(exe_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Whether LANcast runs in portable mode (data next to the exe).
pub fn is_portable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(FILE_NAME).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_keeps_defaults_and_clamps() {
        let config: Config = toml::from_str("fps_cap = 500\nalways_on_top = true\n").unwrap();
        let config = config.sanitized();
        assert_eq!(config.fps_cap, 120);
        assert!(config.always_on_top);
        assert!(config.to_tray);
        assert_eq!((config.max_width, config.max_height), (1920, 1080));
    }

    #[test]
    fn round_trip() {
        let mut config = Config::default();
        config
            .source_names
            .insert("war3.exe".into(), "Warcraft".into());
        config.discovery_server = "192.168.1.10".into();
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }
}
