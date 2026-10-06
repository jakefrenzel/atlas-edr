//! Seeding the key and file maps from the handle table (sensor spec §7.4).
//!
//! The pipeline asks for addresses it cannot name, at most once per second per
//! handle type and only for addresses absent from the latest snapshot. Each
//! snapshot is applied once stream time reaches the time it was taken, and:
//! - a seeded name answers an event at time t only if no Create/Open/Close for
//!   that address happened between t and the snapshot (the address may have
//!   changed hands);
//! - it never overwrites an entry an ETW event set after the snapshot;
//! - addresses that cannot be named go into a negative cache until the address
//!   is reused or its owner exits; events on them do not wait.

use std::collections::{HashMap, HashSet};

use atlas_schema::File;

use super::Pipeline;
use super::file::{Fill, fill_file_event};
use crate::completion::Reason;
use crate::config::Ticks;
use crate::process::file_name;
use crate::recent::Recent;
use crate::services::{HandleKind, Lookups, Request, Snapshot};

/// Address history kept for the reuse rule (Creates, Opens, Closes).
const CHANGES_CAP: usize = 1 << 19;
/// The negative cache's cap.
const UNNAMABLE_CAP: usize = 1 << 16;

pub struct Seeding {
    asks: HashMap<HandleKind, HashSet<u64>>,
    last_request: HashMap<HandleKind, i64>,
    latest: HashMap<HandleKind, HashSet<u64>>,
    queued: Vec<Snapshot>,
    /// (kind, address) → (owner PID, insertion order). Owner 0: the address
    /// was not in the table at all.
    unnamable: HashMap<(HandleKind, u64), (u32, u64)>,
    unnamable_seq: u64,
    changes: Recent<(HandleKind, u64)>,
    /// Kinds whose start-up pass has not been applied yet.
    startup: HashSet<HandleKind>,
}

impl Seeding {
    pub(super) fn new() -> Self {
        Seeding {
            asks: HashMap::new(),
            last_request: HashMap::new(),
            latest: HashMap::new(),
            queued: Vec::new(),
            unnamable: HashMap::new(),
            unnamable_seq: 0,
            changes: Recent::new(CHANGES_CAP),
            startup: HashSet::new(),
        }
    }

    /// The start-up pass over every handle, keys first (§7.4).
    pub(super) fn start(&mut self, requests: &mut Vec<Request>) {
        for kind in [HandleKind::Key, HandleKind::File] {
            requests.push(Request::Seed { kind, addresses: Vec::new() });
            self.startup.insert(kind);
        }
    }

    /// Whether the start-up pass for `kind` is still to come: events on unknown
    /// handles may wait for it even with on-miss seeding off.
    pub(super) fn startup_pending(&self, kind: HandleKind) -> bool {
        self.startup.contains(&kind)
    }

    fn mark_unnamable(&mut self, kind: HandleKind, address: u64, owner: u32) {
        if self.unnamable.len() >= UNNAMABLE_CAP {
            let n = self.unnamable.len();
            for k in crate::evict::oldest(self.unnamable.iter().map(|(k, (_, seq))| (*k, *seq)), n) {
                self.unnamable.remove(&k);
            }
        }
        self.unnamable_seq += 1;
        self.unnamable.insert((kind, address), (owner, self.unnamable_seq));
    }

    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 2] {
        [("address history", self.changes.len()), ("queued snapshots", self.queued.len())]
    }

    /// Forgets address history older than `before`.
    pub(super) fn expire(&mut self, before: i64) {
        self.changes.expire(before);
    }

    pub(super) fn ask(&mut self, kind: HandleKind, address: u64, enabled: bool) {
        if !enabled || self.latest.get(&kind).is_some_and(|s| s.contains(&address)) {
            return;
        }
        self.asks.entry(kind).or_default().insert(address);
    }

    /// Sends the batched questions, at most once per second per kind.
    pub(super) fn flush_asks(&mut self, now: i64, ticks: Ticks, requests: &mut Vec<Request>) {
        let second = ticks.of(std::time::Duration::from_secs(1));
        for kind in [HandleKind::Key, HandleKind::File] {
            let due = self.last_request.get(&kind).is_none_or(|t| now.saturating_sub(*t) >= second);
            if !due {
                continue;
            }
            if let Some(set) = self.asks.remove(&kind).filter(|s| !s.is_empty()) {
                let mut addresses: Vec<u64> = set.into_iter().collect();
                addresses.sort_unstable();
                requests.push(Request::Seed { kind, addresses });
                self.last_request.insert(kind, now);
            }
        }
    }

    pub(super) fn queue(&mut self, s: Snapshot) {
        self.queued.push(s);
    }

    /// A Create/Open/Close for this address: it may now belong to someone else.
    pub(super) fn changed(&mut self, kind: HandleKind, address: u64, ts: i64) {
        self.changes.insert((kind, address), ts);
        self.unnamable.remove(&(kind, address));
    }

    pub(super) fn last_change(&self, kind: HandleKind, address: u64) -> Option<i64> {
        self.changes.get(&(kind, address))
    }

    pub(super) fn is_unnamable(&self, kind: HandleKind, address: u64) -> bool {
        self.unnamable.contains_key(&(kind, address))
    }

    pub(super) fn owner_exited(&mut self, pid: u32) {
        self.unnamable.retain(|_, (owner, _)| *owner != pid);
    }

    pub fn negative_cache_len(&self) -> usize {
        self.unnamable.len()
    }
}

