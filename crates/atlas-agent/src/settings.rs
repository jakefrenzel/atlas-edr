//! `agent.toml` (sensor spec §11.2; plan 1b-4a, D5): the settings an operator
//! may change, over built-in defaults. A missing file means the defaults; an
//! unknown key, a wrong type or a value out of range stops start-up with a
//! message naming the key. No hot reload.
//!
//! ```toml
//! transport = "none"          # the only value until sub-project 2
//! log_level = "info"          # error, warn, info, debug, trace
//!
//! [buffer]
//! cap_mib = 1024              # at least 64
//! overflow = "retention"      # retention, head_tail, drop_oldest, drop_newest
//!
//! [pipeline]
//! hold_ms = 750               # the ordering hold (§3.2)
//! deadline_ms = 1000          # completion deadline: enrichment, join, value read, 8.3 expansion
//! confirm_window_ms = 250     # failure confirmation (§5.5)
//!
//! [enrichment]
//! size_cap_mib = 100          # larger files are not hashed (§6.3)
//!
//! [watchlist]
//! replace = ['\Users\*\.ssh\*']   # instead of the built-in list
//! extend = ['**\secrets.txt']     # added to the list in use
//!
//! [network]
//! udp = true
//! udp_idle_s = 60
//! dns_rate_per_pid = 100      # DNS-Client events per process per second (§4.4)
//!
//! [file]
//! op_end = true               # off: the second §13 fallback
//!
//! [registry]
//! value_reads = true
//!
//! [seeding]
//! on_start = true
//! on_miss = true
//! cpu_ms_per_minute = 600     # the seeder's CPU budget (§7.4)
//! ```

use std::fmt;
use std::path::Path;
use std::time::Duration;

use atlas_buffer::Overflow;
use serde::Deserialize;

use crate::config::{Config, ServiceConfig};
use crate::watchlist::Watchlist;

/// The settings in effect: the defaults with `agent.toml` applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub config: Config,
    pub services: ServiceConfig,
    pub buffer: BufferSettings,
    pub log_level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferSettings {
    pub cap_bytes: u64,
    pub overflow: Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            config: Config::default(),
            services: ServiceConfig::default(),
            buffer: BufferSettings { cap_bytes: 1 << 30, overflow: Overflow::Retention },
            log_level: LogLevel::Info,
        }
    }
}

/// Why `agent.toml` was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SettingsError {}

/// The smallest buffer: four 16 MiB segments (`atlas-buffer`'s rule).
const MIN_CAP_MIB: u64 = 64;

impl Settings {
    /// Reads `path`; the defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Settings, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text).map_err(|e| SettingsError(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(SettingsError(format!("{}: {e}", path.display()))),
        }
    }

    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let file: File = toml::from_str(text).map_err(|e| SettingsError(e.to_string().trim_end().to_owned()))?;
        file.apply()
    }
}

