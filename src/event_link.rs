//! Event links (D27, D28): `lancast://…` links on an event website that
//! configure LANcast in one click. The UI always asks before applying.
//!
//! Two sections of values, with the same keys:
//! - **enforced**: fixed while the event is active (the user cannot change
//!   them). In an event edition (LAN distro) they must be signed (D28).
//! - **defaults**: starting values the user may change afterwards; never
//!   signed. Enforced values always win over defaults.
//!
//! Keys: `discovery` (NDI discovery server / gateway), `groups` (NDI send
//! groups, D39), `max` (max. resolution, a limit), `fps` (max. frame rate, a
//! limit), `level` (`unity`/`ndi`), `border`, `cursor` (`1`/`0`).
//!
//! Link forms:
//! - `lancast://config?event=…&discovery=…&max=…&fps=…&level=…` (unsigned;
//!   `discovery`, `groups`, `max`, `fps` are enforced, the others are
//!   defaults)
//! - `lancast://event?p=<payload>&s=<signature>&level=…` (signed enforced
//!   section in `p`, unsigned defaults as further parameters)
//! - the same with `&id=<payload>.<signature>`: a personal link with the
//!   participant's signed user name (D38)

use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};

use crate::{distro::SignedEvent, ndi_out::AUDIO_GAIN_STUDIO_MONITOR_DB};

/// The URL scheme LANcast registers.
pub const SCHEME: &str = "lancast";
const MAX_LENGTH: usize = 4096;

/// Values of one section; `None` = not set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventValues {
    pub discovery: Option<String>,
    /// NDI send groups, normalized; empty = NDI's own setting (D39).
    pub groups: Option<String>,
    pub max_size: Option<(u32, u32)>,
    pub max_fps: Option<u32>,
    pub audio_gain_db: Option<f32>,
    pub show_border: Option<bool>,
    pub capture_cursor: Option<bool>,
}

impl EventValues {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Sets `key` from its text form. Returns `false` for unknown keys.
    pub fn set(&mut self, key: &str, value: &str) -> Result<bool> {
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "discovery" => self.discovery = Some(discovery(value)?),
            "groups" => self.groups = Some(groups(value)?),
            "max" => {
                let (w, h) = value
                    .split_once(['x', 'X'])
                    .ok_or_else(|| anyhow!("max must look like 1920x1080"))?;
                let (w, h): (u32, u32) = (w.trim().parse()?, h.trim().parse()?);
                if !(320..=3840).contains(&w) || !(180..=2160).contains(&h) {
                    bail!("max resolution {w}x{h} is out of range");
                }
                self.max_size = Some((w, h));
            }
            "fps" => {
                let fps: u32 = value.parse()?;
                if !(10..=120).contains(&fps) {
                    bail!("fps must be between 10 and 120");
                }
                self.max_fps = Some(fps);
            }
            "level" => {
                self.audio_gain_db = Some(match value.to_ascii_lowercase().as_str() {
                    "unity" | "obs" => 0.0,
                    "ndi" | "studio-monitor" => AUDIO_GAIN_STUDIO_MONITOR_DB,
                    other => bail!("level must be \"unity\" or \"ndi\", not {other:?}"),
                })
            }
            "border" => self.show_border = Some(boolean(value)?),
            "cursor" => self.capture_cursor = Some(boolean(value)?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Parses key/value pairs; unknown keys are ignored, invalid values fail.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self> {
        let mut values = Self::default();
        for (key, value) in pairs {
            if !values.set(key, value)? {
                tracing::info!(key, "ignoring unknown event setting");
            }
        }
        Ok(values)
    }

    /// The text form, for storing and signing.
    pub fn to_pairs(&self) -> BTreeMap<String, String> {
        let mut pairs = BTreeMap::new();
        if let Some(d) = &self.discovery {
            pairs.insert("discovery".into(), d.clone());
        }
        if let Some(g) = &self.groups {
            pairs.insert("groups".into(), g.clone());
        }
        if let Some((w, h)) = self.max_size {
            pairs.insert("max".into(), format!("{w}x{h}"));
        }
        if let Some(fps) = self.max_fps {
            pairs.insert("fps".into(), fps.to_string());
        }
        if let Some(gain) = self.audio_gain_db {
            let level = if gain == 0.0 { "unity" } else { "ndi" };
            pairs.insert("level".into(), level.into());
        }
        if let Some(b) = self.show_border {
            pairs.insert("border".into(), u8::from(b).to_string());
        }
        if let Some(c) = self.capture_cursor {
            pairs.insert("cursor".into(), u8::from(c).to_string());
        }
        pairs
    }
}

/// A parsed event link.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventLink {
    pub event: Option<String>,
    pub enforced: EventValues,
    pub defaults: EventValues,
    /// Signed links (event editions): the signed enforced section, still to be
    /// verified by `distro::verify_for_this_build`.
    pub signed: Option<SignedEvent>,
    /// Personal links (D38): the signed user id, still to be verified by
    /// `distro::verify_user_for_this_build`. Only with `signed`.
    pub user: Option<SignedEvent>,
}

impl EventLink {
    /// The user name a personal link claims (not verified).
    pub fn user_name(&self) -> Option<String> {
        let user = crate::distro::decode_user(self.user.as_ref()?).ok()?;
        Some(user.user)
    }
}

/// Whether a command-line argument is an event link.
pub fn is_link(arg: &str) -> bool {
    // `get` rather than slicing: a multi-byte character at the cut must
    // not panic.
    arg.get(..SCHEME.len() + 1)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("lancast:"))
}