impl<L: Lookups> Pipeline<L> {
    /// Applies every queued snapshot taken at or before stream time `stream`.
    pub(super) fn apply_snapshots(&mut self, stream: i64) {
        if self.seeding.queued.is_empty() {
            return;
        }
        let (mut due, later): (Vec<Snapshot>, Vec<Snapshot>) =
            std::mem::take(&mut self.seeding.queued).into_iter().partition(|s| s.taken <= stream);
        self.seeding.queued = later;
        due.sort_by_key(|s| s.taken);
        for s in due {
            self.apply_snapshot(s);
        }
    }

    fn apply_snapshot(&mut self, s: Snapshot) {
        let kind = s.kind;
        let taken = s.taken;
        let in_table: HashSet<u64> = s.named.iter().map(|n| n.address).chain(s.unnamable.iter().map(|u| u.0)).collect();
        // Covered but not in the table: closed before the read. Nothing can name
        // the address until it is used again (the negative cache, owner 0).
        let absent: HashSet<u64> = if s.asked.is_empty() {
            match kind {
                HandleKind::Key => self.reg.waiting.keys().copied().filter(|r| !in_table.contains(r)).collect(),
                HandleKind::File => self
                    .files
                    .map
                    .iter()
                    .filter(|(a, e)| e.nt.is_none() && !in_table.contains(a))
                    .map(|(a, _)| *a)
                    .collect(),
            }
        } else {
            s.asked.iter().copied().filter(|a| !in_table.contains(a)).collect()
        };
        let unchanged = |p: &Self, address: u64| p.seeding.last_change(kind, address).is_none_or(|c| c <= taken);
        for (address, owner) in &s.unnamable {
            if unchanged(self, *address) {
                self.seeding.mark_unnamable(kind, *address, *owner);
            }
        }
        for address in &absent {
            if unchanged(self, *address) {
                self.seeding.mark_unnamable(kind, *address, 0);
            }
        }
        match kind {
            HandleKind::Key => {
                for n in &s.named {
                    self.keys.seed(n.address, n.name.clone(), taken);
                }
                let roots: Vec<u64> =
                    self.reg.waiting.keys().copied().filter(|r| in_table.contains(r) || absent.contains(r)).collect();
                for root in roots {
                    let named = in_table.contains(&root) && !self.seeding.is_unnamable(kind, root);
                    self.answer_key_waiters(root, taken, named);
                }
            }
            HandleKind::File => {
                for n in &s.named {
                    self.seed_file(n.address, n.owner_pid, &n.name, taken);
                }
                for address in s.unnamable.iter().map(|u| u.0).chain(absent.iter().copied()) {
                    self.unnamable_file(address);
                }
            }
        }
        if s.asked.is_empty() {
            self.seeding.startup.remove(&kind);
        }
        self.seeding.latest.insert(kind, in_table);
    }

    fn seed_file(&mut self, address: u64, owner: u32, nt: &str, taken: i64) {
        let opener = self.procs.lookup_at(owner, taken).map(|p| p.to_ref(&self.id));
        match self.files.map.get_mut(&address) {
            // An ETW event after the snapshot set this entry: keep it.
            Some(e) if e.since > taken => return,
            Some(e) if e.nt.is_some() => return,
            Some(e) => {
                e.nt = Some(nt.to_string());
                e.opener = opener.clone();
            }
            None => {
                self.files.map.insert(address, super::file::seeded_entry(nt, opener.clone(), taken));
                return;
            }
        }
        let waiting = self.files.map.get_mut(&address).map(|e| std::mem::take(&mut e.waiting)).unwrap_or_default();
        let changed = self.seeding.last_change(crate::services::HandleKind::File, address);
        let path = self.dos(nt);
        let file = File { name: file_name(&path).to_string(), path, hashes: None, signature: None };
        for (id, ts, fill) in waiting {
            if !self.completion.is_waiting(id, Reason::Seeder) {
                continue;
            }
            // Any Create or Close since the earlier of the event and the read: the
            // address may have changed hands (conservative for a late snapshot).
            let reused = changed.is_some_and(|c| c > ts.min(taken));
            if reused || (fill == Fill::Opened && opener.is_none()) {
                continue; // left to its deadline
            }
            let (f, a) = (file.clone(), opener.clone());
            self.completion.update(id, |e| fill_file_event(e, fill, f, a));
            self.completion.resolve(id, Reason::Seeder);
            self.requests.push(Request::InvalidateHash { nt_path: nt.to_string() });
        }
    }

    /// The seeder saw the handle but could not name it: its waiting events do
    /// not wait any longer (§7.4).
    fn unnamable_file(&mut self, address: u64) {
        let waiting = self.files.map.get_mut(&address).map(|e| std::mem::take(&mut e.waiting)).unwrap_or_default();
        for (id, _, _) in waiting {
            if !self.completion.is_waiting(id, Reason::Seeder) {
                continue;
            }
            self.completion.cancel(id);
            self.counters.unknown_file_object += 1;
        }
    }
}
