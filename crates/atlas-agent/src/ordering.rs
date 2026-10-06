//! The ordering stage (sensor spec §3.2 [2]): holds each event until
//! `now − event time ≥ hold`, then releases events in timestamp order. An event
//! older than the last one released is late: it passes straight through, out of
//! order, and is counted. Nothing is ever dropped here.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::input::Incoming;

struct Held {
    ts: i64,
    /// Arrival order, so equal timestamps keep their order.
    seq: u64,
    inc: Incoming,
}

impl PartialEq for Held {
    fn eq(&self, o: &Self) -> bool {
        (self.ts, self.seq) == (o.ts, o.seq)
    }
}
impl Eq for Held {}
impl PartialOrd for Held {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Held {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.ts, self.seq).cmp(&(o.ts, o.seq))
    }
}

pub struct Ordering {
    heap: BinaryHeap<Reverse<Held>>,
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
        Ordering { heap: BinaryHeap::new(), seq: 0, hold: hold_ticks, last_released: i64::MIN }
    }

    pub fn push(&mut self, inc: Incoming) -> Pushed {
        if inc.header.ts < self.last_released {
            return Pushed::Late(inc);
        }
        self.seq += 1;
        self.heap.push(Reverse(Held { ts: inc.header.ts, seq: self.seq, inc }));
        Pushed::Held
    }

    /// Every held event whose hold has passed at `now`, in timestamp order.
    pub fn release(&mut self, now: i64) -> Vec<Incoming> {
        self.release_until(now.saturating_sub(self.hold))
    }

    /// Everything held, in order (a clean stop, §11.4).
    pub fn drain(&mut self) -> Vec<Incoming> {
        self.release_until(i64::MAX)
    }

    fn release_until(&mut self, limit: i64) -> Vec<Incoming> {
        let mut out = Vec::new();
        while self.heap.peek().is_some_and(|Reverse(h)| h.ts <= limit) {
            let Reverse(h) = self.heap.pop().expect("peeked");
            self.last_released = self.last_released.max(h.ts);
            out.push(h.inc);
        }
        out
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
