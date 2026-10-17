//! Registration of the `lancast:` URL scheme for event links (D27).
//!
//! The installer registers it (for all users or per user). The ZIP/portable
//! version registers itself per user (HKCU, no admin rights) at startup if
//! no working registration exists.

use std::path::Path;

use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegGetValueW,
            RegSetKeyValueW,
        },
    },
    core::{HSTRING, PCWSTR},
};

use crate::event_link::SCHEME;

/// Where `lancast:` links currently lead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemeStatus {
    /// They open this LANcast.
    ThisCopy,
    /// They open another LANcast (e.g. an installed or older copy).
    OtherCopy(String),
    /// Not registered, or the registered program no longer exists.
    Missing,
}

pub fn status() -> SchemeStatus {
    let (Some(command), Ok(this)) = (registered_command(), std::env::current_exe()) else {
        return SchemeStatus::Missing;
    };
    let target = command_target(&command);
    if !Path::new(&target).is_file() {
        SchemeStatus::Missing
    } else if same_path(Path::new(&target), &this) {
        SchemeStatus::ThisCopy
    } else {
        SchemeStatus::OtherCopy(target)
    }
}

/// Registers `lancast:` for the current user unless a registration exists
/// whose program is still present (e.g. after the folder was moved).
pub fn ensure_registered() {
    if status() == SchemeStatus::Missing {
        register_this_copy();
    }
}

/// Makes `lancast:` links open this copy, for the current user (HKCU wins
/// over an all-users registration). Returns whether it worked.
pub fn register_this_copy() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.display().to_string();
    let base = format!(r"Software\Classes\{SCHEME}");
    let command = format!("\"{exe}\" \"%1\"");
    let ok = set(&base, "", "URL:LANcast event link")
        && set(&base, "URL Protocol", "")
        && set(&format!(r"{base}\DefaultIcon"), "", &format!("\"{exe}\",0"))
        && set(&format!(r"{base}\shell\open\command"), "", &command);
    if ok {
        info!(%exe, "registered lancast: links for this user");
    } else {
        warn!("failed to register lancast: links");
    }
    ok
}

/// The open command currently registered (HKCU or HKLM, merged view).
fn registered_command() -> Option<String> {
    let subkey = HSTRING::from(format!(r"{SCHEME}\shell\open\command"));
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: buffer and size describe the same writable memory.
    let status = unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            &subkey,
            PCWSTR::null(),
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

/// The exe of a `"C:\path\LANcast.exe" "%1"` command.
fn command_target(command: &str) -> String {
    command
        .trim()
        .strip_prefix('"')
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| command.split_whitespace().next().unwrap_or(""))
        .to_owned()
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}

/// Writes a string value under HKCU.
fn set(subkey: &str, name: &str, value: &str) -> bool {
    let subkey = HSTRING::from(subkey);
    let name = HSTRING::from(name);
    let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `data` is a NUL-terminated UTF-16 buffer of the size passed.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &subkey,
            &name,
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    status == ERROR_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parsing() {
        assert_eq!(
            command_target(r#""C:\Program Files\LANcast\LANcast.exe" "%1""#),
            r"C:\Program Files\LANcast\LANcast.exe"
        );
        assert_eq!(command_target(r"C:\LANcast.exe %1"), r"C:\LANcast.exe");
    }
}
