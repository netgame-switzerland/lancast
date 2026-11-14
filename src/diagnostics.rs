//! Diagnostics while sending (D57): frame timing, capture and send
//! counters, display refresh rate, CPU load and the network adapter. Shown
//! in a panel under the preview and logged every few seconds, so a stuttering
//! stream can be told apart: uneven pacing, dropped frames, a busy GPU or
//! CPU, or the network.

use std::time::{Duration, Instant};

use tracing::info;

use crate::{
    capture::timeline::GapStats,
    session::{Session, SessionStatus},
    sysinfo::{self, CpuMeter, NetMeter},
};

/// Load of LANcast and of the whole system, in percent of all CPUs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuUsage {
    pub process: f32,
    pub system: f32,
}

/// The network adapter carrying the most outgoing traffic.
#[derive(Debug, Clone, PartialEq)]
pub struct NetUsage {
    /// The interface name, e.g. `Ethernet 2`.
    pub name: String,
    /// The adapter, e.g. `Intel(R) Ethernet Controller I225-V`.
    pub description: String,
    pub wifi: bool,
    /// Negotiated send speed.
    pub link_mbps: u64,
    pub out_mbps: f64,
    /// Send errors and discards since the previous sample.
    pub out_errors: u64,
}

/// The time span of the frame graph and of the panel's gap statistics.
pub const GRAPH_SPAN: Duration = Duration::from_secs(2);
const LOG_INTERVAL: Duration = Duration::from_secs(5);

/// One snapshot for the panel, taken once per second while sending.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    /// Refresh rate of the monitor showing the game.
    pub refresh_hz: Option<u32>,
    /// The effective FPS cap.
    pub fps_cap: u32,
    /// Gaps between the frames handed to NDI, over [`GRAPH_SPAN`].
    pub sent: GapStats,
    /// Gaps between the frames from the capture API, over [`GRAPH_SPAN`].
    pub arrived: GapStats,
    /// The gaps of `sent` in milliseconds, oldest first, for the graph.
    pub recent_ms: Vec<f32>,
    pub cpu: Option<CpuUsage>,
    pub net: Option<NetUsage>,
}

impl Diagnostics {
    /// The expected gap between sent frames.
    pub fn period(&self) -> Duration {
        Duration::from_secs(1) / self.fps_cap.max(1)
    }
}

/// Sums over a log interval.
#[derive(Default)]
struct Totals {
    samples: u32,
    arrived_fps: f64,
    send_fps: f64,
    paced_out: u64,
    backlogged: u64,
    readback_max_ms: f64,
    send_block_max_ms: f64,
    net_errors: u64,
}

/// Takes the diagnostics of a running session; one per session.
pub struct Probe {
    cpu: CpuMeter,
    net: NetMeter,
    logged_at: Instant,
    /// Gap counters at the last log line.
    logged_sent: u64,
    logged_arrived: u64,
    totals: Totals,
}

impl Probe {
    pub fn new(session: &Session) -> Self {
        let (logged_sent, logged_arrived) = session
            .video_stats()
            .timeline
            .lock()
            .map(|t| (t.sent.total(), t.arrived.total()))
            .unwrap_or_default();
        let mut probe = Self {
            cpu: CpuMeter::default(),
            net: NetMeter::default(),
            logged_at: Instant::now(),
            logged_sent,
            logged_arrived,
            totals: Totals::default(),
        };
        // The meters report from their second sample on.
        probe.cpu.sample();
        probe.net.sample();
        info!("diagnostics on");
        probe
    }

