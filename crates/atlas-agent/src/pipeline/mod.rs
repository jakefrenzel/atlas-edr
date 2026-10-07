//! The pipeline thread (sensor spec §3.2 [2] + [3] + [5]; plan 1b-3a decision D2):
//! one loop owns the ordering stage, all mutable state and the completion stage.
//!
//! The driver (plan 1b-4) calls, from one thread:
//! - [`Pipeline::push`] for each event from the kernel and user-mode queues;
//! - [`Pipeline::reply`] for each result from the workers, reader lane and seeder;
//! - [`Pipeline::tick`] with the current QPC, at least every ~50 ms, which returns
//!   the events ready to emit;
//! - [`Pipeline::take_requests`] after each call, and hands the requests out.
//!
//! Time is passed in, so every rule is testable without sleeping.

mod expand;
mod file;
mod net;
mod proc;
mod reg;
mod seed;

use std::collections::{HashMap, HashSet};

use atlas_schema::{Device, Event, EventId, EventKind, EventMeta, ProcessRef, ProcessUid, Sensor};

use crate::completion::{Completion, Expired, PendingId, Reason, Wait};
use crate::config::{Config, Ticks};
use crate::counters::{Class, Counters};
use crate::input::{Header, Incoming};
use crate::keymap::KeyMap;
use crate::ordering::{Ordering, Pushed};
use crate::process::{Identity, ProcInfo, ProcessCache, start_key};
use crate::services::{Lookups, Reply, Request};
use crate::time::{Anchor, Clock};
use crate::watchlist::{BadPattern, Watchlist};

pub use expand::Expands;
pub use file::Files;
pub use net::Network;
pub use proc::Launches;
pub use reg::Registry;
pub use seed::Seeding;

/// Produces event ids from the event's Unix time. The agent uses UUIDv7;
/// tests use a deterministic generator.
pub type IdGen = Box<dyn FnMut(i64) -> EventId + Send>;

/// What the pipeline needs to start.
pub struct Setup {
    pub config: Config,
    pub ticks: Ticks,
    pub anchor: Anchor,
    pub identity: Identity,
    /// `HKLM\SYSTEM\Select\Current`, read at start (§5.5).
    pub current_control_set: u32,
    /// The agent's own start key(s): its events are not emitted (§5.5).
    pub self_keys: Vec<u64>,
    /// QPC at start: the seeder deadline is longer at first (§3.2).
    pub started: i64,
}

pub struct Pipeline<L> {
    pub(crate) cfg: Config,
    pub(crate) ticks: Ticks,
    pub(crate) clock: Clock,
    pub(crate) id: Identity,
    pub(crate) ccs: u32,
    pub(crate) self_keys: HashSet<u64>,
    pub(crate) self_uids: HashSet<ProcessUid>,
    pub(crate) started: i64,
    pub(crate) lookups: L,
    pub(crate) ids: IdGen,
    ordering: Ordering,
    pub(crate) completion: Completion<Event>,
    pub(crate) procs: ProcessCache,
    pub(crate) launches: Launches,
    pub(crate) files: Files,
    pub(crate) keys: KeyMap,
    pub(crate) reg: Registry,
    pub(crate) net: Network,
    pub(crate) watch: Watchlist,
    pub(crate) seeding: Seeding,
    pub(crate) expands: Expands,
    pub(crate) requests: Vec<Request>,
    pub(crate) counters: Counters,
    /// Stream time (§3.2): the ordering watermark.
    pub(crate) stream: i64,
    /// The QPC passed to the last tick (wall time for deadlines).
    pub(crate) now: i64,
    /// Stream time of the next sweep over whole tables (once a second).
    next_sweep: i64,
}

