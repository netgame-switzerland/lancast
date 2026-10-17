//! Deadline-based frame pacer for the FPS cap (D8).

use std::time::{Duration, Instant};

/// WGC delivers frames on the display's refresh grid (e.g. 6.94 ms at
/// 144 Hz), so a fixed minimum interval of 1/60 s rounds up to every third
/// tick (48 fps). Advancing the deadline by exactly one period per accepted
/// frame alternates 2- and 3-tick gaps and averages to the target rate.
#[derive(Debug)]
pub struct Pacer {
    period: Duration,
    due: Option<Instant>,
}

impl Pacer {
    /// Accepts frames up to this early, to absorb delivery jitter.
    const TOLERANCE: Duration = Duration::from_millis(1);

    pub fn new(fps: u32) -> Self {
        Self {
            period: Duration::from_secs(1) / fps.max(1),
            due: None,
        }
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    /// Returns whether a frame arriving at `now` should be used.
    pub fn accept(&mut self, now: Instant) -> bool {
        match self.due {
            Some(due) if now + Self::TOLERANCE < due => false,
            // Fell behind by more than a period (e.g. after a stall or while
            // minimized): restart the schedule instead of bursting.
            Some(due) if now > due + self.period => {
                self.due = Some(now + self.period);
                true
            }
            Some(due) => {
                self.due = Some(due + self.period);
                true
            }
            None => {
                self.due = Some(now + self.period);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds frames on a fixed refresh grid and returns the accepted count.
    fn accepted_per_second(refresh_hz: f64, fps: u32) -> u32 {
        let start = Instant::now();
        let mut pacer = Pacer::new(fps);
        let ticks = refresh_hz as u32 * 10;
        let accepted = (0..ticks)
            .filter(|&i| pacer.accept(start + Duration::from_secs_f64(i as f64 / refresh_hz)))
            .count() as u32;
        accepted / 10
    }

    #[test]
    fn caps_high_refresh_displays_at_target() {
        assert_eq!(accepted_per_second(144.0, 60), 60);
        assert_eq!(accepted_per_second(120.0, 60), 60);
        assert_eq!(accepted_per_second(240.0, 60), 60);
        assert_eq!(accepted_per_second(144.0, 30), 30);
    }

    #[test]
    fn passes_through_slower_sources() {
        assert_eq!(accepted_per_second(60.0, 60), 60);
        assert_eq!(accepted_per_second(50.0, 60), 50);
    }
}
