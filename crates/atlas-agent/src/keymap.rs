//! The registry key map (sensor spec §7.4): `KeyObject → name`.
//!
//! Kernel-Registry names a key relative to a base handle unless the name starts
//! with `\REGISTRY\` (S7). A relative open whose base is named gets its full
//! name at once; otherwise it waits for its base (pending child) and is named,
//! recursively, when the base is. A closed base with pending children stays as
//! a tombstone until they are named or evicted. Names are kept raw (NT form);
//! normalization happens at emission.
//!
//! Key objects are per handle (S7), so a successful open at an address is a new
//! object there. Children waiting on the address belonged to the earlier object:
//! they become orphans, which only the seeder naming them directly can name.
//! The same holds when an unknown base closes, and when the seeder names a
//! tombstone's address after the tombstone closed (plan 1b-3a review B3).

use std::collections::{HashMap, HashSet};

/// What the map knows about a key handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Name {
    /// The full NT name.
    Full(String),
    /// `rel` below `base`, whose name is not known yet.
    Below { base: u64, rel: String },
    /// `rel` below a base object that is gone: only a name for this handle
    /// itself can name it.
    Orphan { rel: String },
}

#[derive(Debug, Clone)]
struct Entry {
    name: Name,
    /// QPC of the event (or snapshot) that set it.
    since: i64,
    /// QPC of its CloseKey: a tombstone kept for its waiting children.
    closed: Option<i64>,
    /// For LRU eviction.
    touched: u64,
}

/// What resolving a handle gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Full(String),
    /// The longest name known: the chain of relative names below the first
    /// unnamed base (`root`), or empty if nothing is known (§7.4 floor).
    Partial {
        known: String,
        root: u64,
    },
}

pub struct KeyMap {
    map: HashMap<u64, Entry>,
    /// base → children waiting for it.
    waiting: HashMap<u64, HashSet<u64>>,
    cap: usize,
    clock: u64,
    evictions: u64,
}

const MAX_DEPTH: usize = 64;

impl KeyMap {
    pub fn new(cap: usize) -> Self {
        KeyMap { map: HashMap::new(), waiting: HashMap::new(), cap, clock: 0, evictions: 0 }
    }

    /// A successful CreateKey or OpenKey (§7.4).
    pub fn open(&mut self, key: u64, base: u64, relative: &str, ts: i64) {
        self.orphan_children(key);
        let name = if starts_with_ci(relative, r"\REGISTRY\") {
            Name::Full(relative.to_string())
        } else {
            match self.map.get(&base).map(|e| &e.name) {
                Some(Name::Full(b)) => Name::Full(join(b, relative)),
                _ => Name::Below { base, rel: relative.to_string() },
            }
        };
        self.set(key, name, ts);
    }

    /// A name from the seeder (§7.4). Never overwrites an entry an ETW event
    /// set after the snapshot was taken.
    pub fn seed(&mut self, key: u64, full: String, taken: i64) -> bool {
        match self.map.get(&key) {
            Some(e) if e.since > taken => return false,
            // Read after the tombstone closed: the name is a later object's.
            Some(e) if e.closed.is_some_and(|c| c <= taken) => self.orphan_children(key),
            _ => {}
        }
        self.set(key, Name::Full(full), taken);
        true
    }

    /// The children waiting on `key` lose their base.
    fn orphan_children(&mut self, key: u64) {
        for child in self.waiting.remove(&key).unwrap_or_default() {
            if let Some(e) = self.map.get_mut(&child)
                && let Name::Below { rel, .. } = &mut e.name
            {
                e.name = Name::Orphan { rel: std::mem::take(rel) };
            }
        }
    }

    fn set(&mut self, key: u64, name: Name, since: i64) {
        self.unlink(key);
        if let Name::Below { base, .. } = &name {
            self.waiting.entry(*base).or_default().insert(key);
        }
        let named = matches!(name, Name::Full(_));
        self.clock += 1;
        self.map.insert(key, Entry { name, since, closed: None, touched: self.clock });
        if named {
            self.name_children(key);
        }
        if self.map.len() > self.cap {
            self.evict_batch();
        }
    }

    /// Names the children waiting for `key`, recursively.
    fn name_children(&mut self, key: u64) {
        let mut stack = vec![key];
        while let Some(base) = stack.pop() {
            let Some(Name::Full(b)) = self.map.get(&base).map(|e| e.name.clone()) else { continue };
            for child in self.waiting.remove(&base).unwrap_or_default() {
                if let Some(e) = self.map.get_mut(&child)
                    && let Name::Below { rel, .. } = &e.name
                {
                    e.name = Name::Full(join(&b, rel));
                    stack.push(child);
                }
            }
            self.drop_if_dead(base);
        }
    }

