//! The agent's on-disk event buffer (sensor spec §8): an append-only log of
//! numbered segment files holding opaque, CRC-checked records.
//!
//! - [`Writer`]: the single writer. [`Writer::append`] only queues in memory;
//!   [`Writer::tick`] does all I/O: it writes and flushes once per
//!   `flush_interval`, and after an I/O error retries once per `retry_interval`
//!   while records wait in a bounded backlog (§8.3).
//! - [`Reader`]: iterates records after a [`Cursor`]; safe to run in another
//!   process while the writer appends (§8.5).
//! - Overflow at the size cap follows an [`Overflow`] policy (§8.4).
//!
//! The crate knows nothing about events or Windows: records are byte strings.
//! Time is passed in (`tick(now)`), so every behaviour is testable without sleeping.

mod cursor;
mod fsx;
mod reader;
pub mod record;
mod writer;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

pub use cursor::Cursor;
pub use fsx::{SEGMENT_HEADER, SEGMENT_MAGIC};
pub use reader::{Reader, ReaderStats, Record};
pub use writer::{Dropped, Recovery, Writer};

/// Extracts an event time (ns since the Unix epoch) from a record, for gap reports.
/// The agent passes a function that decodes the event; the buffer stays opaque.
pub type RecordTime = fn(&[u8]) -> Option<i64>;

/// What happens when the buffer reaches `cap_bytes` (sensor spec §8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// No transport (sub-project 1): the buffer is local history. Delete the
    /// oldest segment; counted, but not a gap.
    Retention,
    /// Keep the oldest ~25% of undelivered data and the newest; delete the
    /// oldest segment after the pinned head, and report a gap.
    HeadTail,
    /// Delete the oldest segment and report a gap.
    DropOldest,
    /// Keep what is on disk; drop new records and report a gap.
    DropNewest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub dir: PathBuf,
    /// Size at which the active segment is sealed and a new one started. A record
    /// larger than this gets a segment of its own.
    pub segment_bytes: u64,
    /// Total size of all segments; at least 4 × `segment_bytes`. It can be
    /// exceeded by one oversized record, or while a segment cannot be deleted.
    pub cap_bytes: u64,
    pub overflow: Overflow,
    pub flush_interval: Duration,
    pub retry_interval: Duration,
    /// In-memory bytes waiting to be written. Records beyond it are dropped.
    pub backlog_bytes: usize,
}

impl Config {
    /// Spec defaults: 16 MiB segments, a 1 GiB cap, rolling retention, flush
    /// every 1 s, retry every 10 s, 32 MiB backlog.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            segment_bytes: 16 << 20,
            cap_bytes: 1 << 30,
            overflow: Overflow::Retention,
            flush_interval: Duration::from_secs(1),
            retry_interval: Duration::from_secs(10),
            backlog_bytes: 32 << 20,
        }
    }

    fn validate(&self) -> io::Result<()> {
        let bad = |msg: &str| Err(io::Error::new(io::ErrorKind::InvalidInput, msg.to_owned()));
        if self.segment_bytes <= SEGMENT_HEADER {
            return bad("segment_bytes must exceed the segment header");
        }
        if self.cap_bytes < self.segment_bytes.saturating_mul(4) {
            return bad("cap_bytes must be at least 4 × segment_bytes");
        }
        if self.backlog_bytes < record::RECORD_HEADER + record::MAX_PAYLOAD {
            return bad("backlog_bytes must hold at least one maximum-size record");
        }
        Ok(())
    }
}

/// Records deleted or dropped before delivery: the content of a Sensor Health gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    pub records: u64,
    /// Smallest and largest [`RecordTime`] among them; `None` if none decoded.
    pub first_time: Option<i64>,
    pub last_time: Option<i64>,
}

impl Gap {
    fn empty() -> Self {
        Self { records: 0, first_time: None, last_time: None }
    }

    fn add(&mut self, time: Option<i64>) {
        self.records += 1;
        if let Some(t) = time {
            self.first_time = Some(self.first_time.map_or(t, |f| f.min(t)));
            self.last_time = Some(self.last_time.map_or(t, |l| l.max(t)));
        }
    }
}

/// Cumulative writer counters; the agent reports deltas in Sensor Health.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub records_written: u64,
    pub bytes_written: u64,
    /// Segments deleted by [`Overflow::Retention`].
    pub retention_evictions: u64,
    /// Segments deleted by the other overflow policies (each also a [`Gap`]).
    pub overflow_evictions: u64,
    /// Records dropped because the policy deletes nothing ([`Overflow::DropNewest`]).
    pub overflow_drops: u64,
    /// Segments that could not be deleted (another process holds them open
    /// without delete sharing). They are kept and retried later.
    pub delete_failures: u64,
    /// Records dropped because the in-memory backlog was full.
    pub backlog_drops: u64,
    /// Records refused by `append`: empty or over `record::MAX_PAYLOAD`.
    pub rejected: u64,
    /// Ticks whose writes failed.
    pub write_errors: u64,
    /// Successful retries after a failure.
    pub recoveries: u64,
}
