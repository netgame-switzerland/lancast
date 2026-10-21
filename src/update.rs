//! Update check (D37).
//!
//! Every release publishes a **signed manifest** (`LANcast-update.json`)
//! next to its packages. It names the version, the release notes and, per
//! edition, the setup and the ZIP with their SHA-256. LANcast fetches it from
//! the configured URL (default: the latest GitHub release), checks the
//! Ed25519 signature against the release key compiled into the exe and
//! offers an update only for its own edition and only if it is newer.
//!
//! Package file names in the manifest are resolved relative to the manifest
//! URL, so a mirror (e.g. a web server on the event LAN) only needs the
//! manifest and the files in one folder. The signature and the hashes make
//! any mirror as trustworthy as GitHub.
//!
//! An installed LANcast updates by running the new setup silently (it keeps
//! the install mode and tasks, closes LANcast and starts it again); a
//! portable copy only links to the download page.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::distro;

#[cfg(windows)]
mod win32;
#[cfg(windows)]
use win32 as backend;

/// Public key of the LANcast release key (base64url); the private key is
/// the GitHub secret `UPDATE_SIGNING_KEY` of the Release workflow. A build
/// can use another key with `LANCAST_UPDATE_KEY` (forks, tests).
pub const UPDATE_KEY: &str = match option_env!("LANCAST_UPDATE_KEY") {
    Some(key) => key,
    None => "8yAlO4t7q9EKYd64fYmPKjsbLzFBYG-y7pOcD2lRmWg",
};
/// Where LANcast looks for updates unless `update_url` is set.
pub const DEFAULT_URL: &str =
    "https://github.com/netgame-switzerland/lancast/releases/latest/download/LANcast-update.json";
/// File name of the manifest in a release.
pub const MANIFEST_FILE: &str = "LANcast-update.json";
/// Edition name of the normal LANcast in the manifest.
pub const STANDARD_EDITION: &str = "standard";
/// Set by `tools\package.ps1`: the organizer edition's name in the manifest,
/// or empty for an event edition, which gets no updates (its setup bundles
/// the event's configuration and is not released on GitHub).
const EDITION_OVERRIDE: Option<&str> = option_env!("LANCAST_UPDATE_EDITION");
const MANIFEST_VERSION: u32 = 1;
const PRODUCT: &str = "LANcast";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// This build's edition in the manifest; `None` = no update check.
pub fn edition() -> Option<&'static str> {
    match EDITION_OVERRIDE {
        Some("") => None,
        Some(edition) => Some(edition),
        None if distro::is_distro() => None,
        None => Some(STANDARD_EDITION),
    }
}

/// The URL to check: the configured one, or the default.
pub fn effective_url(configured: &str) -> &str {
    match configured.trim() {
        "" => DEFAULT_URL,
        url => url,
    }
}

/// The manifest file as published: payload and signature, both base64url.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedManifest {
    pub payload: String,
    pub signature: String,
}

/// What the release key signs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub v: u32,
    pub product: String,
    pub version: String,
    pub notes_url: String,
    /// Set when this release cannot be installed by the update: why, and
    /// what to do instead (shown with a download link).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manual: String,
    pub editions: BTreeMap<String, EditionFiles>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EditionFiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<Asset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip: Option<Asset>,
}

/// A package file, relative to the manifest URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub file: String,
    /// Lowercase hex.
    pub sha256: String,
    pub size: u64,
}

/// A newer release for this edition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub notes_url: String,
    pub manual: Option<String>,
    pub setup: Option<Asset>,
    /// The manifest URL the files are relative to.
    base_url: String,
}

impl Update {
    pub fn asset_url(&self, asset: &Asset) -> String {
        asset_url(&self.base_url, &asset.file)
    }
}

/// `file` next to the manifest: the URL up to its last `/` plus `file`.
fn asset_url(manifest_url: &str, file: &str) -> String {
    let base = manifest_url
        .split(['?', '#'])
        .next()
        .unwrap_or(manifest_url);
    match base.rfind('/') {
        Some(i) => format!("{}{file}", &base[..=i]),
        None => file.to_owned(),
    }
}

/// Signs `manifest` with a 32-byte Ed25519 secret key (release workflow).
pub fn sign_manifest(manifest: &Manifest, secret: &[u8; 32]) -> Result<SignedManifest> {
    check_manifest(manifest)?;
    let bytes = serde_json::to_vec(manifest)?;
    let signature = SigningKey::from_bytes(secret).sign(&bytes);
    Ok(SignedManifest {
        payload: URL_SAFE_NO_PAD.encode(&bytes),
        signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
}

/// Parses a manifest file and verifies it with `public_key` (base64url).
pub fn verify_manifest(text: &str, public_key: &str) -> Result<Manifest> {
    let signed: SignedManifest =
        serde_json::from_str(text).context("not a LANcast update manifest")?;
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(public_key.trim())
        .ok()
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| anyhow!("invalid update key"))?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| anyhow!("invalid update key"))?;
    let signature: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signed.signature.trim())
        .ok()
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| anyhow!("invalid manifest signature encoding"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(signed.payload.trim())
        .context("invalid manifest payload encoding")?;
    key.verify(&payload, &Signature::from_bytes(&signature))
        .map_err(|_| anyhow!("the update manifest is not signed by the LANcast release key"))?;
    let manifest: Manifest =
        serde_json::from_slice(&payload).context("invalid update manifest payload")?;
    check_manifest(&manifest)?;
    Ok(manifest)
}

