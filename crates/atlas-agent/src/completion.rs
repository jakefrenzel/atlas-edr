//! The completion stage (sensor spec §3.2 [5]): emits events in the order the
//! pipeline produced them. A pending event holds the line until each of its
//! reasons is resolved or past its deadline; the pipeline can also cancel it.
//!
//! Deadlines are QPC times. A reason without one (the failure-confirm window,
//! which is stream time) is resolved or cancelled by the pipeline itself.

use std::collections::{HashMap, VecDeque};

/// Why an event is incomplete (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// Hashes and signature (§6.3).
    Enrich,
    /// The other half of a Launch (§5.2).
    Join,
    /// A registry value read (§7.5).
    ValueRead,
    /// An 8.3 path expansion (§7.2).
    Expand,
    /// A handle name from the seeder (§7.4).
    Seeder,
    /// The failure-confirm window (§5.5); resolved by the pipeline.
    Confirm,
}

/// One reason an event waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    pub reason: Reason,
    /// QPC deadline; `None` means the pipeline resolves it.
    pub deadline: Option<i64>,
    /// If this reason expires unresolved, the event is dropped instead of emitted (§7.1).
    pub drop_at_deadline: bool,
}

/// What happened to a pending event that left the stage incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expired {
    pub reason: Reason,
    pub dropped: bool,
}

pub type PendingId = u64;

enum Slot<E> {
    Ready(E),
    Pending(PendingId),
}

struct Entry<E> {
    event: E,
    waits: Vec<(Wait, bool)>,
    forced: bool,
}

pub struct Completion<E> {
    queue: VecDeque<Slot<E>>,
    pending: HashMap<PendingId, Entry<E>>,
    next: PendingId,
    cap: usize,
    overflow: u64,
    /// Pending events that left (emitted, dropped or cancelled) since the last
    /// `take_exited`: the pipeline frees their bookkeeping.
    exited: Vec<PendingId>,
    /// Pending ids in push order, for the overflow rule: the front is the
    /// oldest that may still be pending. Ids that left are popped as reached.
    order: VecDeque<PendingId>,
}

impl<E> Completion<E> {
    pub fn new(pending_cap: usize) -> Self {
        Completion {
            queue: VecDeque::new(),
            pending: HashMap::new(),
            next: 0,
            cap: pending_cap,
            overflow: 0,
            exited: Vec::new(),
            order: VecDeque::new(),
        }
    }

    /// An event with nothing to wait for.
    pub fn push(&mut self, event: E) {
        self.queue.push_back(Slot::Ready(event));
    }

    /// An event that waits; an empty `waits` makes it ready at the next drain.
    pub fn push_pending(&mut self, event: E, waits: Vec<Wait>) -> PendingId {
        self.next += 1;
        let id = self.next;
        self.pending.insert(id, Entry { event, waits: waits.into_iter().map(|w| (w, false)).collect(), forced: false });
        self.queue.push_back(Slot::Pending(id));
        self.order.push_back(id);
        if self.pending.len() > self.cap {
            // The oldest pending event goes out as is (§3.2). Each id is popped
            // once, so this costs O(1) per push on average, overload included.
            while let Some(oldest) = self.order.pop_front() {
                if let Some(e) = self.pending.get_mut(&oldest) {
                    e.forced = true;
                    self.overflow += 1;
                    break;
                }
            }
        }
        id
    }

    /// Changes a pending event (fills in a result). False if it is gone.
    pub fn update(&mut self, id: PendingId, f: impl FnOnce(&mut E)) -> bool {
        match self.pending.get_mut(&id) {
            Some(e) => {
                f(&mut e.event);
                true
            }
            None => false,
        }
    }

    /// Adds a reason to a pending event that is still waiting.
    pub fn add_wait(&mut self, id: PendingId, wait: Wait) -> bool {
        match self.pending.get_mut(&id) {
            Some(e) => {
                e.waits.push((wait, false));
                true
            }
            None => false,
        }
    }

    /// Marks every wait of this reason as resolved.
    pub fn resolve(&mut self, id: PendingId, reason: Reason) {
        if let Some(e) = self.pending.get_mut(&id) {
            for (w, done) in &mut e.waits {
                if w.reason == reason {
                    *done = true;
                }
            }
        }
    }

