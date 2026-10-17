//! Organizer tool for LANcast event editions (D28): creates the signing key,
//! signs an event description into `LANcast.event` and prints the matching
//! `lancast://event?…` link. Not shipped to participants.
//!
//! ```text
//! lancast-event keygen <secret.key>
//! lancast-event pubkey <secret.key>
//! lancast-event sign <event.toml> <secret.key> <LANcast.event>
//! lancast-event verify <LANcast.event> <public key>
//! ```

use std::{collections::BTreeMap, path::Path, process::ExitCode};

use anyhow::{Context, Result, bail};
use lancast::{
    distro::{self, EventFile, EventPayload},
    event_link::EventValues,
};
use serde::Deserialize;

const USAGE: &str = "\
Usage:
  lancast-event keygen <secret.key>                         new signing key; prints the public key
  lancast-event pubkey <secret.key>                         prints the public key
  lancast-event sign <event.toml> <secret.key> <out.event>  writes LANcast.event, prints the link
  lancast-event verify <file.event> <public key>            checks a LANcast.event

Keep secret.key private and out of the repository and the download.
See docs/EVENT_LINKS.md for the format of event.toml.";

/// The organizer's event description (`event.toml`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventSpec {
    event: String,
    event_id: String,
    valid_from: String,
    valid_until: String,
    #[serde(default)]
    enforced: BTreeMap<String, toml::Value>,
    #[serde(default)]
    defaults: BTreeMap<String, toml::Value>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["keygen", secret] => keygen(Path::new(secret)),
        ["pubkey", secret] => read_secret(Path::new(secret)).map(|s| {
            println!("{}", distro::public_key(&s));
        }),
        ["sign", spec, secret, out] => sign(Path::new(spec), Path::new(secret), Path::new(out)),
        ["verify", file, key] => verify(Path::new(file), key),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(path: &Path) -> Result<()> {
    if path.exists() {
        bail!("{} exists; not overwriting a signing key", path.display());
    }
    let secret = distro::generate_secret()?;
    std::fs::write(path, distro::encode_secret(&secret) + "\n")
        .with_context(|| format!("cannot write {}", path.display()))?;
    eprintln!(
        "Wrote the signing key to {}. Keep it private.",
        path.display()
    );
    println!("{}", distro::public_key(&secret));
    Ok(())
}

fn read_secret(path: &Path) -> Result<[u8; 32]> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    distro::decode_secret(&text)
}

fn sign(spec_path: &Path, secret: &Path, out: &Path) -> Result<()> {
    let text = std::fs::read_to_string(spec_path)
        .with_context(|| format!("cannot read {}", spec_path.display()))?;
    let spec: EventSpec =
        toml::from_str(&text).with_context(|| format!("invalid {}", spec_path.display()))?;
    let secret = read_secret(secret)?;

    let event = lancast::event_link::clean_text(&spec.event, 64)?;
    if event.is_empty() {
        bail!("event must not be empty");
    }
    let valid_id = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    if spec.event_id.is_empty() || !spec.event_id.chars().all(valid_id) {
        bail!("event_id must be letters, digits, - and _ (it names the download files)");
    }
    for date in [&spec.valid_from, &spec.valid_until] {
        check_date(date)?;
    }
    if spec.valid_from > spec.valid_until {
        bail!("valid_from is after valid_until");
    }
    let enforced = text_values(&spec.enforced)?;
    let defaults = text_values(&spec.defaults)?;
    for (section, values) in [("enforced", &enforced), ("defaults", &defaults)] {
        let parsed = EventValues::from_pairs(values.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .with_context(|| format!("invalid [{section}]"))?;
        if parsed.to_pairs().len() != values.len() {
            bail!("[{section}] contains unknown keys (see docs/EVENT_LINKS.md)");
        }
    }

    let payload = EventPayload {
        v: 1,
        event,
        event_id: spec.event_id,
        valid_from: spec.valid_from,
        valid_until: spec.valid_until,
        enforced,
    };
    let file = EventFile {
        signed: distro::sign(&payload, &secret)?,
        defaults,
    };
    distro::write_event_file(out, &file)?;
    // Check what was written, with the public key as an event edition would.
    distro::verify(
        &distro::read_event_file(out)?.signed,
        &distro::public_key(&secret),
    )?;
    eprintln!(
        "Wrote {} for \"{}\" (valid {} to {}).",
        out.display(),
        payload.event,
        payload.valid_from,
        payload.valid_until
    );
    println!("{}", file.link());
    Ok(())
}

fn verify(path: &Path, public_key: &str) -> Result<()> {
    let file = distro::read_event_file(path)?;
    let payload = distro::verify(&file.signed, public_key)?;
    file.defaults()?;
    println!(
        "OK: \"{}\" ({}), valid {} to {}",
        payload.event, payload.event_id, payload.valid_from, payload.valid_until
    );
    println!("enforced: {:?}", payload.enforced);
    println!("defaults: {:?}", file.defaults);
    match payload.check_dates(&distro::today_local()) {
        Ok(()) => println!("valid today"),
        Err(err) => println!("not valid today: {err:#}"),
    }
    Ok(())
}

fn check_date(date: &str) -> Result<()> {
    let parts: Vec<&str> = date.split('-').collect();
    let ok = matches!(parts.as_slice(), [y, m, d]
        if y.len() == 4 && m.len() == 2 && d.len() == 2
            && date.chars().all(|c| c.is_ascii_digit() || c == '-')
            && (1..=12).contains(&m.parse::<u32>().unwrap_or(0))
            && (1..=31).contains(&d.parse::<u32>().unwrap_or(0)));
    if !ok {
        bail!("dates must look like 2026-11-01, not {date:?}");
    }
    Ok(())
}

/// TOML values (`fps = 60`, `border = false`) in their link text form.
fn text_values(values: &BTreeMap<String, toml::Value>) -> Result<BTreeMap<String, String>> {
    values
        .iter()
        .map(|(key, value)| {
            let text = match value {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Boolean(b) => u8::from(*b).to_string(),
                other => bail!("{key}: unsupported value {other}"),
            };
            Ok((key.to_ascii_lowercase(), text))
        })
        .collect()
}