impl<L: Lookups> Pipeline<L> {
    pub fn new(setup: Setup, lookups: L, ids: IdGen) -> Result<Self, BadPattern> {
        let Setup { config, ticks, anchor, identity, current_control_set, self_keys, started } = setup;
        let watch = Watchlist::new(config.watchlist.as_deref(), &config.watchlist_extend)?;
        let t = |d| ticks.of(d);
        let mut p = Pipeline {
            ordering: Ordering::new(t(config.hold)),
            completion: Completion::new(config.pending_cap),
            procs: ProcessCache::new(t(config.process_retention), config.process_cap),
            launches: Launches::default(),
            files: Files::new(config.file_map_cap, t(config.confirm_window)),
            keys: KeyMap::new(config.key_map_cap),
            reg: Registry::default(),
            net: Network::new(config.flow_cap, t(config.udp_idle)),
            seeding: Seeding::new(),
            expands: Expands::default(),
            self_uids: self_keys.iter().map(|k| identity.uid(*k)).collect(),
            self_keys: self_keys.into_iter().collect(),
            clock: Clock::new(ticks, anchor),
            cfg: config,
            ticks,
            id: identity,
            ccs: current_control_set,
            started,
            lookups,
            ids,
            watch,
            requests: Vec::new(),
            counters: Counters::default(),
            stream: i64::MIN,
            now: started,
            next_sweep: i64::MIN,
        };
        p.seed_builtins();
        if p.cfg.seed_on_start {
            p.seeding.start(&mut p.requests);
        }
        Ok(p)
    }

    /// One event from a queue (§3.2 [1] → [2]). A late event, one older than
    /// stream time, is processed at once.
    pub fn push(&mut self, inc: Incoming) {
        if inc.header.ts < self.stream {
            self.counters.late_arrivals += 1;
            self.process(inc);
            return;
        }
        if let Pushed::Late(inc) = self.ordering.push(inc) {
            self.counters.late_arrivals += 1;
            self.process(inc);
        }
    }

    /// A result from a worker, the reader lane or the seeder.
    pub fn reply(&mut self, r: Reply) {
        match r {
            Reply::Enriched { id, hashes, signature, error } => self.on_enriched(id, hashes, signature, error),
            Reply::ValueRead { id, result } => self.on_value_read(id, result),
            Reply::EarlyRead { event, read, result } => self.reg.on_early_read(event, read, result),
            Reply::Expanded { id, slot, long_path } => self.on_expanded(id, slot, long_path),
            Reply::Snapshot(s) => self.seeding.queue(s),
        }
    }

    /// Advances to `now` (QPC): releases held events, applies everything that
    /// waits on stream time, and returns the events ready to emit, in order.
    pub fn tick(&mut self, now: i64) -> Vec<Event> {
        self.now = now;
        for inc in self.ordering.release(now) {
            // Stream time reaches each event before it is processed, so a
            // window that closed before it is applied first.
            self.step(inc.header.ts);
            self.process(inc);
        }
        self.advance(self.ordering.watermark(now));
        self.emit(false)
    }

