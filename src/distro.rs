//! LAN distro / event edition (D28).
//!
//! An event edition is a LANcast build with the organizer's Ed25519 **public
//! key** compiled in (`LANCAST_DISTRO_KEY`, set by `tools/package.ps1`). It
//! only sends with a valid **signed event configuration**, which arrives as
//! `LANcast.event` next to the exe (from the event download) or as a signed
//! `lancast://event?p=…&s=…` link. The organizer signs with the private key,
//! which never leaves the organizer; the exe can verify but not sign, so
//! configurations cannot be forged even with the exe in hand.
//!
//! The signed payload holds the event name/id, a validity window (local
//! dates, inclusive: everyone at a LAN party shares the time zone) and the **enforced** settings. **Defaults** travel unsigned next
//! to it and stay user-changeable.
//!
//! A personal link also carries a signed **user id** (D38): the participant's
//! name for the stream name, signed with the same key and bound to the event.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::event_link::{self, EventValues};

/// Public key of the event organizer (base64url), compiled into event
/// editions. `None` in the normal edition.
pub const DISTRO_KEY: Option<&str> = option_env!("LANCAST_DISTRO_KEY");
/// Display name of the event edition, e.g. `NetGame 2026`.
pub const DISTRO_NAME: Option<&str> = option_env!("LANCAST_DISTRO_NAME");
/// File with the signed event configuration, next to the exe.
pub const EVENT_FILE: &str = "LANcast.event";
const PAYLOAD_VERSION: u32 = 1;

pub fn is_distro() -> bool {
    DISTRO_KEY.is_some_and(|k| !k.is_empty())
}

/// The signed enforced section as transported (both base64url).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedEvent {
    pub payload: String,
    pub signature: String,
}

/// What the organizer signs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventPayload {
    pub v: u32,
    pub event: String,
    pub event_id: String,
    /// Local dates `YYYY-MM-DD`, inclusive.
    pub valid_from: String,
    pub valid_until: String,
    pub enforced: BTreeMap<String, String>,
}

impl EventPayload {
    pub fn enforced_values(&self) -> Result<EventValues> {
        EventValues::from_pairs(self.enforced.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// Checks the validity window against `today` (`YYYY-MM-DD`).
    pub fn check_dates(&self, today: &str) -> Result<()> {
        if today < self.valid_from.as_str() {
            bail!("the event configuration is valid from {}", self.valid_from);
        }
        if today > self.valid_until.as_str() {
            bail!("the event configuration expired on {}", self.valid_until);
        }
        Ok(())
    }
}

/// Decodes the payload without checking the signature (for display only).
pub fn decode_payload(signed: &SignedEvent) -> Result<EventPayload> {
    let bytes = URL_SAFE_NO_PAD
        .decode(signed.payload.trim())
        .context("invalid event payload encoding")?;
    let payload: EventPayload = serde_json::from_slice(&bytes).context("invalid event payload")?;
    if payload.v != PAYLOAD_VERSION {
        bail!("unsupported event configuration version {}", payload.v);
    }
    payload.enforced_values()?; // reject invalid values early
    Ok(payload)
}

/// Verifies the signature with `public_key` (base64url) and decodes.
pub fn verify(signed: &SignedEvent, public_key: &str) -> Result<EventPayload> {
    verify_bytes(signed, public_key)
        .map_err(|_| anyhow!("the signature is not valid for this LANcast edition"))?;
    decode_payload(signed)
}

/// Checks the Ed25519 signature of `signed` over its payload bytes.
fn verify_bytes(signed: &SignedEvent, public_key: &str) -> Result<()> {
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(public_key.trim())
        .ok()
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| anyhow!("invalid public key"))?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| anyhow!("invalid public key"))?;
    let signature: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signed.signature.trim())
        .ok()
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| anyhow!("invalid signature encoding"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(signed.payload.trim())
        .context("invalid payload encoding")?;
    key.verify(&payload, &Signature::from_bytes(&signature))
        .map_err(|_| anyhow!("invalid signature"))
}

/// The compiled-in public key of an event edition.
fn this_build_key() -> Result<&'static str> {
    DISTRO_KEY
        .filter(|k| !k.is_empty())
        .ok_or_else(|| anyhow!("this LANcast is not an event edition"))
}