// The file's shape: every key optional, unknown keys refused.

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    transport: Option<String>,
    log_level: Option<LogLevel>,
    #[serde(default)]
    buffer: BufferFile,
    #[serde(default)]
    pipeline: PipelineFile,
    #[serde(default)]
    enrichment: EnrichmentFile,
    #[serde(default)]
    watchlist: WatchlistFile,
    #[serde(default)]
    network: NetworkFile,
    #[serde(default)]
    file: FileFile,
    #[serde(default)]
    registry: RegistryFile,
    #[serde(default)]
    seeding: SeedingFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BufferFile {
    cap_mib: Option<u64>,
    overflow: Option<OverflowFile>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OverflowFile {
    Retention,
    HeadTail,
    DropOldest,
    DropNewest,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PipelineFile {
    hold_ms: Option<u64>,
    deadline_ms: Option<u64>,
    confirm_window_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrichmentFile {
    size_cap_mib: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchlistFile {
    replace: Option<Vec<String>>,
    extend: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkFile {
    udp: Option<bool>,
    udp_idle_s: Option<u64>,
    dns_rate_per_pid: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFile {
    op_end: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    value_reads: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedingFile {
    on_start: Option<bool>,
    on_miss: Option<bool>,
    cpu_ms_per_minute: Option<u64>,
}

/// `key` must lie in `range`; the error names the key.
fn ranged(key: &str, v: u64, range: std::ops::RangeInclusive<u64>) -> Result<u64, SettingsError> {
    if range.contains(&v) {
        Ok(v)
    } else {
        Err(SettingsError(format!("{key} = {v} is outside {}..={}", range.start(), range.end())))
    }
}

impl File {
    fn apply(self) -> Result<Settings, SettingsError> {
        let mut s = Settings::default();
        if let Some(t) = self.transport
            && t != "none"
        {
            return Err(SettingsError(format!("transport = {t:?}: only \"none\" is supported")));
        }
        if let Some(l) = self.log_level {
            s.log_level = l;
        }

        if let Some(mib) = self.buffer.cap_mib {
            s.buffer.cap_bytes = ranged("buffer.cap_mib", mib, MIN_CAP_MIB..=1 << 20)? << 20;
        }
        if let Some(o) = self.buffer.overflow {
            s.buffer.overflow = match o {
                OverflowFile::Retention => Overflow::Retention,
                OverflowFile::HeadTail => Overflow::HeadTail,
                OverflowFile::DropOldest => Overflow::DropOldest,
                OverflowFile::DropNewest => Overflow::DropNewest,
            };
        }

        let c = &mut s.config;
        if let Some(ms) = self.pipeline.hold_ms {
            c.hold = Duration::from_millis(ranged("pipeline.hold_ms", ms, 100..=10_000)?);
        }
        if let Some(ms) = self.pipeline.deadline_ms {
            let d = Duration::from_millis(ranged("pipeline.deadline_ms", ms, 100..=60_000)?);
            c.enrich_deadline = d;
            c.join_deadline = d;
            c.value_read_deadline = d;
            c.expand_deadline = d;
        }
        if let Some(ms) = self.pipeline.confirm_window_ms {
            c.confirm_window = Duration::from_millis(ranged("pipeline.confirm_window_ms", ms, 10..=10_000)?);
        }
        if let Some(list) = self.watchlist.replace {
            c.watchlist = Some(list);
        }
        if let Some(list) = self.watchlist.extend {
            c.watchlist_extend = list;
        }
        // Compiled here so a bad pattern is refused naming its key, not later
        // at the sensor's start.
        if let Err(e) = Watchlist::new(c.watchlist.as_deref(), &c.watchlist_extend) {
            let key = if c.watchlist_extend.contains(&e.pattern) { "watchlist.extend" } else { "watchlist.replace" };
            return Err(SettingsError(format!("{key}: {:?} is not a valid pattern: {}", e.pattern, e.error)));
        }
        if let Some(on) = self.network.udp {
            c.network_udp = on;
        }
        if let Some(sec) = self.network.udp_idle_s {
            c.udp_idle = Duration::from_secs(ranged("network.udp_idle_s", sec, 1..=3_600)?);
        }
        if let Some(rate) = self.network.dns_rate_per_pid {
            c.dns_rate_per_pid = ranged("network.dns_rate_per_pid", u64::from(rate), 1..=100_000)? as u32;
        }
        if let Some(on) = self.file.op_end {
            c.file_op_end = on;
        }
        if let Some(on) = self.registry.value_reads {
            c.registry_value_reads = on;
        }
        if let Some(on) = self.seeding.on_start {
            c.seed_on_start = on;
        }
        if let Some(on) = self.seeding.on_miss {
            c.seed_on_miss = on;
        }

        if let Some(mib) = self.enrichment.size_cap_mib {
            s.services.hash_size_cap = ranged("enrichment.size_cap_mib", mib, 1..=1 << 20)? << 20;
        }
        if let Some(ms) = self.seeding.cpu_ms_per_minute {
            s.services.seeder_cpu = Duration::from_millis(ranged("seeding.cpu_ms_per_minute", ms, 1..=60_000)?);
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
        assert_eq!(Settings::parse("# only a comment\n").unwrap(), Settings::default());
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let s = Settings::load(Path::new("no-such-dir/agent.toml")).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn the_documented_example_parses() {
        let doc = include_str!("settings.rs");
        let example: String = doc
            .lines()
            .skip_while(|l| !l.starts_with("//! ```toml"))
            .skip(1)
            .take_while(|l| !l.starts_with("//! ```"))
            .map(|l| l.strip_prefix("//! ").or(l.strip_prefix("//!")).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let s = Settings::parse(&example).unwrap();
        assert_eq!(s.config.watchlist.as_deref(), Some(&[r"\Users\*\.ssh\*".to_owned()][..]));
        assert_eq!(s.config.watchlist_extend, [r"**\secrets.txt"]);
        // The rest of the example restates the defaults.
        let defaults = Settings::default();
        assert_eq!(Settings { config: defaults.config.clone(), ..s.clone() }, defaults);
        assert_eq!(Config { watchlist: None, watchlist_extend: Vec::new(), ..s.config }, defaults.config);
    }

    #[test]
    fn every_key_is_applied() {
        let s = Settings::parse(
            r#"
            log_level = "debug"
            [buffer]
            cap_mib = 256
            overflow = "drop_newest"
            [pipeline]
            hold_ms = 1000
            deadline_ms = 2000
            confirm_window_ms = 300
            [enrichment]
            size_cap_mib = 10
            [network]
            udp = false
            udp_idle_s = 30
            dns_rate_per_pid = 5
            [file]
            op_end = false
            [registry]
            value_reads = false
            [seeding]
            on_start = false
            on_miss = false
            cpu_ms_per_minute = 100
            "#,
        )
        .unwrap();
        assert_eq!(s.log_level, LogLevel::Debug);
        assert_eq!(s.buffer, BufferSettings { cap_bytes: 256 << 20, overflow: Overflow::DropNewest });
        let c = &s.config;
        assert_eq!(c.hold, Duration::from_secs(1));
        let two = Duration::from_secs(2);
        assert_eq!([c.enrich_deadline, c.join_deadline, c.value_read_deadline, c.expand_deadline], [two; 4]);
        assert_eq!(c.confirm_window, Duration::from_millis(300));
        assert_eq!((c.network_udp, c.udp_idle, c.dns_rate_per_pid), (false, Duration::from_secs(30), 5));
        assert!(!c.file_op_end && !c.registry_value_reads && !c.seed_on_start && !c.seed_on_miss);
        assert_eq!(s.services.hash_size_cap, 10 << 20);
        assert_eq!(s.services.seeder_cpu, Duration::from_millis(100));
    }

    #[test]
    fn unknown_keys_are_refused_by_name() {
        let e = Settings::parse("[network]\nudp = true\ntcp = false\n").unwrap_err();
        assert!(e.0.contains("tcp"), "{e}");
        let e = Settings::parse("[nonsense]\n").unwrap_err();
        assert!(e.0.contains("nonsense"), "{e}");
        let e = Settings::parse("hold_ms = 5\n").unwrap_err();
        assert!(e.0.contains("hold_ms"), "{e}");
    }

    #[test]
    fn wrong_types_and_values_are_refused() {
        for (text, needle) in [
            ("[network]\nudp = \"yes\"\n", "udp"),
            ("log_level = \"loud\"\n", "loud"),
            ("[buffer]\noverflow = \"keep_all\"\n", "keep_all"),
            ("[buffer]\ncap_mib = 16\n", "buffer.cap_mib"),
            ("[pipeline]\nhold_ms = 0\n", "pipeline.hold_ms"),
            ("[network]\ndns_rate_per_pid = 0\n", "network.dns_rate_per_pid"),
            ("[seeding]\ncpu_ms_per_minute = 0\n", "seeding.cpu_ms_per_minute"),
            ("[pipeline]\nhold_ms = -1\n", "hold_ms"),
            ("transport = \"grpc\"\n", "transport"),
            ("[network\n", "network"),
            ("[watchlist]\nextend = ['[']\n", "watchlist.extend: \"[\""),
            ("[watchlist]\nreplace = ['**', '[']\n", "watchlist.replace: \"[\""),
        ] {
            let e = Settings::parse(text).unwrap_err();
            assert!(e.0.contains(needle), "{text:?}: {e}");
        }
    }

    #[test]
    fn the_buffer_cap_matches_the_buffers_own_rule() {
        let s = Settings::parse(&format!("[buffer]\ncap_mib = {MIN_CAP_MIB}\n")).unwrap();
        let cfg = atlas_buffer::Config { cap_bytes: s.buffer.cap_bytes, ..atlas_buffer::Config::new("x") };
        assert_eq!(cfg.cap_bytes, 4 * cfg.segment_bytes);
    }
}
