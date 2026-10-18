//! System tray icon (SPEC §3.2): states, menu commands and the icon
//! image. The tray itself is platform-specific: `Shell_NotifyIconW` on
//! Windows (`win32`); other platforms have a stub without a tray (D32).

#[cfg(not(windows))]
pub use unsupported::{TrayHandle, claim_single_instance, hide_window, start};
#[cfg(windows)]
pub use win32::{TrayHandle, claim_single_instance, hide_window, start};

#[cfg(windows)]
mod win32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Sending,
    /// Sending system audio (incl. Discord etc.): a distinct warning state.
    SendingSystemAudio,
    Error,
}

impl TrayState {
    /// Casting: a glowing red "REC" dot, top right.
    fn on_air(self) -> bool {
        matches!(self, Self::Sending | Self::SendingSystemAudio)
    }

    /// Attention (system audio, or stopped unexpectedly): an amber dot,
    /// bottom right.
    fn warning(self) -> bool {
        matches!(self, Self::SendingSystemAudio | Self::Error)
    }
}

/// Commands from the tray menu, delivered on the tray thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    /// The main window was shown (by left click or the menu).
    WindowShown,
    StartSending,
    StopSending,
    Quit,
    /// An event link (`lancast://…`) forwarded by a second instance.
    OpenLink(String),
}

/// The tray icon for `state` as RGBA: the LANcast icon, plus a glowing red
/// REC dot (top right) while casting and an amber dot (bottom right) for
/// system audio or an error. Returns `(side, rgba)`.
pub fn tray_icon_rgba(size: usize, state: TrayState) -> (usize, Vec<u8>) {
    let (side, mut rgba) = crate::app_icon::icon_rgba(size);
    let s = side as f32;
    if state.warning() {
        let radius = s * 0.17;
        let ring = (s * 0.05).max(1.0);
        let c = s - radius - ring;
        draw_dot(&mut rgba, side, (c, c), radius, ring, AMBER_DOT, None);
    }
    if state.on_air() {
        let radius = s * 0.19;
        let ring = (s * 0.05).max(1.0);
        let glow = (s * 0.11).max(1.3);
        let inset = radius + ring + glow * 0.4;
        draw_dot(
            &mut rgba,
            side,
            (s - inset, inset),
            radius,
            ring,
            REC_DOT,
            Some(glow),
        );
    }
    (side, rgba)
}

/// REC dot: bright center, deeper red edge.
const REC_DOT: ([f32; 3], [f32; 3]) = ([255.0, 95.0, 80.0], [205.0, 20.0, 25.0]);
const AMBER_DOT: ([f32; 3], [f32; 3]) = ([255.0, 200.0, 60.0], [235.0, 150.0, 0.0]);
const RING: [f32; 3] = [18.0, 12.0, 14.0];
/// Halo around the REC dot; brighter than the dot's edge so it reads as light.
const GLOW: [f32; 3] = [255.0, 45.0, 35.0];

/// Paints an anti-aliased dot (radial gradient from the first to the second
/// color) with a dark contrast ring and an optional soft glow of width `glow`.
fn draw_dot(
    rgba: &mut [u8],
    side: usize,
    (cx, cy): (f32, f32),
    radius: f32,
    ring: f32,
    (center_color, edge_color): ([f32; 3], [f32; 3]),
    glow: Option<f32>,
) {
    let reach = radius + ring + glow.unwrap_or(0.0);
    for y in 0..side {
        for x in 0..side {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d > reach + 0.5 {
                continue;
            }
            // Layers from bottom to top: glow, ring, fill (straight alpha).
            let mut layer = [0.0f32; 4];
            if let Some(width) = glow {
                let t = ((d - radius - ring) / width).clamp(0.0, 1.0);
                let a = 0.65 * (1.0 - t).powi(2);
                layer = over([GLOW[0], GLOW[1], GLOW[2], a], layer);
            }
            let ring_cover = (radius + ring - d + 0.5).clamp(0.0, 1.0);
            layer = over([RING[0], RING[1], RING[2], ring_cover], layer);
            let fill_cover = (radius - d + 0.5).clamp(0.0, 1.0);
            let t = (d / radius).clamp(0.0, 1.0);
            let fill = [0, 1, 2].map(|c| center_color[c] + (edge_color[c] - center_color[c]) * t);
            layer = over([fill[0], fill[1], fill[2], fill_cover], layer);

            let px = &mut rgba[(y * side + x) * 4..][..4];
            let base = [
                f32::from(px[0]),
                f32::from(px[1]),
                f32::from(px[2]),
                f32::from(px[3]) / 255.0,
            ];
            let out = over(layer, base);
            for c in 0..3 {
                px[c] = out[c].round().clamp(0.0, 255.0) as u8;
            }
            px[3] = (out[3] * 255.0).round() as u8;
        }
    }
}

/// `top` over `bottom`, straight (non-premultiplied) RGBA with alpha 0..1.
fn over(top: [f32; 4], bottom: [f32; 4]) -> [f32; 4] {
    let alpha = top[3] + bottom[3] * (1.0 - top[3]);
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    let mix = |t: f32, b: f32| (t * top[3] + b * bottom[3] * (1.0 - top[3])) / alpha;
    [
        mix(top[0], bottom[0]),
        mix(top[1], bottom[1]),
        mix(top[2], bottom[2]),
        alpha,
    ]
}

/// Stub for platforms without a tray backend yet (D32): no icon; commands
/// never arrive, notifications go to the log.
#[cfg(not(windows))]
mod unsupported {
    use std::thread::JoinHandle;

    use anyhow::{Context, Result};

    use super::{TrayCommand, TrayState};

    #[derive(Clone)]
    pub struct TrayHandle;

    impl TrayHandle {
        pub fn set_state(
            &self,
            _state: TrayState,
            _tooltip: &str,
            _can_start: bool,
            _sending: bool,
        ) {
        }

        pub fn notify(&self, title: &str, text: &str, warning: bool) {
            if warning {
                tracing::warn!("{title}: {text}");
            } else {
                tracing::info!("{title}: {text}");
            }
        }

        pub fn set_main_window(&self, _hwnd: isize) {}

        pub fn set_minimize_to_tray(&self, _enabled: bool) {}

        pub fn hidden_to_tray(&self) {}

        pub fn show_main_window(&self) {}

        pub fn exit(&self) {}
    }

    pub fn start(
        _on_command: impl Fn(TrayCommand) + Send + 'static,
    ) -> Result<(TrayHandle, JoinHandle<()>)> {
        let thread = std::thread::Builder::new()
            .name("tray".into())
            .spawn(|| {})
            .context("failed to spawn the tray thread")?;
        Ok((TrayHandle, thread))
    }

    /// No single-instance check yet: every start runs.
    pub fn claim_single_instance(_link: Option<&str>) -> bool {
        true
    }

    pub fn hide_window(_window: isize) {}
}
