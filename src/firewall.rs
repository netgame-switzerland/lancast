//! Windows Firewall check for the NDI sender (SPEC §5.4, D10, D40, D55).
//!
//! Receivers connect to the sender, so LANcast needs inbound access. Without
//! it the source is listed (announcing is outbound) but stays black. Windows
//! asks only for an exe without any rule, so a block rule from a cancelled
//! prompt, or an allow rule for other network profiles (the setup's used to
//! leave out Public, the default for a new network), blocks silently.
//!
//! [`check`] reads the rules (no admin rights needed). [`allow_elevated`]
//! starts this exe with `--allow-firewall` as admin (UAC prompt), which runs
//! [`allow_own_exe`]: it deletes every rule for the exe and adds one inbound
//! allow rule for all profiles, the same as the setup.

use std::fmt;

use anyhow::Result;

#[cfg(windows)]
mod win32;

/// Command-line flag of the elevated helper process.
pub const ALLOW_FLAG: &str = "--allow-firewall";
/// Name of LANcast's firewall rule (also used by the setup).
pub const RULE_NAME: &str = "LANcast";

/// A Windows Firewall network profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Domain,
    Private,
    Public,
}

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::Domain, Profile::Private, Profile::Public];

    /// Bit in the firewall API's profile masks (`NET_FW_PROFILE_TYPE2`).
    pub fn bit(self) -> i32 {
        match self {
            Profile::Domain => 1,
            Profile::Private => 2,
            Profile::Public => 4,
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Profile::Domain => "Domain",
            Profile::Private => "Private",
            Profile::Public => "Public",
        })
    }
}

/// IP protocol of a rule, as far as NDI cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Any,
    Tcp,
    Udp,
    Other,
}

/// What a firewall rule for LANcast's exe does (inbound, enabled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub allow: bool,
    pub protocol: Protocol,
    /// Profile bitmask.
    pub profiles: i32,
}

/// The firewall state for LANcast's exe on the active networks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Check {
    /// Profiles of the connected networks.
    pub active: Vec<Profile>,
    /// Active profiles where the firewall is on and receivers cannot connect:
    /// a block rule, or no allow rule for both TCP and UDP.
    pub blocked: Vec<Profile>,
    /// Some of them are blocked by a block rule (not just a missing rule).
    pub block_rule: bool,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.blocked.is_empty()
    }

    /// Evaluates the inbound `rules` for LANcast's exe on the `active`
    /// profiles; `enabled(p)` = the firewall is on for profile `p`.
    pub fn evaluate(
        active: Vec<Profile>,
        enabled: impl Fn(Profile) -> bool,
        rules: &[Rule],
    ) -> Self {
        let mut check = Check {
            active,
            ..Check::default()
        };
        for &profile in &check.active {
            if !enabled(profile) {
                continue;
            }
            let applies = |rule: &&Rule| rule.profiles & profile.bit() != 0;
            let blocking = rules
                .iter()
                .filter(applies)
                .any(|rule| !rule.allow && rule.protocol != Protocol::Other);
            let allows = |protocol| {
                rules.iter().filter(applies).any(|rule| {
                    rule.allow && (rule.protocol == Protocol::Any || rule.protocol == protocol)
                })
            };
            if blocking {
                check.block_rule = true;
                check.blocked.push(profile);
            } else if !(allows(Protocol::Tcp) && allows(Protocol::Udp)) {
                check.blocked.push(profile);
            }
        }
        check
    }

    /// The blocked profiles as text, e.g. "Public".
    pub fn blocked_text(&self) -> String {
        let names: Vec<String> = self.blocked.iter().map(Profile::to_string).collect();
        names.join(", ")
    }
}

/// Reads the firewall state for this exe. Needs no admin rights.
pub fn check() -> Result<Check> {
    #[cfg(windows)]
    return win32::check();
    #[cfg(not(windows))]
    Ok(Check::default())
}

/// Starts this exe elevated with [`ALLOW_FLAG`] (Windows shows the UAC
/// prompt) and waits for it. Blocks: call it off the UI thread.
pub fn allow_elevated() -> Result<()> {
    #[cfg(windows)]
    return win32::allow_elevated();
    #[cfg(not(windows))]
    anyhow::bail!("not supported on this platform")
}

/// Deletes every firewall rule for this exe and adds LANcast's inbound
/// allow rule for all profiles. Needs admin rights.
pub fn allow_own_exe() -> Result<()> {
    #[cfg(windows)]
    return win32::allow_own_exe();
    #[cfg(not(windows))]
    anyhow::bail!("not supported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(allow: bool, protocol: Protocol, profiles: &[Profile]) -> Rule {
        Rule {
            allow,
            protocol,
            profiles: profiles.iter().map(|p| p.bit()).sum(),
        }
    }

    #[test]
    fn setup_rule_without_public_blocks_a_public_network() {
        let rules = [rule(
            true,
            Protocol::Any,
            &[Profile::Domain, Profile::Private],
        )];
        let check = Check::evaluate(vec![Profile::Public], |_| true, &rules);
        assert_eq!(check.blocked, [Profile::Public]);
        assert!(!check.block_rule);
        let check = Check::evaluate(vec![Profile::Private], |_| true, &rules);
        assert!(check.ok());
    }

    #[test]
    fn prompt_rules_need_tcp_and_udp() {
        let tcp = rule(true, Protocol::Tcp, &[Profile::Public]);
        let udp = rule(true, Protocol::Udp, &[Profile::Public]);
        let active = || vec![Profile::Public];
        assert!(!Check::evaluate(active(), |_| true, std::slice::from_ref(&tcp)).ok());
        assert!(Check::evaluate(active(), |_| true, &[tcp, udp]).ok());
    }

    #[test]
    fn a_block_rule_wins() {
        let rules = [
            rule(true, Protocol::Any, &Profile::ALL),
            rule(false, Protocol::Tcp, &[Profile::Public]),
        ];
        let check = Check::evaluate(vec![Profile::Private, Profile::Public], |_| true, &rules);
        assert_eq!(check.blocked, [Profile::Public]);
        assert!(check.block_rule);
    }

    #[test]
    fn a_disabled_firewall_needs_no_rule() {
        let check = Check::evaluate(vec![Profile::Public], |p| p != Profile::Public, &[]);
        assert!(check.ok());
    }
}
