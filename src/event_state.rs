//! The event in force (D27, D28, D38) and its enforced values, computed from
//! the saved settings. Shared by the GUI and the CLI, so both obey the same
//! event (D43).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use crate::{
    capture::video::VideoOptions,
    config::{Config, PreviousUser},
    distro::{self, EventFile, SignedEvent},
    event_link::{self, EventLink, EventValues},
};

/// The event in force.
#[derive(Debug, Clone)]
pub struct ActiveEvent {
    pub name: String,
    /// Values the user cannot change while the event is active.
    pub enforced: EventValues,
    /// Event edition: the validity window of the signed configuration.
    pub valid: Option<(String, String)>,
}

/// What the saved settings say about the event.
#[derive(Debug, Clone, Default)]
pub struct EventState {
    pub event: Option<ActiveEvent>,
    /// Event edition: why sending is not allowed.
    pub error: Option<String>,
    /// Event edition: the verified user name of a personal link (D38).
    pub user: Option<String>,
    /// Event edition: where and how often the configuration is loaded (D44).
    pub source: Option<EventSource>,
}

/// The configuration address of the configuration in force (D44, D45).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSource {
    pub url: String,
    /// How often it is loaded again while LANcast runs; `None` = never.
    pub refresh: Option<Duration>,
}

impl EventState {
    /// Computes the event in force. An event edition verifies the signature
    /// and the dates again (D28).
    pub fn from_config(config: &Config) -> Self {
        if !distro::is_distro() {
            if config.event_name.is_empty() && config.event_enforced.is_empty() {
                return Self::default();
            }
            let pairs = config.event_enforced.iter();
            return Self {
                event: Some(ActiveEvent {
                    name: config.event_name.clone(),
                    enforced: EventValues::from_pairs(pairs.map(|(k, v)| (k.as_str(), v.as_str())))
                        .unwrap_or_default(),
                    valid: None,
                }),
                ..Self::default()
            };
        }
        let Some(signed) = &config.event_signed else {
            return Self {
                error: Some(format!(
                    "this {} edition of LANcast sends only with a signed event configuration. \
                     Click the event link on the event website, or put the event's {} next to \
                     LANcast.exe.",
                    distro::DISTRO_NAME.unwrap_or("event"),
                    distro::EVENT_FILE
                )),
                ..Self::default()
            };
        };
        // The personal name shows even outside the event's dates, so a link
        // clicked before the event visibly took effect (D38).
        let signed_payload = distro::verify_signature_for_this_build(signed).ok();
        // Even an expired configuration names the place of the next one.
        let source = signed_payload.as_ref().and_then(|payload| {
            Some(EventSource {
                url: payload.source.clone()?,
                refresh: payload.refresh(),
            })
        });
        let user = config.event_user.as_ref().and_then(|user| {
            let event = signed_payload.as_ref()?;
            distro::verify_user_for_this_build(user, &event.event_id)
                .inspect_err(|err| warn!("ignoring the stored user id: {err:#}"))
                .ok()
                .map(|u| u.user)
        });
        let checked = distro::verify_for_this_build(signed).and_then(|payload| {
            Ok(ActiveEvent {
                enforced: payload.enforced_values()?,
                name: payload.event,
                valid: Some((payload.valid_from, payload.valid_until)),
            })
        });
        match checked {
            Ok(event) => Self {
                event: Some(event),
                error: None,
                user,
                source,
            },
            Err(err) => Self {
                event: None,
                error: Some(format!("{err:#}.")),
                user,
                source,
            },
        }
    }

    /// The enforced values (empty without an event).
    pub fn enforced(&self) -> EventValues {
        self.event
            .as_ref()
            .map(|e| e.enforced.clone())
            .unwrap_or_default()
    }
}

