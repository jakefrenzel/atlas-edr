//! The single writer (sensor spec §8.1–8.4).

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cursor::{self, Cursor, Loaded};
use crate::fsx::{self, SEGMENT_HEADER, SEGMENT_MAGIC, segment_path};
use crate::record::{self, MAX_PAYLOAD, RECORD_HEADER};
use crate::{Config, Gap, Overflow, RecordTime, Stats};

/// Why `append` refused a record. Each is counted in [`Stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// Empty, or larger than `record::MAX_PAYLOAD`.
    Rejected,
    /// The in-memory backlog is full (the disk is failing or far behind).
    BacklogFull,
}

/// What `Writer::open` found and repaired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Torn or corrupt bytes cut from the end of the newest segment.
    pub truncated_bytes: u64,
    /// The cursor file was damaged; delivery restarts from the oldest record.
    pub cursor_reset: bool,
    /// The newest segment did not start with the segment header; it is kept as
    /// a sealed segment, which readers skip.
    pub foreign_segment: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    seq: u64,
    len: u64,
}

enum Room {
    Made,
    Full,
}

pub struct Writer {
    cfg: Config,
    record_time: RecordTime,
    /// Every segment on disk, oldest first. The last one is active; its file is
    /// created when the first record is written to it.
    segments: VecDeque<Segment>,
    /// The active segment, positioned at its end. `None` until first use and after an I/O error.
    file: Option<File>,
    /// Bytes written to `file` since its last flush.
    unsynced: bool,
    /// Framed records not yet written.
    pending: Vec<u8>,
    acked: Option<Cursor>,
    last_flush: Option<Instant>,
    /// `Some` while failing: when to retry.
    retry_at: Option<Instant>,
    stats: Stats,
    gaps: Vec<Gap>,
    /// Records being dropped by `DropNewest`; closed by `take_gaps`.
    drop_gap: Option<Gap>,
    #[cfg(test)]
    faults: Faults,
}

impl Writer {
    /// Opens (or creates) the buffer (§8.2):
    /// - loads the cursor and deletes segments wholly behind it;
    /// - cuts the newest segment after its last valid record;
    /// - starts a new segment, so nothing written from now on lands in a segment
    ///   a reader may already have read past (an ack can be ahead of what a power cut kept).
    pub fn open(cfg: Config, record_time: RecordTime) -> io::Result<(Self, Recovery)> {
        cfg.validate()?;
        fsx::prepare_dir(&cfg.dir)?;
        let mut recovery = Recovery::default();
        let mut stats = Stats::default();
        let acked = match cursor::load(&cfg.dir)? {
            Loaded::Missing => None,
            Loaded::Valid(c) => Some(c),
            Loaded::Corrupt => {
                recovery.cursor_reset = true;
                None
            }
        };

        let seqs = fsx::list_segments(&cfg.dir)?;
        // The new segment sorts after every segment ever written and after the cursor.
        let next_seq = next(seqs.last().copied().unwrap_or(0).max(acked.map_or(0, |c| c.segment)))?;

        let mut segments = VecDeque::new();
        for (i, &seq) in seqs.iter().enumerate() {
            // Delivered; a crash between the cursor write and the delete can leave them.
            if acked.is_some_and(|c| seq < c.segment) && remove_segment(&cfg.dir, seq).is_ok() {
                continue;
            }
            let len = if i + 1 == seqs.len() {
                match recover_newest(&cfg.dir, seq, &mut recovery)? {
                    Some(len) => len,
                    None => {
                        recovery.foreign_segment = Some(seq);
                        fsx::segment_len(&segment_path(&cfg.dir, seq))?
                    }
                }
            } else {
                fsx::segment_len(&segment_path(&cfg.dir, seq))?
            };
            if acked.is_some_and(|c| seq < c.segment) {
                stats.delete_failures += 1; // kept and counted; deleted by a later ack or eviction
            }
            segments.push_back(Segment { seq, len });
        }
        segments.push_back(Segment { seq: next_seq, len: 0 });

        let writer = Self {
            cfg,
            record_time,
            segments,
            file: None,
            unsynced: false,
            pending: Vec::new(),
            acked,
            last_flush: None,
            retry_at: None,
            stats,
            gaps: Vec::new(),
            drop_gap: None,
            #[cfg(test)]
            faults: Faults::default(),
        };
        Ok((writer, recovery))
    }

