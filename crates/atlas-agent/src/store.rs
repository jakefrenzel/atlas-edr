//! The buffer writer thread (sensor spec §3.2 [6], §8; plan 1b-4a, D1 and D5).
//!
//! - [`Sink`]: what the pipeline thread calls with each emitted event. It moves
//!   the event into a bounded channel and never blocks; a full channel drops
//!   the event and counts it (`loss.buffer_backlog_drops`, with the events the
//!   writer's backlog could not take).
//! - The thread owns the `atlas_buffer::Writer`. It encodes each event, appends
//!   it, and calls `tick` about every 100 ms (at most about 200: a wait for
//!   the first record, then a drain as long); `tick` may block on the disk
//!   while the channel absorbs events. The writer flushes every second, so
//!   neither bound matters to it.
//! - A Sensor Health report arrives from the health thread with everything but
//!   the buffer's figures, which the thread fills in as it appends it: the
//!   `buffer` group, `loss.buffer_backlog_drops`, `housekeeping.retention_evictions`,
//!   the gap, and in the first report what recovery found at open. Counted
//!   here, a report includes the drops of every event queued ahead of it, the
//!   final report's too. A report the backlog cannot
//!   take is held and written before anything else once there is room, so its
//!   counts are not lost.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use atlas_buffer::{Dropped, Gap, Recovery, Stats, Writer};
use atlas_schema::classes::sensor_health::{SensorGap, SensorHealthAction};
use atlas_schema::{Event, EventKind, decode_event, encode_event};

/// What the writer thread receives.
pub enum Record {
    Event(Event),
    /// A Sensor Health event whose buffer figures the thread fills in.
    Health(Event),
}

/// The pipeline's sink (plan 1b-3c's interface): never blocks.
#[derive(Clone)]
pub struct Sink {
    tx: SyncSender<Record>,
    drops: Arc<AtomicU64>,
}

