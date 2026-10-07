//! What each ETW callback does before an event reaches the pipeline (sensor
//! spec §3.2 [1], §4.4, §7.5). It runs on a consumer thread and must stay fast:
//! a hash-map operation or two per event, never a blocking call.
//!
//! - Parse failures are counted (`parse_errors`, `unknown_version`).
//! - Successful OperationEnds are discarded: only failures matter (§5.5), and
//!   the Cleanup outcomes that report a delete (`cleanup`; plan 1b-3c).
//! - Session A keeps the early registry key map and sends fast-path value reads.
//! - DNS-Client events pass a per-PID token bucket, then go to the user-mode
//!   queue; everything else goes to the kernel queue. Full queues drop and count.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, TrySendError};

use atlas_etw::parse::{FileOpEnd, ParseError, RawEvent};

use crate::cleanup;
use crate::config::{Config, Ticks};
use crate::counters::IntakeCounters;
use crate::input::{Header, Incoming, Session};
use crate::recent::Recent;
use crate::services::{EarlyKey, ValueRead};

/// Sends a fast-path read to the reader lane (plan 1b-3b). Must not block.
pub type FastRead = Box<dyn FnMut(EarlyKey, ValueRead) + Send>;

/// The two queues into the pipeline (§3.2): bounded, never blocking.
pub struct Queues {
    /// Kernel providers and Session B (default 65,536 entries).
    pub kernel: SyncSender<Incoming>,
    /// User-mode providers, DNS-Client (default 8,192), so a forger cannot
    /// push kernel events out (§4.4).
    pub user: SyncSender<Incoming>,
}

pub struct Intake {
    kernel: SyncSender<Incoming>,
    user: SyncSender<Incoming>,
    counters: Arc<IntakeCounters>,
    dns: Buckets,
    early: Option<EarlyKeys>,
    fast_read: Option<FastRead>,
    self_keys: HashSet<u64>,
    cleanups: Cleanups,
}

impl Intake {
    /// `fast_read` is `Some` for Session A when value reads are on; Session B
    /// passes `None` (it has no registry events).
    pub fn new(
        session: Session,
        cfg: &Config,
        ticks: Ticks,
        queues: Queues,
        counters: Arc<IntakeCounters>,
        fast_read: Option<FastRead>,
        self_keys: &[u64],
    ) -> Self {
        let early = (session == Session::Sensor && fast_read.is_some() && cfg.registry_value_reads)
            .then(|| EarlyKeys::new(cfg.early_key_map_cap));
        Intake {
            kernel: queues.kernel,
            user: queues.user,
            cleanups: Cleanups::new(ticks, counters.clone()),
            counters,
            dns: Buckets::new(cfg.dns_rate_per_pid, ticks.frequency),
            early,
            fast_read,
            self_keys: self_keys.iter().copied().collect(),
        }
    }

