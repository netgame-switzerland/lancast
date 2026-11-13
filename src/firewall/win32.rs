//! Win32 backend of [`super`]: reads the rules through the Windows Firewall
//! COM API (`INetFwPolicy2`), changes them with `netsh` like the setup, and
//! elevates through the shell's "runas" verb.

use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};

use anyhow::{Context, Result, anyhow, bail};
use tracing::{debug, info};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_CANCELLED, VARIANT_TRUE},
        Globalization::{CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar},
        NetworkManagement::WindowsFirewall::{
            INetFwPolicy2, INetFwRule, NET_FW_ACTION_ALLOW, NET_FW_IP_PROTOCOL_ANY,
            NET_FW_IP_PROTOCOL_TCP, NET_FW_IP_PROTOCOL_UDP, NET_FW_PROFILE_TYPE2,
            NET_FW_RULE_DIR_IN, NetFwPolicy2,
        },
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IDispatch,
            },
            Ole::IEnumVARIANT,
            Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject},
            Variant::VARIANT,
        },
        UI::{
            Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_HIDE,
        },
    },
    core::{HSTRING, Interface, PCWSTR, w},
};

use super::{ALLOW_FLAG, Check, Profile, Protocol, RULE_NAME, Rule};

/// netsh and the helper show no console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn own_exe() -> Result<PathBuf> {
    std::env::current_exe().context("cannot find LANcast.exe")
}

pub fn check() -> Result<Check> {
    let exe = normalize(&own_exe()?.to_string_lossy());
    let _com = Com::init()?;
    // SAFETY: COM is initialized on this thread (`_com`); NetFwPolicy2 is the
    // documented in-process class for INetFwPolicy2.
    let policy: INetFwPolicy2 =
        unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }
            .context("cannot open the Windows Firewall settings")?;
    // SAFETY: plain property reads on a valid interface.
    let current = unsafe { policy.CurrentProfileTypes() }?;
    let active: Vec<Profile> = Profile::ALL
        .into_iter()
        .filter(|p| current & p.bit() != 0)
        .collect();
    let mut enabled = Vec::new();
    for &profile in &active {
        // SAFETY: as above.
        let on = unsafe { policy.get_FirewallEnabled(NET_FW_PROFILE_TYPE2(profile.bit())) }?;
        if on == VARIANT_TRUE {
            enabled.push(profile);
        }
    }

    let mut rules = Vec::new();
    let mut read = 0usize;
    // SAFETY: as above; the enumerator yields VARIANTs holding IDispatch
    // pointers (VT_DISPATCH) to INetFwRule objects, which `VARIANT` releases
    // on drop.
    unsafe {
        let all = policy.Rules()?;
        let items: IEnumVARIANT = all._NewEnum()?.cast()?;
        loop {
            let mut item = [VARIANT::default()];
            let mut fetched = 0;
            let _ = items.Next(&mut item, &mut fetched);
            if fetched == 0 {
                break;
            }
            let Ok(rule) = IDispatch::try_from(&item[0]).and_then(|d| d.cast::<INetFwRule>())
            else {
                continue;
            };
            read += 1;
            if rule.Direction()? != NET_FW_RULE_DIR_IN || rule.Enabled()? != VARIANT_TRUE {
                continue;
            }
            let app = rule.ApplicationName().unwrap_or_default().to_string();
            if app.is_empty() || normalize(&expand(&app)) != exe {
                continue;
            }
            let protocol = match rule.Protocol()? {
                p if p == NET_FW_IP_PROTOCOL_ANY.0 => Protocol::Any,
                p if p == NET_FW_IP_PROTOCOL_TCP.0 => Protocol::Tcp,
                p if p == NET_FW_IP_PROTOCOL_UDP.0 => Protocol::Udp,
                _ => Protocol::Other,
            };
            let found = Rule {
                allow: rule.Action()? == NET_FW_ACTION_ALLOW,
                protocol,
                profiles: rule.Profiles()?,
            };
            debug!(name = %rule.Name().unwrap_or_default(), ?found, "firewall rule");
            rules.push(found);
        }
    }
    // Windows always has rules of its own: none read means the enumeration
    // failed, and "blocked" would be a false alarm.
    if read == 0 {
        bail!("no firewall rules could be read");
    }
    debug!(read, matching = rules.len(), "firewall rules read");
    Ok(Check::evaluate(active, |p| enabled.contains(&p), &rules))
}