impl Sink {
    pub fn event(&self, e: Event) {
        if self.tx.try_send(Record::Event(e)).is_err() {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The running writer thread.
pub struct Store {
    tx: SyncSender<Record>,
    drops: Arc<AtomicU64>,
    thread: JoinHandle<io::Result<Stats>>,
}

/// Settings of the writer thread.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub buffer: atlas_buffer::Config,
    /// Events waiting for the thread (65,536 by default).
    pub channel_cap: usize,
    /// Tests: the writer thread panics after this many appends.
    #[cfg(test)]
    pub panic_after: Option<u64>,
}

impl StoreConfig {
    pub fn new(buffer: atlas_buffer::Config) -> Self {
        StoreConfig {
            buffer,
            channel_cap: 65_536,
            #[cfg(test)]
            panic_after: None,
        }
    }
}

/// The wait for a record, and the drain after it, before each `tick`.
const TICK_EVERY: Duration = Duration::from_millis(100);
/// Held Sensor Health reports kept while the backlog is full; past it the
/// oldest goes.
const HELD_CAP: usize = 16;

/// A gap's time range comes from the record's event (plan 1b-1).
fn record_time(bytes: &[u8]) -> Option<i64> {
    decode_event(bytes).ok().map(|e| e.meta.time)
}

impl Store {
    /// Opens the buffer on the calling thread, so an error stops start-up,
    /// then starts the writer thread. Returns what recovery found.
    pub fn start(cfg: StoreConfig) -> io::Result<(Store, Recovery)> {
        let (writer, recovery) = Writer::open(cfg.buffer, record_time)?;
        #[cfg(test)]
        if let Some(left) = cfg.panic_after {
            let writer = tests::PanicsAfter { inner: writer, left };
            return Ok((Store::spawn(writer, recovery.clone(), cfg.channel_cap), recovery));
        }
        Ok((Store::spawn(writer, recovery.clone(), cfg.channel_cap), recovery))
    }

    /// The writer thread over `writer`.
    pub(crate) fn spawn<W: Log + Send + 'static>(writer: W, recovery: Recovery, channel_cap: usize) -> Store {
        let (tx, rx) = sync_channel(channel_cap);
        let drops = Arc::new(AtomicU64::new(0));
        let mut core = Core::new(writer, drops.clone(), recovery);
        let thread = std::thread::Builder::new()
            .name("atlas-buffer".into())
            .spawn(move || {
                core.run(&rx);
                core.close()
            })
            .expect("spawn the buffer writer thread");
        Store { tx, drops, thread }
    }

    /// Whether the writer thread still runs: once it ended (a panic), nothing
    /// more reaches the disk, reports included.
    pub fn is_running(&self) -> bool {
        !self.thread.is_finished()
    }

    pub fn sink(&self) -> Sink {
        Sink { tx: self.tx.clone(), drops: self.drops.clone() }
    }

    /// Events the buffer could not take (`loss.buffer_backlog_drops`): the
    /// channel or the writer's backlog was full.
    pub fn drops(&self) -> Arc<AtomicU64> {
        self.drops.clone()
    }

    /// Where the health thread sends its reports. It may block while the
    /// writer is stuck on the disk; the health thread can wait.
    pub fn health(&self) -> SyncSender<Record> {
        self.tx.clone()
    }

    /// The clean stop: once every other sender is gone (the sink's with the
    /// pipeline thread, the health thread's), writes and flushes what is left.
    pub fn close(self) -> io::Result<Stats> {
        drop(self.tx);
        self.thread.join().unwrap_or_else(|p| {
            Err(io::Error::other(format!("the buffer writer thread panicked: {}", crate::panic_message(p.as_ref()))))
        })
    }
}

/// What the thread does with the writer. A trait, so tests can fail the disk.
pub(crate) trait Log {
    fn append(&mut self, payload: &[u8]) -> Result<(), Dropped>;
    fn tick(&mut self, now: Instant) -> io::Result<()>;
    fn stats(&self) -> &Stats;
    fn take_gaps(&mut self) -> Vec<Gap>;
    fn is_failing(&self) -> bool;
    fn disk_bytes(&self) -> u64;
    /// The final counts.
    fn close(self) -> io::Result<Stats>;
}

impl Log for Writer {
    fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
        Writer::append(self, payload)
    }
    fn tick(&mut self, now: Instant) -> io::Result<()> {
        Writer::tick(self, now)
    }
    fn stats(&self) -> &Stats {
        Writer::stats(self)
    }
    fn take_gaps(&mut self) -> Vec<Gap> {
        Writer::take_gaps(self)
    }
    fn is_failing(&self) -> bool {
        Writer::is_failing(self)
    }
    fn disk_bytes(&self) -> u64 {
        Writer::disk_bytes(self)
    }
    fn close(self) -> io::Result<Stats> {
        Writer::close(self)
    }
}

pub(crate) struct Core<W: Log> {
    writer: W,
    drops: Arc<AtomicU64>,
    /// Reported once, in the first report.
    recovery: Option<Recovery>,
    /// The writer's counters at the last report.
    reported: Stats,
    /// `drops` at the last report.
    reported_drops: u64,
    held: VecDeque<Vec<u8>>,
    last_tick: Option<Instant>,
    /// Whether the last tick failed: the log says so once per failure.
    failing: bool,
}

impl<W: Log> Core<W> {
    pub(crate) fn new(writer: W, drops: Arc<AtomicU64>, recovery: Recovery) -> Self {
        Core {
            writer,
            drops,
            recovery: Some(recovery),
            reported: Stats::default(),
            reported_drops: 0,
            held: VecDeque::new(),
            last_tick: None,
            failing: false,
        }
    }