    /// A clean stop (§11.4): everything held is processed and everything
    /// pending goes out as is.
    pub fn stop(&mut self) -> Vec<Event> {
        for inc in self.ordering.drain() {
            self.step(inc.header.ts);
            self.process(inc);
        }
        self.advance(i64::MAX);
        self.net.close_all(&mut self.completion, &mut self.counters, &self.clock, &mut self.ids, &self.id);
        self.emit(true)
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub fn counters(&self) -> Counters {
        let mut c = self.counters.clone();
        c.process_cache_evictions = self.procs.evictions();
        c.key_map_evictions = self.keys.evictions();
        c.file_map_evictions = self.files.evictions();
        c.flow_table_evictions = self.net.evictions();
        c.pending_overflow = self.completion.overflow();
        c
    }

    /// Re-anchors `meta.time` (every 60 s, §3.3).
    pub fn set_anchor(&mut self, anchor: Anchor) {
        self.clock.set_anchor(anchor);
    }

    /// Adds a start key to the self-filter (the canary child, §5.5; plan 1b-4).
    pub fn add_self_key(&mut self, key: u64) {
        self.self_keys.insert(key);
        self.self_uids.insert(self.id.uid(key));
    }

    /// Stream time moves to `stream` between two events: seeder snapshots,
    /// confirm windows and launch halves that are due are applied, in that order.
    fn step(&mut self, stream: i64) {
        if stream <= self.stream {
            return;
        }
        self.stream = stream;
        self.apply_snapshots(stream);
        self.file_confirm(stream);
        self.launch_expiry(stream);
    }

    /// The end of a tick: stream time moves to the watermark, the seeder's
    /// questions go out, and once a second of stream time the sweeps over
    /// whole tables run (UDP idle, cache retention).
    fn advance(&mut self, stream: i64) {
        self.step(stream);
        let s = self.stream;
        self.seeding.flush_asks(self.now, self.ticks, &mut self.requests);
        self.reg.prune_early(s.saturating_sub(self.ticks.of(self.cfg.hold)));
        // Address history is only needed while a snapshot or a waiting event can predate it.
        self.seeding.expire(s.saturating_sub(self.ticks.of(self.cfg.seeder_startup_deadline).saturating_mul(4)));
        if s >= self.next_sweep {
            self.next_sweep = s.saturating_add(self.ticks.frequency);
            self.net.expire(s, &mut self.completion, &mut self.counters, &self.clock, &mut self.ids, &self.id);
            self.procs.expire(s);
        }
    }

    fn process(&mut self, inc: Incoming) {
        use atlas_etw::parse::RawEvent as R;
        let Incoming { header: h, event } = inc;
        match event {
            R::ProcessStart(s) => self.on_process_start(&h, s),
            R::ProcessStop(s) => self.on_process_stop(&h, s),
            R::ImageLoad(i) => self.on_image_load(&h, i),
            R::ClassicProcess(c) => self.on_classic(&h, c),
            R::FileCreate(c) => self.on_file_create(&h, c),
            R::FileCreateNew(c) => self.on_file_create_new(&h, c),
            R::FileCleanup(x) => self.on_file_cleanup(&h, x),
            R::FileClose(x) => self.on_file_close(&h, x),
            R::FileWrite(w) => self.on_file_write(&h, w.file_object),
            R::FileSetInfo(i) => self.on_file_set_info(&h, i),
            R::FileSetDelete(_) => {} // the delete logic comes in Task 3
            R::FileOpEnd(o) => self.on_file_op_end(&h, o),
            R::FileDeletePath(p) => self.on_file_delete_path(&h, p),
            R::FileRenamePath(p) => self.on_file_rename_path(&h, p),
            R::RegCreateKey(o) => self.on_reg_open(&h, o, true),
            R::RegOpenKey(o) => self.on_reg_open(&h, o, false),
            R::RegCloseKey(k) => self.on_reg_close(&h, k),
            R::RegDeleteKey(k) => self.on_reg_delete_key(&h, k),
            R::RegSetValue(v) => self.on_reg_set_value(&h, v),
            R::RegDeleteValue(v) => self.on_reg_delete_value(&h, v),
            R::TcpConnect(n) => self.on_tcp(&h, n, net::Tcp::Connect),
            R::TcpAccept(n) => self.on_tcp(&h, n, net::Tcp::Accept),
            R::TcpDisconnect(n) => self.on_tcp(&h, n, net::Tcp::Disconnect),
            R::UdpSend(n) => self.on_udp(&h, n, true),
            R::UdpRecv(n) => self.on_udp(&h, n, false),
            R::DnsQuery(q) => self.on_dns(&h, q),
        }
    }

    /// Drains the completion stage and applies the self-filter (§5.5).
    fn emit(&mut self, all: bool) -> Vec<Event> {
        let Pipeline { completion, counters, lookups, procs, now, id, .. } = self;
        let on_incomplete = |e: &mut Event, x: &[Expired]| finish_incomplete(e, x, counters, lookups, procs, id);
        let events = if all { completion.flush(on_incomplete) } else { completion.drain(*now, on_incomplete) };
        for id in self.completion.take_exited() {
            self.forget(id);
        }
        let mut out = Vec::with_capacity(events.len());
        for e in events {
            if self.is_self(&e) {
                self.counters.self_filtered += 1;
            } else {
                out.push(e);
            }
        }
        out
    }

    /// What is kept for pending events and recent history, by name (tests check
    /// it all goes once events leave and windows pass).
    #[cfg(test)]
    pub(crate) fn bookkeeping(&self) -> Vec<(&'static str, usize)> {
        let mut v = Vec::new();
        v.extend(self.launches.sizes());
        v.extend(self.expands.sizes());
        v.extend(self.reg.sizes());
        v.extend(self.files.sizes());
        v.extend(self.seeding.sizes());
        v.push(("pending", self.completion.pending_len()));
        v
    }

    /// A pending event left the completion stage: its request bookkeeping goes,
    /// whether or not a reply came.
    fn forget(&mut self, id: PendingId) {
        self.launches.forget(id);
        self.expands.forget(id);
        self.reg.forget(id);
        self.files.forget(id);
    }

    fn is_self(&self, e: &Event) -> bool {
        let uid = match &e.kind {
            EventKind::Process(atlas_schema::classes::process::ProcessActivity::Launch { actor, .. }) => actor.uid,
            EventKind::Process(atlas_schema::classes::process::ProcessActivity::Terminate { process, .. }) => {
                process.uid
            }
            EventKind::Module(a) => a.actor.uid,
            EventKind::Network(a) => a.actor.uid,
            EventKind::File(a) => a.actor.uid,
            EventKind::RegistryKey(a) => a.actor.uid,
            EventKind::RegistryValue(a) => a.actor.uid,
            EventKind::Dns(a) => a.actor.uid,
            EventKind::EventLog(_) | EventKind::SensorHealth(_) => return false,
        };
        self.self_uids.contains(&uid)
    }

    /// A new event at QPC `ts`.
    pub(crate) fn event(&mut self, ts: i64, kind: EventKind) -> Event {
        make_event(&self.clock, &mut self.ids, &self.id, ts, kind)
    }

    pub(crate) fn wait(&self, reason: Reason, deadline: std::time::Duration) -> Wait {
        Wait { reason, deadline: Some(self.now.saturating_add(self.ticks.of(deadline))), drop_at_deadline: false }
    }

    pub(crate) fn seeder_wait(&self, drop_at_deadline: bool) -> Wait {
        let startup = self.now.saturating_sub(self.started) < self.ticks.of(self.cfg.startup_period);
        let d = if startup { self.cfg.seeder_startup_deadline } else { self.cfg.seeder_deadline };
        Wait { drop_at_deadline, ..self.wait(Reason::Seeder, d) }
    }

    /// The actor of a synchronous event: header PID and start key (§5.3).
    pub(crate) fn actor_sync(&mut self, h: &Header, class: Class) -> Option<ProcessRef> {
        if let Some(info) = self.procs.lookup_at(h.pid, h.ts)
            && h.start_key.is_none_or(|k| k == info.start_key)
        {
            return Some(info.to_ref(&self.id));
        }
        let Some(key) = h.start_key else {
            if let Some(r) = self.builtin(h.pid) {
                return Some(r);
            }
            Counters::add_class(&mut self.counters.actor_dropped, class);
            return None;
        };
        // A miss with a known start key: ask Windows, accept only the same process.
        if let Some(live) = self.lookups.live_process(h.pid)
            && live.start_key == key
        {
            let info =
                ProcInfo { path: self.dos(&live.image_path), cmd_line: live.command_line, ..ProcInfo::new(key, h.pid) };
            let r = info.to_ref(&self.id);
            // Started at some time before this event: from here on, the cache
            // answers for this PID with this process, not an earlier one.
            self.procs.insert(info, h.ts);
            return Some(r);
        }
        Counters::add_class(&mut self.counters.actor_unresolved, class);
        Some(self.id.bare_ref(key, h.pid))
    }

    /// The actor named by a payload PID (network, image load; §5.3): the cache
    /// entry live at the event's time, or nothing (no start key, no uid).
    pub(crate) fn actor_payload(&mut self, pid: u32, ts: i64, class: Class) -> Option<ProcessRef> {
        if let Some(info) = self.procs.lookup_at(pid, ts) {
            return Some(info.to_ref(&self.id));
        }
        if let Some(r) = self.builtin(pid) {
            return Some(r);
        }
        Counters::add_class(&mut self.counters.actor_dropped, class);
        None
    }

    /// Idle (PID 0) has no telemetry: a synthetic reference with sequence 0
    /// (§5.3 rule 1). Other built-ins come from the rundown.
    fn builtin(&self, pid: u32) -> Option<ProcessRef> {
        (pid == 0).then(|| {
            let mut r = self.id.bare_ref(start_key(self.id.kernel_boot_id, 0), 0);
            r.file.name = "Idle".into();
            r
        })
    }

    fn seed_builtins(&mut self) {
        let mut idle = ProcInfo::new(start_key(self.id.kernel_boot_id, 0), 0);
        idle.path = String::new();
        self.procs.insert(idle, i64::MIN);
    }

    /// The drive form of an NT file path, when mappable (§5.5).
    pub(crate) fn dos(&mut self, nt: &str) -> String {
        let path = self.lookups.dos_path(nt).unwrap_or_else(|| nt.to_string());
        crate::paths::fit(path, atlas_schema::limits::PATH_MAX)
    }

    pub(crate) fn is_self_key(&self, key: Option<u64>) -> bool {
        key.is_some_and(|k| self.self_keys.contains(&k))
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn make_event(clock: &Clock, ids: &mut IdGen, id: &Identity, ts: i64, kind: EventKind) -> Event {
    let time = clock.unix_ns(ts);
    Event {
        meta: EventMeta { event_id: ids(time), time, sensor: Sensor::Etw },
        device: Device { uid: id.device, boot_id: id.boot },
        kind,
    }
}

/// An event leaves the completion stage with unresolved reasons: count them,
/// and for a Launch whose other half never came, try the live process (§5.2).
fn finish_incomplete<L: Lookups>(
    e: &mut Event,
    expired: &[Expired],
    c: &mut Counters,
    lookups: &mut L,
    procs: &mut ProcessCache,
    id: &Identity,
) {
    for x in expired {
        match x.reason {
            Reason::Enrich => c.enrichment_misses += 1,
            Reason::Join => {
                c.launch_join_miss += 1;
                proc::join_fallback(e, lookups, procs, id);
            }
            Reason::ValueRead => c.value_read_failed += 1,
            Reason::Seeder => match &mut e.kind {
                EventKind::File(_) if x.dropped => c.unknown_file_object += 1,
                EventKind::File(_) => {}
                kind => {
                    c.registry_unresolved += 1;
                    if reg::no_read(kind) {
                        c.value_read_failed += 1;
                    }
                }
            },
            Reason::Expand | Reason::Confirm => {}
        }
    }
}

/// Whether an id is still pending (for replies that arrive after a deadline).
pub(crate) fn alive<E>(c: &Completion<E>, id: PendingId, reason: Reason) -> bool {
    c.is_waiting(id, reason)
}

/// Request bookkeeping shared by the domain modules.
pub(crate) struct Pending<T> {
    pub(crate) by_id: HashMap<PendingId, T>,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Pending { by_id: HashMap::new() }
    }
}

impl<T> Pending<T> {
    pub(crate) fn insert(&mut self, id: PendingId, t: T) {
        self.by_id.insert(id, t);
    }

    pub(crate) fn take(&mut self, id: PendingId) -> Option<T> {
        self.by_id.remove(&id)
    }
}
