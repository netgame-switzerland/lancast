//! Fitting the stream to the network link (D58): on a link too slow for
//! full-bandwidth NDI (e.g. a cable or port that fell back to 100 Mbit/s),
//! LANcast lowers the max. resolution and keeps the frame rate, unless the
//! user insists on the resolution.

use std::net::{Ipv4Addr, ToSocketAddrs};

use tracing::info;

use crate::{capture::video::VideoOptions, sysinfo};

/// A network adapter and its negotiated send speed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    /// The interface name, e.g. `Ethernet 2`.
    pub name: String,
    /// The adapter, e.g. `Realtek USB GbE Family Controller`.
    pub description: String,
    pub mbps: u64,
}

impl LinkInfo {
    /// E.g. `100 Mbit/s` or `2.5 Gbit/s`.
    pub fn speed_text(&self) -> String {
        if self.mbps >= 1000 && self.mbps.is_multiple_of(1000) {
            format!("{} Gbit/s", self.mbps / 1000)
        } else if self.mbps >= 1000 {
            format!("{:.1} Gbit/s", self.mbps as f64 / 1000.0)
        } else {
            format!("{} Mbit/s", self.mbps)
        }
    }
}

/// NDI full bandwidth at 1920×1080, 60 fps (SpeedHQ2 4:2:2, the maximum,
/// D6). Other sizes and rates scale with the pixel rate.
const MBPS_1080P60: f64 = 132.14;
/// The share of the link the stream may take: NDI's bitrate peaks in busy
/// scenes, and audio, the game and other traffic need room too.
const LINK_SHARE: f64 = 0.7;
/// The sizes to step down to, largest first (16:9, even widths).
const STEPS: [(u32, u32); 6] = [
    (2560, 1440),
    (1920, 1080),
    (1600, 900),
    (1280, 720),
    (960, 540),
    (640, 360),
];
/// mDNS: the multicast route stands for "the LAN" when there is no
/// discovery server, also on a LAN without a default gateway.
const MDNS: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);

/// Estimated peak bandwidth of a stream filling `size` at `fps`.
pub fn estimate_mbps(size: (u32, u32), fps: u32) -> f64 {
    let pixel_rate = f64::from(size.0) * f64::from(size.1) * f64::from(fps);
    MBPS_1080P60 * pixel_rate / (1920.0 * 1080.0 * 60.0)
}

/// The max. size to send instead of `max` on a link of `link_mbps`, or
/// `None` if `max` fits. Keeps the frame rate: on a big screen, fluid motion
/// matters more than sharpness (D58).
pub fn cap_for_link(max: (u32, u32), fps: u32, link_mbps: u64) -> Option<(u32, u32)> {
    let budget = link_mbps as f64 * LINK_SHARE;
    if estimate_mbps(max, fps) <= budget {
        return None;
    }
    let smaller: Vec<(u32, u32)> = STEPS
        .into_iter()
        .filter(|&(w, h)| w <= max.0 && h <= max.1 && (w, h) != max)
        .collect();
    smaller
        .iter()
        .copied()
        .find(|&size| estimate_mbps(size, fps) <= budget)
        .or_else(|| smaller.last().copied())
}

/// On a slow `link`, lowers `video.max_size` to what fits, at the same frame
/// rate, unless `keep` (the user's override). Logs what it decided.
pub fn fit_to_link(video: &mut VideoOptions, link: Option<&LinkInfo>, keep: bool) {
    let Some(link) = link else { return };
    let Some(cap) = cap_for_link(video.max_size, video.fps_cap, link.mbps) else {
        return;
    };
    let (w, h) = video.max_size;
    if keep {
        info!(
            adapter = %link.name,
            speed = %link.speed_text(),
            "slow network link: keeping {w}x{h} (override)"
        );
    } else {
        info!(
            adapter = %link.name,
            speed = %link.speed_text(),
            "slow network link: max. resolution {}x{} instead of {w}x{h}",
            cap.0,
            cap.1
        );
        video.max_size = cap;
    }
}

/// The physical link the stream most likely leaves on: the route to the
/// discovery server(s) (`host[:port],…`; empty = mDNS), else the route to
/// the LAN's multicast, else the fastest connected adapter. `None` if
/// unknown (e.g. a virtual adapter): then nothing is lowered. Resolves host
/// names, so it may block briefly.
pub fn detect(servers: &str) -> Option<LinkInfo> {
    servers
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(server_ipv4)
        .filter(|ip| !ip.is_loopback())
        .find_map(sysinfo::link_to)
        .or_else(|| sysinfo::link_to(MDNS))
        .or_else(sysinfo::fastest_link)
}

fn server_ipv4(server: &str) -> Option<Ipv4Addr> {
    let host = match server.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => host,
        _ => server,
    };
    (host, 0)
        .to_socket_addrs()
        .ok()?
        .find_map(|addr| match addr.ip() {
            std::net::IpAddr::V4(ip) => Some(ip),
            std::net::IpAddr::V6(_) => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gigabit_link_keeps_everything() {
        assert_eq!(cap_for_link((2560, 1440), 60, 1000), None);
        assert_eq!(cap_for_link((3840, 2160), 60, 1000), None);
    }

    #[test]
    fn a_100_mbit_link_drops_to_720p_at_60_fps() {
        assert_eq!(cap_for_link((1920, 1080), 60, 100), Some((1280, 720)));
        assert_eq!(cap_for_link((2560, 1440), 60, 100), Some((1280, 720)));
        // At 30 fps 1080p fits (~66 Mbit/s of 70).
        assert_eq!(cap_for_link((1920, 1080), 30, 100), None);
        assert_eq!(cap_for_link((1280, 720), 60, 100), None);
    }

    #[test]
    fn very_slow_links_end_at_the_smallest_step() {
        assert_eq!(cap_for_link((1920, 1080), 60, 10), Some((640, 360)));
        // Nothing smaller than the choice itself: no cut to offer.
        assert_eq!(cap_for_link((640, 360), 60, 1), None);
    }

    #[test]
    fn estimate_matches_the_ndi_figure() {
        assert!((estimate_mbps((1920, 1080), 60) - 132.14).abs() < 0.01);
        assert!((estimate_mbps((1280, 720), 60) - 58.73).abs() < 0.01);
    }

    #[test]
    fn server_addresses_with_and_without_port() {
        assert_eq!(
            server_ipv4("192.168.1.10"),
            Some(Ipv4Addr::new(192, 168, 1, 10))
        );
        assert_eq!(
            server_ipv4("192.168.1.10:5959"),
            Some(Ipv4Addr::new(192, 168, 1, 10))
        );
        assert_eq!(server_ipv4("127.0.0.1"), Some(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn speed_texts() {
        let link = |mbps| LinkInfo {
            name: String::new(),
            description: String::new(),
            mbps,
        };
        assert_eq!(link(100).speed_text(), "100 Mbit/s");
        assert_eq!(link(1000).speed_text(), "1 Gbit/s");
        assert_eq!(link(2500).speed_text(), "2.5 Gbit/s");
    }
}
