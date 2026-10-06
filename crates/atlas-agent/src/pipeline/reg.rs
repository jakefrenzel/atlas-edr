//! Registry (sensor spec §5.1, §7.4, §7.5): key and value events with paths from
//! the key map, the seeder for unknown handles, the `path_unresolved` floor,
//! and value reads after the event.

use std::collections::HashMap;

use atlas_etw::parse::{RegDeleteValue, RegKey, RegOpen, RegSetValue, WStr};
use atlas_schema::classes::registry::{
    RegType, RegistryKeyAction, RegistryKeyActivity, RegistryValueAction, RegistryValueActivity,
};
use atlas_schema::limits::{PATH_MAX, REG_DATA_MAX};
use atlas_schema::{Event, EventKind, ProcessRef};

use super::{Pending, Pipeline, alive};
use crate::completion::{PendingId, Reason};
use crate::counters::Class;
use crate::input::Header;
use crate::keymap::Resolved;
use crate::paths;
use crate::services::{EarlyKey, HandleKind, Lookups, Request, ValueData, ValueRead};

/// What a value read must match to be accepted (§7.5).
#[derive(Debug, Clone, Copy)]
struct Expected {
    value_type: u32,
    data_size: u32,
}

/// An event waiting for the seeder to name a key.
#[derive(Debug, Clone)]
pub(crate) struct KeyWaiter {
    pub(crate) id: PendingId,
    /// QPC of the event (the seeder's answer must not postdate a reuse, §7.4).
    pub(crate) ts: i64,
    /// The handle whose name the event needs.
    pub(crate) key: u64,
    /// For a Value Set: the read to start once named.
    read: Option<(Vec<u16>, Expected, EarlyKey)>,
}

#[derive(Default)]
pub struct Registry {
    /// Fast-path read results (§7.5), until their event is processed.
    early: HashMap<EarlyKey, (ValueRead, Option<ValueData>)>,
    reads: Pending<Expected>,
    /// Unnamed root address → events waiting for it.
    pub(crate) waiting: HashMap<u64, Vec<KeyWaiter>>,
    /// Pending id → the root it waits on, to forget it when it leaves.
    waiter_root: HashMap<PendingId, u64>,
}

impl Registry {
    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 4] {
        [
            ("value reads", self.reads.by_id.len()),
            ("key waiters", self.waiting.values().map(Vec::len).sum()),
            ("key waiter index", self.waiter_root.len()),
            ("early reads", self.early.len()),
        ]
    }

    /// The event left the completion stage.
    pub(super) fn forget(&mut self, id: PendingId) {
        self.reads.take(id);
        if let Some(root) = self.waiter_root.remove(&id)
            && let Some(ws) = self.waiting.get_mut(&root)
        {
            ws.retain(|w| w.id != id);
            if ws.is_empty() {
                self.waiting.remove(&root);
            }
        }
    }

    pub(super) fn on_early_read(&mut self, event: EarlyKey, read: ValueRead, result: Option<ValueData>) {
        // Results are pruned every tick (`prune_early`); this cap only guards
        // against a flood within one tick, and clearing it only costs redone reads.
        if self.early.len() >= 65_536 {
            self.early.clear();
        }
        self.early.insert(event, (read, result));
    }

    /// Drops fast-path results older than `before` (their events have passed).
    pub(super) fn prune_early(&mut self, before: i64) {
        self.early.retain(|k, _| k.ts >= before);
    }
}

/// What the key map can say about a handle's name, for an event at `ts`.
enum KeyPath {
    Known(String),
    /// Waiting for the seeder to name `root`; `partial` is what is known now.
    Wait {
        partial: String,
        root: u64,
    },
    /// The floor (§7.4): only what ETW logged.
    Unresolved(String),
}

fn units_to_string(w: &WStr) -> String {
    paths::fit(w.to_string_lossy(), PATH_MAX)
}

impl<L: Lookups> Pipeline<L> {
    fn key_path(&mut self, key: u64) -> KeyPath {
        match self.keys.resolve(key) {
            Some(Resolved::Full(nt)) => KeyPath::Known(nt),
            Some(Resolved::Partial { known, root }) => self.key_wait(known, root),
            None => self.key_wait(String::new(), key),
        }
    }

    fn key_wait(&mut self, partial: String, root: u64) -> KeyPath {
        let can_wait = self.cfg.seed_on_miss || self.seeding.startup_pending(HandleKind::Key);
        if !can_wait || self.seeding.is_unnamable(HandleKind::Key, root) {
            return KeyPath::Unresolved(partial);
        }
        self.seeding.ask(HandleKind::Key, root, self.cfg.seed_on_miss);
        KeyPath::Wait { partial, root }
    }

