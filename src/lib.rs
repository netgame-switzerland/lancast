//! lancast library: modules shared by the binary and the examples.

/// The LANcast version: `0.2.0` for a release, `0.2.0+snapshot.g1a2b3c4`
/// for a local build (set by `build.rs`, D30).
pub const VERSION: &str = env!("LANCAST_VERSION");

pub mod app;
pub mod app_icon;
pub mod capture;
pub mod cli;
pub mod config;
pub mod distro;
pub mod event_link;
pub mod exe_icon;
pub mod logging;
pub mod ndi_config;
pub mod ndi_out;
pub mod os_caps;
pub mod session;
pub mod tray;
pub mod tray_visibility;
pub mod url_scheme;
pub mod win_thumbnail;
pub mod windows_list;
