//! Files (sensor spec §5.5, §7.1, §7.2): the FileObject map, the failure-confirm
//! window, Update coalescing, delete-on-close, renames and watchlist Opens.
//! Every emitted path with 8.3 components is expanded first (`expand`).

use std::collections::{HashMap, VecDeque};

use atlas_etw::parse::{FileCreate, FileHandle, FileOpEnd, FilePath, FileSetInfo};
use atlas_schema::classes::file::{FileAction, FileSystemActivity};
use atlas_schema::{Event, EventKind, File, ProcessRef, ProcessUid};

use super::Pipeline;
use super::expand::{Expansion, Slot};
use crate::completion::{PendingId, Reason, Wait};
use crate::counters::Class;
use crate::input::Header;
use crate::paths::has_short_name;
use crate::process::{empty_file, file_name};
use crate::recent::Recent;
use crate::services::{HandleKind, Lookups, Request};

/// Caps of the Irp and coalescing sets (each also expires by time).
const FAILED_CAP: usize = 1 << 16;
const CONFIRMED_CAP: usize = 1 << 18;
const OPENED_CAP: usize = 1 << 16;

/// A watchlist Open's coalescing key: (actor, logged path lowercased).
type OpenKey = (ProcessUid, String);

/// `FileInformationClass` values (§5.1).
const FILE_BASIC_INFORMATION: u32 = 4;
const FILE_END_OF_FILE_INFORMATION: u32 = 19;

/// One FileObject (§7.1).
#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    /// The NT path as logged, if known (a provisional entry has none until seeded).
    pub(crate) nt: Option<String>,
    /// Its long form, once an expansion of `nt` came back.
    pub(crate) expanded: Option<String>,
    /// Who opened the handle: the actor of its Update and delete-on-close Delete.
    pub(crate) opener: Option<ProcessRef>,
    written: bool,
    delete_on_close: bool,
    cleaned: bool,
    /// QPC of the Create (or snapshot) that set it.
    pub(crate) since: i64,
    touched: u64,
    /// Events waiting for the seeder to name this handle.
    pub(crate) waiting: Vec<(PendingId, i64, Fill)>,
}

/// What a seeded name fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fill {
    /// `file` (SetAttributes: its actor is the event's header).
    File,
    /// `file` and the actor, the handle's owner (an Update or Delete at Cleanup).
    Opened,
}

enum Held {
    /// A Create: on failure the map entry goes, and any watchlist Open with it,
    /// and its coalescing record, so a retry is not suppressed.
    Create {
        fo: u64,
        open: Option<PendingId>,
        coalesce: Option<(OpenKey, i64)>,
    },
    /// The event's id, unless it went out at once (`file.op_end` off).
    Delete {
        id: Option<PendingId>,
    },
    Rename {
        id: Option<PendingId>,
        fo: u64,
        new_nt: String,
    },
}

pub struct Files {
    pub(crate) map: HashMap<u64, FileEntry>,
    cap: usize,
    clock: u64,
    evictions: u64,
    window: i64,
    /// Operations in their failure-confirm window: (QPC, Irp, what), in stream
    /// order; a failed one is taken out (`None`) and skipped.
    held: VecDeque<(i64, u64, Option<Held>)>,
    /// The position of `held`'s front, counted since the start.
    held_base: u64,
    /// Irp → position of the latest held operation with it (Irps are recycled).
    held_by_irp: HashMap<u64, u64>,
    /// Failed OperationEnds no held operation claimed yet (a late operation): Irp → QPC.
    failed: Recent<u64>,
    /// Recently confirmed operations, to recognise a failure that comes too late.
    confirmed: Recent<u64>,
    /// Watchlist Opens → when last emitted, for the 60 s coalescing.
    opened: Recent<OpenKey>,
    /// Pending id → the handle it waits on, to forget it when it leaves.
    waiter_fo: HashMap<PendingId, u64>,
}

