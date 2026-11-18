//! Frame pacer for the frame rate cap (D8, D59).

use std::time::Duration;

/// WGC delivers frames only on the display's refresh grid (e.g. every
/// 6.94 ms at 144 Hz), at the compositor's timestamps. The pacer keeps an
/// ideal schedule of one tick per period and takes, for each tick, the frame
/// whose timestamp is nearest to it. On a refresh rate that is a multiple of
/// the frame rate (60 fps from 60, 120 or 240 Hz) every gap is exactly the
/// period; otherwise (144 Hz) the gaps mix whole refreshes, each frame at
/// most half a refresh off its tick. Sources slower than the cap pass
/// through.
#[derive(Debug)]
pub struct Pacer {
    period: Duration,
    /// The next tick of the ideal schedule.
    due: Option<Duration>,
    /// Timestamp of the previous delivered frame, accepted or not.
    last: Option<Duration>,
}

impl Pacer {
    pub fn new(fps: u32) -> Self {
        Self {
            period: Duration::from_secs(1) / fps.max(1),
            due: None,
            last: None,
        }
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    /// Returns whether the frame with this capture `timestamp` (any fixed
    /// epoch) should be used.
    pub fn accept(&mut self, timestamp: Duration) -> bool {
        // The next frame most likely follows after the same gap as this one
        // (one refresh while the content changes on every refresh).
        let step = self
            .last
            .map(|last| timestamp.saturating_sub(last))
            .filter(|gap| !gap.is_zero())
            .unwrap_or(self.period)
            .min(self.period);
        self.last = Some(timestamp);
        match self.due {
            // The next frame will be nearer to the tick than this one.
            Some(due) if timestamp + step / 2 < due => false,
            // Fell behind by more than a period (e.g. after a stall or while
            // minimized): restart the schedule instead of bursting.
            Some(due) if timestamp > due + self.period => {
                self.due = Some(timestamp + self.period);
                true
            }
            Some(due) => {
                self.due = Some(due + self.period);
                true
            }
            None => {
                self.due = Some(timestamp + self.period);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds 10 s of frames on a refresh grid starting at `offset` and returns
    /// the gaps between accepted frames in whole refreshes.
    fn accepted_gaps(refresh_hz: f64, fps: u32, offset: f64) -> Vec<u32> {
        let mut pacer = Pacer::new(fps);
        let ticks = (refresh_hz * 10.0) as u32;
        let accepted: Vec<u32> = (0..ticks)
            .filter(|&i| pacer.accept(Duration::from_secs_f64(offset + i as f64 / refresh_hz)))
            .collect();
        accepted.windows(2).map(|w| w[1] - w[0]).collect()
    }

    fn per_second(gaps: &[u32]) -> usize {
        (gaps.len() + 1) / 10
    }

    #[test]
    fn multiples_of_the_rate_are_perfectly_even() {
        for (hz, every) in [(60.0, 1), (120.0, 2), (240.0, 4)] {
            let gaps = accepted_gaps(hz, 60, 0.123_456);
            assert!(gaps.iter().all(|&g| g == every), "{hz} Hz: {gaps:?}");
            assert_eq!(per_second(&gaps), 60);
        }
        let gaps = accepted_gaps(240.0, 40, 0.0);
        assert!(gaps.iter().all(|&g| g == 6), "{gaps:?}");
    }

    /// 60 from 144 Hz: 2 or 3 refreshes per frame, never a skipped extra
    /// refresh, and 60 frames per second on average.
    #[test]
    fn other_refresh_rates_mix_neighbouring_gaps() {
        let gaps = accepted_gaps(144.0, 60, 0.0);
        assert!(gaps.iter().all(|&g| g == 2 || g == 3), "{gaps:?}");
        assert_eq!(per_second(&gaps), 60);
        let gaps = accepted_gaps(165.0, 60, 0.004);
        assert!(gaps.iter().all(|&g| g == 2 || g == 3), "{gaps:?}");
        assert_eq!(per_second(&gaps), 60);
    }

    /// Each accepted frame is at most half a refresh away from its tick.
    #[test]
    fn frames_are_nearest_to_their_tick() {
        let (hz, fps) = (144.0, 60);
        let mut pacer = Pacer::new(fps);
        let mut first = None;
        let mut n = 0;
        for i in 0..1440 {
            let t = i as f64 / hz;
            if pacer.accept(Duration::from_secs_f64(t)) {
                let first = *first.get_or_insert(t);
                let tick = first + n as f64 / fps as f64;
                assert!(
                    (t - tick).abs() <= 0.5 / hz + 1e-6,
                    "frame {i}: {t} vs {tick}"
                );
                n += 1;
            }
        }
    }

    #[test]
    fn passes_through_slower_sources() {
        assert_eq!(per_second(&accepted_gaps(60.0, 60, 0.0)), 60);
        assert_eq!(per_second(&accepted_gaps(50.0, 60, 0.0)), 50);
        assert_eq!(per_second(&accepted_gaps(30.0, 40, 0.0)), 30);
    }

    #[test]
    fn restarts_after_a_stall() {
        let mut pacer = Pacer::new(60);
        let ms = Duration::from_millis;
        assert!(pacer.accept(ms(0)));
        // No frames for a second (e.g. minimized), then the grid resumes:
        // no burst of catch-up frames.
        assert!(pacer.accept(ms(1000)));
        assert!(!pacer.accept(ms(1004)));
        assert!(!pacer.accept(ms(1008)));
        assert!(pacer.accept(ms(1017)));
    }
}