fn check_manifest(manifest: &Manifest) -> Result<()> {
    if manifest.v != MANIFEST_VERSION {
        bail!("unsupported update manifest version {}", manifest.v);
    }
    if manifest.product != PRODUCT {
        bail!("the manifest is for {:?}, not LANcast", manifest.product);
    }
    semver::Version::parse(&manifest.version)
        .with_context(|| format!("invalid version {:?} in the manifest", manifest.version))?;
    let assets = manifest
        .editions
        .values()
        .flat_map(|files| [&files.setup, &files.zip])
        .flatten();
    for asset in assets {
        // A plain file name: it is joined to the manifest URL and to the
        // download folder.
        let plain = !asset.file.is_empty()
            && asset
                .file
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            && !asset.file.starts_with('.');
        if !plain {
            bail!("invalid file name {:?} in the manifest", asset.file);
        }
        if asset.sha256.len() != 64 || !asset.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("invalid SHA-256 for {} in the manifest", asset.file);
        }
    }
    Ok(())
}

/// The update `manifest` offers to `current` (this version) of `edition`,
/// if it is newer. Build metadata (`+snapshot…`) does not count, so a
/// snapshot of a release is not offered that release.
fn newer(manifest: Manifest, current: &str, edition: &str, url: &str) -> Result<Option<Update>> {
    let current = semver::Version::parse(current)
        .with_context(|| format!("invalid own version {current:?}"))?;
    let offered = semver::Version::parse(&manifest.version)?;
    if offered.cmp_precedence(&current).is_le() {
        return Ok(None);
    }
    let Some(files) = manifest.editions.get(edition) else {
        info!(
            version = manifest.version,
            edition, "a newer release exists, but not for this edition"
        );
        return Ok(None);
    };
    Ok(Some(Update {
        version: manifest.version,
        notes_url: manifest.notes_url,
        manual: Some(manifest.manual).filter(|m| !m.is_empty()),
        setup: files.setup.clone(),
        base_url: url.to_owned(),
    }))
}

/// Fetches and verifies the manifest at `url`; `Ok(None)` = up to date.
pub fn check(url: &str) -> Result<Option<Update>> {
    let edition = edition().ok_or_else(|| anyhow!("this edition is not updated online"))?;
    let mut text = String::new();
    backend::open(url, CHECK_TIMEOUT)?
        .take(MAX_MANIFEST_BYTES)
        .read_to_string(&mut text)
        .with_context(|| format!("cannot read {url}"))?;
    let manifest = verify_manifest(&text, UPDATE_KEY)?;
    newer(manifest, crate::VERSION, edition, url)
}

/// Folder for downloaded setups (in the data folder, per user).
pub fn download_dir() -> PathBuf {
    crate::config::data_dir().join("updates")
}

