//! Watches the Windows Firewall for LANcast in the background (D55): checks
//! at startup and every [`RECHECK`] (the network type changes when the PC is
//! plugged into the LAN), and allows LANcast on request (UAC prompt).

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use eframe::egui;
use tracing::{info, warn};

use crate::firewall::{self, Check};

const RECHECK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub struct FirewallState {
    /// `None` before the first check, or if checking failed.
    pub check: Option<Check>,
    /// The elevated helper runs (Windows shows the UAC prompt).
    pub fixing: bool,
    /// Why the last "allow" did not work.
    pub fix_error: Option<String>,
}

enum Request {
    Check,
    Allow,
}

pub struct FirewallWatch {
    state: Arc<Mutex<FirewallState>>,
    requests: Sender<Request>,
}

impl FirewallWatch {
    /// `blocked` is called with each check's verdict (`false` if unknown).
    pub fn new(ctx: egui::Context, blocked: impl Fn(bool) + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new(FirewallState::default()));
        let (requests, rx) = unbounded();
        let shared = state.clone();
        let spawned = std::thread::Builder::new()
            .name("firewall".into())
            .spawn(move || run(rx, shared, ctx, blocked));
        if let Err(err) = spawned {
            warn!("cannot start the firewall check: {err}");
        }
        Self { state, requests }
    }

    pub fn state(&self) -> FirewallState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Checks again now, e.g. before going live.
    pub fn recheck(&self) {
        let _ = self.requests.send(Request::Check);
    }

    /// Allows LANcast in the firewall (asks for admin rights), then checks.
    /// `fixing` is set at once, so the UI can wait for it to clear.
    pub fn allow(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.fixing = true;
            state.fix_error = None;
        }
        let _ = self.requests.send(Request::Allow);
    }
}

fn check() -> Option<Check> {
    match firewall::check() {
        Ok(check) => Some(check),
        Err(err) => {
            warn!("cannot check the Windows Firewall: {err:#}");
            None
        }
    }
}

fn run(
    rx: Receiver<Request>,
    state: Arc<Mutex<FirewallState>>,
    ctx: egui::Context,
    blocked: impl Fn(bool),
) {
    let update = |change: &dyn Fn(&mut FirewallState)| {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        change(&mut state);
        blocked(state.check.as_ref().is_some_and(|c| !c.ok()));
        drop(state);
        ctx.request_repaint();
    };
    let mut last: Option<Check> = None;
    let mut log = |check: &Option<Check>| {
        if *check != last {
            if let Some(check) = check {
                info!(
                    active = ?check.active,
                    blocked = ?check.blocked,
                    block_rule = check.block_rule,
                    "Windows Firewall checked"
                );
            }
            last = check.clone();
        }
    };
    loop {
        let found = check();
        log(&found);
        update(&|s| s.check = found.clone());
        match rx.recv_timeout(RECHECK) {
            Ok(Request::Check) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Request::Allow) => {
                let result = firewall::allow_elevated();
                // Checked before `fixing` clears: the UI acts on the result.
                let found = check();
                log(&found);
                let error = match result {
                    Ok(()) if found.as_ref().is_some_and(|c| !c.ok()) => {
                        warn!("LANcast was allowed, but the firewall still blocks it");
                        Some(
                            "the rule was added, but Windows still blocks LANcast (a policy \
                             of your organization or another firewall?)"
                                .to_owned(),
                        )
                    }
                    Ok(()) => {
                        info!("LANcast allowed in the Windows Firewall");
                        None
                    }
                    Err(err) => {
                        warn!("allowing LANcast in the firewall failed: {err:#}");
                        Some(format!("{err:#}"))
                    }
                };
                update(&|s| {
                    s.check = found.clone();
                    s.fixing = false;
                    s.fix_error = error.clone();
                });
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
