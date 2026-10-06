//! What each ETW callback does before an event reaches the pipeline (sensor
//! spec §3.2 [1], §4.4, §7.5). It runs on a consumer thread and must stay fast:
//! a hash-map operation or two per event, never a blocking call.
//!
//! - Parse failures are counted (`parse_errors`, `unknown_version`).
//! - Successful OperationEnds are discarded: only failures matter (§5.5).
//! - Session A keeps the early registry key map and sends fast-path value reads.
//! - DNS-Client events pass a per-PID token bucket, then go to the user-mode
//!   queue; everything else goes to the kernel queue. Full queues drop and count.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, TrySendError};

use atlas_etw::parse::{ParseError, RawEvent};

use crate::config::{Config, Ticks};
use crate::counters::IntakeCounters;
use crate::input::{Header, Incoming, Session};
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
        match &event {
            RawEvent::FileOpEnd(o) if !o.failed() => {
                IntakeCounters::bump(&self.counters.op_end_discarded);
                return;
            }
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
        let inc = Incoming { header, event };
        if matches!(inc.event, RawEvent::DnsQuery(_)) {
            if !self.dns.take(header.pid, header.ts) {
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
    use atlas_etw::parse::{DnsQuery, FileOpEnd, RegKey, RegOpen, RegSetValue, WStr};
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
