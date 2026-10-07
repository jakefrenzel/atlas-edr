//! The pipeline's settings, with the sensor spec's defaults (§3.2, §5.5, §7).
//! Reading them from `agent.toml` comes with plan 1b-4; until then the agent
//! uses `Config::default()`.

use std::time::Duration;

/// Pipeline settings. Durations are converted to QPC ticks by [`Ticks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Ordering stage: how long an event is held before release (§3.2).
    pub hold: Duration,
    /// Failure-confirm window for Create and RenamePath (§5.5).
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
    /// OperationEnds: failure confirmation for Create and RenamePath, and the
    /// Cleanup outcomes that report deletes (§5.1, §5.5). Off (the second §13
    /// fallback, with the OP_END keyword disabled), Create and RenamePath are
    /// emitted at once and a DeletePath is taken as the Delete.
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

/// Settings of the Windows services (sensor spec §6.3, §7.4; plan 1b-3b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    /// Hash and signature workers (§6.3).
    pub hash_workers: usize,
    /// Files larger than this are not hashed (§6.3).
    pub hash_size_cap: u64,
    /// Cached hash results; the cache starts over when full.
    pub hash_cache_cap: usize,
    /// The seeder's CPU budget: at most `seeder_cpu` per `seeder_cpu_window`
    /// (default 1% of one core over 60 s). Re-reads past it are deferred (§7.4).
    pub seeder_cpu: Duration,
    pub seeder_cpu_window: Duration,
    /// A file name query that takes longer is abandoned (§7.4).
    pub name_timeout: Duration,
    /// Past this many stuck name queries, file seeding pauses (§7.4).
    pub max_stuck_helpers: usize,
    /// Requests waiting per lane (hash workers, reader, expander, seeder). A full
    /// lane drops the request; its event waits out its deadline.
    pub lane_cap: usize,
}

impl ServiceConfig {
    /// Refuses settings under which a service cannot work: no hash worker, a
    /// lane that holds nothing, no CPU budget or window (the seeder would wake
    /// every 10 ms forever), no name timeout, no stuck helper allowed.
    pub fn check(&self) -> Result<(), String> {
        let zero = [
            ("hash_workers", self.hash_workers == 0),
            ("hash_cache_cap", self.hash_cache_cap == 0),
            ("lane_cap", self.lane_cap == 0),
            ("seeder_cpu", self.seeder_cpu.is_zero()),
            ("seeder_cpu_window", self.seeder_cpu_window.is_zero()),
            ("name_timeout", self.name_timeout.is_zero()),
            ("max_stuck_helpers", self.max_stuck_helpers == 0),
        ];
        match zero.iter().find(|(_, z)| *z) {
            Some((name, _)) => Err(format!("{name} must be greater than zero")),
            None => Ok(()),
        }
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        ServiceConfig {
            hash_workers: 2,
            hash_size_cap: 100 << 20,
            hash_cache_cap: 65_536,
            seeder_cpu: Duration::from_millis(600),
            seeder_cpu_window: Duration::from_secs(60),
            name_timeout: Duration::from_millis(200),
            max_stuck_helpers: 2,
            lane_cap: 8_192,
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
    fn service_settings_are_checked() {
        assert_eq!(ServiceConfig::default().check(), Ok(()));
        let bad = ServiceConfig { lane_cap: 0, ..ServiceConfig::default() };
        assert_eq!(bad.check(), Err("lane_cap must be greater than zero".into()));
        let bad = ServiceConfig { seeder_cpu: Duration::ZERO, ..ServiceConfig::default() };
        assert!(bad.check().is_err());
    }

    #[test]
    fn defaults_follow_the_spec() {
        let c = Config::default();
        assert_eq!((c.hold, c.confirm_window), (Duration::from_millis(750), Duration::from_millis(250)));
        assert_eq!(c.dns_rate_per_pid, 100);
        assert!(c.network_udp && c.registry_value_reads && c.seed_on_miss);
    }
}