    /// Pushes a registry event whose path is `path`; waits for the seeder when
    /// needed. `set_path` writes a (normalized) path into the event.
    fn push_registry(&mut self, ts: i64, key: u64, mut ev: Event, read: Option<(Vec<u16>, Expected, EarlyKey)>) {
        match self.key_path(key) {
            KeyPath::Known(nt) => {
                set_path(&mut ev, &paths::registry(&nt, self.ccs), false);
                match read {
                    Some((name, exp, early)) => {
                        let id = self.completion.push_pending(ev, vec![]);
                        self.start_read(id, nt, name, exp, early);
                    }
                    None => self.completion.push(ev),
                }
            }
            KeyPath::Unresolved(partial) => {
                set_path(&mut ev, &partial, true);
                self.counters.registry_unresolved += 1;
                if no_read(&mut ev.kind) {
                    self.counters.value_read_failed += 1;
                }
                self.completion.push(ev);
            }
            KeyPath::Wait { partial, root } => {
                set_path(&mut ev, &partial, true);
                let w = self.seeder_wait(false);
                let id = self.completion.push_pending(ev, vec![w]);
                self.reg.waiting.entry(root).or_default().push(KeyWaiter { id, ts, key, read });
                self.reg.waiter_root.insert(id, root);
            }
        }
    }

    /// Starts the value read for a pending Value Set whose key is named (§7.5):
    /// a fast-path result for the same key and name is used; otherwise the
    /// ordered path reads now.
    fn start_read(&mut self, id: PendingId, nt: String, name: Vec<u16>, exp: Expected, early: EarlyKey) {
        if let Some((read, result)) = self.reg.early.remove(&early) {
            if read.key_path == nt && read.value_name == name {
                self.apply_read(id, exp, result);
                return;
            }
            self.counters.early_read_redone += 1;
        }
        let w = self.wait(Reason::ValueRead, self.cfg.value_read_deadline);
        self.completion.add_wait(id, w);
        self.reg.reads.insert(id, exp);
        self.requests.push(Request::ReadValue { id, read: ValueRead { key_path: nt, value_name: name } });
    }

    pub(super) fn on_value_read(&mut self, id: PendingId, result: Option<ValueData>) {
        let Some(exp) = self.reg.reads.take(id) else { return };
        if !alive(&self.completion, id, Reason::ValueRead) {
            return;
        }
        self.apply_read(id, exp, result);
        self.completion.resolve(id, Reason::ValueRead);
    }

    /// Accepts the data only if its type and length match the event (§7.5).
    fn apply_read(&mut self, id: PendingId, exp: Expected, result: Option<ValueData>) {
        let ok = result.filter(|r| r.value_type == exp.value_type && r.size == exp.data_size);
        if ok.is_none() {
            self.counters.value_read_failed += 1;
        }
        self.completion.update(id, |e| {
            if let EventKind::RegistryValue(RegistryValueActivity {
                action: RegistryValueAction::Set { data, data_truncated, data_unavailable, .. },
                ..
            }) = &mut e.kind
                && let Some(r) = ok
            {
                let mut d = r.data;
                d.truncate(REG_DATA_MAX);
                *data = d;
                *data_truncated = exp.data_size as usize > REG_DATA_MAX;
                *data_unavailable = false;
            }
        });
    }

    pub(super) fn on_reg_open(&mut self, h: &Header, o: RegOpen, create: bool) {
        if o.status != 0 {
            return; // includes STATUS_REPARSE's first attempt (§5.5)
        }
        let rel = units_to_string(&o.relative_name);
        self.keys.open(o.key_object, o.base_object, &rel, h.ts);
        self.seeding.changed(HandleKind::Key, o.key_object, h.ts);
        if !(create && o.disposition == 1) {
            return; // an open, or a create that opened an existing key
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryKey) else { return };
        let ev = self.key_event(h.ts, actor, RegistryKeyAction::Create);
        self.push_registry(h.ts, o.key_object, ev, None);
    }

    pub(super) fn on_reg_close(&mut self, h: &Header, k: RegKey) {
        // The agent's own closes (seeding duplicates, value reads) are ignored (§7.4).
        if self.is_self_key(h.start_key) {
            return;
        }
        self.keys.close(k.key_object, h.ts);
        self.seeding.changed(HandleKind::Key, k.key_object, h.ts);
    }

    pub(super) fn on_reg_delete_key(&mut self, h: &Header, k: RegKey) {
        if k.status != 0 {
            return;
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryKey) else { return };
        let ev = self.key_event(h.ts, actor, RegistryKeyAction::Delete);
        self.push_registry(h.ts, k.key_object, ev, None);
    }