    /// Queues one record in memory. Does no I/O. (Only the thread that owns the
    /// writer calls it, so it waits while that thread is inside `tick`.)
    pub fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
        if payload.is_empty() || payload.len() > MAX_PAYLOAD {
            self.stats.rejected += 1;
            return Err(Dropped::Rejected);
        }
        if self.pending.len() + RECORD_HEADER + payload.len() > self.cfg.backlog_bytes {
            self.stats.backlog_drops += 1;
            return Err(Dropped::BacklogFull);
        }
        record::encode(payload, &mut self.pending);
        Ok(())
    }

    /// Does the I/O that is due: writes and flushes once `flush_interval` has
    /// passed since the last flush (or sooner if half the backlog is used); while
    /// failing, retries once `retry_interval` has passed since the failure.
    /// An `Err` means this attempt failed; records stay queued for the retry.
    pub fn tick(&mut self, now: Instant) -> io::Result<()> {
        if let Some(retry_at) = self.retry_at {
            if now < retry_at {
                return Ok(());
            }
        } else {
            let due = self.last_flush.is_none_or(|t| now.duration_since(t) >= self.cfg.flush_interval);
            let crowded = self.pending.len() >= self.cfg.backlog_bytes / 2;
            if !due && !crowded {
                return Ok(());
            }
        }
        self.last_flush = Some(now);
        match self.flush_now() {
            Ok(()) => {
                if self.retry_at.take().is_some() {
                    self.stats.recoveries += 1;
                }
                Ok(())
            }
            Err(e) => {
                // Reopened (and truncated to the last good length) on the retry.
                self.file = None;
                self.retry_at = Some(now + self.cfg.retry_interval);
                self.stats.write_errors += 1;
                Err(e)
            }
        }
    }

    /// Clean stop: writes and flushes everything queued.
    pub fn close(mut self) -> io::Result<()> {
        self.flush_now()
    }

    /// Persists `cursor` as delivered and deletes the segments wholly before it (§8.5).
    /// A cursor at or before the current one is ignored. A segment that cannot be
    /// deleted (another process has it open without delete sharing) is kept,
    /// counted in `Stats::delete_failures`, and retried by the next ack.
    pub fn ack(&mut self, cursor: Cursor) -> io::Result<()> {
        if self.acked.is_some_and(|a| cursor <= a) {
            return Ok(());
        }
        if cursor.segment > self.active().seq {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cursor is past the newest segment"));
        }
        cursor::store(&self.cfg.dir, cursor)?;
        self.acked = Some(cursor);
        let active = self.active().seq;
        let dir = self.cfg.dir.clone();
        let mut failures = 0;
        self.segments.retain(|s| {
            if s.seq >= cursor.segment || s.seq == active {
                return true;
            }
            let kept = remove_segment(&dir, s.seq).is_err();
            failures += u64::from(kept);
            kept
        });
        self.stats.delete_failures += failures;
        Ok(())
    }

    /// The last acknowledged position: where a transport's reader starts.
    pub fn acked(&self) -> Option<Cursor> {
        self.acked
    }

    pub fn is_failing(&self) -> bool {
        self.retry_at.is_some()
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Gaps since the last call. A `DropNewest` gap still in progress is closed
    /// and returned, so a periodic report always includes current drops.
    pub fn take_gaps(&mut self) -> Vec<Gap> {
        if let Some(g) = self.drop_gap.take() {
            self.gaps.push(g);
        }
        std::mem::take(&mut self.gaps)
    }

    /// Bytes in all segments, including the active one.
    pub fn disk_bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.len).sum()
    }

    fn active(&self) -> Segment {
        *self.segments.back().expect("there is always an active segment")
    }

    fn path(&self, seq: u64) -> PathBuf {
        segment_path(&self.cfg.dir, seq)
    }

    /// Writes what is queued and flushes what was written. Idle: no I/O at all.
    fn flush_now(&mut self) -> io::Result<()> {
        if self.file.is_none() && (!self.pending.is_empty() || self.unsynced) {
            self.open_active()?;
        }
        if !self.pending.is_empty() {
            self.write_pending()?;
        }
        if self.unsynced {
            self.file.as_ref().expect("opened above").sync_data()?;
            self.unsynced = false;
        }
        Ok(())
    }

    /// Opens the active segment at its last good length, creating it with its
    /// header if it is new. Also removes a torn write left by an I/O error.
    fn open_active(&mut self) -> io::Result<()> {
        let active = self.active();
        let mut file = fsx::open_rw(&self.path(active.seq))?;
        file.set_len(active.len)?;
        file.seek(SeekFrom::Start(active.len))?;
        if active.len == 0 {
            file.write_all(&SEGMENT_MAGIC)?;
            self.segments.back_mut().expect("active").len = SEGMENT_HEADER;
            self.unsynced = true;
        }
        self.file = Some(file);
        Ok(())
    }

    fn write_pending(&mut self) -> io::Result<()> {
        let mut done = 0;
        let result = self.write_from(&mut done);
        self.pending.drain(..done);
        result
    }

    /// Writes `pending[*done..]`, rotating as segments fill. `*done` always marks
    /// the bytes already written (or dropped), even when an error is returned.
    fn write_from(&mut self, done: &mut usize) -> io::Result<()> {
        while *done < self.pending.len() {
            let active_len = self.active().len;
            // Whole records that fit; an empty segment always takes at least one.
            let (mut end, mut count) = (*done, 0u64);
            while end < self.pending.len() {
                let next = end + record_len(&self.pending, end);
                let fits = active_len + (next - *done) as u64 <= self.cfg.segment_bytes;
                if !fits && !(end == *done && active_len == SEGMENT_HEADER) {
                    break;
                }
                (end, count) = (next, count + 1);
            }
            if end == *done {
                match self.rotate()? {
                    Room::Made => continue,
                    Room::Full => {
                        self.drop_pending(*done);
                        *done = self.pending.len();
                        return Ok(());
                    }
                }
            }
            self.fault()?;
            let file = self.file.as_mut().expect("opened by flush_now or rotate");
            file.write_all(&self.pending[*done..end])?;
            self.unsynced = true;
            self.segments.back_mut().expect("active").len += (end - *done) as u64;
            self.stats.records_written += count;
            self.stats.bytes_written += (end - *done) as u64;
            *done = end;
        }
        Ok(())
    }

    /// Seals the active segment and starts the next one, after making room for it.
    fn rotate(&mut self) -> io::Result<Room> {
        if let Room::Full = self.make_room() {
            return Ok(Room::Full);
        }
        if let Some(file) = self.file.take() {
            file.sync_data()?;
            self.unsynced = false;
        }
        let seq = next(self.active().seq)?;
        self.segments.push_back(Segment { seq, len: 0 });
        self.open_active()?;
        Ok(Room::Made)
    }

    /// Deletes segments until a new full segment fits under the cap (§8.4). A
    /// segment that cannot be deleted is skipped and counted; if none can be,
    /// the new segment goes over the cap rather than stalling every write.
    fn make_room(&mut self) -> Room {
        let mut undeletable = Vec::new();
        while self.disk_bytes() + self.cfg.segment_bytes > self.cfg.cap_bytes {
            let Some(i) = self.victim(&undeletable) else {
                // Nothing the policy may delete (DropNewest): drop. Only undeletable ones left: go over.
                return if undeletable.is_empty() { Room::Full } else { Room::Made };
            };
            let seq = self.segments[i].seq;
            // Read before deleting, count only once deleted.
            let gap = (self.cfg.overflow != Overflow::Retention).then(|| self.segment_gap(seq));
            if remove_segment(&self.cfg.dir, seq).is_err() {
                self.stats.delete_failures += 1;
                undeletable.push(seq);
                continue;
            }
            self.segments.remove(i);
            match gap {
                None => self.stats.retention_evictions += 1,
                Some(gap) => {
                    self.stats.overflow_evictions += 1;
                    if gap.records > 0 {
                        self.gaps.push(gap);
                    }
                }
            }
        }
        Room::Made
    }

    /// The index of the next segment the policy deletes, never the active one
    /// and never one in `skip`.
    fn victim(&self, skip: &[u64]) -> Option<usize> {
        let sealed = self.segments.len() - 1;
        let first = match self.cfg.overflow {
            Overflow::Retention | Overflow::DropOldest => 0,
            Overflow::DropNewest => return None,
            Overflow::HeadTail => {
                // Pin the oldest segments holding ~25% of the undelivered bytes.
                let target = self.disk_bytes() / 4;
                let (mut pinned, mut bytes) = (0, 0);
                while pinned < sealed && bytes < target {
                    bytes += self.segments[pinned].len;
                    pinned += 1;
                }
                pinned
            }
        };
        (first..sealed).find(|&i| !skip.contains(&self.segments[i].seq))
    }

    /// Counts the records in a segment about to be deleted, and their time range.
    /// An unreadable segment yields an empty gap rather than blocking eviction.
    fn segment_gap(&self, seq: u64) -> Gap {
        let mut gap = Gap::empty();
        let mut data = Vec::new();
        let read = fsx::open_read(&self.path(seq)).and_then(|mut f| f.read_to_end(&mut data));
        if read.is_ok() && data.starts_with(&SEGMENT_MAGIC) {
            for (start, end) in record::scan(&data, SEGMENT_HEADER as usize).records {
                gap.add((self.record_time)(&data[start..end]));
            }
        }
        gap
    }

    /// Drops `pending[from..]` for lack of room (`DropNewest`), adding it to the open gap.
    fn drop_pending(&mut self, from: usize) {
        let gap = self.drop_gap.get_or_insert_with(Gap::empty);
        let mut at = from;
        while at < self.pending.len() {
            let next = at + record_len(&self.pending, at);
            gap.add((self.record_time)(&self.pending[at + RECORD_HEADER..next]));
            self.stats.overflow_drops += 1;
            at = next;
        }
    }

    #[cfg(not(test))]
    fn fault(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Test hook: after `skip` good writes, fails the next `fail` writes, leaving
    /// torn bytes behind as a real failure might.
    #[cfg(test)]
    fn fault(&mut self) -> io::Result<()> {
        if self.faults.skip > 0 {
            self.faults.skip -= 1;
            return Ok(());
        }
        if self.faults.fail == 0 {
            return Ok(());
        }
        self.faults.fail -= 1;
        if let Some(file) = self.file.as_mut() {
            file.write_all(b"torn")?;
        }
        Err(io::Error::other("injected fault"))
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct Faults {
    skip: u32,
    fail: u32,
}

/// The sequence number after `seq`. A hostile file name or cursor near `u64::MAX`
/// is refused rather than wrapping to a number that sorts first.
fn next(seq: u64) -> io::Result<u64> {
    seq.checked_add(1).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "segment numbers exhausted"))
}