/// The event link a browser started LANcast with, if any.
///
/// The registered command is `"LANcast.exe" "%1"`, and Windows inserts the
/// link verbatim: a `"` in a crafted link closes the quotes and the rest
/// becomes extra arguments. So a link as the first argument always means
/// GUI mode and the rest is ignored; it must never reach the command line,
/// which starts sending without asking.
pub fn from_args(args: &[String]) -> Option<&str> {
    args.first().map(String::as_str).filter(|arg| is_link(arg))
}

/// Parses an event link. Unknown keys are ignored; invalid values are
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
    let pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((decode(key)?.to_ascii_lowercase(), decode(value)?))
        })
        .collect::<Result<_>>()?;

    let mut link = EventLink::default();
    match action.trim_end_matches('/').to_ascii_lowercase().as_str() {
        "config" => {
            for (key, value) in &pairs {
                match key.as_str() {
                    "event" => link.event = Some(clean_text(value, 64)?),
                    // The network and bandwidth settings stay fixed while
                    // the event is active; the rest are defaults.
                    "discovery" | "groups" | "max" | "fps" => {
                        link.enforced.set(key, value)?;
                    }
                    _ => {
                        if !link.defaults.set(key, value)? {
                            tracing::info!(key, "ignoring unknown event link key");
                        }
                    }
                }
            }
        }
        "event" => {
            let mut payload = None;
            let mut signature = None;
            let mut user = None;
            for (key, value) in &pairs {
                match key.as_str() {
                    "p" => payload = Some(value.clone()),
                    "s" => signature = Some(value.clone()),
                    "id" => user = Some(crate::distro::parse_user_token(value)?),
                    _ => {
                        if !link.defaults.set(key, value)? {
                            tracing::info!(key, "ignoring unknown event link key");
                        }
                    }
                }
            }
            let (Some(payload), Some(signature)) = (payload, signature) else {
                bail!("a signed event link needs p= and s=");
            };
            let signed = SignedEvent { payload, signature };
            // Show what the link claims; the signature is checked separately.
            let claimed = crate::distro::decode_payload(&signed)?;
            link.event = Some(clean_text(&claimed.event, 64)?);
            link.enforced = claimed.enforced_values()?;
            if let Some(user) = &user {
                let claimed_user = crate::distro::decode_user(user)?;
                if claimed_user.event_id != claimed.event_id {
                    bail!(
                        "the personal id belongs to another event ({})",
                        claimed_user.event_id
                    );
                }
            }
            link.signed = Some(signed);
            link.user = user;
        }
        other => bail!("unknown link action {other:?} (expected config or event)"),
    }
    if link.event.is_none() && link.enforced.is_empty() && link.defaults.is_empty() {
        bail!("the link contains no settings");
    }
    Ok(link)
}

/// Percent-decoding (`%20`) plus `+` as space, as in HTML forms.
pub fn decode(text: &str) -> Result<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text
                    .get(i + 1..i + 3)
                    .ok_or_else(|| anyhow!("broken %-escape in the link"))?;
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

/// Percent-encoding for link parameters.
pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn boolean(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => bail!("expected 1 or 0, not {other:?}"),
    }
}