impl Files {
    pub fn new(cap: usize, window_ticks: i64) -> Self {
        Files {
            map: HashMap::new(),
            cap,
            clock: 0,
            evictions: 0,
            window: window_ticks,
            held: VecDeque::new(),
            held_base: 0,
            held_by_irp: HashMap::new(),
            failed: Recent::new(FAILED_CAP),
            confirmed: Recent::new(CONFIRMED_CAP),
            opened: Recent::new(OPENED_CAP),
            waiter_fo: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 6] {
        [
            ("file waiters", self.map.values().map(|e| e.waiting.len()).sum()),
            ("file waiter index", self.waiter_fo.len()),
            ("held operations", self.held.len() + self.held_by_irp.len()),
            ("failed irps", self.failed.len()),
            ("confirmed irps", self.confirmed.len()),
            ("watchlist opens", self.opened.len()),
        ]
    }

    /// The event left the completion stage.
    pub(super) fn forget(&mut self, id: PendingId) {
        if let Some(fo) = self.waiter_fo.remove(&id)
            && let Some(e) = self.map.get_mut(&fo)
        {
            e.waiting.retain(|(w, _, _)| *w != id);
        }
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    fn insert(&mut self, fo: u64, e: FileEntry) {
        self.clock += 1;
        self.map.insert(fo, FileEntry { touched: self.clock, ..e });
        if self.map.len() > self.cap {
            for k in crate::evict::oldest(self.map.iter().map(|(k, e)| (*k, e.touched)), self.map.len()) {
                self.map.remove(&k);
                self.evictions += 1;
            }
        }
    }

    fn touch(&mut self, fo: u64) -> Option<&mut FileEntry> {
        self.clock += 1;
        let e = self.map.get_mut(&fo)?;
        e.touched = self.clock;
        Some(e)
    }
}

fn entry(nt: Option<String>, opener: Option<ProcessRef>, since: i64) -> FileEntry {
    FileEntry {
        nt,
        expanded: None,
        opener,
        written: false,
        delete_on_close: false,
        cleaned: false,
        since,
        touched: 0,
        waiting: Vec::new(),
    }
}

/// The expansion to ask for, if `emit_file` found one.
fn expansion(nt: Option<String>, x: Expansion) -> Vec<(String, Expansion)> {
    nt.map(|n| (n, x)).into_iter().collect()
}

impl<L: Lookups> Pipeline<L> {
    pub(crate) fn file_obj(&mut self, nt: &str) -> File {
        let path = self.dos(nt);
        File { name: file_name(&path).to_string(), path, hashes: None, signature: None }
    }

    fn file_event(&mut self, ts: i64, actor: ProcessRef, file: File, action: FileAction) -> Event {
        self.event(ts, EventKind::File(FileSystemActivity { actor, file, action }))
    }

    /// The failure-confirm wait, unless `file.op_end` is off (§5.5).
    fn confirm_waits(&self) -> Vec<Wait> {
        if self.cfg.file_op_end {
            vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: false }]
        } else {
            Vec::new()
        }
    }

    /// Holds an operation for its confirm window, unless a failure for its Irp
    /// was already seen: the operation arrived late, after its OperationEnd (§5.5).
    fn hold(&mut self, ts: i64, irp: u64, held: Held) {
        if !self.cfg.file_op_end {
            self.confirm(held);
            return;
        }
        let window = self.files.window;
        if let Some(fts) = self.files.failed.get(&irp)
            && fts >= ts
            && fts - ts <= window
        {
            self.files.failed.remove(&irp);
            self.fail(held);
            return;
        }
        let at = self.files.held_base + self.files.held.len() as u64;
        self.files.held_by_irp.insert(irp, at);
        self.files.held.push_back((ts, irp, Some(held)));
    }

    fn fail(&mut self, held: Held) {
        self.counters.file_op_failed += 1;
        match held {
            Held::Create { fo, open, coalesce } => {
                self.files.map.remove(&fo);
                if let Some(id) = open {
                    self.completion.cancel(id);
                }
                if let Some((key, at)) = coalesce
                    && self.files.opened.get(&key) == Some(at)
                {
                    self.files.opened.remove(&key);
                }
            }
            Held::Delete { id } | Held::Rename { id, .. } => {
                if let Some(id) = id {
                    self.completion.cancel(id);
                }
            }
        }
    }