    /// CloseKey at `ts` (§7.4): removes the entry, or keeps it as a tombstone
    /// while children wait for it. An unknown base that closes orphans its
    /// children: its name can no longer be learned.
    pub fn close(&mut self, key: u64, ts: i64) {
        let has_children = self.waiting.get(&key).is_some_and(|c| !c.is_empty());
        match self.map.get_mut(&key) {
            Some(e) if has_children => e.closed = Some(ts),
            Some(_) => self.remove(key),
            None => self.orphan_children(key),
        }
    }

    /// When the address was last set by an event or snapshot.
    pub fn since(&self, key: u64) -> Option<i64> {
        self.map.get(&key).map(|e| e.since)
    }

    pub fn contains(&self, key: u64) -> bool {
        self.map.contains_key(&key)
    }

    /// The name of a handle (§7.4). `None` if the address is not in the map.
    pub fn resolve(&mut self, key: u64) -> Option<Resolved> {
        self.clock += 1;
        if let Some(e) = self.map.get_mut(&key) {
            e.touched = self.clock;
        }
        let mut rels: Vec<&str> = Vec::new();
        let mut at = key;
        for _ in 0..MAX_DEPTH {
            match &self.map.get(&at)?.name {
                Name::Full(n) => {
                    let mut out = n.clone();
                    for r in rels.iter().rev() {
                        out = join(&out, r);
                    }
                    return Some(Resolved::Full(out));
                }
                Name::Below { base, rel } => {
                    rels.push(rel);
                    if !self.map.contains_key(base) {
                        let known = rels.iter().rev().copied().collect::<Vec<_>>().join("\\");
                        return Some(Resolved::Partial { known, root: *base });
                    }
                    at = *base;
                }
                Name::Orphan { rel } => {
                    rels.push(rel);
                    let known = rels.iter().rev().copied().collect::<Vec<_>>().join("\\");
                    return Some(Resolved::Partial { known, root: at });
                }
            }
        }
        // A cycle or an absurd depth: give what is known, never loop.
        Some(Resolved::Partial { known: rels.iter().rev().copied().collect::<Vec<_>>().join("\\"), root: at })
    }

    fn unlink(&mut self, key: u64) {
        if let Some(Entry { name: Name::Below { base, .. }, .. }) = self.map.get(&key) {
            let base = *base;
            if let Some(c) = self.waiting.get_mut(&base) {
                c.remove(&key);
                if c.is_empty() {
                    self.waiting.remove(&base);
                }
            }
            self.drop_if_dead(base);
        }
    }

    /// A tombstone with no waiting children goes.
    fn drop_if_dead(&mut self, key: u64) {
        let waited_on = self.waiting.get(&key).is_some_and(|c| !c.is_empty());
        if !waited_on && self.map.get(&key).is_some_and(|e| e.closed.is_some()) {
            self.remove(key);
        }
    }

    fn remove(&mut self, key: u64) {
        self.unlink(key);
        self.map.remove(&key);
        // Children of a removed base keep waiting for its address; the seeder may still name it.
    }