/// Plain display text without control characters, at most `max` chars.
pub fn clean_text(value: &str, max: usize) -> Result<String> {
    if value.chars().any(char::is_control) {
        bail!("the text contains control characters");
    }
    Ok(value.trim().chars().take(max).collect())
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

/// NDI's limit for a group list (bytes, NDI Groups white paper).
const MAX_GROUPS_LENGTH: usize = 248;

/// Comma-separated NDI group names of letters, digits, space, `.`, `-` and
/// `_`, at most 64 characters each, without duplicates (NDI compares them
/// case-insensitively). Empty = NDI's own setting.
pub fn groups(value: &str) -> Result<String> {
    let mut names: Vec<&str> = Vec::new();
    for name in value.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let valid = name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_'));
        if !valid || name.len() > 64 {
            bail!("invalid NDI group {name:?}");
        }
        if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            names.push(name);
        }
    }
    let joined = names.join(",");
    if joined.len() > MAX_GROUPS_LENGTH {
        bail!("the NDI group list is longer than {MAX_GROUPS_LENGTH} characters");
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_link_splits_sections() {
        let link = parse(
            "lancast://config?event=NetGame%202026&discovery=10.0.0.5:5959&level=ndi&max=1280x720&fps=30&border=0",
        )
        .unwrap();
        assert_eq!(link.event.as_deref(), Some("NetGame 2026"));
        assert_eq!(link.enforced.discovery.as_deref(), Some("10.0.0.5:5959"));
        assert_eq!(link.enforced.max_size, Some((1280, 720)));
        assert_eq!(link.enforced.max_fps, Some(30));
        assert_eq!(
            link.defaults.audio_gain_db,
            Some(AUDIO_GAIN_STUDIO_MONITOR_DB)
        );
        assert_eq!(link.defaults.show_border, Some(false));
        assert!(link.signed.is_none());
    }

    #[test]
    fn variants_and_unknown_keys() {
        let link = parse("LANCAST:config/?event=LAN+Party&foo=bar&discovery=").unwrap();
        assert_eq!(link.event.as_deref(), Some("LAN Party"));
        assert_eq!(link.enforced.discovery.as_deref(), Some(""));
    }

    #[test]
    fn rejects_bad_links() {
        assert!(parse("lancast://config").is_err());
        assert!(parse("lancast://run?x=1").is_err());
        assert!(parse("lancast://config?fps=500").is_err());
        assert!(parse("lancast://config?discovery=evil%20host;rm").is_err());
        assert!(parse("lancast://config?event=a%0Ab").is_err());
        assert!(parse("lancast://event?level=ndi").is_err()); // unsigned "event" link
        assert!(parse("https://example.com").is_err());
    }

    #[test]
    fn link_detection_never_panics() {
        assert!(is_link("lancast://config?event=x"));
        assert!(is_link("LANCAST:config"));
        assert!(!is_link("lancasté")); // é straddles the prefix boundary
        assert!(!is_link("Zürich Kart"));
        assert!(!is_link("€€€"));
        assert!(!is_link("lancast"));
    }

    #[test]
    fn link_arguments_never_reach_the_cli() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let injected = args(&["lancast://x", "--cursor", "--name", "Pwned"]);
        assert_eq!(from_args(&injected), Some("lancast://x"));
        let single = args(&["lancast://config?fps=30"]);
        assert_eq!(from_args(&single), Some("lancast://config?fps=30"));
        assert_eq!(from_args(&args(&["Counter-Strike", "--fps", "30"])), None);
        assert_eq!(from_args(&args(&["--list"])), None);
        assert_eq!(from_args(&[]), None);
    }

    #[test]
    fn groups_are_enforced_and_normalized() {
        let link = parse("lancast://config?event=x&groups=+NetGame%2C+Public+%2C,netgame").unwrap();
        assert_eq!(link.enforced.groups.as_deref(), Some("NetGame,Public"));
        let link = parse("lancast://config?event=x&groups=").unwrap();
        assert_eq!(link.enforced.groups.as_deref(), Some(""));
        assert!(parse("lancast://config?groups=a;b").is_err());
        assert!(groups(&"g".repeat(65)).is_err());
        let many: Vec<String> = (0..10).map(|i| format!("{i}{}", "g".repeat(30))).collect();
        assert!(groups(&many.join(",")).is_err());
        assert_eq!(groups("Public,lancast").unwrap(), "Public,lancast");
    }

    #[test]
    fn values_round_trip() {
        let values = EventValues::from_pairs([
            ("max", "1280x720"),
            ("fps", "30"),
            ("cursor", "0"),
            ("groups", "NetGame"),
        ])
        .unwrap();
        let pairs = values.to_pairs();
        let back =
            EventValues::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))).unwrap();
        assert_eq!(back, values);
        assert_eq!(encode("NetGame 2026/1"), "NetGame%202026%2F1");
    }
}