/// Verifies the signature with the compiled-in key (not the dates: a
/// configuration may arrive before the event starts).
pub fn verify_signature_for_this_build(signed: &SignedEvent) -> Result<EventPayload> {
    verify(signed, this_build_key()?)
}

/// A participant's personal identity (D38), signed by the organizer with the
/// event key and carried as `id=` in a personal event link. It sets the
/// user name in the stream name while its event is in force.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPayload {
    pub v: u32,
    /// The event it belongs to (`EventPayload::event_id`).
    pub event_id: String,
    pub user: String,
}

/// Longest user name a personal link may set.
pub const MAX_USER_LEN: usize = 32;

/// The `id=` token: `<payload>.<signature>`, both base64url (the dot is not
/// in the base64url alphabet).
pub fn user_token(signed: &SignedEvent) -> String {
    format!("{}.{}", signed.payload, signed.signature)
}

pub fn parse_user_token(token: &str) -> Result<SignedEvent> {
    let (payload, signature) = token
        .trim()
        .split_once('.')
        .ok_or_else(|| anyhow!("id= must look like <payload>.<signature>"))?;
    Ok(SignedEvent {
        payload: payload.to_owned(),
        signature: signature.to_owned(),
    })
}

/// Decodes a signed identity without checking the signature.
pub fn decode_user(signed: &SignedEvent) -> Result<UserPayload> {
    let bytes = URL_SAFE_NO_PAD
        .decode(signed.payload.trim())
        .context("invalid id encoding")?;
    let payload: UserPayload = serde_json::from_slice(&bytes).context("invalid id payload")?;
    if payload.v != PAYLOAD_VERSION {
        bail!("unsupported id version {}", payload.v);
    }
    check_user(&payload.user)?;
    Ok(payload)
}

/// A user name for the stream name: 1–32 characters, no control characters.
pub fn check_user(user: &str) -> Result<()> {
    if user.trim().is_empty()
        || user.trim() != user
        || user.chars().count() > MAX_USER_LEN
        || user.chars().any(char::is_control)
    {
        bail!(
            "the user name must be 1–{MAX_USER_LEN} characters without control characters or surrounding spaces"
        );
    }
    Ok(())
}

/// Verifies a signed identity with `public_key` and checks that it belongs
/// to `event_id`.
pub fn verify_user(signed: &SignedEvent, public_key: &str, event_id: &str) -> Result<UserPayload> {
    verify_bytes(signed, public_key)
        .map_err(|_| anyhow!("the personal id is not signed for this LANcast edition"))?;
    let payload = decode_user(signed)?;
    if payload.event_id != event_id {
        bail!(
            "the personal id belongs to another event ({})",
            payload.event_id
        );
    }
    Ok(payload)
}

/// [`verify_user`] with the compiled-in key.
pub fn verify_user_for_this_build(signed: &SignedEvent, event_id: &str) -> Result<UserPayload> {
    verify_user(signed, this_build_key()?, event_id)
}

/// Signs a personal identity (organizer tool, event website).
pub fn sign_user(payload: &UserPayload, secret: &[u8; 32]) -> Result<SignedEvent> {
    check_user(&payload.user)?;
    sign_bytes(&serde_json::to_vec(payload)?, secret)
}

/// Verifies with the compiled-in key and checks today's date.
pub fn verify_for_this_build(signed: &SignedEvent) -> Result<EventPayload> {
    let payload = verify_signature_for_this_build(signed)?;
    payload.check_dates(&today_local())?;
    Ok(payload)
}

/// Signs `payload` with a 32-byte Ed25519 secret key (organizer tool).
pub fn sign(payload: &EventPayload, secret: &[u8; 32]) -> Result<SignedEvent> {
    sign_bytes(&serde_json::to_vec(payload)?, secret)
}

fn sign_bytes(bytes: &[u8], secret: &[u8; 32]) -> Result<SignedEvent> {
    let signature = SigningKey::from_bytes(secret).sign(bytes);
    Ok(SignedEvent {
        payload: URL_SAFE_NO_PAD.encode(bytes),
        signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
}

/// A new secret key (organizer tool).
pub fn generate_secret() -> Result<[u8; 32]> {
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|e| anyhow!("no system randomness: {e}"))?;
    Ok(secret)
}

/// The public key (base64url) for a secret key.
pub fn public_key(secret: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(secret).verifying_key().to_bytes())
}