/// Event edition: the `LANcast.event` shipped with the download (D28) as a
/// link, if it is valid and not applied yet (`config.event_bundled`).
pub fn bundled_link(config: &Config) -> Option<EventLink> {
    if !distro::is_distro() {
        return None;
    }
    let file = distro::bundled_event_file()?;
    if file.signed.signature == config.event_bundled {
        return None;
    }
    let checked = distro::verify_signature_for_this_build(&file.signed).and_then(|payload| {
        Ok(EventLink {
            event: Some(payload.event.clone()),
            enforced: payload.enforced_values()?,
            defaults: file.defaults()?,
            signed: Some(file.signed.clone()),
            user: None,
        })
    });
    match checked {
        Ok(link) => {
            info!(event = ?link.event, "bundled event configuration found");
            Some(link)
        }
        Err(err) => {
            warn!("{} is not valid: {err:#}", distro::EVENT_FILE);
            None
        }
    }
}

/// Event edition: the `source` URL of the stored configuration (D44), where
/// the current configuration is loaded from.
pub fn source_url(config: &Config) -> Option<String> {
    EventState::from_config(config).source.map(|s| s.url)
}

/// Whether two signed event configurations are for the same `event_id`.
pub fn same_event_id(a: Option<&SignedEvent>, b: Option<&SignedEvent>) -> bool {
    let id = |s: Option<&SignedEvent>| {
        s.and_then(|s| distro::decode_payload(s).ok())
            .map(|p| p.event_id)
    };
    let a = id(a);
    a.is_some() && a == id(b)
}

/// Event edition: sets the personal id for the configuration `signed` that
/// replaces the one in force, before it is stored (D38, D47).
/// - A personal id (`user`) sets the name and ends an earlier event's name.
/// - Without one, the stored id stays if `signed` is the same event.
/// - Otherwise the stored id is dropped, but its name is kept as
///   `event_user_previous`, marked with the event it was confirmed for.
pub fn switch_event_user(
    config: &mut Config,
    signed: Option<&SignedEvent>,
    user: Option<&SignedEvent>,
) {
    switch_event_user_with_key(config, distro::this_build_key().ok(), signed, user);
}

fn switch_event_user_with_key(
    config: &mut Config,
    public_key: Option<&str>,
    signed: Option<&SignedEvent>,
    user: Option<&SignedEvent>,
) {
    if let Some(user) = user {
        config.event_user = Some(user.clone());
        config.event_user_previous = None;
        return;
    }
    if same_event_id(config.event_signed.as_ref(), signed) {
        return;
    }
    let Some(user) = config.event_user.take() else {
        return;
    };
    let previous = public_key.and_then(|key| {
        let event = distro::verify(config.event_signed.as_ref()?, key).ok()?;
        let name = distro::verify_user(&user, key, &event.event_id).ok()?.user;
        let label = if event.event.trim().is_empty() {
            event.event_id
        } else {
            event.event.trim().to_owned()
        };
        Some(PreviousUser { name, event: label })
    });
    if previous.is_some() {
        info!(
            ?previous,
            "the new configuration is for another event; keeping the personal name as unconfirmed"
        );
        config.event_user_previous = previous;
    }
}

/// Longest configuration file accepted from a `source`.
const MAX_SOURCE_BYTES: u64 = 64 * 1024;
/// A `source` that does not answer within this does not delay sending any
/// longer (D44).
pub const SOURCE_TIMEOUT: Duration = Duration::from_secs(5);

/// Loads the `LANcast.event` file at `url` (D44); not verified yet.
pub fn fetch_event_file(url: &str) -> Result<EventFile> {
    let text = crate::update::fetch_text(url, SOURCE_TIMEOUT, MAX_SOURCE_BYTES)?;
    toml::from_str(&text).with_context(|| format!("{url} is not a LANcast event configuration"))
}

/// Checks a configuration loaded from a `source` (D44) and returns it as a
/// link, or `None` if it is the one in force already.
/// - The signature must be valid for `public_key`.
/// - It must not be expired; one valid only from a later date is accepted,
///   as for links (D28).
/// - With `stored` (the configuration in force), it must have been signed
///   later (`issued`), so a replayed older file cannot bring back looser
///   settings. A configuration the user confirms in the link dialog passes
///   `None`.
pub fn fetched_link(
    file: &EventFile,
    public_key: &str,
    stored: Option<&SignedEvent>,
    today: &str,
) -> Result<Option<EventLink>> {
    let payload = distro::verify(&file.signed, public_key)?;
    payload.check_not_expired(today)?;
    let defaults = file.defaults()?;
    if let Some(stored) = stored {
        if *stored == file.signed {
            return Ok(None);
        }
        if payload.issued.unwrap_or(0) <= distro::issued(stored) {
            bail!("the event configuration is not newer than the one in force");
        }
    }
    Ok(Some(EventLink {
        event: Some(event_link::clean_text(&payload.event, 64)?),
        enforced: payload.enforced_values()?,
        defaults,
        signed: Some(file.signed.clone()),
        user: None,
    }))
}

