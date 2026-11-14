//! Frame timing for the diagnostics (D57): the gaps between frames, taken
//! from the compositor's timestamps, so they show when the picture was made
//! rather than when a busy thread got to it.

use std::{collections::VecDeque, time::Duration};

/// The last gaps kept; about 8 s at 120 fps.
const CAPACITY: usize = 1024;

/// The gaps between consecutive frames of one stream, in microseconds.
#[derive(Debug, Default)]
pub struct Intervals {
    last: Option<Duration>,
    gaps: VecDeque<u32>,
    /// Gaps recorded so far, including those no longer kept.
    total: u64,
}

impl Intervals {
    /// Records a frame with the given timestamp (any fixed epoch).
    pub fn record(&mut self, timestamp: Duration) {
        if let Some(gap) = self.last.and_then(|last| timestamp.checked_sub(last)) {
            if self.gaps.len() == CAPACITY {
                self.gaps.pop_front();
            }
            self.gaps
                .push_back(gap.as_micros().min(u128::from(u32::MAX)) as u32);
            self.total += 1;
        }
        self.last = Some(timestamp);
    }

    /// The gaps recorded after `seen` (an earlier [`Self::total`]), at most
    /// the kept ones, oldest first.
    pub fn since(&self, seen: u64) -> Vec<u32> {
        let n = (self.total.saturating_sub(seen) as usize).min(self.gaps.len());
        self.last_n(n)
    }

    /// The last `n` gaps (or fewer), oldest first.
    pub fn last_n(&self, n: usize) -> Vec<u32> {
        let skip = self.gaps.len().saturating_sub(n);
        self.gaps.iter().skip(skip).copied().collect()
    }

    pub fn total(&self) -> u64 {
        self.total
    }
}

/// Frames arriving from the capture API and frames handed to NDI.
#[derive(Debug, Default)]
pub struct Timeline {
    pub arrived: Intervals,
    pub sent: Intervals,
}

/// Summary of a run of gaps, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GapStats {
    pub count: usize,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    /// Standard deviation: 0 for a perfectly even stream.
    pub jitter_ms: f64,
    /// Gaps longer than 1.5 periods: a receiver at that rate shows a frame
    /// twice (a visible hitch). 0 without a period.
    pub hitches: usize,
}

impl GapStats {
    /// Summarizes `gaps` (µs); `period` is the expected gap, if any.
    pub fn of(gaps: &[u32], period: Option<Duration>) -> Self {
        if gaps.is_empty() {
            return Self::default();
        }
        let ms: Vec<f64> = gaps.iter().map(|&g| f64::from(g) / 1000.0).collect();
        let n = ms.len() as f64;
        let avg = ms.iter().sum::<f64>() / n;
        let variance = ms.iter().map(|g| (g - avg).powi(2)).sum::<f64>() / n;
        let hitches = period.map_or(0, |p| {
            let limit = p.as_secs_f64() * 1000.0 * 1.5;
            ms.iter().filter(|&&g| g > limit).count()
        });
        Self {
            count: ms.len(),
            min_ms: ms.iter().copied().fold(f64::INFINITY, f64::min),
            avg_ms: avg,
            max_ms: ms.iter().copied().fold(0.0, f64::max),
            jitter_ms: variance.sqrt(),
            hitches,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: f64) -> Duration {
        Duration::from_secs_f64(v / 1000.0)
    }

    #[test]
    fn records_gaps_and_reads_new_ones() {
        let mut intervals = Intervals::default();
        for t in [0.0, 16.0, 33.0, 50.0] {
            intervals.record(ms(t));
        }
        assert_eq!(intervals.total(), 3);
        assert_eq!(intervals.since(0), vec![16_000, 17_000, 17_000]);
        assert_eq!(intervals.since(2), vec![17_000]);
        assert_eq!(intervals.last_n(2), vec![17_000, 17_000]);
        // A timestamp going backwards (new capture) records no gap.
        intervals.record(ms(10.0));
        assert_eq!(intervals.total(), 3);
    }

    #[test]
    fn keeps_only_the_capacity() {
        let mut intervals = Intervals::default();
        for i in 0..CAPACITY as u64 + 11 {
            intervals.record(Duration::from_millis(i));
        }
        assert_eq!(intervals.total(), CAPACITY as u64 + 10);
        assert_eq!(intervals.since(0).len(), CAPACITY);
    }

    #[test]
    fn stats_of_an_uneven_stream() {
        // 144 Hz picked down to 60 fps: 2 and 3 refresh ticks alternate.
        let gaps = [13_889, 20_833, 13_889, 20_833];
        let stats = GapStats::of(&gaps, Some(ms(16.667)));
        assert_eq!(stats.count, 4);
        assert!((stats.avg_ms - 17.361).abs() < 0.01);
        assert!((stats.jitter_ms - 3.472).abs() < 0.01);
        assert_eq!(stats.hitches, 0);
        // A dropped frame doubles a gap.
        let stats = GapStats::of(&[16_667, 33_333, 16_667], Some(ms(16.667)));
        assert_eq!(stats.hitches, 1);
        assert_eq!(GapStats::of(&[], None), GapStats::default());
    }
}