    /// Whether the event still waits for this reason.
    pub fn is_waiting(&self, id: PendingId, reason: Reason) -> bool {
        self.pending.get(&id).is_some_and(|e| e.waits.iter().any(|(w, done)| w.reason == reason && !done))
    }

    /// Drops a pending event (for example a delete whose operation failed).
    pub fn cancel(&mut self, id: PendingId) -> bool {
        let gone = self.pending.remove(&id).is_some();
        if gone {
            self.exited.push(id);
        }
        gone
    }

    /// The pending events that left since the last call.
    pub fn take_exited(&mut self) -> Vec<PendingId> {
        std::mem::take(&mut self.exited)
    }

    /// Emits everything that can go at `now`, in order. `on_incomplete` sees
    /// each event that leaves with unresolved reasons, and what happened to
    /// them, before it is emitted or dropped.
    pub fn drain(&mut self, now: i64, mut on_incomplete: impl FnMut(&mut E, &[Expired])) -> Vec<E> {
        let mut out = Vec::new();
        while let Some(front) = self.queue.front() {
            let id = match front {
                Slot::Ready(_) => {
                    let Some(Slot::Ready(e)) = self.queue.pop_front() else { unreachable!() };
                    out.push(e);
                    continue;
                }
                Slot::Pending(id) => *id,
            };
            let Some(entry) = self.pending.get(&id) else {
                self.queue.pop_front(); // cancelled
                continue;
            };
            let open: Vec<&Wait> = entry.waits.iter().filter(|(_, done)| !done).map(|(w, _)| w).collect();
            let blocked = !entry.forced && open.iter().any(|w| w.deadline.is_none_or(|d| now < d));
            if blocked {
                break;
            }
            let expired: Vec<Expired> = open
                .iter()
                .map(|w| Expired { reason: w.reason, dropped: w.drop_at_deadline && !entry.forced })
                .collect();
            self.queue.pop_front();
            let mut entry = self.pending.remove(&id).expect("present");
            self.exited.push(id);
            if !expired.is_empty() {
                on_incomplete(&mut entry.event, &expired);
            }
            if !expired.iter().any(|x| x.dropped) {
                out.push(entry.event);
            }
        }
        while self.order.front().is_some_and(|p| !self.pending.contains_key(p)) {
            self.order.pop_front();
        }
        out
    }

    /// Everything, in order (a clean stop, §11.4): unresolved reasons count as
    /// expired, as if their deadlines had passed, so a reason that drops at its
    /// deadline drops here too.
    pub fn flush(&mut self, mut on_incomplete: impl FnMut(&mut E, &[Expired])) -> Vec<E> {
        let mut out = Vec::new();
        while let Some(slot) = self.queue.pop_front() {
            match slot {
                Slot::Ready(e) => out.push(e),
                Slot::Pending(id) => {
                    if let Some(mut entry) = self.pending.remove(&id) {
                        self.exited.push(id);
                        let expired: Vec<Expired> = entry
                            .waits
                            .iter()
                            .filter(|(_, done)| !done)
                            .map(|(w, _)| Expired { reason: w.reason, dropped: w.drop_at_deadline && !entry.forced })
                            .collect();
                        if !expired.is_empty() {
                            on_incomplete(&mut entry.event, &expired);
                        }
                        if !expired.iter().any(|x| x.dropped) {
                            out.push(entry.event);
                        }
                    }
                }
            }
        }
        self.order.clear();
        out
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Events forced out by the pending cap so far (`pending_overflow`).
    pub fn overflow(&self) -> u64 {
        self.overflow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn w(reason: Reason, deadline: i64) -> Wait {
        Wait { reason, deadline: Some(deadline), drop_at_deadline: false }
    }

    #[test]
    fn a_pending_event_holds_the_line() {
        let mut c = Completion::new(10);
        c.push(1);
        let id = c.push_pending(2, vec![w(Reason::Enrich, 100)]);
        c.push(3);
        assert_eq!(c.drain(0, |_, _| {}), [1]);
        c.update(id, |e| *e = 20);
        c.resolve(id, Reason::Enrich);
        assert_eq!(c.drain(0, |_, _| {}), [20, 3]);
        assert!(c.is_empty());
    }

    #[test]
    fn a_deadline_emits_as_is_and_reports_it() {
        let mut c = Completion::new(10);
        c.push_pending(1, vec![w(Reason::Enrich, 100), w(Reason::Join, 50)]);
        assert!(c.drain(99, |_, _| {}).is_empty());
        let mut seen = Vec::new();
        assert_eq!(c.drain(100, |_, x| seen.extend_from_slice(x)), [1]);
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|x| !x.dropped));
    }

