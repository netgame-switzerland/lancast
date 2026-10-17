//! NDI network configuration for LANcast (SPEC §5.4, D10, D14, D23).
//!
//! The standard NDI SDK reads `ndi-config.v1.json` from the folder in
//! `NDI_CONFIG_DIR` when it initializes, else from `C:\ProgramData\NDI\`
//! (written by NDI Access Manager). LANcast sets `NDI_CONFIG_DIR` once at
//! startup to its own folder and, before each send session, writes a copy of
//! the system file there with only the discovery server adjusted. The system
//! file is never modified.

use std::{
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use tracing::{info, warn};

const FILE_NAME: &str = "ndi-config.v1.json";
/// Default port of the NDI Discovery Service.
pub const DISCOVERY_PORT: u16 = 5959;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);

/// Which discovery server(s) LANcast's NDI sender uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    /// Whatever NDI Access Manager configured (often none, i.e. mDNS).
    System,
    /// No discovery server: announce via mDNS even if the system has one.
    Mdns,
    /// These servers (comma-separated `host[:port]`).
    Servers(String),
}

/// `C:\ProgramData\NDI\ndi-config.v1.json`.
fn system_file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|dir| PathBuf::from(dir).join("NDI").join(FILE_NAME))
}

/// `ndi\` in LANcast's data folder.
fn own_dir() -> Option<PathBuf> {
    Some(crate::config::data_dir().join("ndi"))
}

/// Points the NDI SDK at LANcast's own config folder and writes a copy of
/// the system config there.
///
/// # Safety
/// Sets an environment variable, which is only sound while no other thread
/// runs: call this first thing in `main`.
pub unsafe fn init_before_threads() {
    let Some(dir) = own_dir() else {
        warn!("no data folder; NDI uses the system configuration");
        return;
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        warn!(
            "cannot create {}: {err}; NDI uses the system configuration",
            dir.display()
        );
        return;
    }
    // SAFETY: the caller guarantees that no other thread exists yet.
    unsafe { std::env::set_var("NDI_CONFIG_DIR", &dir) };
    if let Err(err) = write_effective(&Discovery::System) {
        warn!("failed to write the NDI configuration: {err:#}");
    }
}

/// The discovery server(s) set in NDI Access Manager, if any.
pub fn system_discovery() -> Option<String> {
    let value = read_json(&system_file()?)?;
    let servers = value.pointer("/ndi/networks/discovery")?.as_str()?.trim();
    (!servers.is_empty()).then(|| servers.to_owned())
}

/// Writes LANcast's `ndi-config.v1.json` for the next NDI initialization:
/// the system file with `ndi.networks.discovery` set according to `discovery`.
pub fn write_effective(discovery: &Discovery) -> Result<()> {
    let dir = own_dir().context("no data folder")?;
    let mut config = system_file()
        .and_then(|path| read_json(&path))
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "ndi": {} }));
    let servers = match discovery {
        Discovery::System => None,
        Discovery::Mdns => Some(String::new()),
        Discovery::Servers(servers) => Some(servers.trim().to_owned()),
    };
    if let Some(servers) = servers {
        let networks = object_at(&mut config, "ndi").and_then(|ndi| {
            ndi.entry("networks")
                .or_insert_with(|| json!({}))
                .as_object_mut()
        });
        match networks {
            Some(networks) => {
                networks.insert("discovery".into(), Value::String(servers));
            }
            None => warn!("unexpected NDI config layout; discovery setting not applied"),
        }
    }
    let text = serde_json::to_string_pretty(&config)?;
    std::fs::write(dir.join(FILE_NAME), text).context("cannot write the NDI configuration")?;
    info!(?discovery, "NDI configuration written");
    Ok(())
}

/// Checks each comma-separated `host[:port]` with a TCP connect. Returns
/// `(server, reachable)` pairs. Blocks for up to about 1.5 s per server.
pub fn check_reachable(servers: &str) -> Vec<(String, bool)> {
    servers
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|server| {
            let address = if server.contains(':') {
                server.to_owned()
            } else {
                format!("{server}:{DISCOVERY_PORT}")
            };
            let reachable = address
                .to_socket_addrs()
                .ok()
                .into_iter()
                .flatten()
                .any(|addr| TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok());
            (server.to_owned(), reachable)
        })
        .collect()
}

fn read_json(path: &PathBuf) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn object_at<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Map<String, Value>> {
    value
        .as_object_mut()?
        .entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
}
