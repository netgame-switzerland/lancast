//! Runtime detection of the OS build and of optional capture features
//! (SPEC §5.0). A missing feature only disables that feature. On platforms
//! without capture backends yet, every feature counts as missing (D32).

use std::fmt;

#[cfg(windows)]
use windows::Wdk::System::SystemServices::RtlGetVersion;
#[cfg(windows)]
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
#[cfg(windows)]
use windows_capture::graphics_capture_api::GraphicsCaptureApi;

/// First build with `IGraphicsCaptureItemInterop::CreateForWindow` (Win10 1903).
pub const MIN_BUILD: u32 = 18362;
/// Documented minimum for process-loopback audio (D7).
pub const PROCESS_LOOPBACK_BUILD: u32 = 20348;

#[derive(Debug, Clone, Copy)]
pub struct OsCaps {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
    /// Windows Graphics Capture is available at all.
    pub wgc: bool,
    /// `GraphicsCaptureSession.IsCursorCaptureEnabled` (19041+).
    pub cursor_control: bool,
    /// `GraphicsCaptureSession.IsBorderRequired` (20348+).
    pub border_control: bool,
    /// `GraphicsCaptureSession.MinUpdateInterval` (26100+).
    pub min_update_interval: bool,
}

impl OsCaps {
    /// Probes the running system. Probe failures count as "not supported".
    #[cfg(windows)]
    pub fn detect() -> Self {
        let (major, minor, build) = os_version();
        let probe = |result: Result<bool, _>, name: &str| match result {
            Ok(supported) => supported,
            Err(err) => {
                tracing::warn!(%err, "failed to probe {name}; assuming unsupported");
                false
            }
        };
        Self {
            major,
            minor,
            build,
            wgc: probe(GraphicsCaptureApi::is_supported(), "WGC support"),
            cursor_control: probe(
                GraphicsCaptureApi::is_cursor_settings_supported(),
                "cursor control",
            ),
            border_control: probe(
                GraphicsCaptureApi::is_border_settings_supported(),
                "border control",
            ),
            min_update_interval: probe(
                GraphicsCaptureApi::is_minimum_update_interval_supported(),
                "MinUpdateInterval",
            ),
        }
    }

    /// No capture backends yet (D32): nothing is supported.
    #[cfg(not(windows))]
    pub fn detect() -> Self {
        Self {
            major: 0,
            minor: 0,
            build: 0,
            wgc: false,
            cursor_control: false,
            border_control: false,
            min_update_interval: false,
        }
    }

    /// Process loopback is documented from build 20348 but may work earlier,
    /// so this is only a hint; activation is always attempted (D7).
    pub fn process_loopback_documented(&self) -> bool {
        self.build >= PROCESS_LOOPBACK_BUILD
    }
}

impl fmt::Display for OsCaps {
    #[cfg(not(windows))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (no window or audio capture on this platform yet)",
            std::env::consts::OS
        )
    }

    #[cfg(windows)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Windows {}.{} build {} (WGC: {}, cursor control: {}, border control: {}, MinUpdateInterval: {}, process loopback documented: {})",
            self.major,
            self.minor,
            self.build,
            self.wgc,
            self.cursor_control,
            self.border_control,
            self.min_update_interval,
            self.process_loopback_documented()
        )
    }
}

/// `RtlGetVersion` reports the real version regardless of the exe manifest,
/// unlike `GetVersionEx`.
#[cfg(windows)]
fn os_version() -> (u32, u32, u32) {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a properly sized OSVERSIONINFOW with its size field set.
    let status = unsafe { RtlGetVersion(&raw mut info) };
    if status.is_err() {
        tracing::warn!(?status, "RtlGetVersion failed");
        return (0, 0, 0);
    }
    (info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber)
}