    /// Over the cap: the least recently used eighth goes (`crate::evict`).
    fn evict_batch(&mut self) {
        let victims = crate::evict::oldest(self.map.iter().map(|(k, e)| (*k, e.touched)), self.map.len());
        for k in victims {
            self.unlink(k);
            if self.map.remove(&k).is_some() {
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

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len()).is_some_and(|h| h.eq_ignore_ascii_case(prefix))
}

fn join(base: &str, rel: &str) -> String {
    if rel.is_empty() {
        base.to_string()
    } else if base.ends_with('\\') {
        format!("{base}{rel}")
    } else {
        format!("{base}\\{rel}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const HKCU: &str = r"\REGISTRY\USER\S-1-5-21-1";

    fn full(s: &str) -> Option<Resolved> {
        Some(Resolved::Full(s.into()))
    }

    #[test]
    fn absolute_and_relative_opens() {
        let mut m = KeyMap::new(100);
        m.open(1, 0, HKCU, 10);
        m.open(2, 1, "Software", 11);
        m.open(3, 2, r"Atlas\A", 12);
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\Software\Atlas\A")));
        // Case of the prefix does not matter.
        m.open(4, 0, r"\Registry\Machine\Software", 13);
        assert_eq!(m.resolve(4), full(r"\Registry\Machine\Software"));
    }

    #[test]
    fn a_child_of_an_unknown_base_is_partial_then_named_by_the_seeder() {
        let mut m = KeyMap::new(100);
        m.open(2, 1, "Software", 10);
        m.open(3, 2, "Run", 11);
        assert_eq!(m.resolve(3), Some(Resolved::Partial { known: r"Software\Run".into(), root: 1 }));
        assert!(m.seed(1, HKCU.into(), 5));
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\Software\Run")));
        assert_eq!(m.resolve(99), None);
    }

    #[test]
    fn a_closed_base_stays_while_children_wait() {
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10); // waits for 1
        m.open(3, 2, "b", 11); // waits for 2
        m.close(2, 11);
        assert!(m.contains(2)); // tombstone: 3 still waits for it
        m.seed(1, HKCU.into(), 12);
        assert!(!m.contains(2)); // named its child, then went
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\a\b")));
        m.close(3, 13);
        assert!(m.is_empty() || !m.contains(3));
    }

    #[test]
    fn a_reused_or_closed_base_does_not_name_old_children() {
        // 0x50 was opened before we watched; "Run" waits for it.
        let mut m = KeyMap::new(100);
        m.open(0x60, 0x50, "Run", 10);
        m.close(0x50, 11); // the old base object is gone
        m.open(0x50, 0, r"\REGISTRY\MACHINE\SOFTWARE\Benign", 12);
        // Not named "Benign\Run": only a name for 0x60 itself helps.
        assert_eq!(m.resolve(0x60), Some(Resolved::Partial { known: "Run".into(), root: 0x60 }));
        m.seed(0x60, format!(r"{HKCU}\Software\Run"), 20);
        assert_eq!(m.resolve(0x60), full(&format!(r"{HKCU}\Software\Run")));

        // A reopen with no close seen (an agent close is ignored) also orphans.
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10);
        m.open(1, 0, r"\REGISTRY\MACHINE\x", 11);
        assert_eq!(m.resolve(2), Some(Resolved::Partial { known: "a".into(), root: 2 }));

        // A tombstone named by a snapshot read after it closed: orphans too;
        // one read before the close names the children.
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10); // 2 waits for 1
        m.open(3, 2, "b", 11); // 3 waits for 2
        m.open(4, 5, "c", 12); // 4 waits for 5
        m.close(2, 13);
        m.seed(2, r"\REGISTRY\MACHINE\new".into(), 14);
        assert!(matches!(m.resolve(3), Some(Resolved::Partial { root: 3, .. })));
        m.close(5, 30);
        assert!(matches!(m.resolve(4), Some(Resolved::Partial { root: 4, .. })));
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10);
        m.open(3, 2, "b", 11);
        m.close(2, 13);
        m.seed(1, HKCU.into(), 12); // read before the close
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\a\b")));
    }

    #[test]
    fn a_seed_never_overwrites_a_newer_event() {
        let mut m = KeyMap::new(100);
        m.open(1, 0, r"\REGISTRY\MACHINE\new", 20);
        assert!(!m.seed(1, r"\REGISTRY\MACHINE\old".into(), 10));
        assert_eq!(m.resolve(1), full(r"\REGISTRY\MACHINE\new"));
        assert!(m.seed(1, r"\REGISTRY\MACHINE\newer".into(), 30));
    }

    #[test]
    fn reopening_an_address_replaces_it() {
        let mut m = KeyMap::new(100);
        m.open(1, 0, r"\REGISTRY\MACHINE\a", 1);
        m.open(1, 0, r"\REGISTRY\MACHINE\b", 2);
        assert_eq!(m.resolve(1), full(r"\REGISTRY\MACHINE\b"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn the_cap_evicts_the_least_recently_used() {
        let mut m = KeyMap::new(2);
        m.open(1, 0, r"\REGISTRY\MACHINE\a", 1);
        m.open(2, 0, r"\REGISTRY\MACHINE\b", 2);
        m.resolve(1);
        m.open(3, 0, r"\REGISTRY\MACHINE\c", 3);
        assert_eq!(m.evictions(), 1);
        assert!(m.contains(1) && !m.contains(2) && m.contains(3));
        // Past a larger cap, an eighth goes at once.
        let mut m = KeyMap::new(16);
        for k in 0..17 {
            m.open(k, 0, r"\REGISTRY\MACHINE\x", k as i64);
        }
        assert_eq!((m.len(), m.evictions()), (15, 2));
    }

    #[test]
    fn a_cycle_cannot_loop() {
        let mut m = KeyMap::new(100);
        m.open(1, 2, "a", 1);
        m.open(2, 1, "b", 2);
        assert!(matches!(m.resolve(1), Some(Resolved::Partial { .. })));
    }

    proptest! {
        /// Any chain of relative opens resolves to the base's name plus every
        /// relative part, whether the base is named before or after.
        #[test]
        fn chains_resolve_whatever_the_order(parts in proptest::collection::vec("[a-z]{1,4}", 1..8), seed_first: bool) {
            let mut m = KeyMap::new(1000);
            if seed_first { m.seed(100, HKCU.into(), 0); }
            for (i, p) in parts.iter().enumerate() {
                m.open(101 + i as u64, 100 + i as u64, p, 10 + i as i64);
            }
            if !seed_first { m.seed(100, HKCU.into(), 0); }
            let want = format!("{HKCU}\\{}", parts.join("\\"));
            prop_assert_eq!(m.resolve(100 + parts.len() as u64), full(&want));
        }
    }
}