    pub(super) fn on_reg_set_value(&mut self, h: &Header, v: RegSetValue) {
        if v.status != 0 {
            return;
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryValue) else { return };
        if v.value_name_ambiguous {
            self.counters.reg_name_ambiguous += 1;
        }
        let value_type = RegType::from_raw(v.value_type);
        let unusual = matches!(value_type, RegType::Raw(_));
        if unusual {
            self.counters.reg_type_unusual += 1;
        }
        let read = (self.cfg.registry_value_reads && !unusual).then(|| {
            let early = EarlyKey { ts: h.ts, tid: h.tid, key_object: v.key_object };
            (v.value_name.as_units().to_vec(), Expected { value_type: v.value_type, data_size: v.data_size }, early)
        });
        let action = RegistryValueAction::Set {
            value_type,
            data: Vec::new(),
            data_truncated: false,
            data_read_after: read.is_some(),
            data_unavailable: true,
        };
        let ev = self.value_event(h.ts, actor, units_to_string(&v.value_name), action);
        self.push_registry(h.ts, v.key_object, ev, read);
    }

    pub(super) fn on_reg_delete_value(&mut self, h: &Header, v: RegDeleteValue) {
        if v.status != 0 {
            return;
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryValue) else { return };
        let ev = self.value_event(h.ts, actor, units_to_string(&v.value_name), RegistryValueAction::Delete);
        self.push_registry(h.ts, v.key_object, ev, None);
    }

    fn key_event(&mut self, ts: i64, actor: ProcessRef, action: RegistryKeyAction) -> Event {
        self.event(
            ts,
            EventKind::RegistryKey(RegistryKeyActivity { actor, path: String::new(), path_unresolved: true, action }),
        )
    }

    fn value_event(&mut self, ts: i64, actor: ProcessRef, name: String, action: RegistryValueAction) -> Event {
        self.event(
            ts,
            EventKind::RegistryValue(RegistryValueActivity {
                actor,
                key_path: String::new(),
                name,
                path_unresolved: true,
                action,
            }),
        )
    }

    /// The seeder named (or could not name) `root`: answer the events waiting
    /// for it whose time allows it (§7.4).
    pub(super) fn answer_key_waiters(&mut self, root: u64, taken: i64, named: bool) {
        let Some(waiters) = self.reg.waiting.remove(&root) else { return };
        let changed = self.seeding.last_change(HandleKind::Key, root);
        for w in waiters {
            if !alive(&self.completion, w.id, Reason::Seeder) {
                continue;
            }
            // The address changed hands since the earlier of the event and the
            // read: the seeded name may belong to the new owner (conservative for
            // a snapshot applied late).
            let reused = changed.is_some_and(|c| c > w.ts.min(taken));
            let full = if named && !reused {
                match self.keys.resolve(w.key) {
                    Some(Resolved::Full(nt)) => Some(nt),
                    _ => None,
                }
            } else {
                None
            };
            match full {
                Some(nt) => {
                    let path = paths::registry(&nt, self.ccs);
                    self.completion.update(w.id, |e| set_path(e, &path, false));
                    self.completion.resolve(w.id, Reason::Seeder);
                    if let Some((name, exp, early)) = w.read {
                        self.start_read(w.id, nt, name, exp, early);
                    }
                }
                None => {
                    // Emitted with the floor already in the event.
                    self.counters.registry_unresolved += 1;
                    let mut no = false;
                    self.completion.update(w.id, |e| no = no_read(&mut e.kind));
                    if no {
                        self.counters.value_read_failed += 1;
                    }
                    self.completion.resolve(w.id, Reason::Seeder);
                }
            }
        }
    }
}

/// A Value Set whose key stayed unresolved: no read was attempted (§7.5).
/// True if it was a Set that expected one.
pub(super) fn no_read(kind: &mut EventKind) -> bool {
    match kind {
        EventKind::RegistryValue(RegistryValueActivity {
            action: RegistryValueAction::Set { data_read_after, .. },
            ..
        }) if *data_read_after => {
            *data_read_after = false;
            true
        }
        _ => false,
    }
}

fn set_path(e: &mut Event, path: &str, unresolved: bool) {
    let path = &paths::fit(path.to_string(), PATH_MAX);
    match &mut e.kind {
        EventKind::RegistryKey(k) => {
            k.path = path.to_string();
            k.path_unresolved = unresolved;
        }
        EventKind::RegistryValue(v) => {
            v.key_path = path.to_string();
            v.path_unresolved = unresolved;
        }
        _ => {}
    }
}
