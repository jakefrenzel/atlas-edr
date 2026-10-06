//! 8.3 short names in emitted paths (plan 1b-3a decision D4, refining sensor
//! spec §7.2): every emitted file path with a short component waits for its
//! expansion, so paths have one spelling and short names cannot dodge
//! path-based rules. A failed or late expansion leaves the path as logged.
//!
//! One event may need several paths expanded (a Rename's source and result);
//! each is a slot of the same pending event, and the event waits until every
//! slot has answered or the deadline passes.

use std::collections::HashMap;

use atlas_schema::classes::file::FileAction;
use atlas_schema::classes::module::ModuleAction;
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::{Event, EventKind, File};

use super::{Pipeline, alive};
use crate::completion::{PendingId, Reason, Wait};
use crate::paths::has_short_name;
use crate::process::file_name;
use crate::services::{Lookups, Request};

/// Which path of the event an expansion is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// `file` of a File System Activity.
    File,
    /// `file_result` of a Rename.
    RenameResult,
    /// The image of a Launch (`process.file`) or a Module Load (`module.file`).
    Image,
}

/// One path waiting for its long form.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Expansion {
    pub(crate) slot: Slot,
    /// The FileObject whose map entry keeps the expanded path.
    pub(crate) fo: Option<u64>,
    /// The process whose cached image path gets the expanded path.
    pub(crate) image_key: Option<u64>,
    /// A watchlist Open: whether the logged path already matched (§7.2).
    pub(crate) open_logged: Option<bool>,
}

impl Expansion {
    pub(crate) fn file(fo: Option<u64>) -> Self {
        Expansion { slot: Slot::File, fo, image_key: None, open_logged: None }
    }
}

#[derive(Default)]
pub struct Expands {
    by_slot: HashMap<(PendingId, u8), Expansion>,
    /// Per event: slots still unanswered, and how many it asked for.
    outstanding: HashMap<PendingId, (u8, u8)>,
}

impl Expands {
    /// The event left the completion stage: unanswered slots are forgotten.
    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 2] {
        [("expand slots", self.by_slot.len()), ("expand events", self.outstanding.len())]
    }

    pub(super) fn forget(&mut self, id: PendingId) {
        if let Some((_, total)) = self.outstanding.remove(&id) {
            for slot in 0..total {
                self.by_slot.remove(&(id, slot));
            }
        }
    }
}

impl<L: Lookups> Pipeline<L> {
    /// The `File` for an NT path, and the path to expand if it has 8.3
    /// components. A handle whose path was expanded before reuses it.
    pub(crate) fn emit_file(&mut self, nt: &str, fo: Option<u64>) -> (File, Option<String>) {
        if let Some(long) = fo.and_then(|f| self.files.map.get(&f)).and_then(|e| e.expanded.clone()) {
            return (self.file_obj(&long), None);
        }
        let file = self.file_obj(nt);
        (file, has_short_name(nt).then(|| nt.to_string()))
    }

    /// Pushes an event that may wait: for its `waits`, and for each path in
    /// `expansions`. Returns its id, or `None` if it was ready at once.
    pub(crate) fn push_with(
        &mut self,
        ev: Event,
        mut waits: Vec<Wait>,
        expansions: Vec<(String, Expansion)>,
    ) -> Option<PendingId> {
        if waits.is_empty() && expansions.is_empty() {
            self.completion.push(ev);
            return None;
        }
        if !expansions.is_empty() {
            // An Open whose logged path did not match is dropped if no expansion comes.
            let drop = expansions.iter().any(|(_, x)| x.open_logged == Some(false));
            waits.push(Wait { drop_at_deadline: drop, ..self.wait(Reason::Expand, self.cfg.expand_deadline) });
        }
        let id = self.completion.push_pending(ev, waits);
        if !expansions.is_empty() {
            let n = expansions.len() as u8;
            self.expands.outstanding.insert(id, (n, n));
        }
        for (slot, (nt_path, x)) in expansions.into_iter().enumerate() {
            let slot = slot as u8;
            self.expands.by_slot.insert((id, slot), x);
            self.requests.push(Request::Expand { id, slot, nt_path });
        }
        Some(id)
    }

    pub(super) fn on_expanded(&mut self, id: PendingId, slot: u8, long: Option<String>) {
        let Some(x) = self.expands.by_slot.remove(&(id, slot)) else { return };
        let left = match self.expands.outstanding.get_mut(&id) {
            Some((n, _)) => {
                *n = n.saturating_sub(1);
                *n
            }
            None => 0,
        };
        if left == 0 {
            self.expands.outstanding.remove(&id);
        }
        if !alive(&self.completion, id, Reason::Expand) {
            return;
        }
        if let Some(logged) = x.open_logged {
            let long_matches = long.as_deref().is_some_and(|l| self.watch.matches(l));
            if !logged && !long_matches {
                self.completion.cancel(id);
                return;
            }
        }
        if let Some(l) = long {
            let file = self.file_obj(&l);
            self.completion.update(id, |e| set_path(e, x.slot, file));
            if let Some(fo) = x.fo
                && let Some(e) = self.files.map.get_mut(&fo)
            {
                e.expanded = Some(l.clone());
            }
            if let Some(key) = x.image_key {
                let path = self.dos(&l);
                if let Some(p) = self.procs.get_mut(key) {
                    p.path = path; // later actor references carry the long name
                }
            }
        }
        if left == 0 {
            self.completion.resolve(id, Reason::Expand);
        }
    }
}

/// Writes the expanded path (and name) into the event, keeping any hashes.
fn set_path(e: &mut Event, slot: Slot, file: File) {
    let target = match (&mut e.kind, slot) {
        (EventKind::File(f), Slot::File) => &mut f.file,
        (EventKind::File(f), Slot::RenameResult) => match &mut f.action {
            FileAction::Rename { file_result } => file_result,
            _ => return,
        },
        (EventKind::Process(ProcessActivity::Launch { process, .. }), Slot::Image) => &mut process.file,
        (EventKind::Module(m), Slot::Image) => match &mut m.action {
            ModuleAction::Load { file, .. } => file,
        },
        _ => return,
    };
    target.name = file_name(&file.path).to_string();
    target.path = file.path;
}
