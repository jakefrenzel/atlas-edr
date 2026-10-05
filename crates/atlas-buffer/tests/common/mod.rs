//! Shared helpers. Records carry their "event time" in the first 8 bytes.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::Path;
use std::time::Instant;

use atlas_buffer::{Config, Cursor, Reader, Writer};

/// Framed size of every `rec`: 8-byte header + 32-byte payload.
pub const FRAMED: u64 = 40;

pub fn time_of(payload: &[u8]) -> Option<i64> {
    Some(i64::from_le_bytes(payload.get(..8)?.try_into().ok()?))
}

/// A 32-byte record whose time is `t`.
pub fn rec(t: i64) -> Vec<u8> {
    let mut p = t.to_le_bytes().to_vec();
    p.extend_from_slice(&[0x5a; 24]);
    p
}

/// Small segments so tests rotate quickly: 8-byte header + 6 records = 248 ≤ 256.
pub fn cfg(dir: &Path) -> Config {
    Config { segment_bytes: 256, cap_bytes: 1024, ..Config::new(dir) }
}

pub fn open(cfg: Config) -> Writer {
    Writer::open(cfg, time_of).expect("open").0
}

/// Appends and writes `times` in one tick.
pub fn write(w: &mut Writer, times: impl IntoIterator<Item = i64>) {
    for t in times {
        w.append(&rec(t)).expect("append");
    }
    flush(w);
}

/// Forces a flush now, whatever the interval.
pub fn flush(w: &mut Writer) {
    // Each call is a new `Instant` far enough apart for the 1 s interval.
    thread_local!(static CLOCK: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) });
    let now = CLOCK.with(|c| {
        let next = c.get().map_or_else(Instant::now, |t| t + std::time::Duration::from_secs(2));
        c.set(Some(next));
        next
    });
    w.tick(now).expect("tick");
}

/// The times of every record a fresh reader sees from `from`.
pub fn read_times(dir: &Path, from: Cursor) -> Vec<i64> {
    let mut reader = Reader::open(dir, from);
    std::iter::from_fn(|| reader.next_record().expect("read")).map(|r| time_of(&r.payload).unwrap()).collect()
}

/// The total size of the segment files actually on disk.
pub fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "seg"))
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum()
}

pub fn segment_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "seg"))
        .count()
}
