//! The ordering stage (sensor spec §3.2 [2]): holds each event until
//! `now − event time ≥ hold`, then releases events in timestamp order. An event
//! older than the last one released is late: it passes straight through, out of
//! order, and is counted. Nothing is ever dropped here.
//!
//! The heap holds keys only (timestamp, arrival order, slot), so sifting moves
//! 24 bytes rather than a whole event; events wait in reusable slots and leave
//! one at a time (plan 1b-4a-p2, R3).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::input::Incoming;

/// A held event's key: its timestamp, its arrival order (so equal timestamps
/// keep their order) and the slot it waits in.
type Key = (i64, u64, u32);

pub struct Ordering {
    heap: BinaryHeap<Reverse<Key>>,
    /// The held events, by slot. A freed slot is reused by the next push.
    slots: Vec<Option<Incoming>>,
    free: Vec<u32>,
    seq: u64,
    /// The hold, in QPC ticks.
    hold: i64,
    /// Timestamp of the last event released in order.
    last_released: i64,
}

/// What `push` did with an event.
#[derive(Debug, PartialEq, Eq)]
pub enum Pushed {
    Held,
    /// Older than an event already released: process it now (§3.2).
    Late(Incoming),
}

impl Ordering {
    pub fn new(hold_ticks: i64) -> Self {
        Ordering {
            heap: BinaryHeap::new(),
            slots: Vec::new(),
            free: Vec::new(),
            seq: 0,
            hold: hold_ticks,
            last_released: i64::MIN,
        }
    }

    pub fn push(&mut self, inc: Incoming) -> Pushed {
        if inc.header.ts < self.last_released {
            return Pushed::Late(inc);
        }
        self.seq += 1;
        let ts = inc.header.ts;
        let slot = match self.free.pop() {
            Some(s) => {
                self.slots[s as usize] = Some(inc);
                s
            }
            None => {
                self.slots.push(Some(inc));
                (self.slots.len() - 1) as u32
            }
        };
        self.heap.push(Reverse((ts, self.seq, slot)));
        Pushed::Held
    }

    /// The next held event whose hold has passed at `now`, in timestamp order.
    pub fn next_due(&mut self, now: i64) -> Option<Incoming> {
        self.next_until(now.saturating_sub(self.hold))
    }

    /// The next held event of all, in order (a clean stop, §11.4).
    pub fn next_held(&mut self) -> Option<Incoming> {
        self.next_until(i64::MAX)
    }

    fn next_until(&mut self, limit: i64) -> Option<Incoming> {
        let &Reverse((ts, _, slot)) = self.heap.peek().filter(|Reverse(k)| k.0 <= limit)?;
        self.heap.pop();
        self.last_released = self.last_released.max(ts);
        self.free.push(slot);
        Some(self.slots[slot as usize].take().expect("a held key has its event"))
    }

    /// Every held event whose hold has passed at `now`, in timestamp order.
    #[cfg(test)]
    pub fn release(&mut self, now: i64) -> Vec<Incoming> {
        std::iter::from_fn(|| self.next_due(now)).collect()
    }

    /// Everything held, in order.
    #[cfg(test)]
    pub fn drain(&mut self) -> Vec<Incoming> {
        std::iter::from_fn(|| self.next_held()).collect()
    }

    /// Stream time (§3.2): `max(last released, now − hold)`. It advances while
    /// no events arrive.
    pub fn watermark(&self, now: i64) -> i64 {
        self.last_released.max(now.saturating_sub(self.hold))
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Header, Session};
    use atlas_etw::parse::{FileOpEnd, RawEvent};
    use proptest::prelude::*;

    fn inc(ts: i64, tag: u64) -> Incoming {
        Incoming {
            header: Header { session: Session::Sensor, pid: 1, tid: 1, ts, start_key: None },
            event: RawEvent::FileOpEnd(FileOpEnd { irp: tag, extra_information: 0, status: 0 }),
        }
    }

    fn tag(i: &Incoming) -> u64 {
        match &i.event {
            RawEvent::FileOpEnd(o) => o.irp,
            _ => unreachable!(),
        }
    }

    #[test]
    fn holds_then_releases_in_order() {
        let mut o = Ordering::new(100);
        assert_eq!(o.push(inc(50, 1)), Pushed::Held);
        assert_eq!(o.push(inc(10, 2)), Pushed::Held);
        assert!(o.release(105).is_empty()); // 105 − 100 = 5 < 10
        assert_eq!(o.release(150).iter().map(tag).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(o.watermark(150), 50);
        assert_eq!(o.watermark(1000), 900); // advances with no events
    }

    #[test]
    fn a_late_event_passes_through() {
        let mut o = Ordering::new(100);
        o.push(inc(50, 1));
        o.release(150);
        assert!(matches!(o.push(inc(49, 2)), Pushed::Late(i) if tag(&i) == 2));
        assert_eq!(o.push(inc(50, 3)), Pushed::Held); // equal is not late
    }

    #[test]
    fn freed_slots_are_reused() {
        // Plan 1b-4a-p2, R3: the slots grow to the most events held at once.
        let mut o = Ordering::new(100);
        for round in 0..3 {
            for i in 0..4 {
                o.push(inc(round * 1000 + i, i as u64));
            }
            assert_eq!(o.release(round * 1000 + 200).len(), 4);
        }
        assert_eq!((o.slots.len(), o.free.len()), (4, 4));
    }

    #[test]
    fn equal_timestamps_keep_arrival_order() {
        let mut o = Ordering::new(0);
        for t in 1..=5 {
            o.push(inc(7, t));
        }
        assert_eq!(o.drain().iter().map(tag).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    }

    proptest! {
        /// Whatever the arrival order and release times: every event comes out
        /// exactly once; in-order releases never go back in time; an event is
        /// late exactly when it is older than something already released.
        #[test]
        fn releases_in_order_and_loses_nothing(
            stamps in proptest::collection::vec(0i64..1000, 1..200),
            ticks in proptest::collection::vec(0usize..4, 1..200),
        ) {
            let mut o = Ordering::new(100);
            let mut out = Vec::new();
            let mut late = Vec::new();
            let mut now = 0;
            for (i, ts) in stamps.iter().enumerate() {
                let max_released = out.iter().map(|x: &Incoming| x.header.ts).max();
                match o.push(inc(*ts, i as u64)) {
                    Pushed::Late(x) => {
                        prop_assert!(max_released.is_some_and(|m| x.header.ts < m));
                        late.push(x);
                    }
                    Pushed::Held => prop_assert!(max_released.is_none_or(|m| *ts >= m)),
                }
                now += 37 * ticks[i % ticks.len()] as i64;
                out.extend(o.release(now));
            }
            out.extend(o.drain());
            prop_assert!(out.windows(2).all(|w| w[0].header.ts <= w[1].header.ts));
            let mut seen: Vec<u64> = out.iter().chain(&late).map(tag).collect();
            seen.sort_unstable();
            prop_assert_eq!(seen, (0..stamps.len() as u64).collect::<Vec<_>>());
        }
    }
}
