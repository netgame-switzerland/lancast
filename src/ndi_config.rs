//! NDI network configuration for LANcast (SPEC §5.4, D10, D14, D23).
//!
//! The standard NDI SDK reads `ndi-config.v1.json` from the folder in
//! `NDI_CONFIG_DIR`, else from `C:\ProgramData\NDI\` (written by NDI Access
//! Manager). It reads it **once per process, when its DLL loads** (D41).
//! LANcast delay-loads the DLL, sets `NDI_CONFIG_DIR` at startup to its own
//! folder and, until the first send loads the DLL, writes a copy of the
//! system file there with only the discovery server adjusted. A later change
//! needs a restart. The system file is never modified.

use std::{
    net::{Ipv4Addr, TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use tracing::{info, warn};

const FILE_NAME: &str = "ndi-config.v1.json";
/// Default port of the NDI Discovery Service.
pub const DISCOVERY_PORT: u16 = 5959;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// The NDI runtime DLL, delay-loaded at the first send (`build.rs`).
const NDI_DLL: &str = "Processing.NDI.Lib.x64.dll";

/// Discovery servers in the file LANcast last wrote (`""` = mDNS); `None`
/// if it wrote none, so NDI uses the system file.
static WRITTEN: Mutex<Option<String>> = Mutex::new(None);

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

/// The discovery servers `discovery` amounts to; `""` = mDNS.
pub fn resolve(discovery: &Discovery) -> String {
    match discovery {
        Discovery::System => system_discovery().unwrap_or_default(),
        Discovery::Mdns => String::new(),
        Discovery::Servers(servers) => servers.trim().to_owned(),
    }
}

/// Whether the NDI runtime is loaded, i.e. has read its configuration.
pub fn ndi_loaded() -> bool {
    crate::process_info::module_loaded(NDI_DLL)
}

/// The discovery servers NDI uses in this process (`""` = mDNS), once the
/// runtime is loaded; `None` before.
pub fn in_use() -> Option<String> {
    if !ndi_loaded() {
        return None;
    }
    let written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Some(written.unwrap_or_else(|| resolve(&Discovery::System)))
}

/// Prepares the configuration for the next send: writes it while the NDI
/// runtime is not loaded yet; afterwards NDI would ignore it (D41).
pub fn apply(discovery: &Discovery) -> Result<()> {
    if ndi_loaded() {
        let wanted = resolve(discovery);
        if in_use().is_some_and(|used| used != wanted) {
            warn!(%wanted, "the discovery setting applies only after a restart");
        }
        return Ok(());
    }
    write_effective(discovery)
}

/// Writes LANcast's `ndi-config.v1.json` for the NDI runtime: the system
/// file with `ndi.networks.discovery` set according to `discovery`.
fn write_effective(discovery: &Discovery) -> Result<()> {
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
    *WRITTEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(resolve(discovery));
    info!(?discovery, "NDI configuration written");
    Ok(())
}

/// Whether this process has a TCP connection to each comma-separated
/// `host[:port]`. Returns `(server, connected)` pairs. A host name is
/// matched by port only (no DNS lookup); IPv6 connections are not seen.
pub fn connections(servers: &str) -> Vec<(String, bool)> {
    let open = crate::process_info::own_tcp_connections();
    servers
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|server| {
            let (host, port) = split_host_port(server);
            let connected = open
                .iter()
                .any(|remote| remote.port() == port && host.is_none_or(|ip| *remote.ip() == ip));
            (server.to_owned(), connected)
        })
        .collect()
}

/// IPv4 address (if the host is one) and port of `host[:port]`.
fn split_host_port(server: &str) -> (Option<Ipv4Addr>, u16) {
    let (host, port) = match server.rsplit_once(':') {
        Some((host, port)) => match port.parse() {
            Ok(port) => (host, port),
            Err(_) => (server, DISCOVERY_PORT),
        },
        None => (server, DISCOVERY_PORT),
    };
    (host.parse().ok(), port)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_port() {
        let ip = Some(Ipv4Addr::new(10, 0, 0, 5));
        assert_eq!(split_host_port("10.0.0.5"), (ip, DISCOVERY_PORT));
        assert_eq!(split_host_port("10.0.0.5:6000"), (ip, 6000));
        assert_eq!(split_host_port("ndi.lan:6000"), (None, 6000));
        assert_eq!(split_host_port("ndi.lan"), (None, DISCOVERY_PORT));
    }
}
