//! Keys with the time they were last seen, forgotten oldest first. Inserting
//! and expiring cost O(1) on average, with no scans: the pipeline expires these
//! sets every tick, and a scan of a large map per event or per tick is the
//! trap `evict` describes.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub struct Recent<K> {
    map: HashMap<K, i64>,
    /// Insertion order, with the time inserted. An entry whose time no longer
    /// matches the map (the key was seen again, or removed) is stale and skipped.
    order: VecDeque<(i64, K)>,
    cap: usize,
    evictions: u64,
}

impl<K: Hash + Eq + Clone> Recent<K> {
    /// At most `cap` keys (and queued times) are kept; past it the oldest go.
    pub fn new(cap: usize) -> Self {
        Recent { map: HashMap::new(), order: VecDeque::new(), cap: cap.max(1), evictions: 0 }
    }

    /// Records `key` at `ts`; the latest time wins.
    pub fn insert(&mut self, key: K, ts: i64) {
        match self.map.get_mut(&key) {
            Some(t) if *t >= ts => return,
            Some(t) => *t = ts,
            None => {
                self.map.insert(key.clone(), ts);
            }
        }
        self.order.push_back((ts, key));
        while self.order.len() > self.cap {
            self.pop_front(true);
        }
    }

    pub fn get(&self, key: &K) -> Option<i64> {
        self.map.get(key).copied()
    }

    pub fn remove(&mut self, key: &K) -> Option<i64> {
        self.map.remove(key)
    }

    /// Forgets keys last seen before `before` (in insertion order: a key
    /// inserted out of time order may stay a little longer).
    pub fn expire(&mut self, before: i64) {
        while self.order.front().is_some_and(|(ts, _)| *ts < before) {
            self.pop_front(false);
        }
    }

    fn pop_front(&mut self, evicting: bool) {
        let Some((ts, key)) = self.order.pop_front() else { return };
        if self.map.get(&key) == Some(&ts) {
            self.map.remove(&key);
            if evicting {
                self.evictions += 1;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

/// Like [`Recent`], with a value per key: forgotten oldest first, by time or
/// past the cap, with no scans.
pub struct RecentMap<K, V> {
    map: HashMap<K, (i64, V)>,
    order: VecDeque<(i64, K)>,
    cap: usize,
}

impl<K: Hash + Eq + Clone, V> RecentMap<K, V> {
    pub fn new(cap: usize) -> Self {
        RecentMap { map: HashMap::new(), order: VecDeque::new(), cap: cap.max(1) }
    }

    /// Records `key` at `ts`; a second insert replaces the value and the time.
    pub fn insert(&mut self, key: K, ts: i64, value: V) {
        self.map.insert(key.clone(), (ts, value));
        self.order.push_back((ts, key));
        while self.order.len() > self.cap {
            self.pop_front();
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key).map(|(_, v)| v)
    }

    /// The value and the time it was recorded.
    pub fn remove(&mut self, key: &K) -> Option<(i64, V)> {
        self.map.remove(key)
    }

    /// Forgets keys recorded before `before`.
    pub fn expire(&mut self, before: i64) {
        while self.order.front().is_some_and(|(ts, _)| *ts < before) {
            self.pop_front();
        }
    }

    fn pop_front(&mut self) {
        let Some((ts, key)) = self.order.pop_front() else { return };
        if self.map.get(&key).is_some_and(|(t, _)| *t == ts) {
            self.map.remove(&key);
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recent_map_keeps_the_latest_value_and_forgets_the_oldest() {
        let mut r = RecentMap::new(3);
        r.insert(1, 10, "a");
        r.insert(2, 20, "b");
        r.insert(1, 30, "c"); // replaced: its first queue entry is stale
        r.expire(25);
        assert_eq!((r.get(&1), r.get(&2)), (Some(&"c"), None));
        for (k, ts) in [(3, 31), (4, 32), (5, 33)] {
            r.insert(k, ts, "x");
        }
        assert_eq!((r.len(), r.get(&1)), (3, None), "the cap drops the oldest");
        assert_eq!(r.remove(&5), Some((33, "x")));
        // The queue stays bounded however often one key is replaced.
        for ts in 40..100 {
            r.insert(9, ts, "y");
        }
        assert!(r.order.len() <= 3);
    }

    #[test]
    fn the_latest_time_wins_and_old_keys_expire() {
        let mut r = Recent::new(100);
        r.insert("a", 10);
        r.insert("b", 20);
        r.insert("a", 30); // seen again
        r.insert("a", 5); // older: ignored
        r.expire(25);
        assert_eq!((r.get(&"a"), r.get(&"b")), (Some(30), None));
        assert_eq!(r.remove(&"a"), Some(30));
        assert!(r.is_empty());
    }

    #[test]
    fn the_cap_drops_the_oldest() {
        let mut r = Recent::new(3);
        for (k, ts) in [(1, 1), (2, 2), (3, 3), (4, 4)] {
            r.insert(k, ts);
        }
        assert_eq!((r.len(), r.get(&1), r.evictions()), (3, None, 1));
        // Repeated sightings of one key are bounded too.
        for ts in 5..100 {
            r.insert(9, ts);
        }
        assert!(r.order.len() <= 3);
    }
}
