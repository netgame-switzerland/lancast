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
    /// Main window: inner size and outer position in points (position
    /// unset = let Windows choose), and whether it was maximized.
    pub window_width: f32,
    pub window_height: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_y: Option<f32>,
    pub window_maximized: bool,
    /// Who is casting; part of the stream name (D26). Defaults to the
    /// Windows login name.
    pub user_name: String,
    /// Last game name per process (lowercase exe name).
    pub source_names: BTreeMap<String, String>,
}

/// The NDI source name for `user` and `game`: `User - Game` (D26). NDI
/// prepends the computer name, giving `HOST (User - Game)`. Either part may
/// be empty.
pub fn source_name(user: &str, game: &str) -> String {
    match (user.trim(), game.trim()) {
        ("", game) => game.to_owned(),
        (user, "") => user.to_owned(),
        (user, game) => format!("{user} - {game}"),
    }
}

/// The Windows login name, the default user name.
fn login_name() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

/// Default inner size of the main window, in points.
pub const DEFAULT_WINDOW_SIZE: (f32, f32) = (1050.0, 980.0);
/// Minimum inner size of the main window, in points.
pub const MIN_WINDOW_SIZE: (f32, f32) = (600.0, 680.0);

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
            window_width: DEFAULT_WINDOW_SIZE.0,
            window_height: DEFAULT_WINDOW_SIZE.1,
            window_x: None,
            window_y: None,
            window_maximized: false,
            user_name: login_name(),
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
        self.user_name = self.user_name.trim().to_owned();
        let finite_or = |v: f32, default: f32| if v.is_finite() { v } else { default };
        self.window_width =
            finite_or(self.window_width, DEFAULT_WINDOW_SIZE.0).clamp(MIN_WINDOW_SIZE.0, 8000.0);
        self.window_height =
            finite_or(self.window_height, DEFAULT_WINDOW_SIZE.1).clamp(MIN_WINDOW_SIZE.1, 8000.0);
        self.window_x = self.window_x.filter(|v| v.is_finite());
        self.window_y = self.window_y.filter(|v| v.is_finite());
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
    fn source_name_combines_user_and_game() {
        assert_eq!(source_name("Kay", "RocketLeague"), "Kay - RocketLeague");
        assert_eq!(source_name(" ", "RocketLeague"), "RocketLeague");
        assert_eq!(source_name("Kay", ""), "Kay");
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
