//! Whether Windows shows LANcast's tray icon (D29).
//!
//! Windows 11 puts new notification-area icons into the overflow and keeps
//! the user's choice per executable path under
//! `HKCU\Control Panel\NotifyIconSettings\<id>` (`ExecutablePath`,
//! `IsPromoted`). The overflow menu (^) can be turned off as well
//! (`SystemTrayChevronVisibility` under `TrayNotify`); then an icon in the
//! overflow cannot be reached at all. Both values are undocumented, so they
//! are only read, never written: the user decides which icons are shown.
//! `Shell_NotifyIconGetRect` cannot tell these cases apart (D29).

use std::path::{Path, PathBuf};

use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::{
            Com::CoTaskMemFree,
            Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey,
                RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
            },
        },
        UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    },
    core::{GUID, HSTRING, PCWSTR, PWSTR, w},
};

/// Where the user can find this exe's tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPlacement {
    /// On the taskbar, or Windows does not record it (Windows 10, which
    /// always shows the overflow arrow).
    Shown,
    /// In the overflow, reachable with the ^ arrow.
    Overflow,
    /// In the overflow while the overflow menu is turned off: not reachable.
    Hidden,
}

/// Whether the user can reach the tray icon (on the taskbar or in the
/// overflow).
pub fn icon_visible() -> bool {
    icon_placement() != IconPlacement::Hidden
}

pub fn icon_placement() -> IconPlacement {
    if promoted() {
        return IconPlacement::Shown;
    }
    let chevron = Key::open(w!(
        r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\TrayNotify"
    ))
    .and_then(|key| key.dword("", w!("SystemTrayChevronVisibility")));
    // A missing value counts as on: the overflow arrow is the default.
    if chevron == Some(0) {
        IconPlacement::Hidden
    } else {
        IconPlacement::Overflow
    }
}

/// Whether this exe's icon is on the taskbar; `true` where Windows does not
/// record it (Windows 10).
fn promoted() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return true;
    };
    let Some(settings) = Key::open(w!(r"Control Panel\NotifyIconSettings")) else {
        return true;
    };
    for name in settings.subkeys() {
        let path = settings.string(&name, w!("ExecutablePath"));
        if path.is_some_and(|path| same_exe(&expand_known_folder(&path), &exe)) {
            return settings.dword(&name, w!("IsPromoted")) == Some(1);
        }
    }
    // No entry yet: a new icon, which Windows 11 puts into the overflow.
    false
}

/// Expands a leading `{known-folder-GUID}` as Explorer stores it, e.g.
/// `{6D809377-…}\LANcast\LANcast.exe` for Program Files.
fn expand_known_folder(path: &str) -> PathBuf {
    let parsed = path
        .strip_prefix('{')
        .and_then(|rest| rest.split_once('}'))
        .and_then(|(id, rest)| Some((GUID::try_from(id).ok()?, rest)));
    let Some((id, rest)) = parsed else {
        return PathBuf::from(path);
    };
    // SAFETY: `id` outlives the call; the returned string is freed with
    // CoTaskMemFree after copying, as the documentation requires.
    let folder = unsafe {
        match SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) {
            Ok(raw) => {
                let folder = raw.to_string().ok();
                CoTaskMemFree(Some(raw.0 as *const _));
                folder
            }
            Err(_) => None,
        }
    };
    match folder {
        Some(folder) => Path::new(&folder).join(rest.trim_start_matches('\\')),
        None => PathBuf::from(path),
    }
}

fn same_exe(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// An open HKCU key, closed on drop.
struct Key(HKEY);

impl Key {
    fn open(subkey: PCWSTR) -> Option<Self> {
        let mut key = HKEY::default();
        // SAFETY: `key` receives the handle, which `Drop` closes.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey, None, KEY_READ, &mut key) };
        (status == ERROR_SUCCESS).then_some(Self(key))
    }

    fn subkeys(&self) -> Vec<String> {
        let mut names = Vec::new();
        for index in 0.. {
            let mut buffer = [0u16; 256];
            let mut len = buffer.len() as u32;
            // SAFETY: buffer and len describe the same writable memory.
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &mut len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&buffer[..len as usize]));
        }
        names
    }

    fn string(&self, subkey: &str, value: PCWSTR) -> Option<String> {
        let mut buffer = [0u16; 1024];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: buffer and size describe the same writable memory.
        let status = unsafe {
            RegGetValueW(
                self.0,
                &HSTRING::from(subkey),
                value,
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

    fn dword(&self, subkey: &str, value: PCWSTR) -> Option<u32> {
        let mut data = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: `data` is a writable u32 of the size passed.
        let status = unsafe {
            RegGetValueW(
                self.0,
                &HSTRING::from(subkey),
                value,
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut data).cast()),
                Some(&mut size),
            )
        };
        (status == ERROR_SUCCESS).then_some(data)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `Key::open` and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_are_kept() {
        assert_eq!(
            expand_known_folder(r"C:\Games\LANcast\LANcast.exe"),
            PathBuf::from(r"C:\Games\LANcast\LANcast.exe")
        );
    }

    #[test]
    fn known_folders_are_expanded() {
        // FOLDERID_Windows
        let path = expand_known_folder(r"{F38BF404-1D43-42F2-9305-67DE0B28FC23}\explorer.exe");
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(same_exe(&path, &Path::new(&windows).join("explorer.exe")));
    }
}