pub fn encode_secret(secret: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(secret)
}

pub fn decode_secret(text: &str) -> Result<[u8; 32]> {
    URL_SAFE_NO_PAD
        .decode(text.trim())
        .ok()
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| anyhow!("invalid secret key file"))
}

/// The `LANcast.event` file: signed enforced section plus unsigned defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventFile {
    #[serde(flatten)]
    pub signed: SignedEvent,
    #[serde(default)]
    pub defaults: BTreeMap<String, String>,
}

impl EventFile {
    pub fn defaults(&self) -> Result<EventValues> {
        EventValues::from_pairs(self.defaults.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// The equivalent `lancast://event?…` link.
    pub fn link(&self) -> String {
        let mut link = format!(
            "lancast://event?p={}&s={}",
            self.signed.payload, self.signed.signature
        );
        for (key, value) in &self.defaults {
            link.push_str(&format!("&{key}={}", event_link::encode(value)));
        }
        link
    }

    /// The personal link (D38): [`Self::link`] plus the signed user id.
    pub fn personal_link(&self, user: &SignedEvent) -> String {
        format!("{}&id={}", self.link(), user_token(user))
    }
}

pub fn read_event_file(path: &Path) -> Result<EventFile> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))
}

pub fn write_event_file(path: &Path, file: &EventFile) -> Result<()> {
    let text = format!(
        "# LANcast event configuration (D28). The signed part cannot be changed;\n\
         # [defaults] are starting values participants may change.\n{}",
        toml::to_string_pretty(file)?
    );
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))
}

/// The event file shipped next to the exe, if any.
pub fn bundled_event_file() -> Option<EventFile> {
    let path = std::env::current_exe().ok()?.parent()?.join(EVENT_FILE);
    if !path.is_file() {
        return None;
    }
    match read_event_file(&path) {
        Ok(file) => Some(file),
        Err(err) => {
            tracing::warn!("{err:#}");
            None
        }
    }
}

