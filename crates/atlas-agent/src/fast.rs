//! Hash maps for keys no outsider chooses (plan 1b-4a-p2, R3): kernel object
//! addresses, IRPs, PIDs, start keys and the agent's own ids. They hash with
//! foldhash, seeded per process, which costs a fraction of std's SipHash per
//! lookup. Keys an outsider can choose (paths, names, network endpoints) keep
//! std's SipHash, which resists hash flooding.

use std::collections::{HashMap, HashSet};

pub type FastMap<K, V> = HashMap<K, V, foldhash::fast::RandomState>;
pub type FastSet<K> = HashSet<K, foldhash::fast::RandomState>;