    #[test]
    fn a_clean_stop_drops_what_its_deadline_would() {
        let mut c = Completion::new(10);
        let a = c.push_pending(1, vec![Wait { reason: Reason::Seeder, deadline: Some(10), drop_at_deadline: true }]);
        let b = c.push_pending(2, vec![w(Reason::Enrich, 10)]);
        let x = c.push_pending(3, vec![w(Reason::Enrich, 10)]);
        c.cancel(x);
        assert_eq!(c.flush(|_, _| {}), [2]);
        let mut gone = c.take_exited();
        gone.sort_unstable();
        assert_eq!(gone, [a, b, x]);
    }

    #[test]
    fn drop_at_deadline_drops() {
        let mut c = Completion::new(10);
        c.push_pending(1, vec![Wait { reason: Reason::Seeder, deadline: Some(10), drop_at_deadline: true }]);
        c.push(2);
        let mut seen = Vec::new();
        assert_eq!(c.drain(10, |_, x| seen.extend_from_slice(x)), [2]);
        assert_eq!(seen, [Expired { reason: Reason::Seeder, dropped: true }]);
    }

    #[test]
    fn a_reason_without_deadline_waits_for_the_pipeline() {
        let mut c = Completion::new(10);
        let id = c.push_pending(1, vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: false }]);
        assert!(c.drain(i64::MAX, |_, _| {}).is_empty());
        assert!(c.is_waiting(id, Reason::Confirm));
        c.resolve(id, Reason::Confirm);
        assert_eq!(c.drain(0, |_, _| {}), [1]);
    }

    #[test]
    fn cancel_removes_without_emitting() {
        let mut c = Completion::new(10);
        let id = c.push_pending(1, vec![w(Reason::Confirm, 10)]);
        c.push(2);
        assert!(c.cancel(id));
        assert_eq!(c.drain(0, |_, _| {}), [2]);
        assert!(!c.update(id, |_| {}));
    }

    #[test]
    fn overflow_forces_the_oldest_out() {
        let mut c = Completion::new(2);
        for i in 0..3 {
            c.push_pending(i, vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: true }]);
        }
        assert_eq!(c.overflow(), 1);
        let mut seen = Vec::new();
        // The forced one leaves (as is, not dropped); the next still waits.
        assert_eq!(c.drain(0, |_, x| seen.extend_from_slice(x)), [0]);
        assert_eq!(seen, [Expired { reason: Reason::Confirm, dropped: false }]);
        assert_eq!(c.pending_len(), 2);
    }

    #[test]
    fn flush_emits_everything_in_order() {
        let mut c = Completion::new(10);
        c.push(1);
        c.push_pending(2, vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: false }]);
        c.push(3);
        assert_eq!(c.flush(|_, _| {}), [1, 2, 3]);
    }

    proptest! {
        /// Output order is push order, minus cancelled and dropped events,
        /// whatever order the results arrive in.
        #[test]
        fn order_is_preserved(ops in proptest::collection::vec((any::<bool>(), 0u8..4), 1..100)) {
            let mut c = Completion::new(1000);
            let mut ids = Vec::new();
            let mut expect = Vec::new();
            for (i, (pending, fate)) in ops.iter().enumerate() {
                if *pending {
                    let id = c.push_pending(i, vec![w(Reason::Enrich, 1_000)]);
                    ids.push((id, i, *fate));
                    if *fate != 0 { expect.push(i); }
                } else {
                    c.push(i);
                    expect.push(i);
                }
            }
            let mut out = c.drain(0, |_, _| {});
            for (id, _, fate) in ids.iter().rev() {
                match fate {
                    0 => { c.cancel(*id); }
                    1 => c.resolve(*id, Reason::Enrich),
                    _ => {}
                }
                out.extend(c.drain(0, |_, _| {}));
            }
            out.extend(c.drain(1_000, |_, _| {}));
            prop_assert_eq!(out, expect);
        }
    }
}
