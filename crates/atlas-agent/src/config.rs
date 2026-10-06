//! The pipeline's settings, with the sensor spec's defaults (§3.2, §5.5, §7).
//! Reading them from `agent.toml` comes with plan 1b-4; until then the agent
//! uses `Config::default()`.

use std::time::Duration;

/// Pipeline settings. Durations are converted to QPC ticks by [`Ticks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Ordering stage: how long an event is held before release (§3.2).
    pub hold: Duration,
    /// Failure-confirm window for Create, DeletePath and RenamePath (§5.5).
    pub confirm_window: Duration,
    /// Completion deadlines (§3.2).
    pub enrich_deadline: Duration,
    pub join_deadline: Duration,
    pub value_read_deadline: Duration,
    pub expand_deadline: Duration,
    pub seeder_deadline: Duration,
    /// The seeder deadline during the first `startup_period` after start (§3.2).
    pub seeder_startup_deadline: Duration,
    pub startup_period: Duration,
    /// Launch join: the two halves match within this window (§5.2).
    pub join_window: Duration,
    /// Process cache: retention after Terminate, and the entry cap (§6.2).
    pub process_retention: Duration,
    pub process_cap: usize,
    /// FileObject and KeyObject maps: caps (§7.1, §7.4).
    pub file_map_cap: usize,
    pub key_map_cap: usize,
    /// The callback's early key map cap (§7.5).
    pub early_key_map_cap: usize,
    /// UDP flows (§7.3).
    pub network_udp: bool,
    pub udp_idle: Duration,
    pub flow_cap: usize,
    /// DNS-Client per-PID rate limit, events per second (§4.4).
    pub dns_rate_per_pid: u32,
    /// Watchlist (§7.2): `None` uses the built-in list.
    pub watchlist: Option<Vec<String>>,
    /// Extra patterns added to the list in use.
    pub watchlist_extend: Vec<String>,
    /// Repeated opens of one path by one process count once per this period (§7.2).
    pub watchlist_coalesce: Duration,
    /// More pending events than this and the oldest goes out as is (§3.2).
    pub pending_cap: usize,
    /// Registry value reads after the event (§7.5).
    pub registry_value_reads: bool,
    /// Failure confirmation for Create, DeletePath and RenamePath (§5.5). Off
    /// (the second §13 fallback, with the OP_END keyword disabled), they are
    /// emitted at once.
    pub file_op_end: bool,
    /// Seed the key and file maps from the handle table at start (§7.4).
    pub seed_on_start: bool,
    /// Ask the seeder about unknown handles (§7.4). Off, an event on an unknown
    /// handle waits only for the start-up pass, while it is outstanding.
    pub seed_on_miss: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hold: Duration::from_millis(750),
            confirm_window: Duration::from_millis(250),
            enrich_deadline: Duration::from_secs(1),
            join_deadline: Duration::from_secs(1),
            value_read_deadline: Duration::from_secs(1),
            expand_deadline: Duration::from_secs(1),
            seeder_deadline: Duration::from_secs(2),
            seeder_startup_deadline: Duration::from_secs(5),
            startup_period: Duration::from_secs(30),
            join_window: Duration::from_millis(200),
            process_retention: Duration::from_secs(30),
            process_cap: 65_536,
            file_map_cap: 262_144,
            key_map_cap: 262_144,
            early_key_map_cap: 200_000,
            network_udp: true,
            udp_idle: Duration::from_secs(60),
            flow_cap: 65_536,
            dns_rate_per_pid: 100,
            watchlist: None,
            watchlist_extend: Vec::new(),
            watchlist_coalesce: Duration::from_secs(60),
            pending_cap: 100_000,
            registry_value_reads: true,
            file_op_end: true,
            seed_on_start: true,
            seed_on_miss: true,
        }
    }
}

/// Converts durations to QPC ticks (the pipeline's only clock, §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticks {
    /// QPC ticks per second.
    pub frequency: i64,
}

impl Ticks {
    pub fn new(frequency: i64) -> Self {
        assert!(frequency > 0, "QPC frequency must be positive");
        Ticks { frequency }
    }

    pub fn of(&self, d: Duration) -> i64 {
        let ticks = d.as_nanos().saturating_mul(self.frequency as u128) / 1_000_000_000;
        i64::try_from(ticks).unwrap_or(i64::MAX)
    }

    /// Ticks to nanoseconds, saturating.
    pub fn to_nanos(&self, ticks: i64) -> i64 {
        let ns = i128::from(ticks) * 1_000_000_000 / i128::from(self.frequency);
        i64::try_from(ns).unwrap_or(if ns < 0 { i64::MIN } else { i64::MAX })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_round_trip_at_the_usual_frequency() {
        let t = Ticks::new(10_000_000);
        assert_eq!(t.of(Duration::from_millis(750)), 7_500_000);
        assert_eq!(t.to_nanos(7_500_000), 750_000_000);
        assert_eq!(t.of(Duration::MAX), i64::MAX);
    }

    #[test]
    fn defaults_follow_the_spec() {
        let c = Config::default();
        assert_eq!((c.hold, c.confirm_window), (Duration::from_millis(750), Duration::from_millis(250)));
        assert_eq!(c.dns_rate_per_pid, 100);
        assert!(c.network_udp && c.registry_value_reads && c.seed_on_miss);
    }
}