/// Length of the framed record at `at` in `pending` (built by `record::encode`).
fn record_len(pending: &[u8], at: usize) -> usize {
    let len = u32::from_le_bytes(pending[at..at + 4].try_into().expect("4 bytes"));
    RECORD_HEADER + len as usize
}

fn remove_segment(dir: &Path, seq: u64) -> io::Result<()> {
    match fs::remove_file(segment_path(dir, seq)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A header torn while the segment was being created: a prefix of the magic,
/// followed by nothing or by zeros (NTFS can keep a file's size after a power
/// cut while the unwritten bytes read as zeros).
fn torn_header(data: &[u8]) -> bool {
    let head = &data[..data.len().min(SEGMENT_MAGIC.len())];
    let written = head.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    SEGMENT_MAGIC.starts_with(&head[..written]) && data[head.len()..].iter().all(|&b| b == 0)
}

/// Scans the newest segment and cuts it after its last valid record; a torn
/// header becomes an empty segment. `None` if it is not an Atlas segment (it is
/// then left untouched).
fn recover_newest(dir: &Path, seq: u64, recovery: &mut Recovery) -> io::Result<Option<u64>> {
    let mut file = fsx::open_rw(&segment_path(dir, seq))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    let header = SEGMENT_MAGIC.len();
    if data.len() >= header && data[..header] == SEGMENT_MAGIC {
        let valid_end = record::scan(&data, header).valid_end;
        if valid_end < data.len() {
            file.set_len(valid_end as u64)?;
            file.sync_all()?;
            recovery.truncated_bytes += (data.len() - valid_end) as u64;
        }
        return Ok(Some(valid_end as u64));
    }
    if !torn_header(&data) {
        return Ok(None);
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?; // `read_to_end` left the position at the old end
    file.write_all(&SEGMENT_MAGIC)?;
    file.sync_all()?;
    recovery.truncated_bytes += data.len() as u64;
    Ok(Some(SEGMENT_HEADER))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Reader;

    fn time_of(payload: &[u8]) -> Option<i64> {
        Some(i64::from_le_bytes(payload.get(..8)?.try_into().ok()?))
    }

    fn rec(t: i64) -> Vec<u8> {
        let mut p = t.to_le_bytes().to_vec();
        p.extend_from_slice(&[0xaa; 24]);
        p
    }

    fn cfg(dir: &Path) -> Config {
        Config { segment_bytes: 256, cap_bytes: 1024, ..Config::new(dir) }
    }

    fn read_all(dir: &Path) -> Vec<Vec<u8>> {
        let mut reader = Reader::open(dir, Cursor::default());
        std::iter::from_fn(|| reader.next_record().unwrap()).map(|r| r.payload).collect()
    }

    fn segment_files(dir: &Path) -> usize {
        fsx::list_segments(dir).unwrap().len()
    }

    #[test]
    fn backlog_survives_a_failure_and_is_written_on_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let t0 = Instant::now();
        w.append(&rec(1)).unwrap();
        w.tick(t0).unwrap();

        w.faults.fail = 1;
        w.append(&rec(2)).unwrap();
        assert!(w.tick(t0 + Duration::from_secs(1)).is_err());
        assert!(w.is_failing());
        w.append(&rec(3)).unwrap();

        // Before the retry interval nothing is attempted, even though a flush is due.
        w.tick(t0 + Duration::from_secs(5)).unwrap();
        assert!(w.is_failing());
        assert_eq!(read_all(tmp.path()), vec![rec(1)], "the torn bytes are not a record");

        w.tick(t0 + Duration::from_secs(11)).unwrap();
        assert!(!w.is_failing());
        assert_eq!((w.stats().write_errors, w.stats().recoveries), (1, 1));
        assert_eq!(read_all(tmp.path()), vec![rec(1), rec(2), rec(3)]);
    }

    #[test]
    fn a_failure_right_after_a_rotation_is_retried_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let t0 = Instant::now();
        let records: Vec<_> = (0..12).map(rec).collect(); // 40 B framed: 6 per segment
        for r in &records {
            w.append(r).unwrap();
        }
        // The first batch (segment 1) is written, then the write into the new segment 2
        // fails, twice, each time leaving torn bytes in segment 2.
        w.faults = Faults { skip: 1, fail: 2 };
        assert!(w.tick(t0).is_err());
        assert_eq!(segment_files(tmp.path()), 2, "the failure happened after the rotation");
        assert!(w.tick(t0 + Duration::from_secs(10)).is_err());
        w.tick(t0 + Duration::from_secs(20)).unwrap();
        assert_eq!(segment_files(tmp.path()), 2);
        assert_eq!(read_all(tmp.path()), records);
    }

    #[test]
    fn a_full_backlog_drops_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let backlog_bytes = RECORD_HEADER + MAX_PAYLOAD;
        let (mut w, _) = Writer::open(Config { backlog_bytes, ..cfg(tmp.path()) }, time_of).unwrap();
        w.append(&vec![1; MAX_PAYLOAD - 40]).unwrap();
        assert_eq!(w.append(&[1; 64]), Err(Dropped::BacklogFull));
        assert_eq!(w.stats().backlog_drops, 1);
    }

    #[test]
    fn empty_and_oversized_records_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        assert_eq!(w.append(&[]), Err(Dropped::Rejected));
        assert_eq!(w.append(&vec![0; MAX_PAYLOAD + 1]), Err(Dropped::Rejected));
        assert_eq!(w.stats().rejected, 2);
        w.append(&vec![0; MAX_PAYLOAD]).unwrap();
    }

    #[test]
    fn idle_ticks_touch_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut w, _) = Writer::open(cfg(tmp.path()), time_of).unwrap();
        let t0 = Instant::now();
        for s in 0..3 {
            w.tick(t0 + Duration::from_secs(s)).unwrap();
        }
        assert_eq!(segment_files(tmp.path()), 0, "no empty segment is created");
        w.append(&rec(1)).unwrap();
        w.tick(t0 + Duration::from_secs(5)).unwrap();
        assert!(!w.unsynced);
        w.tick(t0 + Duration::from_secs(6)).unwrap();
        assert!(!w.unsynced);
    }

    #[test]
    fn torn_headers_are_recognised() {
        for torn in [&b""[..], b"ATL", b"ATLSEG0", &[0; 8], &[0; 64], b"ATL\0\0\0\0\0\0\0"] {
            assert!(torn_header(torn), "{torn:?}");
        }
        for foreign in [&b"ATX"[..], b"not an atlas segment", b"ATL\0\0\0\0\0x", b"\0\0\0x"] {
            assert!(!torn_header(foreign), "{foreign:?}");
        }
    }

    #[test]
    fn sequence_numbers_never_wrap() {
        assert_eq!(next(7).unwrap(), 8);
        assert_eq!(next(u64::MAX).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