    pub fn on_event(&mut self, header: Header, parsed: Result<RawEvent, ParseError>) {
        let event = match parsed {
            Ok(e) => e,
            Err(ParseError::UnknownEvent) => return,
            Err(ParseError::UnsupportedVersion { .. } | ParseError::NewerVersion { .. }) => {
                IntakeCounters::bump(&self.counters.unknown_version);
                return;
            }
            Err(_) => {
                IntakeCounters::bump(&self.counters.parse_errors);
                return;
            }
        };
        if let Some(irp) = cleanup::op_irp(&event) {
            self.cleanups.next_op(irp);
        }
        let mut early_outcome = None;
        match &event {
            RawEvent::FileOpEnd(o) if !o.failed() => {
                if let End::Discard = self.cleanups.end(header, o.irp, o.extra_information) {
                    IntakeCounters::bump(&self.counters.op_end_discarded);
                    return;
                }
            }
            RawEvent::FileOpEnd(o) => self.cleanups.failed(o.irp),
            RawEvent::FileCleanup(x) => {
                early_outcome = self
                    .cleanups
                    .cleanup(x.irp, x.file_object, x.file_key, header.ts)
                    .map(|(h, info)| (h, FileOpEnd { irp: x.irp, extra_information: info, status: 0 }));
            }
            RawEvent::FileClose(x) => self.cleanups.close(x.file_object),
            RawEvent::FileCreate(c) if c.delete_on_close() => self.cleanups.delete_on_close(c.file_object, header.ts),
            RawEvent::FileDeletePath(p) => self.cleanups.request(p.file_key, p.irp, header.ts),
            RawEvent::FileSetDelete(i) if i.extra_information != 0 => {
                self.cleanups.request(i.file_key, i.irp, header.ts)
            }
            RawEvent::FileSetDelete(i) => self.cleanups.clear(i.file_key, i.file_object),
            RawEvent::RegCreateKey(o) | RawEvent::RegOpenKey(o) if o.status == 0 => {
                if let Some(m) = &mut self.early {
                    let evicted = m.open(o.key_object, o.base_object, &o.relative_name.to_string_lossy());
                    if evicted > 0 {
                        self.counters.early_key_map_evictions.fetch_add(evicted, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            RawEvent::RegCloseKey(k) => {
                // The agent's own closes are ignored here too (§7.4).
                if let Some(m) = &mut self.early
                    && !header.start_key.is_some_and(|s| self.self_keys.contains(&s))
                {
                    m.close(k.key_object);
                }
            }
            RawEvent::RegSetValue(v) if v.status == 0 => {
                if let (Some(m), Some(send)) = (&self.early, &mut self.fast_read)
                    && let Some(path) = m.name(v.key_object)
                {
                    IntakeCounters::bump(&self.counters.fast_reads);
                    send(
                        EarlyKey { ts: header.ts, tid: header.tid, key_object: v.key_object },
                        ValueRead { key_path: path.to_string(), value_name: v.value_name.as_units().to_vec() },
                    );
                }
            }
            _ => {}
        }
        self.send(Incoming { header, event });
        if let Some((h, o)) = early_outcome {
            // After its Cleanup in the queue; the ordering stage sorts by time anyway.
            self.send(Incoming { header: h, event: RawEvent::FileOpEnd(o) });
        }
    }

    fn send(&mut self, inc: Incoming) {
        if matches!(inc.event, RawEvent::DnsQuery(_)) {
            if !self.dns.take(inc.header.pid, inc.header.ts) {
                IntakeCounters::bump(&self.counters.dns_rate_limit_drops);
                return;
            }
            if let Err(TrySendError::Full(_)) = self.user.try_send(inc) {
                IntakeCounters::bump(&self.counters.user_queue_drops);
            }
        } else if let Err(TrySendError::Full(_)) = self.kernel.try_send(inc) {
            IntakeCounters::bump(&self.counters.kernel_queue_drops);
        }
    }
}

/// What the callback keeps to pass on Cleanup outcomes (plan 1b-3c, D3):
/// - each Cleanup's Irp until its OperationEnd;
/// - an outcome that arrived before its Cleanup (logged on another CPU, R-M4);
/// - the files with a delete request outstanding, for the file systems that
///   report no outcome (SMB), and each request's Irp until its OperationEnd.
///
/// Irps are per thread and reused for every operation, so an event that starts
/// another operation on an Irp ends whatever the Irp did before: a Cleanup still
/// waiting is counted as unpaired (`file_cleanup_unpaired`), never paired with a
/// later operation's OperationEnd. Bounded: the Irp maps clear past their cap
/// (their entries go at once unless events are lost), the requests forget the
/// oldest.
pub(crate) struct Cleanups {
    /// Cleanup Irp → (FileObject, FileKey, QPC).
    irps: HashMap<u64, (u64, u64, i64)>,
    /// A successful OperationEnd that reports a removed name and found no
    /// Cleanup: Irp → its header and outcome.
    early: HashMap<u64, (Header, u64)>,
    /// FileKeys with a delete requested (`DeletePath`, `SetDelete` set, or a
    /// delete-on-close handle's Cleanup) and not cleared or reported since.
    requested: Recent<u64>,
    /// A request's Irp → its FileKey, until the request's OperationEnd: a
    /// failed one takes the request back, a successful one is passed on so the
    /// pipeline knows the request stood (R-M1).
    request_irps: HashMap<u64, u64>,
    /// Handles opened delete-on-close and not closed yet.
    on_close: Recent<u64>,
    /// An outcome comes within this many ticks of its Cleanup (1 s). The rules
    /// that keep a Cleanup from pairing with another operation are the next
    /// operation and the value check; the window bounds what neither sees
    /// (an operation whose start is not logged, such as a query).
    window: i64,
    counters: Arc<IntakeCounters>,
}

/// What to do with a successful OperationEnd.
enum End {
    Pass,
    Discard,
}

const CLEANUP_IRPS_CAP: usize = 4096;
const DELETE_REQUESTS_CAP: usize = 4096;

impl Cleanups {
    fn new(ticks: Ticks, counters: Arc<IntakeCounters>) -> Self {
        Cleanups {
            irps: HashMap::new(),
            early: HashMap::new(),
            requested: Recent::new(DELETE_REQUESTS_CAP),
            request_irps: HashMap::new(),
            on_close: Recent::new(DELETE_REQUESTS_CAP),
            window: ticks.frequency.max(1),
            counters,
        }
    }

    /// Another operation starts on `irp` (any Kernel-File event with an Irp
    /// but a Cleanup or an OperationEnd).
    fn next_op(&mut self, irp: u64) {
        self.early.remove(&irp);
        self.request_irps.remove(&irp);
        if self.irps.remove(&irp).is_some() {
            IntakeCounters::bump(&self.counters.cleanup_unpaired);
        }
    }

    /// A Cleanup. Returns the outcome to pass on now, if it arrived first.
    fn cleanup(&mut self, irp: u64, fo: u64, key: u64, ts: i64) -> Option<(Header, u64)> {
        let early = self.early.remove(&irp);
        self.next_op(irp);
        if self.on_close.get(&fo).is_some() {
            // The FileKey is first known here: a later Cleanup on another
            // handle may be the one that deletes.
            self.requested.insert(key, ts);
        }
        if let Some((h, info)) = early
            && h.ts >= ts
            && h.ts - ts <= self.window
        {
            return self.decide(fo, key, info).then_some((h, info));
        }
        if self.irps.len() >= CLEANUP_IRPS_CAP {
            self.irps.clear();
        }
        self.irps.insert(irp, (fo, key, ts));
        None
    }

    /// A successful OperationEnd: passed on if it is a Cleanup's and reports a
    /// delete, or reports nothing for a file whose delete was requested; or if
    /// it is a delete request's.
    fn end(&mut self, header: Header, irp: u64, info: u64) -> End {
        let request = self.request_irps.remove(&irp).is_some();
        let pass = match self.irps.remove(&irp) {
            Some((fo, key, ts)) if header.ts >= ts && header.ts - ts <= self.window => self.decide(fo, key, info),
            Some(_) => {
                IntakeCounters::bump(&self.counters.cleanup_outcome_late);
                false
            }
            None => {
                if cleanup::removed(info) {
                    if self.early.len() >= CLEANUP_IRPS_CAP {
                        self.early.clear();
                    }
                    self.early.insert(irp, (header, info));
                }
                false
            }
        };
        if pass || request { End::Pass } else { End::Discard }
    }

    /// Whether a Cleanup's outcome is passed on. A delete passed on is no
    /// longer outstanding.
    fn decide(&mut self, fo: u64, key: u64, info: u64) -> bool {
        let requested = self.requested.get(&key).is_some() || self.on_close.get(&fo).is_some();
        let pass = cleanup::removed(info) || (info == cleanup::UNKNOWN && requested);
        if pass {
            self.requested.remove(&key);
        }
        pass
    }

    /// A failed OperationEnd: a failed request is taken back.
    fn failed(&mut self, irp: u64) {
        self.irps.remove(&irp);
        self.early.remove(&irp);
        if let Some(key) = self.request_irps.remove(&irp) {
            self.requested.remove(&key);
        }
    }

    fn close(&mut self, fo: u64) {
        if !self.on_close.is_empty() {
            self.on_close.remove(&fo);
        }
    }

    fn delete_on_close(&mut self, fo: u64, ts: i64) {
        self.on_close.insert(fo, ts);
    }

    fn request(&mut self, key: u64, irp: u64, ts: i64) {
        if self.request_irps.len() >= CLEANUP_IRPS_CAP {
            self.request_irps.clear();
        }
        self.request_irps.insert(irp, key);
        self.requested.insert(key, ts);
    }

    /// A `SetDelete` clear. On a delete-on-close handle it is taken as
    /// `FileDispositionInformationEx` clearing that handle's flag, and leaves a
    /// disposition another handle set (R-m3); otherwise it clears the file's.
    fn clear(&mut self, key: u64, fo: u64) {
        if self.on_close.remove(&fo).is_none() {
            self.requested.remove(&key);
        }
    }
}

/// The early key map (§7.5): full names only, in arrival order, no seeding.
/// A relative open whose base is unknown is simply not cached; a miss costs
/// only a fall back to the ordered path. Bounded, evicting the oldest.
pub struct EarlyKeys {
    names: HashMap<u64, (String, u64)>,
    order: VecDeque<(u64, u64)>,
    generation: u64,
    cap: usize,
}

impl EarlyKeys {
    pub fn new(cap: usize) -> Self {
        EarlyKeys { names: HashMap::new(), order: VecDeque::new(), generation: 0, cap }
    }

    /// Returns how many entries were evicted to make room.
    pub fn open(&mut self, key: u64, base: u64, relative: &str) -> u64 {
        let full = if relative.get(..10).is_some_and(|p| p.eq_ignore_ascii_case(r"\REGISTRY\")) {
            Some(relative.to_string())
        } else {
            self.names.get(&base).map(|(b, _)| if relative.is_empty() { b.clone() } else { format!("{b}\\{relative}") })
        };
        match full {
            Some(name) => {
                self.generation += 1;
                self.names.insert(key, (name, self.generation));
                self.order.push_back((key, self.generation));
                self.trim()
            }
            None => {
                self.names.remove(&key);
                0
            }
        }
    }

    pub fn close(&mut self, key: u64) {
        self.names.remove(&key);
    }

    pub fn name(&self, key: u64) -> Option<&str> {
        self.names.get(&key).map(|(n, _)| n.as_str())
    }

    fn trim(&mut self) -> u64 {
        let mut evicted = 0;
        while self.names.len() > self.cap {
            let Some((k, g)) = self.order.pop_front() else { break };
            if self.names.get(&k).is_some_and(|(_, gen_)| *gen_ == g) {
                self.names.remove(&k);
                evicted += 1;
            }
        }
        // Stale order entries (closed or replaced keys) are dropped as they surface.
        if self.order.len() > self.cap.saturating_mul(2) {
            let names = &self.names;
            self.order.retain(|(k, g)| names.get(k).is_some_and(|(_, gen_)| gen_ == g));
        }
        evicted
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Per-PID token buckets for DNS-Client (§4.4), refilled by event time.
struct Buckets {
    /// Tokens are kept in millitokens.
    rate: u64,
    frequency: i64,
    by_pid: HashMap<u32, (u64, i64)>,
}

impl Buckets {
    fn new(rate: u32, frequency: i64) -> Self {
        Buckets { rate: u64::from(rate) * 1000, frequency, by_pid: HashMap::new() }
    }

    fn take(&mut self, pid: u32, ts: i64) -> bool {
        if self.rate == 0 {
            return true;
        }
        if self.by_pid.len() > 8192 {
            // Forget PIDs idle for 10 s: a full bucket is the same as no bucket.
            let horizon = ts.saturating_sub(self.frequency.saturating_mul(10));
            self.by_pid.retain(|_, (_, last)| *last >= horizon);
        }
        let (rate, freq) = (self.rate, self.frequency);
        let (tokens, last) = self.by_pid.entry(pid).or_insert((rate, ts));
        let elapsed = u64::try_from(ts.saturating_sub(*last)).unwrap_or(0);
        let refill = u128::from(elapsed) * u128::from(rate) / u128::from(freq.max(1) as u64);
        *tokens = (u128::from(*tokens) + refill).min(u128::from(rate)) as u64;
        *last = (*last).max(ts);
        if *tokens >= 1000 {
            *tokens -= 1000;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_etw::parse::{
        DnsQuery, FileCreate, FileHandle, FileOpEnd, FileSetInfo, RegKey, RegOpen, RegSetValue, WStr,
    };
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, sync_channel};

    fn header(pid: u32, ts: i64) -> Header {
        Header { session: Session::Sensor, pid, tid: 9, ts, start_key: Some(77) }
    }

    type Reads = Arc<Mutex<Vec<(EarlyKey, ValueRead)>>>;

    fn intake(kernel_cap: usize) -> (Intake, Receiver<Incoming>, Receiver<Incoming>, Arc<IntakeCounters>, Reads) {
        let (ktx, krx) = sync_channel(kernel_cap);
        let (utx, urx) = sync_channel(4);
        let counters = Arc::new(IntakeCounters::default());
        let reads: Reads = Arc::new(Mutex::new(Vec::new()));
        let r = reads.clone();
        let cfg = Config { dns_rate_per_pid: 2, ..Config::default() };
        let i = Intake::new(
            Session::Sensor,
            &cfg,
            Ticks::new(1000),
            Queues { kernel: ktx, user: utx },
            counters.clone(),
            Some(Box::new(move |k, v| r.lock().unwrap().push((k, v)))),
            &[123],
        );
        (i, krx, urx, counters, reads)
    }

    fn dns() -> RawEvent {
        RawEvent::DnsQuery(DnsQuery {
            query_name: "a".into(),
            query_type: 1,
            query_options: 0,
            query_status: 0,
            query_results: WStr::default(),
        })
    }

    #[test]
    fn successful_op_ends_are_discarded_failures_pass() {
        let (mut i, krx, _, c, _) = intake(8);
        i.on_event(header(1, 0), Ok(RawEvent::FileOpEnd(FileOpEnd { irp: 1, extra_information: 0, status: 0 })));
        i.on_event(header(1, 0), Ok(RawEvent::FileOpEnd(FileOpEnd { irp: 2, extra_information: 0, status: 0x104 })));
        i.on_event(
            header(1, 0),
            Ok(RawEvent::FileOpEnd(FileOpEnd { irp: 3, extra_information: 0, status: 0xC000_0035 })),
        );
        assert_eq!(IntakeCounters::get(&c.op_end_discarded), 2);
        assert!(matches!(krx.try_recv().unwrap().event, RawEvent::FileOpEnd(o) if o.irp == 3));
        assert!(krx.try_recv().is_err());
    }

    fn ev(i: &mut Intake, e: RawEvent) {
        i.on_event(header(1, 0), Ok(e));
    }

    fn cleanup(irp: u64, fo: u64, file_key: u64) -> RawEvent {
        RawEvent::FileCleanup(FileHandle { irp, file_object: fo, file_key, issuing_tid: 1 })
    }

    fn outcome(irp: u64, info: u64) -> RawEvent {
        RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: info, status: 0 })
    }

    fn passed_outcomes(krx: &Receiver<Incoming>) -> Vec<(u64, u64)> {
        krx.try_iter()
            .filter_map(|inc| match inc.event {
                RawEvent::FileOpEnd(o) if !o.failed() => Some((o.irp, o.extra_information)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cleanup_outcomes_pass_when_they_report_a_delete() {
        let (mut i, krx, _, c, _) = intake(64);
        // Removed names pass; a file that remains does not, nor an OperationEnd
        // of another operation that happens to carry the same number.
        for (irp, info) in [(1, cleanup::FILE_DELETED), (2, cleanup::LINK_DELETED), (3, cleanup::STREAM_DELETED)] {
            ev(&mut i, cleanup(irp, 0x10 + irp, 0x20 + irp));
            ev(&mut i, outcome(irp, info));
        }
        ev(&mut i, cleanup(4, 0x14, 0x24));
        ev(&mut i, outcome(4, cleanup::FILE_REMAINS));
        ev(&mut i, outcome(5, cleanup::FILE_DELETED)); // a write of 4 bytes, say
        assert_eq!(passed_outcomes(&krx), [(1, 4), (2, 8), (3, 0x10)]);
        assert_eq!(IntakeCounters::get(&c.op_end_discarded), 2);
    }

    #[test]
    fn an_unknown_outcome_passes_only_for_a_requested_delete() {
        let (mut i, krx, _, _, _) = intake(64);
        let set = |key, on| {
            RawEvent::FileSetDelete(FileSetInfo {
                irp: 0,
                file_object: 0x10,
                file_key: key,
                extra_information: on,
                issuing_tid: 1,
                info_class: 13,
            })
        };
        // Asked for: passes.
        ev(&mut i, set(0x20, 1));
        ev(&mut i, cleanup(1, 0x10, 0x20));
        ev(&mut i, outcome(1, cleanup::UNKNOWN));
        // Asked for, then cleared: does not.
        ev(&mut i, set(0x21, 1));
        ev(&mut i, set(0x21, 0));
        ev(&mut i, cleanup(2, 0x10, 0x21));
        ev(&mut i, outcome(2, cleanup::UNKNOWN));
        // Never asked for: does not.
        ev(&mut i, cleanup(3, 0x11, 0x22));
        ev(&mut i, outcome(3, cleanup::UNKNOWN));
        // Asked for, but the request failed (a read-only file): does not, even
        // once the FileKey is reused.
        ev(
            &mut i,
            RawEvent::FileSetDelete(FileSetInfo {
                irp: 8,
                file_object: 0x10,
                file_key: 0x24,
                extra_information: 1,
                issuing_tid: 1,
                info_class: 13,
            }),
        );
        ev(&mut i, RawEvent::FileOpEnd(FileOpEnd { irp: 8, extra_information: 0, status: 0xC000_0121 }));
        ev(&mut i, cleanup(6, 0x14, 0x24));
        ev(&mut i, outcome(6, cleanup::UNKNOWN));
        // A delete-on-close handle: its own Cleanup, and another handle's after it.
        ev(
            &mut i,
            RawEvent::FileCreate(FileCreate {
                irp: 9,
                file_object: 0x12,
                issuing_tid: 1,
                create_options: FileCreate::DELETE_ON_CLOSE,
                create_attributes: 0,
                share_access: 7,
                file_name: WStr::default(),
            }),
        );
        ev(&mut i, cleanup(4, 0x12, 0x23));
        ev(&mut i, outcome(4, cleanup::UNKNOWN));
        // That outcome was the delete: the file's later Cleanups are not (R-m2).
        ev(&mut i, cleanup(5, 0x13, 0x23));
        ev(&mut i, outcome(5, cleanup::UNKNOWN));
        assert_eq!(passed_outcomes(&krx), [(1, 0), (4, 0)]);
    }

    fn ev_at(i: &mut Intake, ts: i64, e: RawEvent) {
        i.on_event(header(1, ts), Ok(e));
    }

    /// The thread moved to another CPU between the Cleanup and its
    /// OperationEnd, and the OperationEnd's buffer came first (review R-M4).
    #[test]
    fn an_outcome_delivered_before_its_cleanup_is_paired() {
        let (mut i, krx, _, _, _) = intake(64);
        ev(&mut i, outcome(1, cleanup::FILE_DELETED));
        ev(&mut i, cleanup(1, 0x10, 0x20));
        // The thread's next operation on the same Irp: a 100-byte write.
        ev(&mut i, outcome(1, 100));
        assert_eq!(passed_outcomes(&krx), [(1, 4)], "the delete passes, the write does not");
    }

    /// A Cleanup whose outcome never came is not paired with a later operation
    /// on its Irp, nor with an OperationEnd past the window; both are counted.
    #[test]
    fn an_unpaired_cleanup_is_counted_never_paired_later() {
        let (mut i, krx, _, c, _) = intake(64);
        ev_at(&mut i, 0, cleanup(1, 0x10, 0x20));
        ev_at(&mut i, 1, RawEvent::FileClose(FileHandle { irp: 1, file_object: 0x10, file_key: 0x20, issuing_tid: 1 }));
        ev_at(&mut i, 2, outcome(1, cleanup::FILE_DELETED)); // the Close's own: not a Cleanup's
        ev_at(&mut i, 3, cleanup(2, 0x11, 0x21));
        ev_at(&mut i, 1_500, outcome(2, cleanup::FILE_DELETED)); // 1.5 s later; the window is 1 s
        assert!(passed_outcomes(&krx).is_empty());
        assert_eq!(IntakeCounters::get(&c.cleanup_unpaired), 1);
        assert_eq!(IntakeCounters::get(&c.cleanup_outcome_late), 1);
    }

    /// A delete request's own OperationEnd is passed on, so the pipeline knows
    /// the request stood (review R-M1).
    #[test]
    fn a_requests_operation_end_is_passed_on() {
        let (mut i, krx, _, _, _) = intake(64);
        ev(
            &mut i,
            RawEvent::FileSetDelete(FileSetInfo {
                irp: 7,
                file_object: 0x10,
                file_key: 0x20,
                extra_information: 1,
                issuing_tid: 1,
                info_class: 13,
            }),
        );
        ev(&mut i, outcome(7, 0));
        assert_eq!(passed_outcomes(&krx), [(7, 0)]);
    }

    /// Clearing a delete-on-close handle's flag leaves another handle's
    /// request (review R-m3): an SMB Cleanup still passes.
    #[test]
    fn clearing_a_handles_delete_on_close_keeps_another_handles_request() {
        let (mut i, krx, _, _, _) = intake(64);
        let set = |fo, on| {
            RawEvent::FileSetDelete(FileSetInfo {
                irp: 9,
                file_object: fo,
                file_key: 0x20,
                extra_information: on,
                issuing_tid: 1,
                info_class: 64,
            })
        };
        ev(&mut i, set(0x10, 1));
        ev(
            &mut i,
            RawEvent::FileCreate(FileCreate {
                irp: 8,
                file_object: 0x12,
                issuing_tid: 1,
                create_options: FileCreate::DELETE_ON_CLOSE,
                create_attributes: 0,
                share_access: 7,
                file_name: WStr::default(),
            }),
        );
        ev(&mut i, set(0x12, 0));
        ev(&mut i, cleanup(3, 0x14, 0x20));
        ev(&mut i, outcome(3, cleanup::UNKNOWN));
        assert!(passed_outcomes(&krx).contains(&(3, 0)));
    }

    #[test]
    fn parse_failures_are_counted_by_kind() {
        let (mut i, _, _, c, _) = intake(8);
        i.on_event(header(1, 0), Err(ParseError::UnknownEvent));
        i.on_event(header(1, 0), Err(ParseError::NewerVersion { version: 9, newest: 4 }));
        i.on_event(header(1, 0), Err(ParseError::UnsupportedVersion { version: 1 }));
        i.on_event(header(1, 0), Err(ParseError::Truncated { field: "x", offset: 0 }));
        assert_eq!((IntakeCounters::get(&c.unknown_version), IntakeCounters::get(&c.parse_errors)), (2, 1));
    }

    #[test]
    fn a_full_queue_drops_and_counts() {
        let (mut i, krx, _, c, _) = intake(1);
        for irp in 0..3 {
            i.on_event(
                header(1, 0),
                Ok(RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: 0, status: 0xC000_0001 })),
            );
        }
        assert_eq!(IntakeCounters::get(&c.kernel_queue_drops), 2);
        assert!(krx.try_recv().is_ok());
    }

    #[test]
    fn dns_is_rate_limited_per_pid_on_its_own_queue() {
        let (mut i, krx, urx, c, _) = intake(8);
        // Rate 2/s at 1000 ticks/s: two pass, the third in the same instant drops.
        for _ in 0..3 {
            i.on_event(header(5, 0), Ok(dns()));
        }
        i.on_event(header(6, 0), Ok(dns())); // another PID has its own bucket
        i.on_event(header(5, 500), Ok(dns())); // half a second refills one token
        assert_eq!(IntakeCounters::get(&c.dns_rate_limit_drops), 1);
        assert_eq!(urx.try_iter().count(), 4);
        assert!(krx.try_recv().is_err());
    }

    fn open(key: u64, base: u64, rel: &str) -> RawEvent {
        RawEvent::RegOpenKey(RegOpen {
            base_object: base,
            key_object: key,
            status: 0,
            disposition: 0,
            base_name: WStr::default(),
            relative_name: rel.into(),
        })
    }

    fn set(key: u64, name: &str) -> RawEvent {
        RawEvent::RegSetValue(RegSetValue {
            key_object: key,
            status: 0,
            value_type: 4,
            data_size: 4,
            key_name: WStr::default(),
            value_name: name.into(),
            value_name_ambiguous: false,
            captured_data: Box::default(),
            previous_data_type: 0,
            previous_data_size: 0,
            previous_data: Box::default(),
        })
    }

    #[test]
    fn the_fast_path_reads_named_keys_only() {
        let (mut i, _, _, c, reads) = intake(64);
        i.on_event(header(1, 1), Ok(open(10, 0, r"\REGISTRY\MACHINE\SOFTWARE")));
        i.on_event(header(1, 2), Ok(open(11, 10, r"Microsoft\Windows\CurrentVersion\Run")));
        i.on_event(header(1, 3), Ok(set(11, "evil")));
        i.on_event(header(1, 4), Ok(open(12, 999, "Unknown")));
        i.on_event(header(1, 5), Ok(set(12, "x"))); // base unknown: no fast read
        let r = reads.lock().unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, EarlyKey { ts: 3, tid: 9, key_object: 11 });
        assert_eq!(r[0].1.key_path, r"\REGISTRY\MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Run");
        assert_eq!(IntakeCounters::get(&c.fast_reads), 1);
    }

    #[test]
    fn the_agents_own_closes_do_not_remove_names() {
        let (mut i, _, _, _, reads) = intake(64);
        i.on_event(header(1, 1), Ok(open(10, 0, r"\REGISTRY\MACHINE\X")));
        let close = RawEvent::RegCloseKey(RegKey { key_object: 10, status: 0, key_name: WStr::default() });
        let mut agent = header(1, 2);
        agent.start_key = Some(123);
        i.on_event(agent, Ok(close.clone()));
        i.on_event(header(1, 3), Ok(set(10, "v")));
        assert_eq!(reads.lock().unwrap().len(), 1);
        i.on_event(header(1, 4), Ok(close));
        i.on_event(header(1, 5), Ok(set(10, "v")));
        assert_eq!(reads.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_early_map_evicts_the_oldest() {
        let mut m = EarlyKeys::new(2);
        m.open(1, 0, r"\REGISTRY\A");
        m.open(2, 0, r"\REGISTRY\B");
        assert_eq!(m.open(3, 0, r"\REGISTRY\C"), 1);
        assert!(m.name(1).is_none() && m.name(3).is_some());
        // Re-opening refreshes an address's place.
        m.open(2, 0, r"\REGISTRY\B2");
        assert_eq!(m.open(4, 0, r"\REGISTRY\D"), 1);
        assert_eq!(m.name(2), Some(r"\REGISTRY\B2"));
        assert!(m.name(3).is_none());
    }
}