    pub(super) fn on_file_create(&mut self, h: &Header, c: FileCreate) {
        let nt = c.file_name.to_string_lossy();
        let opener = self.actor_sync(h, Class::File);
        if self.files.map.contains_key(&c.file_object) {
            self.counters.file_object_replaced += 1;
        }
        self.seeding.changed(HandleKind::File, c.file_object, h.ts);
        let mut e = entry(Some(nt.clone()), opener.clone(), h.ts);
        e.delete_on_close = c.delete_on_close();
        self.files.insert(c.file_object, e);
        let (open, coalesce) = match opener {
            Some(actor) => self.watch_open(h.ts, actor, &nt, c.file_object),
            None => (None, None),
        };
        self.hold(h.ts, c.irp, Held::Create { fo: c.file_object, open, coalesce });
    }

    /// A Create on a watchlisted path emits `Open` after the confirm window; a
    /// path with 8.3 components is matched again once expanded (§7.2).
    /// Returns the Open's id, if it waits, and its coalescing record.
    fn watch_open(
        &mut self,
        ts: i64,
        actor: ProcessRef,
        nt: &str,
        fo: u64,
    ) -> (Option<PendingId>, Option<(OpenKey, i64)>) {
        let logged = self.watch.matches(nt);
        if !logged && !has_short_name(nt) {
            return (None, None);
        }
        let key = (actor.uid, nt.to_lowercase());
        let period = self.ticks.of(self.cfg.watchlist_coalesce);
        if self.files.opened.get(&key).is_some_and(|last| ts.saturating_sub(last) < period) {
            return (None, None);
        }
        let (file, expand) = self.emit_file(nt, Some(fo));
        let x = Expansion { open_logged: Some(logged), ..Expansion::file(Some(fo)) };
        let ev = self.file_event(ts, actor, file, FileAction::Open);
        let waits = self.confirm_waits();
        let id = self.push_with(ev, waits, expansion(expand, x));
        self.files.opened.insert(key.clone(), ts);
        (id, Some((key, ts)))
    }

    pub(super) fn on_file_create_new(&mut self, h: &Header, c: FileCreate) {
        let nt = c.file_name.to_string_lossy();
        let Some(actor) = self.actor_sync(h, Class::File) else { return };
        if !self.files.map.contains_key(&c.file_object) {
            self.seeding.changed(HandleKind::File, c.file_object, h.ts);
            self.files.insert(c.file_object, entry(Some(nt.clone()), Some(actor.clone()), h.ts));
        }
        let (file, expand) = self.emit_file(&nt, Some(c.file_object));
        let ev = self.file_event(h.ts, actor, file, FileAction::Create);
        self.push_with(ev, vec![], expansion(expand, Expansion::file(Some(c.file_object))));
    }

    /// A Write, or a truncation (the overwrite case, §5.1).
    pub(super) fn on_file_write(&mut self, h: &Header, fo: u64) {
        match self.files.touch(fo) {
            Some(e) => {
                if e.cleaned {
                    self.counters.writes_after_cleanup += 1;
                }
                e.written = true;
            }
            None => {
                let mut e = entry(None, None, h.ts);
                e.written = true;
                self.files.insert(fo, e);
                self.seeding.ask(HandleKind::File, fo, self.cfg.seed_on_miss);
            }
        }
    }

    pub(super) fn on_file_set_info(&mut self, h: &Header, i: FileSetInfo) {
        match i.info_class {
            FILE_END_OF_FILE_INFORMATION => self.on_file_write(h, i.file_object),
            FILE_BASIC_INFORMATION => {
                let Some(actor) = self.actor_sync(h, Class::File) else { return };
                self.file_action_on(h.ts, i.file_object, actor, FileAction::SetAttributes, Fill::File, true);
            }
            _ => {}
        }
    }

