//! The event in force (D27, D28, D38) and its enforced values, computed from
//! the saved settings. Shared by the GUI and the CLI, so both obey the same
//! event (D43).

use tracing::{info, warn};

use crate::{
    capture::video::VideoOptions,
    config::Config,
    distro,
    event_link::{EventLink, EventValues},
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
        let user = config.event_user.as_ref().and_then(|user| {
            let event = distro::verify_signature_for_this_build(signed).ok()?;
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
            },
            Err(err) => Self {
                event: None,
                error: Some(format!("{err:#}.")),
                user,
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
}