    /// Until every sender is gone.
    pub(crate) fn run(&mut self, rx: &Receiver<Record>) {
        loop {
            match rx.recv_timeout(TICK_EVERY) {
                Ok(r) => {
                    self.handle(r);
                    // Take what is queued, but tick on time.
                    let since = Instant::now();
                    while since.elapsed() < TICK_EVERY {
                        match rx.try_recv() {
                            Ok(r) => self.handle(r),
                            Err(_) => break,
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick(Instant::now());
        }
    }

    pub(crate) fn handle(&mut self, r: Record) {
        self.retry_held();
        match r {
            Record::Event(e) => {
                if let Err(Dropped::BacklogFull) = self.writer.append(&encode_event(e)) {
                    self.drops.fetch_add(1, Ordering::Relaxed);
                }
                // `Rejected` is counted by the writer (`buffer.rejected`).
            }
            Record::Health(e) => {
                let bytes = encode_event(self.complete(e));
                if !self.held.is_empty() || self.writer.append(&bytes).is_err() {
                    if self.held.len() == HELD_CAP {
                        self.held.pop_front();
                    }
                    self.held.push_back(bytes);
                }
            }
        }
    }

    fn retry_held(&mut self) {
        while let Some(bytes) = self.held.front() {
            if self.writer.append(bytes).is_err() {
                return;
            }
            self.held.pop_front();
        }
    }

    pub(crate) fn tick(&mut self, now: Instant) {
        self.retry_held();
        let result = self.writer.tick(now);
        self.last_tick = Some(now);
        match result {
            Err(e) if !self.failing => {
                self.failing = true;
                tracing::warn!("buffer: writes are failing, retrying every 10 s: {e}");
            }
            Ok(()) if self.failing && !self.writer.is_failing() => {
                self.failing = false;
                tracing::info!("buffer: writes recovered");
            }
            _ => {}
        }
    }

    /// Fills in the buffer's figures since the last report.
    fn complete(&mut self, mut e: Event) -> Event {
        let EventKind::SensorHealth(a) = &mut e.kind else { return e };
        let SensorHealthAction::Report(r) = &mut a.action;
        let now = self.writer.stats().clone();
        let (s0, s1) = (&self.reported, &now);
        let d = |f: fn(&Stats) -> u64| Some(f(s1).saturating_sub(f(s0)));
        let b = &mut r.buffer;
        b.write_errors = d(|s| s.write_errors);
        b.recoveries = d(|s| s.recoveries);
        b.rejected = d(|s| s.rejected);
        b.delete_failures = d(|s| s.delete_failures);
        b.failing = Some(self.writer.is_failing());
        b.disk_bytes = Some(self.writer.disk_bytes());
        r.housekeeping.retention_evictions = d(|s| s.retention_evictions);
        // Replaces the health thread's figure, read before the events queued
        // ahead of this report were appended.
        let drops = self.drops.load(Ordering::Relaxed);
        r.loss.buffer_backlog_drops = Some(drops.saturating_sub(self.reported_drops));
        self.reported_drops = drops;
        if let Some(rec) = self.recovery.take() {
            b.truncated_bytes = Some(rec.truncated_bytes);
            b.cursor_reset = Some(rec.cursor_reset);
            b.foreign_segments = Some(u64::from(rec.foreign_segment.is_some()));
        }
        r.gap = merge(self.writer.take_gaps());
        self.reported = now;
        e
    }

    /// The clean stop: what is held, then what is queued, is written; the
    /// counts are taken after that last write.
    pub(crate) fn close(mut self) -> io::Result<Stats> {
        self.retry_held();
        self.writer.close()
    }
}

/// A report carries one gap: the sum of the counts, the widest time range.
fn merge(gaps: Vec<Gap>) -> Option<SensorGap> {
    gaps.into_iter().map(|g| SensorGap { first_time: g.first_time, last_time: g.last_time, events: g.records }).reduce(
        |a, b| SensorGap {
            first_time: min_some(a.first_time, b.first_time),
            last_time: a.last_time.max(b.last_time),
            events: a.events + b.events,
        },
    )
}

fn min_some(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Snapshot, event, report};
    use atlas_buffer::{Cursor, Reader};
    use atlas_schema::classes::sensor_health::HealthReport;
    use atlas_schema::{BootId, Device, DeviceUid, EventId};

    fn device() -> Device {
        Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) }
    }

    fn health(time: i64) -> Event {
        let (r, _) = report(&Snapshot::at_start(true, 0), &Snapshot::at_start(true, 0));
        event(EventId::new_v7(), time, device(), 0, r)
    }

    /// Any small valid event: a Sensor Health one, sent as a plain event.
    fn plain(time: i64) -> Event {
        let mut e = health(time);
        let EventKind::SensorHealth(a) = &mut e.kind else { unreachable!() };
        a.interval_start = -1; // marks it as not passed through `complete`
        e
    }

    fn the_report(e: &Event) -> &HealthReport {
        let EventKind::SensorHealth(a) = &e.kind else { panic!("not Sensor Health") };
        let SensorHealthAction::Report(r) = &a.action;
        r
    }

    fn read_all(dir: &std::path::Path) -> Vec<Event> {
        let mut r = Reader::open(dir, Cursor::default());
        let mut out = Vec::new();
        while let Some(rec) = r.next_record().unwrap() {
            out.push(decode_event(&rec.payload).unwrap());
        }
        out
    }

    fn tempdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("atlas-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn events_and_reports_reach_the_disk_in_order_with_the_buffers_figures() {
        let dir = tempdir("order");
        let buf = dir.join("buffer");
        let (store, recovery) = Store::start(StoreConfig::new(atlas_buffer::Config::new(&buf))).unwrap();
        assert_eq!(recovery, Recovery::default());
        let sink = store.sink();
        for t in 0..1_000 {
            sink.event(plain(t));
        }
        store.health().send(Record::Health(health(5_000))).unwrap();
        sink.event(plain(5_001));
        drop(sink);
        let stats = store.close().unwrap();
        assert_eq!(stats.records_written, 1_002);

        let got = read_all(&buf);
        assert_eq!(got.len(), 1_002);
        assert!(got.iter().map(|e| e.meta.time).is_sorted());
        let r = the_report(&got[1_000]);
        assert_eq!(r.buffer.failing, Some(false));
        assert_eq!(r.buffer.write_errors, Some(0));
        assert_eq!(r.buffer.truncated_bytes, Some(0), "the first report carries the recovery");
        assert_eq!(r.buffer.cursor_reset, Some(false));
        assert!(r.buffer.disk_bytes.is_some());
        assert_eq!(r.housekeeping.retention_evictions, Some(0));
        assert_eq!(r.gap, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The counts `close` returns include what its own last write wrote
    /// (found by the binary test: `records=` fell short of the disk).
    #[test]
    fn close_counts_what_its_last_write_wrote() {
        let dir = tempdir("close-counts");
        let buf = dir.join("buffer");
        let (store, _) = Store::start(StoreConfig::new(atlas_buffer::Config::new(&buf))).unwrap();
        let sink = store.sink();
        sink.event(plain(0)); // written by the first tick, which is always due
        std::thread::sleep(Duration::from_millis(300));
        for t in 1..=10 {
            sink.event(plain(t)); // the next flush is a second away: left to close
        }
        drop(sink);
        let stats = store.close().unwrap();
        assert_eq!(read_all(&buf).len(), 11);
        assert_eq!(stats.records_written, 11);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_first_report_carries_the_recovery() {
        let dir = tempdir("recovery");
        let buf = dir.join("buffer");
        let (store, _) = Store::start(StoreConfig::new(atlas_buffer::Config::new(&buf))).unwrap();
        store.health().send(Record::Health(health(1))).unwrap();
        store.health().send(Record::Health(health(2))).unwrap();
        store.close().unwrap();
        let got = read_all(&buf);
        assert_eq!(the_report(&got[0]).buffer.cursor_reset, Some(false));
        assert_eq!(the_report(&got[1]).buffer.cursor_reset, None);
        assert_eq!(the_report(&got[1]).buffer.truncated_bytes, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_channel_drops_and_counts_never_blocks() {
        let (tx, rx) = sync_channel(2);
        let drops = Arc::new(AtomicU64::new(0));
        let sink = Sink { tx, drops: drops.clone() };
        // On its own thread: a send that blocked would fail the test, not hang
        // it (review finding 8); dropping `rx` at the end frees such a thread.
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for t in 0..10 {
                sink.event(plain(t));
            }
            let _ = done_tx.send(());
        });
        assert!(done.recv_timeout(Duration::from_secs(5)).is_ok(), "the sink blocked on a full channel");
        assert_eq!(drops.load(Ordering::Relaxed), 8);
        drop(rx);
    }

    /// A disk that refuses everything, with a backlog that holds `room` records.
    #[derive(Default)]
    struct FullDisk {
        room: usize,
        appended: Vec<Vec<u8>>,
        stats: Stats,
        gaps: Vec<Gap>,
    }

    impl Log for FullDisk {
        fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
            if self.appended.len() >= self.room {
                self.stats.backlog_drops += 1;
                return Err(Dropped::BacklogFull);
            }
            self.appended.push(payload.to_vec());
            Ok(())
        }
        fn tick(&mut self, _: Instant) -> io::Result<()> {
            self.stats.write_errors += 1;
            Err(io::Error::other("disk full"))
        }
        fn stats(&self) -> &Stats {
            &self.stats
        }
        fn take_gaps(&mut self) -> Vec<Gap> {
            std::mem::take(&mut self.gaps)
        }
        fn is_failing(&self) -> bool {
            true
        }
        fn disk_bytes(&self) -> u64 {
            0
        }
        fn close(self) -> io::Result<Stats> {
            Ok(self.stats)
        }
    }

    #[test]
    fn a_report_the_backlog_cannot_take_is_held_and_written_first() {
        let drops = Arc::new(AtomicU64::new(0));
        let mut core = Core::new(FullDisk { room: 1, ..FullDisk::default() }, drops.clone(), Recovery::default());
        core.handle(Record::Event(plain(1)));
        core.handle(Record::Event(plain(2))); // the backlog is full
        core.handle(Record::Health(health(3)));
        core.tick(Instant::now());
        assert_eq!(drops.load(Ordering::Relaxed), 1, "the event is counted as lost");
        assert_eq!(core.held.len(), 1, "the report is held");

        // Room again: the held report goes before the next event.
        core.writer.room = 3;
        core.handle(Record::Event(plain(4)));
        let times: Vec<i64> = core.writer.appended.iter().map(|b| decode_event(b).unwrap().meta.time).collect();
        assert_eq!(times, [1, 3, 4]);
        let held = decode_event(&core.writer.appended[1]).unwrap();
        assert_eq!(the_report(&held).buffer.write_errors, Some(0), "figures as of when it was made");
        assert_eq!(the_report(&held).buffer.failing, Some(true));
    }

    /// Each report counts the writer's figures since the report before it.
    #[test]
    fn each_report_counts_since_the_last_one() {
        let mut core =
            Core::new(FullDisk { room: 10, ..FullDisk::default() }, Arc::new(AtomicU64::new(0)), Recovery::default());
        core.tick(Instant::now());
        core.tick(Instant::now());
        core.handle(Record::Health(health(1)));
        core.tick(Instant::now());
        core.handle(Record::Health(health(2)));
        let reports: Vec<Event> = core.writer.appended.iter().map(|b| decode_event(b).unwrap()).collect();
        assert_eq!(the_report(&reports[0]).buffer.write_errors, Some(2));
        assert_eq!(the_report(&reports[1]).buffer.write_errors, Some(1));
    }

    /// A report counts the drops up to it: the writer's, of the events queued
    /// ahead of it, and the sink's (review finding 3: the final report missed
    /// those made while the writer drained at stop).
    #[test]
    fn a_report_counts_the_backlog_drops_of_everything_queued_before_it() {
        let drops = Arc::new(AtomicU64::new(0));
        let mut core = Core::new(FullDisk { room: 1, ..FullDisk::default() }, drops.clone(), Recovery::default());
        core.handle(Record::Event(plain(1)));
        core.handle(Record::Event(plain(2))); // the backlog is full: dropped
        drops.fetch_add(2, Ordering::Relaxed); // two the sink could not queue
        core.handle(Record::Health(health(3))); // made before any drop: says 0
        core.writer.room = 10;
        core.handle(Record::Event(plain(4))); // writes the held report first
        core.handle(Record::Health(health(5)));
        let reports: Vec<Event> = core
            .writer
            .appended
            .iter()
            .map(|b| decode_event(b).unwrap())
            .filter(|e| matches!(&e.kind, EventKind::SensorHealth(a) if a.interval_start == 0))
            .collect();
        assert_eq!(reports.len(), 2);
        assert_eq!(the_report(&reports[0]).loss.buffer_backlog_drops, Some(3));
        assert_eq!(the_report(&reports[1]).loss.buffer_backlog_drops, Some(0), "since the last report");
    }

    /// Tests: a writer that panics once it has taken `left` records.
    pub(super) struct PanicsAfter<W> {
        pub(super) inner: W,
        pub(super) left: u64,
    }

    impl<W: Log> Log for PanicsAfter<W> {
        fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
            assert!(self.left > 0, "test: the buffer writer fails");
            self.left -= 1;
            self.inner.append(payload)
        }
        fn tick(&mut self, now: Instant) -> io::Result<()> {
            self.inner.tick(now)
        }
        fn stats(&self) -> &Stats {
            self.inner.stats()
        }
        fn take_gaps(&mut self) -> Vec<Gap> {
            self.inner.take_gaps()
        }
        fn is_failing(&self) -> bool {
            self.inner.is_failing()
        }
        fn disk_bytes(&self) -> u64 {
            self.inner.disk_bytes()
        }
        fn close(self) -> io::Result<Stats> {
            self.inner.close()
        }
    }

    /// Review finding 7: a writer thread that dies is seen, and `close` says
    /// why instead of panicking in turn.
    #[test]
    fn a_writer_that_dies_is_seen_and_close_says_why() {
        let writer = PanicsAfter { inner: FullDisk { room: 10, ..FullDisk::default() }, left: 1 };
        let store = Store::spawn(writer, Recovery::default(), 16);
        let sink = store.sink();
        sink.event(plain(1));
        sink.event(plain(2)); // the writer panics on it
        let t0 = Instant::now();
        while store.is_running() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!store.is_running(), "the writer's end is seen");
        drop(sink);
        let e = store.close().unwrap_err();
        assert!(e.to_string().contains("test: the buffer writer fails"), "{e}");
    }

    #[test]
    fn held_reports_are_bounded() {
        let mut core = Core::new(FullDisk::default(), Arc::new(AtomicU64::new(0)), Recovery::default());
        for t in 0..(HELD_CAP as i64 + 4) {
            core.handle(Record::Health(health(t)));
        }
        assert_eq!(core.held.len(), HELD_CAP);
        assert_eq!(decode_event(&core.held[0]).unwrap().meta.time, 4, "the oldest went");
    }

    #[test]
    fn gaps_are_merged_into_one() {
        let gaps = vec![
            Gap { records: 3, first_time: Some(50), last_time: Some(60) },
            Gap { records: 2, first_time: None, last_time: None },
            Gap { records: 1, first_time: Some(10), last_time: Some(20) },
        ];
        assert_eq!(merge(gaps), Some(SensorGap { first_time: Some(10), last_time: Some(60), events: 6 }));
        assert_eq!(merge(Vec::new()), None);
        let mut core = Core::new(FullDisk::default(), Arc::new(AtomicU64::new(0)), Recovery::default());
        core.writer.gaps = vec![Gap { records: 4, first_time: Some(1), last_time: Some(2) }];
        let e = core.complete(health(9));
        assert_eq!(the_report(&e).gap, Some(SensorGap { first_time: Some(1), last_time: Some(2), events: 4 }));
    }
}