/// Downloads `asset` into `dir` (emptied first) and checks its size and
/// SHA-256. Returns the file's path.
pub fn download(update: &Update, asset: &Asset, dir: &Path) -> Result<PathBuf> {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let url = update.asset_url(asset);
    let part = dir.join(format!("{}.part", asset.file));
    let mut reader = backend::open(&url, DOWNLOAD_TIMEOUT)?.take(asset.size + 1);
    let mut out =
        fs::File::create(&part).with_context(|| format!("cannot write {}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buffer)
            .with_context(|| format!("download of {url} failed"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        out.write_all(&buffer[..n])?;
        size += n as u64;
    }
    out.sync_all()?;
    drop(out);
    let hash = hex(&hasher.finalize());
    if size != asset.size || !hash.eq_ignore_ascii_case(&asset.sha256) {
        let _ = fs::remove_file(&part);
        bail!("the downloaded {} does not match the manifest", asset.file);
    }
    let path = dir.join(&asset.file);
    fs::rename(&part, &path)?;
    info!(path = %path.display(), "update downloaded and verified");
    Ok(path)
}

/// SHA-256 of a file, lowercase hex (release workflow).
pub fn file_sha256(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How this copy of LANcast was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// By the setup, for all users (`Program Files`, needs admin rights).
    AllUsers,
    /// By the setup, for the current user only.
    CurrentUser,
    /// From the ZIP, or a build: no setup to run.
    Portable,
}

/// Whether this exe is the one the setup installed, and how.
pub fn install_mode() -> InstallMode {
    backend::install_mode()
}

/// Starts the downloaded setup silently in `mode`; it closes LANcast,
/// installs, and starts LANcast again. The caller quits right after.
pub fn launch_setup(setup: &Path, mode: InstallMode) -> Result<()> {
    let scope = match mode {
        InstallMode::AllUsers => "/ALLUSERS",
        InstallMode::CurrentUser => "/CURRENTUSER",
        InstallMode::Portable => bail!("this LANcast was not installed by the setup"),
    };
    // /LANCASTSTART=1 makes the setup start LANcast afterwards (LANcast.iss).
    let params =
        format!("/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS {scope} /LANCASTSTART=1");
    info!(setup = %setup.display(), params, "starting the update setup");
    backend::run(setup, &params)
}

/// Not implemented on other platforms (D32).
#[cfg(not(windows))]
mod backend {
    use std::{io::Read, path::Path, time::Duration};

    use anyhow::{Result, bail};

    use super::InstallMode;

    pub fn open(_url: &str, _timeout: Duration) -> Result<Box<dyn Read + Send>> {
        bail!("update checks are not supported on this platform")
    }

    pub fn install_mode() -> InstallMode {
        InstallMode::Portable
    }

    pub fn run(_setup: &Path, _params: &str) -> Result<()> {
        bail!("updates are not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str) -> Manifest {
        let asset = |file: &str| Asset {
            file: file.into(),
            sha256: "ab".repeat(32),
            size: 10,
        };
        Manifest {
            v: 1,
            product: "LANcast".into(),
            version: version.into(),
            notes_url: "https://example.org/notes".into(),
            manual: String::new(),
            editions: BTreeMap::from([
                (
                    "standard".into(),
                    EditionFiles {
                        setup: Some(asset(&format!("LANcast-{version}-setup.exe"))),
                        zip: Some(asset(&format!("LANcast-{version}-win64.zip"))),
                    },
                ),
                (
                    "netgame".into(),
                    EditionFiles {
                        setup: Some(asset(&format!("LANcast-{version}-netgame-setup.exe"))),
                        zip: None,
                    },
                ),
            ]),
        }
    }

    const URL: &str = "https://example.org/releases/latest/download/LANcast-update.json";

    #[test]
    fn sign_and_verify() {
        let secret = [7u8; 32];
        let key = distro::public_key(&secret);
        let signed = sign_manifest(&manifest("1.2.0"), &secret).unwrap();
        let text = serde_json::to_string(&signed).unwrap();
        assert_eq!(verify_manifest(&text, &key).unwrap(), manifest("1.2.0"));

        let other = distro::public_key(&[8u8; 32]);
        assert!(verify_manifest(&text, &other).is_err());
        let mut forged = signed.clone();
        let mut changed = manifest("9.0.0");
        changed.notes_url = "https://evil.example/".into();
        forged.payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&changed).unwrap());
        let text = serde_json::to_string(&forged).unwrap();
        assert!(verify_manifest(&text, &key).is_err());
    }

    #[test]
    fn rejects_paths_in_file_names() {
        let mut bad = manifest("1.2.0");
        bad.editions
            .get_mut("standard")
            .unwrap()
            .setup
            .as_mut()
            .unwrap()
            .file = "../LANcast.exe".into();
        assert!(sign_manifest(&bad, &[7u8; 32]).is_err());
    }

    #[test]
    fn offers_only_newer_versions_of_this_edition() {
        let offer = |version: &str, current: &str, edition: &str| {
            newer(manifest(version), current, edition, URL)
                .unwrap()
                .map(|u| u.version)
        };
        assert_eq!(
            offer("1.2.0", "1.1.0", "standard").as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            offer("1.0.0", "1.0.0-rc.2", "standard").as_deref(),
            Some("1.0.0")
        );
        assert_eq!(offer("1.2.0", "1.2.0", "standard"), None);
        assert_eq!(offer("1.1.0", "1.2.0", "standard"), None);
        // A snapshot of the release itself is not offered the release.
        assert_eq!(
            offer("1.2.0", "1.2.0+snapshot.g1a2b3c4.dirty", "standard"),
            None
        );
        assert_eq!(offer("1.2.0", "1.1.0", "netgame").as_deref(), Some("1.2.0"));
        assert_eq!(offer("1.2.0", "1.1.0", "other"), None);
    }

    #[test]
    fn files_are_relative_to_the_manifest() {
        let update = newer(manifest("1.2.0"), "1.1.0", "netgame", URL)
            .unwrap()
            .unwrap();
        assert_eq!(
            update.asset_url(update.setup.as_ref().unwrap()),
            "https://example.org/releases/latest/download/LANcast-1.2.0-netgame-setup.exe"
        );
        assert_eq!(
            asset_url(
                "http://10.0.0.5:8080/lancast/LANcast-update.json?x=1",
                "a.exe"
            ),
            "http://10.0.0.5:8080/lancast/a.exe"
        );
    }

    #[test]
    fn default_url_when_empty() {
        assert_eq!(effective_url("  "), DEFAULT_URL);
        assert_eq!(effective_url(" http://x/m.json "), "http://x/m.json");
    }
}