/// Today's local date as `YYYY-MM-DD`.
#[cfg(windows)]
pub fn today_local() -> String {
    // SAFETY: GetLocalTime only fills the returned struct.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

/// Today's date as `YYYY-MM-DD`, in UTC: without a time zone library the
/// local date is not known here, so around midnight it may be a day off
/// (D32).
#[cfg(not(windows))]
pub fn today_local() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Proleptic Gregorian date of a day count since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
#[cfg(any(not(windows), test))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_744), (2026, 10, 18));
    }

    fn payload() -> EventPayload {
        EventPayload {
            v: 1,
            event: "NetGame 2026".into(),
            event_id: "netgame-2026".into(),
            valid_from: "2026-11-01".into(),
            valid_until: "2026-11-05".into(),
            enforced: [
                ("discovery", "10.0.0.5"),
                ("max", "1280x720"),
                ("fps", "60"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        }
    }

    #[test]
    fn sign_and_verify() {
        let secret = generate_secret().unwrap();
        let signed = sign(&payload(), &secret).unwrap();
        let verified = verify(&signed, &public_key(&secret)).unwrap();
        assert_eq!(verified, payload());
        let enforced = verified.enforced_values().unwrap();
        assert_eq!(enforced.max_size, Some((1280, 720)));
    }

    #[test]
    fn tampering_and_wrong_keys_fail() {
        let secret = generate_secret().unwrap();
        let mut signed = sign(&payload(), &secret).unwrap();
        let other = generate_secret().unwrap();
        assert!(verify(&signed, &public_key(&other)).is_err());
        // Change the payload (e.g. raise the resolution limit): rejected.
        let mut changed = payload();
        changed.enforced.insert("max".into(), "3840x2160".into());
        signed.payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&changed).unwrap());
        assert!(verify(&signed, &public_key(&secret)).is_err());
    }

    #[test]
    fn validity_window() {
        let p = payload();
        assert!(p.check_dates("2026-10-31").is_err());
        assert!(p.check_dates("2026-11-01").is_ok());
        assert!(p.check_dates("2026-11-05").is_ok());
        assert!(p.check_dates("2026-11-06").is_err());
    }

    #[test]
    fn local_date_format() {
        let today = today_local();
        assert_eq!(today.len(), 10, "{today}");
        assert!(today.starts_with("20"), "{today}");
    }

    fn user(event_id: &str, name: &str) -> UserPayload {
        UserPayload {
            v: 1,
            event_id: event_id.into(),
            user: name.into(),
        }
    }

    #[test]
    fn user_ids_sign_and_verify() {
        let secret = generate_secret().unwrap();
        let key = public_key(&secret);
        let signed = sign_user(&user("netgame-2026", "Alice"), &secret).unwrap();
        let token = user_token(&signed);
        let parsed = parse_user_token(&token).unwrap();
        assert_eq!(parsed, signed);
        assert_eq!(
            verify_user(&parsed, &key, "netgame-2026").unwrap(),
            user("netgame-2026", "Alice")
        );
        // Another event, another key, a changed name: rejected.
        assert!(verify_user(&parsed, &key, "netgame-2027").is_err());
        let other = generate_secret().unwrap();
        assert!(verify_user(&parsed, &public_key(&other), "netgame-2026").is_err());
        let mut changed = parsed.clone();
        changed.payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&user("netgame-2026", "Bob")).unwrap());
        assert!(verify_user(&changed, &key, "netgame-2026").is_err());
    }

    #[test]
    fn user_ids_and_event_configurations_are_not_interchangeable() {
        let secret = generate_secret().unwrap();
        let key = public_key(&secret);
        let event = sign(&payload(), &secret).unwrap();
        let id = sign_user(&user("netgame-2026", "Alice"), &secret).unwrap();
        assert!(verify_user(&event, &key, "netgame-2026").is_err());
        assert!(verify(&id, &key).is_err());
    }

    #[test]
    fn user_names_are_checked() {
        assert!(check_user("Alice").is_ok());
        assert!(check_user("Zoë the 2nd").is_ok());
        assert!(check_user("").is_err());
        assert!(check_user(" Alice").is_err());
        assert!(check_user("a\nb").is_err());
        assert!(check_user(&"x".repeat(MAX_USER_LEN + 1)).is_err());
        let secret = generate_secret().unwrap();
        assert!(sign_user(&user("netgame-2026", ""), &secret).is_err());
        assert!(parse_user_token("no-dot").is_err());
    }

    #[test]
    fn event_file_and_link_round_trip() {
        let secret = generate_secret().unwrap();
        let file = EventFile {
            signed: sign(&payload(), &secret).unwrap(),
            defaults: [("level".to_owned(), "unity".to_owned())]
                .into_iter()
                .collect(),
        };
        let text = toml::to_string_pretty(&file).unwrap();
        assert_eq!(toml::from_str::<EventFile>(&text).unwrap(), file);
        let link = event_link::parse(&file.link()).unwrap();
        assert_eq!(link.event.as_deref(), Some("NetGame 2026"));
        assert_eq!(link.signed.as_ref(), Some(&file.signed));
        assert_eq!(link.defaults.audio_gain_db, Some(0.0));
        assert_eq!(link.enforced.max_fps, Some(60));
        assert_eq!(link.user, None);
    }

    #[test]
    fn personal_links() {
        let secret = generate_secret().unwrap();
        let file = EventFile {
            signed: sign(&payload(), &secret).unwrap(),
            defaults: BTreeMap::new(),
        };
        let id = sign_user(&user("netgame-2026", "Zoë"), &secret).unwrap();
        let link = event_link::parse(&file.personal_link(&id)).unwrap();
        assert_eq!(link.user.as_ref(), Some(&id));
        assert_eq!(link.user_name().as_deref(), Some("Zoë"));
        // An id for another event, or a broken one, rejects the whole link.
        let foreign = sign_user(&user("netgame-2027", "Zoë"), &secret).unwrap();
        assert!(event_link::parse(&file.personal_link(&foreign)).is_err());
        assert!(event_link::parse(&format!("{}&id=abc", file.link())).is_err());
        // Only signed event links carry an id.
        let config =
            event_link::parse(&format!("lancast://config?event=X&id={}", user_token(&id))).unwrap();
        assert_eq!(config.user, None);
    }
}
