//! The process cache (sensor spec §6.2) and process identity (§5.2).
//!
//! Entries are keyed by start key, never by PID. A PID index keeps each PID's
//! entries, so "the entry live at the event's timestamp" (§5.3) survives PID
//! reuse. An entry stays `retention` after its Terminate, then is removed.

use std::collections::HashMap;

use atlas_schema::{BootId, DeviceUid, File, Integrity, ProcessRef, ProcessUid, User, process_uid};

/// The start key of a new process (§5.2, S1): `(BootId << 48) | sequence`.
pub fn start_key(boot_id: u16, sequence: u64) -> u64 {
    (u64::from(boot_id) << 48) | (sequence & ((1 << 48) - 1))
}

/// Integrity from the mandatory label's RID (§5.2).
pub fn integrity(rid: u32) -> Option<Integrity> {
    Some(match rid {
        0 => Integrity::Untrusted,
        4096 => Integrity::Low,
        8192 | 8448 => Integrity::Medium,
        12288 => Integrity::High,
        16384 => Integrity::System,
        20480 | 28672 => Integrity::Protected,
        _ => return None,
    })
}

/// The last path component (`file.name`).
pub fn file_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

/// This machine and boot, for computing process uids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub device: DeviceUid,
    pub boot: BootId,
    /// `KUSER_SHARED_DATA.BootId` (§6.1): the start key's high 16 bits.
    pub kernel_boot_id: u16,
}

impl Identity {
    pub fn uid(&self, start_key: u64) -> ProcessUid {
        process_uid(&self.device, &self.boot, start_key)
    }

    /// A reference with only what the uid gives: empty path and name (E11).
    pub fn bare_ref(&self, start_key: u64, pid: u32) -> ProcessRef {
        ProcessRef { uid: self.uid(start_key), pid, file: empty_file(), user: None }
    }
}

pub fn empty_file() -> File {
    File { path: String::new(), name: String::new(), hashes: None, signature: None }
}

/// What the cache knows about one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub start_key: u64,
    pub pid: u32,
    /// Normalized image path (drive form when mappable).
    pub path: String,
    pub user: Option<User>,
    pub cmd_line: Option<String>,
    /// Unix ns.
    pub created_time: Option<i64>,
    pub integrity: Option<Integrity>,
    pub parent: Option<ProcessRef>,
}

impl ProcInfo {
    pub fn new(start_key: u64, pid: u32) -> Self {
        ProcInfo {
            start_key,
            pid,
            path: String::new(),
            user: None,
            cmd_line: None,
            created_time: None,
            integrity: None,
            parent: None,
        }
    }