    /// Takes a snapshot; `status` is the session status of the same moment.
    /// Writes a log line every [`LOG_INTERVAL`].
    pub fn sample(&mut self, session: &Session, status: &SessionStatus) -> Diagnostics {
        let fps_cap = session.fps_cap();
        let period = Duration::from_secs(1) / fps_cap.max(1);
        let span = |fps: f64| (fps * GRAPH_SPAN.as_secs_f64()).ceil() as usize;
        // Half a second early is fine: the callers sample every 1 or 5 s.
        let log_due = self.logged_at.elapsed() + Duration::from_millis(500) >= LOG_INTERVAL;
        let (sent, arrived, logged) = match session.video_stats().timeline.lock() {
            Ok(t) => {
                let logged = log_due.then(|| {
                    let gaps = (
                        t.sent.since(self.logged_sent),
                        t.arrived.since(self.logged_arrived),
                    );
                    self.logged_sent = t.sent.total();
                    self.logged_arrived = t.arrived.total();
                    gaps
                });
                (
                    t.sent.last_n(span(status.capture_fps)),
                    t.arrived.last_n(span(status.arrived_fps)),
                    logged,
                )
            }
            Err(_) => Default::default(),
        };
        let diagnostics = Diagnostics {
            refresh_hz: sysinfo::refresh_rate(session.hwnd()),
            fps_cap,
            sent: GapStats::of(&sent, Some(period)),
            // Arrivals follow the game or the display; no expected gap.
            arrived: GapStats::of(&arrived, None),
            recent_ms: sent.iter().map(|&g| g as f32 / 1000.0).collect(),
            cpu: self.cpu.sample(),
            net: self.net.sample(),
        };

        let t = &mut self.totals;
        t.samples += 1;
        t.arrived_fps += status.arrived_fps;
        t.send_fps += status.send_fps;
        t.paced_out += status.paced_out;
        t.backlogged += status.backlogged;
        t.readback_max_ms = t.readback_max_ms.max(status.readback_max_ms);
        t.send_block_max_ms = t.send_block_max_ms.max(status.send_block_max_ms);
        t.net_errors += diagnostics.net.as_ref().map_or(0, |n| n.out_errors);
        if let Some((sent, arrived)) = logged {
            self.logged_at = Instant::now();
            let totals = std::mem::take(&mut self.totals);
            log(&diagnostics, status, &totals, &sent, &arrived, period);
        }
        diagnostics
    }
}

fn gaps_text(stats: &GapStats) -> String {
    format!(
        "{:.1}/{:.1}/{:.1}±{:.1}ms",
        stats.min_ms, stats.avg_ms, stats.max_ms, stats.jitter_ms
    )
}

/// One line over the last [`LOG_INTERVAL`].
fn log(
    d: &Diagnostics,
    status: &SessionStatus,
    t: &Totals,
    sent: &[u32],
    arrived: &[u32],
    period: Duration,
) {
    let n = f64::from(t.samples.max(1));
    let sent = GapStats::of(sent, Some(period));
    let arrived = GapStats::of(arrived, None);
    let cpu = d.cpu.map_or_else(
        || "-".to_owned(),
        |c| format!("{:.0}%/{:.0}%", c.process, c.system),
    );
    let net = d.net.as_ref().map_or_else(
        || "-".to_owned(),
        |n| {
            format!(
                "{}{} link {} Mbit/s, out {:.0} Mbit/s, {} errors",
                n.name,
                if n.wifi { " (Wi-Fi)" } else { "" },
                n.link_mbps,
                n.out_mbps,
                t.net_errors
            )
        },
    );
    info!(
        refresh_hz = d.refresh_hz.unwrap_or(0),
        cap = d.fps_cap,
        output = format!("{}x{}", status.output_size.0, status.output_size.1),
        wgc_fps = format!("{:.1}", t.arrived_fps / n),
        send_fps = format!("{:.1}", t.send_fps / n),
        sent_gaps = gaps_text(&sent),
        hitches = sent.hitches,
        wgc_gaps = gaps_text(&arrived),
        paced_out = t.paced_out,
        dropped = t.backlogged,
        readback_max_ms = format!("{:.1}", t.readback_max_ms),
        send_block_max_ms = format!("{:.1}", t.send_block_max_ms),
        receivers = status.receivers,
        cpu_lancast_system = cpu,
        net,
        "diagnostics"
    );
}