    /// Emits an action whose path comes from the FileObject map; an unknown
    /// handle waits for the seeder (§7.1).
    fn file_action_on(
        &mut self,
        ts: i64,
        fo: u64,
        actor: ProcessRef,
        action: FileAction,
        fill: Fill,
        drop_unresolved: bool,
    ) {
        let known = self.files.touch(fo).and_then(|e| e.nt.clone());
        match known {
            Some(nt) => {
                self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
                let (file, expand) = self.emit_file(&nt, Some(fo));
                let ev = self.file_event(ts, actor, file, action);
                self.push_with(ev, vec![], expansion(expand, Expansion::file(Some(fo))));
            }
            None => {
                let can_wait = self.cfg.seed_on_miss || self.seeding.startup_pending(HandleKind::File);
                if !can_wait || self.seeding.is_unnamable(HandleKind::File, fo) {
                    if drop_unresolved {
                        self.counters.unknown_file_object += 1;
                    }
                    return;
                }
                if !self.files.map.contains_key(&fo) {
                    self.files.insert(fo, entry(None, None, ts));
                    self.seeding.ask(HandleKind::File, fo, self.cfg.seed_on_miss);
                }
                // A seeded name is the handle's final (long) path: no expansion needed.
                let ev = self.file_event(ts, actor, empty_file(), action);
                let w = self.seeder_wait(drop_unresolved);
                let id = self.completion.push_pending(ev, vec![w]);
                if let Some(e) = self.files.map.get_mut(&fo) {
                    e.waiting.push((id, ts, fill));
                    self.files.waiter_fo.insert(id, fo);
                }
            }
        }
    }

    pub(super) fn on_file_delete_path(&mut self, h: &Header, p: FilePath) {
        let Some(actor) = self.actor_sync(h, Class::File) else { return };
        let nt = p.file_path.to_string_lossy();
        self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
        let (file, expand) = self.emit_file(&nt, None);
        let ev = self.file_event(h.ts, actor, file, FileAction::Delete);
        let waits = self.confirm_waits();
        let id = self.push_with(ev, waits, expansion(expand, Expansion::file(None)));
        self.hold(h.ts, p.irp, Held::Delete { id });
    }

    pub(super) fn on_file_rename_path(&mut self, h: &Header, p: FilePath) {
        let Some(actor) = self.actor_sync(h, Class::File) else { return };
        let new_nt = p.file_path.to_string_lossy();
        let (file_result, expand_result) = self.emit_file(&new_nt, None);
        let source = self.files.touch(p.file_object).and_then(|e| e.nt.clone());
        let waits = self.confirm_waits();
        let mut expands = expansion(expand_result, Expansion { slot: Slot::RenameResult, ..Expansion::file(None) });
        let file = match &source {
            Some(nt) => {
                self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
                let (f, x) = self.emit_file(nt, Some(p.file_object));
                expands.extend(expansion(x, Expansion::file(None)));
                f
            }
            None => {
                // A handle opened before we watched. A snapshot read now could only
                // give the new name, so the source goes out empty (clarification 7);
                // the handle takes the new name once the rename stands.
                if !self.files.map.contains_key(&p.file_object) {
                    self.files.insert(p.file_object, entry(None, None, h.ts));
                }
                empty_file()
            }
        };
        let ev = self.file_event(h.ts, actor, file, FileAction::Rename { file_result });
        let id = self.push_with(ev, waits, expands);
        self.hold(h.ts, p.irp, Held::Rename { id, fo: p.file_object, new_nt });
    }

    pub(super) fn on_file_op_end(&mut self, h: &Header, o: FileOpEnd) {
        if !o.failed() {
            return; // the callback discards these (§3.2); replay may not
        }
        // The most recent held operation with this Irp (Irps are recycled).
        let base = self.files.held_base;
        let held = self
            .files
            .held_by_irp
            .remove(&o.irp)
            .and_then(|at| self.files.held.get_mut(usize::try_from(at - base).ok()?))
            .and_then(|(_, _, held)| held.take());
        if let Some(held) = held {
            self.fail(held);
        } else if self.files.confirmed.get(&o.irp).is_some() {
            self.counters.file_op_late_failure += 1;
        } else {
            self.files.failed.insert(o.irp, h.ts);
        }
    }