    pub fn to_ref(&self, id: &Identity) -> ProcessRef {
        ProcessRef {
            uid: id.uid(self.start_key),
            pid: self.pid,
            file: File {
                path: self.path.clone(),
                name: file_name(&self.path).to_string(),
                hashes: None,
                signature: None,
            },
            user: self.user.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    info: ProcInfo,
    /// QPC of the Launch; `i64::MIN` for processes seeded at start (§6.2).
    start: i64,
    /// QPC of the Terminate.
    end: Option<i64>,
    /// For eviction: the last lookup (a long-running service is kept).
    touched: std::cell::Cell<u64>,
}

pub struct ProcessCache {
    by_key: HashMap<u64, Entry>,
    by_pid: HashMap<u32, Vec<u64>>,
    retention: i64,
    cap: usize,
    evictions: u64,
    clock: std::cell::Cell<u64>,
}

impl ProcessCache {
    pub fn new(retention_ticks: i64, cap: usize) -> Self {
        ProcessCache {
            by_key: HashMap::new(),
            by_pid: HashMap::new(),
            retention: retention_ticks,
            cap,
            evictions: 0,
            clock: std::cell::Cell::new(0),
        }
    }

    fn tick(&self) -> u64 {
        self.clock.set(self.clock.get() + 1);
        self.clock.get()
    }

    /// Adds or updates a process. `start` is the Launch's QPC (`i64::MIN` if it
    /// was running when the agent started).
    pub fn insert(&mut self, info: ProcInfo, start: i64) {
        let key = info.start_key;
        let pid = info.pid;
        match self.by_key.get_mut(&key) {
            Some(e) => {
                e.info = info;
                e.start = e.start.min(start);
            }
            None => {
                let touched = std::cell::Cell::new(self.tick());
                self.by_key.insert(key, Entry { info, start, end: None, touched });
                self.by_pid.entry(pid).or_default().push(key);
                if self.by_key.len() > self.cap {
                    self.evict_batch();
                }
            }
        }
    }

    pub fn get(&self, start_key: u64) -> Option<&ProcInfo> {
        self.by_key.get(&start_key).map(|e| &e.info)
    }

    pub fn get_mut(&mut self, start_key: u64) -> Option<&mut ProcInfo> {
        self.by_key.get_mut(&start_key).map(|e| &mut e.info)
    }

    /// Records the Terminate; the entry stays `retention` longer.
    pub fn end(&mut self, start_key: u64, ts: i64) {
        if let Some(e) = self.by_key.get_mut(&start_key) {
            e.end = Some(ts);
        }
    }

    /// The process with this PID live at `ts` (§5.3): the latest one started at
    /// or before `ts`, if it had not ended more than `retention` before.
    pub fn lookup_at(&self, pid: u32, ts: i64) -> Option<&ProcInfo> {
        let keys = self.by_pid.get(&pid)?;
        let e = keys.iter().filter_map(|k| self.by_key.get(k)).filter(|e| e.start <= ts).max_by_key(|e| e.start)?;
        e.touched.set(self.tick());
        e.end.is_none_or(|end| ts <= end.saturating_add(self.retention)).then_some(&e.info)
    }

    /// Removes entries whose retention has passed by stream time `now`.
    pub fn expire(&mut self, now: i64) {
        let gone: Vec<u64> = self
            .by_key
            .iter()
            .filter(|(_, e)| e.end.is_some_and(|end| end.saturating_add(self.retention) < now))
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            self.remove(k);
        }
    }

    fn remove(&mut self, key: u64) {
        if let Some(e) = self.by_key.remove(&key)
            && let Some(keys) = self.by_pid.get_mut(&e.info.pid)
        {
            keys.retain(|k| *k != key);
            if keys.is_empty() {
                self.by_pid.remove(&e.info.pid);
            }
        }
    }

    /// Over the cap: processes that ended longest ago go first, then the least
    /// recently looked up (a lost Terminate cannot grow memory without limit).
    fn evict_batch(&mut self) {
        // Ended processes first (oldest end first), then the least recently used.
        let ages = self.by_key.iter().map(|(k, e)| (*k, (e.end.is_none(), e.end.unwrap_or(0), e.touched.get())));
        for k in crate::evict::oldest(ages, self.by_key.len()) {
            self.remove(k);
            self.evictions += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(key: u64, pid: u32, path: &str) -> ProcInfo {
        ProcInfo { path: path.into(), ..ProcInfo::new(key, pid) }
    }

    #[test]
    fn the_start_key_formula_matches_s1() {
        // S1: high 16 bits are the kernel BootId, low 48 the sequence number.
        assert_eq!(start_key(7, 1_373_253), 0x0007_0000_0014_F445);
        assert_eq!(start_key(1, u64::MAX), 0x0001_FFFF_FFFF_FFFF);
    }

    #[test]
    fn integrity_levels() {
        assert_eq!(integrity(8448), Some(Integrity::Medium));
        assert_eq!(integrity(28672), Some(Integrity::Protected));
        assert_eq!(integrity(9999), None);
    }

    #[test]
    fn eviction_keeps_processes_in_use() {
        let mut c = ProcessCache::new(10, 8);
        c.insert(ProcInfo::new(1, 100), i64::MIN); // a service running since boot, in use
        for k in 2..=8u64 {
            c.insert(ProcInfo::new(k, 100 + k as u32), k as i64);
        }
        assert!(c.lookup_at(100, 50).is_some());
        c.insert(ProcInfo::new(9, 109), 9); // over the cap: the least recently used goes
        assert!(c.get(1).is_some() && c.get(2).is_none());
        assert_eq!(c.evictions(), 1);
    }

    #[test]
    fn lookup_at_survives_pid_reuse() {
        let mut c = ProcessCache::new(30, 100);
        c.insert(info(1, 500, r"C:\a.exe"), 10);
        c.end(1, 20);
        c.insert(info(2, 500, r"C:\b.exe"), 25);
        assert_eq!(c.lookup_at(500, 15).unwrap().start_key, 1);
        assert_eq!(c.lookup_at(500, 22).unwrap().start_key, 1); // ended, within retention, not yet reused
        assert_eq!(c.lookup_at(500, 26).unwrap().start_key, 2);
        assert!(c.lookup_at(500, 5).is_none()); // before either started
        assert!(c.lookup_at(501, 15).is_none());
    }

    #[test]
    fn retention_then_expiry() {
        let mut c = ProcessCache::new(30, 100);
        c.insert(info(1, 7, ""), i64::MIN);
        c.end(1, 100);
        assert!(c.lookup_at(7, 130).is_some());
        assert!(c.lookup_at(7, 131).is_none());
        c.expire(130);
        assert_eq!(c.len(), 1);
        c.expire(131);
        assert!(c.is_empty());
    }

    #[test]
    fn the_cap_evicts_ended_processes_first() {
        let mut c = ProcessCache::new(1_000, 2);
        c.insert(info(1, 1, ""), 0);
        c.insert(info(2, 2, ""), 1);
        c.end(2, 5);
        c.insert(info(3, 3, ""), 2);
        assert_eq!(c.evictions(), 1);
        assert!(c.get(2).is_none() && c.get(1).is_some() && c.get(3).is_some());
    }

    #[test]
    fn refs_carry_the_uid_path_and_name() {
        let id =
            Identity { device: DeviceUid::from_bytes([1; 16]), boot: BootId::from_bytes([2; 16]), kernel_boot_id: 7 };
        let r = info(42, 9, r"C:\Windows\System32\cmd.exe").to_ref(&id);
        assert_eq!(r.uid, id.uid(42));
        assert_eq!((r.pid, r.file.name.as_str()), (9, "cmd.exe"));
        assert_eq!(id.bare_ref(42, 9).file.path, "");
    }
}
