//! Win32 backend of [`super`]: HTTP(S) with the Windows TLS stack and
//! certificate store, the setup's uninstall entry, and starting the setup.

use std::{io::Read, path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
        },
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    },
    core::{HSTRING, w},
};

use super::InstallMode;

/// Uninstall entry written by Inno Setup for LANcast's fixed AppId (D25).
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{F98C4151-0F3E-495C-B8D3-B823EE2D71F2}_is1";

/// A GET request for `url`; the reader streams the body. `timeout` covers
/// the whole request including the body.
pub fn open(url: &str, timeout: Duration) -> Result<Box<dyn Read + Send>> {
    // native-tls is SChannel on Windows: the system's TLS and certificate
    // store, so a proxy's or company's root certificates work as in a browser.
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(timeout))
        .user_agent(format!("LANcast/{}", crate::VERSION))
        .build()
        .into();
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("cannot fetch {url}"))?;
    Ok(Box::new(response.into_body().into_reader()))
}

pub fn install_mode() -> InstallMode {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return InstallMode::Portable;
    };
    for (root, mode) in [
        (HKEY_LOCAL_MACHINE, InstallMode::AllUsers),
        (HKEY_CURRENT_USER, InstallMode::CurrentUser),
    ] {
        if let Some(location) = read_string(root, UNINSTALL_KEY, "InstallLocation")
            && same_dir(Path::new(&location), &dir)
        {
            return mode;
        }
    }
    InstallMode::Portable
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn read_string(root: HKEY, subkey: &str, name: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let name = HSTRING::from(name);
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: buffer and size describe the same writable memory.
    let status = unsafe {
        RegGetValueW(
            root,
            &subkey,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// Starts `setup` with `params` through the shell, which shows the UAC
/// prompt when the setup needs admin rights (CreateProcess would fail).
pub fn run(setup: &Path, params: &str) -> Result<()> {
    let file = HSTRING::from(setup.as_os_str());
    let params = HSTRING::from(params);
    // SAFETY: all strings are NUL-terminated HSTRINGs that outlive the call.
    let result = unsafe { ShellExecuteW(None, w!("open"), &file, &params, None, SW_SHOWNORMAL) };
    // Values above 32 mean success (ShellExecuteW documentation).
    let code = result.0 as isize;
    if code <= 32 {
        bail!("cannot start the setup (error {code})");
    }
    Ok(())
}