/// [`fetched_link`] for this event edition, today.
pub fn fetched_link_for_this_build(
    file: &EventFile,
    stored: Option<&SignedEvent>,
) -> Result<Option<EventLink>> {
    fetched_link(
        file,
        distro::this_build_key()?,
        stored,
        &distro::today_local(),
    )
}

/// Puts the enforced values on top of the user's choices: limits cap them,
/// the others replace them. Returns a description of each change.
pub fn enforce(
    enforced: &EventValues,
    video: &mut VideoOptions,
    audio_gain_db: &mut f32,
) -> Vec<String> {
    let mut changes = Vec::new();
    if let Some(max) = enforced.max_fps
        && video.fps_cap > max
    {
        changes.push(format!("frame rate {max} fps instead of {}", video.fps_cap));
        video.fps_cap = max;
    }
    if let Some((w, h)) = enforced.max_size {
        let capped = (video.max_size.0.min(w), video.max_size.1.min(h));
        if capped != video.max_size {
            changes.push(format!(
                "max. size {}×{} instead of {}×{}",
                capped.0, capped.1, video.max_size.0, video.max_size.1
            ));
            video.max_size = capped;
        }
    }
    if let Some(gain) = enforced.audio_gain_db
        && gain != *audio_gain_db
    {
        changes.push(format!(
            "audio gain {gain} dB instead of {audio_gain_db} dB"
        ));
        *audio_gain_db = gain;
    }
    let on_off = |b: bool| if b { "on" } else { "off" };
    if let Some(border) = enforced.show_border
        && border != video.show_border
    {
        changes.push(format!("capture border {}", on_off(border)));
        video.show_border = border;
    }
    if let Some(cursor) = enforced.capture_cursor
        && cursor != video.capture_cursor
    {
        changes.push(format!("cursor capture {}", on_off(cursor)));
        video.capture_cursor = cursor;
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_cap_and_other_values_replace() {
        let enforced =
            EventValues::from_pairs([("fps", "60"), ("max", "1280x720"), ("border", "0")]).unwrap();
        let mut video = VideoOptions {
            fps_cap: 120,
            max_size: (1920, 1080),
            show_border: true,
            capture_cursor: false,
        };
        let mut gain = 0.0;
        let changes = enforce(&enforced, &mut video, &mut gain);
        assert_eq!((video.fps_cap, video.max_size), (60, (1280, 720)));
        assert!(!video.show_border);
        assert_eq!(changes.len(), 3);
        // Within the limits nothing changes.
        assert!(enforce(&enforced, &mut video, &mut gain).is_empty());
    }

    fn event(secret: &[u8; 32], name: &str, id: &str) -> SignedEvent {
        let payload = distro::EventPayload {
            v: 1,
            event: name.into(),
            event_id: id.into(),
            valid_from: "2026-11-01".into(),
            valid_until: "2026-11-05".into(),
            enforced: Default::default(),
            source: None,
            issued: None,
            refresh_minutes: None,
        };
        distro::sign(&payload, secret).unwrap()
    }

    fn user(secret: &[u8; 32], id: &str, name: &str) -> SignedEvent {
        let payload = distro::UserPayload {
            v: 1,
            event_id: id.into(),
            user: name.into(),
        };
        distro::sign_user(&payload, secret).unwrap()
    }

    #[test]
    fn another_event_keeps_the_personal_name_as_previous() {
        let secret = distro::generate_secret().unwrap();
        let key = distro::public_key(&secret);
        let key = Some(key.as_str());
        let mut config = Config {
            event_signed: Some(event(&secret, "NetGame 2026", "netgame-2026")),
            event_user: Some(user(&secret, "netgame-2026", "Alice")),
            ..Config::default()
        };
        // The same event (e.g. re-signed with new dates): the id stays.
        let same = event(&secret, "NetGame 2026", "netgame-2026");
        switch_event_user_with_key(&mut config, key, Some(&same), None);
        assert!(config.event_user.is_some());
        assert_eq!(config.event_user_previous, None);

        // Another event: the id goes, the name stays, marked.
        let next = event(&secret, "NetGame 2027", "netgame-2027");
        switch_event_user_with_key(&mut config, key, Some(&next), None);
        assert_eq!(config.event_user, None);
        let previous = config.event_user_previous.clone().unwrap();
        assert_eq!(previous.marked(), "Alice [NetGame 2026]");

        // Yet another event without a personal link keeps it.
        config.event_signed = Some(next);
        let later = event(&secret, "", "netgame-2028");
        switch_event_user_with_key(&mut config, key, Some(&later), None);
        assert_eq!(config.event_user_previous, Some(previous));

        // The new event's personal link replaces it.
        let alice = user(&secret, "netgame-2028", "Alice B.");
        switch_event_user_with_key(&mut config, key, Some(&later), Some(&alice));
        assert_eq!(config.event_user, Some(alice));
        assert_eq!(config.event_user_previous, None);

        // Without a name for the event, its id marks the name.
        config.event_signed = Some(later);
        switch_event_user_with_key(&mut config, key, Some(&same), None);
        assert_eq!(
            config.event_user_previous.unwrap().marked(),
            "Alice B. [netgame-2028]"
        );
    }

    fn file(secret: &[u8; 32], until: &str, issued: Option<u64>) -> EventFile {
        let payload = distro::EventPayload {
            v: 1,
            event: "NetGame 2026".into(),
            event_id: "netgame-2026".into(),
            valid_from: "2026-11-01".into(),
            valid_until: until.into(),
            enforced: [("fps".to_owned(), "60".to_owned())].into_iter().collect(),
            source: Some("http://10.0.0.5/LANcast.event".into()),
            issued,
            refresh_minutes: None,
        };
        EventFile {
            signed: distro::sign(&payload, secret).unwrap(),
            defaults: [("level".to_owned(), "unity".to_owned())]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn fetched_configurations_must_be_signed_valid_and_newer() {
        let secret = distro::generate_secret().unwrap();
        let key = distro::public_key(&secret);
        let today = "2026-11-02";
        let first = file(&secret, "2026-11-05", Some(100));
        let link = fetched_link(&first, &key, None, today).unwrap().unwrap();
        assert_eq!(link.enforced.max_fps, Some(60));
        assert_eq!(link.defaults.audio_gain_db, Some(0.0));
        assert_eq!(link.signed.as_ref(), Some(&first.signed));

        // The same file again: nothing to do.
        let stored = Some(&first.signed);
        assert!(fetched_link(&first, &key, stored, today).unwrap().is_none());
        // Signed later: taken. Older or as old (a replay): rejected.
        let newer = file(&secret, "2026-11-06", Some(200));
        assert!(fetched_link(&newer, &key, stored, today).unwrap().is_some());
        let older = file(&secret, "2026-11-06", Some(50));
        assert!(fetched_link(&older, &key, stored, today).is_err());
        let undated = file(&secret, "2026-11-06", None);
        assert!(fetched_link(&undated, &key, stored, today).is_err());
        // Without a configuration in force (link dialog), the age does not matter.
        assert!(fetched_link(&older, &key, None, today).unwrap().is_some());

        // Expired, or signed with another key: rejected.
        let expired = file(&secret, "2026-11-01", Some(300));
        assert!(fetched_link(&expired, &key, stored, today).is_err());
        let other = distro::generate_secret().unwrap();
        let forged = file(&other, "2026-11-06", Some(300));
        assert!(fetched_link(&forged, &key, stored, today).is_err());
        // Not valid yet (downloaded early): accepted, as for links.
        assert!(
            fetched_link(&newer, &key, None, "2026-10-01")
                .unwrap()
                .is_some()
        );
    }
}