pub fn allow_elevated() -> Result<()> {
    let report = std::env::temp_dir().join(format!("LANcast-firewall-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&report);
    let exe = HSTRING::from(own_exe()?.as_os_str());
    let params = HSTRING::from(format!("{ALLOW_FLAG} \"{}\"", report.display()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the strings it points to outlive the call.
    if let Err(err) = unsafe { ShellExecuteExW(&mut info) } {
        if err.code() == ERROR_CANCELLED.to_hresult() {
            bail!("cancelled");
        }
        return Err(anyhow!(err).context("cannot start LANcast as administrator"));
    }
    let process = info.hProcess;
    if process.is_invalid() {
        bail!("no process handle for the elevated LANcast");
    }
    let mut code = 1u32;
    // SAFETY: `process` is the handle ShellExecuteExW returned
    // (SEE_MASK_NOCLOSEPROCESS); it is closed exactly once.
    unsafe {
        WaitForSingleObject(process, INFINITE);
        let read = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        read.context("cannot read the exit code of the elevated LANcast")?;
    }
    let message = std::fs::read_to_string(&report).unwrap_or_default();
    let _ = std::fs::remove_file(&report);
    if code != 0 {
        let message = message.trim();
        if message.is_empty() {
            bail!("changing the firewall failed (exit code {code})");
        }
        bail!("{message}");
    }
    Ok(())
}

pub fn allow_own_exe() -> Result<()> {
    let exe = own_exe()?;
    let program = format!("program={}", exe.display());
    // "No rules match" is an error exit too, so the result is not checked.
    let deleted = netsh(&["delete", "rule", "name=all", &program])?;
    info!(
        output = deleted.trim(),
        "firewall rules for LANcast deleted"
    );
    let name = format!("name={RULE_NAME}");
    let added = netsh_checked(&[
        "add",
        "rule",
        &name,
        "dir=in",
        "action=allow",
        &program,
        "profile=any",
        "enable=yes",
    ])?;
    info!(output = added.trim(), "firewall rule for LANcast added");
    Ok(())
}

fn netsh_command(args: &[&str]) -> Command {
    let mut command = Command::new(system32("netsh.exe"));
    command
        .args(["advfirewall", "firewall"])
        .args(args)
        .creation_flags(CREATE_NO_WINDOW);
    command
}

fn netsh(args: &[&str]) -> Result<String> {
    let output = netsh_command(args).output().context("cannot run netsh")?;
    Ok(oem_text(&output.stdout))
}

fn netsh_checked(args: &[&str]) -> Result<String> {
    let output = netsh_command(args).output().context("cannot run netsh")?;
    // netsh prints its errors to stdout, in the language of Windows.
    let text = oem_text(&output.stdout) + &oem_text(&output.stderr);
    if !output.status.success() {
        bail!("netsh failed: {}", text.trim());
    }
    Ok(text)
}

/// netsh runs in its own (hidden) console, which writes in the OEM code page,
/// e.g. 850 on German Windows.
fn oem_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    // SAFETY: both buffers are slices, passed with their lengths; the second
    // call writes at most `wide.len()` characters.
    unsafe {
        let len = MultiByteToWideChar(CP_OEMCP, flags, bytes, None);
        if len <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; len as usize];
        let len = MultiByteToWideChar(CP_OEMCP, flags, bytes, Some(&mut wide));
        String::from_utf16_lossy(&wide[..len.max(0) as usize])
    }
}

fn system32(file: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(file)
}

/// Expands `%NAME%` environment variables (rules may store e.g.
/// `%ProgramFiles%\…`); unknown names stay as they are.
fn expand(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else {
            break;
        };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

/// Paths compare case-insensitively, with backslashes.
fn normalize(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// COM for the current thread, uninitialized on drop if this call set it up.
struct Com(bool);

impl Com {
    fn init() -> Result<Self> {
        // SAFETY: plain call; paired with CoUninitialize in Drop on success.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // RPC_E_CHANGED_MODE: the thread already has COM (as STA); use it.
        Ok(Com(hr.is_ok()))
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx in `init`.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_environment_variables() {
        let windir = std::env::var("SystemRoot").unwrap();
        assert_eq!(expand(r"%SystemRoot%\x.exe"), format!(r"{windir}\x.exe"));
        assert_eq!(
            expand(r"%NoSuchVar_lancast%\x.exe"),
            r"%NoSuchVar_lancast%\x.exe"
        );
        assert_eq!(expand("100% sure"), "100% sure");
    }
}