    pub(super) fn on_file_cleanup(&mut self, h: &Header, x: FileHandle) {
        let Some(e) = self.files.touch(x.file_object) else { return };
        let update = e.written && !e.cleaned;
        let delete = e.delete_on_close;
        e.cleaned = true;
        let opener = e.opener.clone();
        for action in [update.then_some(FileAction::Update), delete.then_some(FileAction::Delete)].into_iter().flatten()
        {
            // The actor is the handle's opener, never the Cleanup's header (§7.1).
            match opener.clone() {
                Some(actor) => self.file_action_on(h.ts, x.file_object, actor, action, Fill::Opened, true),
                None => {
                    // A provisional entry: the seeder may still name it and its owner;
                    // dropped at the deadline otherwise (the §7.1 carve-out from E11).
                    let placeholder = self.id.bare_ref(0, 0);
                    let known = self.files.map.get(&x.file_object).and_then(|e| e.nt.clone());
                    if known.is_some() {
                        // Named but without an owner we can resolve: not attributable.
                        self.counters.unknown_file_object += 1;
                        continue;
                    }
                    self.file_action_on(h.ts, x.file_object, placeholder, action, Fill::Opened, true);
                }
            }
        }
    }

    pub(super) fn on_file_close(&mut self, h: &Header, x: FileHandle) {
        self.files.map.remove(&x.file_object);
        self.seeding.changed(HandleKind::File, x.file_object, h.ts);
    }

    /// An operation stands (§5.5).
    fn confirm(&mut self, held: Held) {
        match held {
            Held::Create { open, .. } | Held::Delete { id: open } => {
                if let Some(id) = open {
                    self.completion.resolve(id, Reason::Confirm);
                }
            }
            Held::Rename { id, fo, new_nt } => {
                if let Some(id) = id {
                    self.completion.resolve(id, Reason::Confirm);
                }
                if let Some(e) = self.files.map.get_mut(&fo) {
                    e.nt = Some(new_nt); // later Updates carry the new name (§7.1)
                    e.expanded = None;
                }
            }
        }
    }

    /// Stream time passed: operations past their window stand (§5.5).
    pub(super) fn file_confirm(&mut self, stream: i64) {
        let window = self.files.window;
        while let Some((ts, _, _)) = self.files.held.front() {
            if ts.saturating_add(window) > stream {
                break;
            }
            let (ts, irp, held) = self.files.held.pop_front().expect("peeked");
            let at = self.files.held_base;
            self.files.held_base += 1;
            if self.files.held_by_irp.get(&irp) == Some(&at) {
                self.files.held_by_irp.remove(&irp);
            }
            if let Some(held) = held {
                self.confirm(held);
                self.files.confirmed.insert(irp, ts);
            }
        }
        self.files.failed.expire(stream.saturating_sub(window));
        self.files.confirmed.expire(stream.saturating_sub(window.saturating_mul(40)));
        self.files.opened.expire(stream.saturating_sub(self.ticks.of(self.cfg.watchlist_coalesce)));
    }
}

/// An entry named by the seeder for a handle opened before we watched (§7.4).
pub(crate) fn seeded_entry(nt: &str, opener: Option<ProcessRef>, taken: i64) -> FileEntry {
    entry(Some(nt.to_string()), opener, taken)
}

/// Fills a file event waiting for a seeded name.
pub(crate) fn fill_file_event(e: &mut Event, fill: Fill, file: File, actor: Option<ProcessRef>) {
    if let EventKind::File(f) = &mut e.kind {
        f.file = file;
        if let (Fill::Opened, Some(a)) = (fill, actor) {
            f.actor = a;
        }
    }
}
