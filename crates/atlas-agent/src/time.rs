//! The time base (sensor spec §3.3). Everything inside the pipeline is raw QPC;
//! `meta.time` (Unix ns, UTC) comes from the newest anchor pair.

use crate::config::Ticks;

/// A (QPC, wall clock) pair taken together. Re-taken every 60 s by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub qpc: i64,
    /// Unix time in nanoseconds (from `GetSystemTimePreciseAsFileTime`).
    pub unix_ns: i64,
}

/// Converts QPC timestamps to Unix nanoseconds.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    ticks: Ticks,
    anchor: Anchor,
}

impl Clock {
    pub fn new(ticks: Ticks, anchor: Anchor) -> Self {
        Clock { ticks, anchor }
    }

    pub fn ticks(&self) -> Ticks {
        self.ticks
    }

    /// Replaces the anchor; later conversions use it (§3.3).
    pub fn set_anchor(&mut self, anchor: Anchor) {
        self.anchor = anchor;
    }

    /// `anchor_unix_ns + (qpc − anchor_qpc) × 10⁹ / frequency`, saturating.
    pub fn unix_ns(&self, qpc: i64) -> i64 {
        self.anchor.unix_ns.saturating_add(self.ticks.to_nanos(qpc.saturating_sub(self.anchor.qpc)))
    }
}

/// FILETIME (100 ns since 1601) to Unix nanoseconds, saturating.
pub fn filetime_to_unix_ns(ft: u64) -> i64 {
    const EPOCH_DIFF_100NS: i128 = 116_444_736_000_000_000;
    let ns = (i128::from(ft) - EPOCH_DIFF_100NS) * 100;
    i64::try_from(ns).unwrap_or(if ns < 0 { i64::MIN } else { i64::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_relative_to_the_anchor() {
        let c = Clock::new(Ticks::new(10_000_000), Anchor { qpc: 1_000, unix_ns: 1_700_000_000_000_000_000 });
        assert_eq!(c.unix_ns(1_000), 1_700_000_000_000_000_000);
        assert_eq!(c.unix_ns(11_000), 1_700_000_000_001_000_000); // +10 000 ticks = +1 ms
        assert_eq!(c.unix_ns(0), 1_699_999_999_999_900_000); // before the anchor
    }

    #[test]
    fn a_new_anchor_moves_later_conversions() {
        let mut c = Clock::new(Ticks::new(10_000_000), Anchor { qpc: 0, unix_ns: 0 });
        c.set_anchor(Anchor { qpc: 0, unix_ns: 5 });
        assert_eq!(c.unix_ns(0), 5);
    }

    #[test]
    fn filetime_epoch_and_a_known_date() {
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_000), 0);
        // 2026-10-02T18:37:27.6102067Z, from spike S8.
        assert_eq!(filetime_to_unix_ns(134_354_398_476_102_067), 1_790_966_247_610_206_700);
    }
}
