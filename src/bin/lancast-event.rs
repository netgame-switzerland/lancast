//! Organizer tool for LANcast event editions (D28): creates the signing key,
//! signs an event description into `LANcast.event` and prints the matching
//! `lancast://event?…` link, and personal links with a signed user name
//! (D38). The release workflow also uses it to write the signed update
//! manifest (D37). Not shipped to participants.
//!
//! ```text
//! lancast-event keygen <secret.key>
//! lancast-event pubkey <secret.key>
//! lancast-event sign <event.toml> <secret.key> <LANcast.event>
//! lancast-event verify <LANcast.event> <public key>
//! lancast-event user <LANcast.event> <name> <secret.key>
//! lancast-event update-manifest <dist> <version> <notes url> <secret.key> <out.json>
//! ```

use std::{collections::BTreeMap, path::Path, process::ExitCode};

use anyhow::{Context, Result, bail};
use lancast::{
    distro::{self, EventFile, EventPayload, UserPayload},
    event_link::EventValues,
    update::{self, Asset, EditionFiles, Manifest},
};
use serde::Deserialize;

const USAGE: &str = "\
Usage:
  lancast-event keygen <secret.key>                         new signing key; prints the public key
  lancast-event pubkey <secret.key>                         prints the public key
  lancast-event sign <event.toml> <secret.key> <out.event>  writes LANcast.event, prints the link
  lancast-event verify <file.event> <public key>            checks a LANcast.event
  lancast-event user <file.event> <name> <secret.key>       prints the personal link for <name>
  lancast-event update-manifest <dist> <version> <notes url> <secret.key> <out.json>
                                                            signed update manifest of a release

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
        ["user", file, name, secret] => user(Path::new(file), name, Path::new(secret)),
        ["update-manifest", dist, version, notes, secret, out] => update_manifest(
            Path::new(dist),
            version,
            notes,
            Path::new(secret),
            Path::new(out),
        ),
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

/// Prints the personal link (D38) of the event in `path` for `name`.
fn user(path: &Path, name: &str, secret: &Path) -> Result<()> {
    let file = distro::read_event_file(path)?;
    let secret = read_secret(secret)?;
    let public_key = distro::public_key(&secret);
    let event = distro::verify(&file.signed, &public_key)
        .with_context(|| format!("{} is not signed with this key", path.display()))?;
    let payload = UserPayload {
        v: 1,
        event_id: event.event_id.clone(),
        user: name.trim().to_owned(),
    };
    let id = distro::sign_user(&payload, &secret)?;
    distro::verify_user(&id, &public_key, &event.event_id)?;
    eprintln!(
        "Personal link of \"{}\" for \"{}\" ({}):",
        payload.user, event.event, event.event_id
    );
    println!("{}", file.personal_link(&id));
    Ok(())
}

/// Writes the signed update manifest (D37) for the packages of `version` in
/// `dist`: `LANcast-<version>-setup.exe` / `-win64.zip` is the standard
/// edition, `LANcast-<version>-<edition>-setup.exe` / `-win64.zip` an
/// organizer edition.
fn update_manifest(
    dist: &Path,
    version: &str,
    notes_url: &str,
    secret: &Path,
    out: &Path,
) -> Result<()> {
    let prefix = format!("LANcast-{version}-");
    let mut editions: BTreeMap<String, EditionFiles> = BTreeMap::new();
    let entries =
        std::fs::read_dir(dist).with_context(|| format!("cannot read {}", dist.display()))?;
    for entry in entries {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let (edition, setup) = if let Some(edition) = rest.strip_suffix("setup.exe") {
            (edition, true)
        } else if let Some(edition) = rest.strip_suffix("win64.zip") {
            (edition, false)
        } else {
            continue;
        };
        let edition = match edition.strip_suffix('-') {
            Some(edition) => edition,
            None if edition.is_empty() => update::STANDARD_EDITION,
            None => continue,
        };
        let asset = Asset {
            file: name.to_owned(),
            sha256: update::file_sha256(&path)?,
            size: std::fs::metadata(&path)?.len(),
        };
        let files = editions.entry(edition.to_owned()).or_default();
        if setup {
            files.setup = Some(asset);
        } else {
            files.zip = Some(asset);
        }
    }
    if !editions.contains_key(update::STANDARD_EDITION) {
        bail!(
            "no {prefix}setup.exe or {prefix}win64.zip in {}",
            dist.display()
        );
    }
    let manifest = Manifest {
        v: 1,
        product: "LANcast".into(),
        version: version.to_owned(),
        notes_url: notes_url.to_owned(),
        manual: String::new(),
        editions,
    };
    let secret = read_secret(secret)?;
    let signed = update::sign_manifest(&manifest, &secret)?;
    let text = serde_json::to_string_pretty(&signed)?;
    // Check what is written, with the public key as LANcast would.
    update::verify_manifest(&text, &distro::public_key(&secret))?;
    std::fs::write(
        out,
        text + "
",
    )
    .with_context(|| format!("cannot write {}", out.display()))?;
    eprintln!("Wrote {} for {version}:", out.display());
    for (edition, files) in &manifest.editions {
        for asset in [&files.setup, &files.zip].into_iter().flatten() {
            eprintln!("  {edition}: {} ({} bytes)", asset.file, asset.size);
        }
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
