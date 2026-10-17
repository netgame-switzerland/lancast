//! Event links (D27): `lancast://config?event=…&discovery=…` on an event
//! website configures LANcast in one click. Only known keys with valid values
//! are accepted, and the UI always asks before applying anything.
//!
//! Keys (all optional):
//! - `event`: display name of the event, e.g. `NetGame 2026`
//! - `discovery`: the event's NDI discovery server ("NDI gateway"),
//!   `host[:port]`, comma-separated; empty = automatic (mDNS)
//!
//! Event **limits** (enforced; users can choose lower values only), to keep
//! the network bandwidth in check:
//! - `max`: max. resolution, e.g. `1280x720`
//! - `fps`: max. frame rate, 10–120
//!
//! Event **defaults** (applied once; users may change them afterwards):
//! - `level`: receiver audio level, `unity` (OBS/DistroAV) or `ndi` (+20 dB,
//!   NDI Studio Monitor)
//! - `border`, `cursor`: `1`/`0` (or `true`/`false`)

use anyhow::{Result, bail};

use crate::ndi_out::AUDIO_GAIN_STUDIO_MONITOR_DB;

/// The URL scheme LANcast registers.
pub const SCHEME: &str = "lancast";
const MAX_LENGTH: usize = 2048;

/// Settings carried by an event link; `None` = not set by the link.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventLink {
    pub event: Option<String>,
    pub discovery: Option<String>,
    /// Limits.
    pub max_size: Option<(u32, u32)>,
    pub max_fps: Option<u32>,
    /// Defaults.
    pub audio_gain_db: Option<f32>,
    pub show_border: Option<bool>,
    pub capture_cursor: Option<bool>,
}

impl EventLink {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Whether a command-line argument is an event link.
pub fn is_link(arg: &str) -> bool {
    arg.len() > SCHEME.len() && arg[..SCHEME.len() + 1].eq_ignore_ascii_case("lancast:")
}

/// Parses `lancast://config?…`. Unknown keys are ignored; invalid values are
/// errors, so a broken link is reported instead of half-applied.
pub fn parse(url: &str) -> Result<EventLink> {
    if url.len() > MAX_LENGTH {
        bail!("the link is too long");
    }
    if !is_link(url) {
        bail!("not a lancast: link");
    }
    let rest = url[SCHEME.len() + 1..].trim_start_matches('/');
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    if !action.trim_end_matches('/').eq_ignore_ascii_case("config") {
        bail!("unknown link action {action:?} (expected lancast://config?...)");
    }

    let mut link = EventLink::default();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key)?.to_ascii_lowercase();
        let value = decode(value)?.trim().to_owned();
        match key.as_str() {
            "event" => link.event = Some(clean_text(&value, 64)?),
            "discovery" => link.discovery = Some(discovery(&value)?),
            "level" => {
                link.audio_gain_db = Some(match value.to_ascii_lowercase().as_str() {
                    "unity" | "obs" => 0.0,
                    "ndi" | "studio-monitor" => AUDIO_GAIN_STUDIO_MONITOR_DB,
                    other => bail!("level must be \"unity\" or \"ndi\", not {other:?}"),
                })
            }
            "max" => {
                let (w, h) = value
                    .split_once(['x', 'X'])
                    .ok_or_else(|| anyhow::anyhow!("max must look like 1920x1080"))?;
                let (w, h): (u32, u32) = (w.parse()?, h.parse()?);
                if !(320..=3840).contains(&w) || !(180..=2160).contains(&h) {
                    bail!("max resolution {w}x{h} is out of range");
                }
                link.max_size = Some((w, h));
            }
            "fps" => {
                let fps: u32 = value.parse()?;
                if !(10..=120).contains(&fps) {
                    bail!("fps must be between 10 and 120");
                }
                link.max_fps = Some(fps);
            }
            "border" => link.show_border = Some(boolean(&value)?),
            "cursor" => link.capture_cursor = Some(boolean(&value)?),
            other => tracing::info!(key = other, "ignoring unknown event link key"),
        }
    }
    if link.is_empty() {
        bail!("the link contains no settings");
    }
    Ok(link)
}

/// Percent-decoding (`%20`) plus `+` as space, as in HTML forms.
fn decode(text: &str) -> Result<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text
                    .get(i + 1..i + 3)
                    .ok_or_else(|| anyhow::anyhow!("broken %-escape in the link"))?;
                out.push(u8::from_str_radix(hex, 16)?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(String::from_utf8(out)?)
}

fn boolean(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => bail!("expected 1 or 0, not {other:?}"),
    }
}

/// Plain display text without control characters, at most `max` chars.
fn clean_text(value: &str, max: usize) -> Result<String> {
    if value.chars().any(char::is_control) {
        bail!("the link contains control characters");
    }
    Ok(value.chars().take(max).collect())
}

/// Comma-separated `host[:port]` entries with plain host characters.
fn discovery(value: &str) -> Result<String> {
    for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let valid = entry
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'));
        if !valid || entry.len() > 253 {
            bail!("invalid discovery server {entry:?}");
        }
    }
    Ok(value
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect::<Vec<_>>()
        .join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_link() {
        let link = parse(
            "lancast://config?event=NetGame%202026&discovery=10.0.0.5:5959&level=ndi&max=1280x720&fps=30&border=0",
        )
        .unwrap();
        assert_eq!(link.event.as_deref(), Some("NetGame 2026"));
        assert_eq!(link.discovery.as_deref(), Some("10.0.0.5:5959"));
        assert_eq!(link.audio_gain_db, Some(AUDIO_GAIN_STUDIO_MONITOR_DB));
        assert_eq!(link.max_size, Some((1280, 720)));
        assert_eq!(link.max_fps, Some(30));
        assert_eq!(link.show_border, Some(false));
        assert_eq!(link.capture_cursor, None);
    }

    #[test]
    fn variants_and_unknown_keys() {
        // Browsers may pass "lancast:config?…" or add a trailing slash.
        let link = parse("LANCAST:config/?event=LAN+Party&foo=bar&discovery=").unwrap();
        assert_eq!(link.event.as_deref(), Some("LAN Party"));
        assert_eq!(link.discovery.as_deref(), Some(""));
    }

    #[test]
    fn rejects_bad_links() {
        assert!(parse("lancast://config").is_err()); // nothing to apply
        assert!(parse("lancast://run?x=1").is_err());
        assert!(parse("lancast://config?fps=500").is_err());
        assert!(parse("lancast://config?discovery=evil%20host;rm").is_err());
        assert!(parse("lancast://config?event=a%0Ab").is_err());
        assert!(parse("https://example.com").is_err());
    }
}
