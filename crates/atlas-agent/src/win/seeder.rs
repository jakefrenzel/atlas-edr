//! The seeder ([8], sensor spec §7.4): names key and file handles from the
//! system handle table, at start and when the pipeline meets an unknown one.
//!
//! - Each address is answered once: `named` if any holder's handle could be
//!   named, else `unnamable`. Its owner is the holder with the lowest PID, the
//!   agent excluded (§7.4); an address only the agent holds is unnamable.
//! - A holder that cannot be opened or duplicated is skipped for the next one.
//!   A name query is made once per address: every handle reaches the same object.
//! - **Verification** (plan 1b-3b, finding F2): a handle can be closed and its
//!   value reused for another object between the table read and the duplicate.
//!   After naming, the table is read a second time, and a name is kept only if
//!   its handle still sits at the address asked about. `taken` is the QPC of
//!   that second read.
//!   - Keys: the agent's duplicates stay open until then, and the check is on
//!     the duplicate itself: exact.
//!   - Files: each duplicate is closed as soon as it is named, as §7.4 says, so
//!     the agent never holds another process's file open for the whole pass
//!     (R-M5): an owner's close then still releases its sharing and runs its
//!     cleanup at once. The check is on the holder's (PID, handle). It misses
//!     only a handle value reused twice in between with the second object at
//!     the old address.
//! - The start-up pass covers the whole table and is not charged to the CPU
//!   budget. Re-reads on a miss are; while it is spent they wait, merged per
//!   kind and served oldest first (R-M6). A re-read that had to wait counts
//!   once in `seeder_deferred_rereads`.
//! - When file seeding pauses (too many stuck name queries), it stays paused
//!   for the rest of that snapshot: the remaining file addresses are unnamable.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW};
use windows::Win32::System::Threading::{
    GetCurrentThread, GetThreadTimes, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};
use windows::core::w;

use super::handles::{Entry, FileName, FileNamer, duplicate, object_name, open_process, table};
use super::privilege;
use super::util::{Owned, qpc_now};
use crate::config::ServiceConfig;
use crate::counters::ServiceCounters;
use crate::services::{HandleKind, Named, Reply, Snapshot};

/// The holders of each covered address, the agent excluded, lowest PID first;
/// and the covered addresses only the agent holds.
pub(crate) fn plan(table: &[Entry], type_index: u16, me: u32, asked: &[u64]) -> (BTreeMap<u64, Vec<Entry>>, Vec<u64>) {
    let asked: HashSet<u64> = asked.iter().copied().collect();
    let mut holders: BTreeMap<u64, Vec<Entry>> = BTreeMap::new();
    let mut own: BTreeSet<u64> = BTreeSet::new();
    for e in table.iter().filter(|e| e.type_index == type_index && e.object != 0) {
        if !asked.is_empty() && !asked.contains(&e.object) {
            continue;
        }
        if e.pid == me {
            own.insert(e.object);
        } else {
            holders.entry(e.object).or_default().push(*e);
        }
    }
    for h in holders.values_mut() {
        h.sort_by_key(|e| (e.pid, e.handle));
    }
    let own_only = own.into_iter().filter(|a| !holders.contains_key(a)).collect();
    (holders, own_only)
}

/// The addresses whose handle, given as (address, PID, handle value), still
/// refers to them in the second read of the table.
pub(crate) fn verified(after: &[Entry], handles: impl Iterator<Item = (u64, u32, u64)>) -> HashSet<u64> {
    let by_handle: HashMap<(u32, u64), u64> = after.iter().map(|e| ((e.pid, e.handle), e.object)).collect();
    handles.filter(|(address, pid, handle)| by_handle.get(&(*pid, *handle)) == Some(address)).map(|(a, ..)| a).collect()
}

/// Re-reads waiting for the CPU budget: one entry per kind, with the
/// addresses merged, served oldest first so neither kind starves (R-M6).
#[derive(Debug, Default)]
pub(crate) struct Waiting(VecDeque<(HandleKind, BTreeSet<u64>)>);

impl Waiting {
    /// Adds the addresses; true if `kind` was not waiting already.
    pub(crate) fn add(&mut self, kind: HandleKind, addresses: Vec<u64>) -> bool {
        if let Some((_, set)) = self.0.iter_mut().find(|(k, _)| *k == kind) {
            set.extend(addresses);
            return false;
        }
        self.0.push_back((kind, addresses.into_iter().collect()));
        true
    }

    pub(crate) fn next(&mut self) -> Option<(HandleKind, Vec<u64>)> {
        self.0.pop_front().map(|(k, set)| (k, set.into_iter().collect()))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// CPU spent per sliding window (§7.4).
pub(crate) struct Budget {
    limit: Duration,
    window: Duration,
    spent: VecDeque<(Instant, Duration)>,
}

impl Budget {
    pub(crate) fn new(limit: Duration, window: Duration) -> Self {
        Budget { limit, window, spent: VecDeque::new() }
    }

    fn prune(&mut self, now: Instant) {
        while self.spent.front().is_some_and(|(t, _)| now.duration_since(*t) >= self.window) {
            self.spent.pop_front();
        }
    }

    pub(crate) fn allows(&mut self, now: Instant) -> bool {
        self.prune(now);
        self.spent.iter().map(|(_, d)| *d).sum::<Duration>() < self.limit
    }

    /// When to look again: once the oldest charge leaves the window.
    pub(crate) fn retry_in(&mut self, now: Instant) -> Duration {
        self.prune(now);
        let next =
            self.spent.front().map_or(Duration::ZERO, |(t, _)| self.window.saturating_sub(now.duration_since(*t)));
        next.max(Duration::from_millis(10))
    }

    pub(crate) fn charge(&mut self, now: Instant, cpu: Duration) {
        self.spent.push_back((now, cpu));
    }
}

/// This thread's CPU time (kernel + user).
fn thread_cpu() -> Duration {
    let (mut c, mut e, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the current-thread pseudo-handle; four writable FILETIMEs.
    if unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) }.is_err() {
        return Duration::ZERO;
    }
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Duration::from_nanos((ticks(k) + ticks(u)) * 100)
}

pub(crate) struct Seeder {
    me: u32,
    key_type: u16,
    file_type: u16,
    namer: FileNamer,
    counters: Arc<ServiceCounters>,
}

impl Seeder {
    /// `None` when seeding is unavailable: without `SeDebugPrivilege` the table
    /// shows no object addresses (§7.4).
    pub(crate) fn new(cfg: &ServiceConfig, counters: Arc<ServiceCounters>) -> Option<Self> {
        if !privilege::enable(privilege::DEBUG) {
            return None;
        }
        // The type indices come from one handle of each type the agent opens itself.
        let file = std::fs::File::open(std::env::current_exe().ok()?).ok()?;
        let mut key = HKEY::default();
        // SAFETY: opens a key; closed below.
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, w!("SOFTWARE"), None, KEY_READ, &mut key) }.ok().ok()?;
        let me = std::process::id();
        let t = table();
        // SAFETY: opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
        let t = t?;
        let own = |h: u64| t.iter().find(|e| e.pid == me && e.handle == h && e.object != 0).map(|e| e.type_index);
        use std::os::windows::io::AsRawHandle;
        let key_type = own(key.0 as u64)?;
        let file_type = own(file.as_raw_handle() as u64)?;
        Some(Seeder {
            me,
            key_type,
            file_type,
            namer: FileNamer::new(cfg.name_timeout, cfg.max_stuck_helpers),
            counters,
        })
    }

    /// Answers one `Request::Seed`. `None` if the table could not be read: no
    /// reply, and the waiting events run to their deadline.
    pub(crate) fn snapshot(&mut self, kind: HandleKind, asked: Vec<u64>) -> Option<Snapshot> {
        let c = &self.counters;
        let before = table()?;
        c.seeder_table_reads.fetch_add(1, Ordering::Relaxed);
        let type_index = match kind {
            HandleKind::Key => self.key_type,
            HandleKind::File => self.file_type,
        };
        let (holders, own_only) = plan(&before, type_index, self.me, &asked);
        let mut processes: HashMap<u32, Option<Owned>> = HashMap::new();
        // A name, the (PID, handle) to verify it by, and for keys the duplicate kept open until then.
        let mut held: Vec<(Named, (u32, u64), Option<Owned>)> = Vec::new();
        let mut unnamable: Vec<(u64, u32)> = own_only.into_iter().map(|a| (a, self.me)).collect();
        let mut paused = false;
        for (address, hs) in holders {
            let owner = hs[0].pid;
            if kind == HandleKind::File && (paused || self.namer.paused()) {
                paused = true;
                unnamable.push((address, owner));
                continue;
            }
            let mut answer = None;
            for e in &hs {
                let Some(p) = processes.entry(e.pid).or_insert_with(|| open_process(e.pid)).as_ref() else {
                    continue;
                };
                let Some(dup) = duplicate(p, e.handle) else { continue };
                answer = match kind {
                    HandleKind::Key => object_name(&dup).map(|n| (n, (self.me, dup.raw().0 as u64), Some(dup))),
                    HandleKind::File => match self.namer.name(dup) {
                        // The duplicate closes here: verified by the holder's handle.
                        FileName::Named(n, _dup) => Some((n, (e.pid, e.handle), None)),
                        FileName::Unnamable => None,
                        FileName::TimedOut => {
                            c.seeder_handles_timed_out.fetch_add(1, Ordering::Relaxed);
                            None
                        }
                    },
                };
                break;
            }
            match answer {
                Some((name, by, dup)) => held.push((Named { address, owner_pid: owner, name }, by, dup)),
                None => unnamable.push((address, owner)),
            }
        }
        c.seeder_stuck_helpers.store(self.namer.stuck() as u64, Ordering::Relaxed);
        let taken = qpc_now();
        let after = table()?;
        c.seeder_table_reads.fetch_add(1, Ordering::Relaxed);
        let ok = verified(&after, held.iter().map(|(n, (pid, h), _)| (n.address, *pid, *h)));
        let mut named = Vec::with_capacity(held.len());
        for (n, _, _dup) in held {
            if ok.contains(&n.address) {
                named.push(n);
            } else {
                unnamable.push((n.address, n.owner_pid));
            }
        }
        c.seeder_handles_named.fetch_add(named.len() as u64, Ordering::Relaxed);
        c.seeder_handles_failed.fetch_add(unnamable.len() as u64, Ordering::Relaxed);
        Some(Snapshot { kind, taken, asked, named, unnamable })
    }
}

/// The seeder thread: answers `Request::Seed`, below normal priority.
pub(crate) fn run(
    mut seeder: Seeder,
    jobs: Receiver<(HandleKind, Vec<u64>)>,
    replies: Sender<Reply>,
    cfg: &ServiceConfig,
) {
    // SAFETY: lowers this thread's priority.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let counters = seeder.counters.clone();
    let mut budget = Budget::new(cfg.seeder_cpu, cfg.seeder_cpu_window);
    let mut waiting = Waiting::default();
    loop {
        let job = if waiting.is_empty() {
            jobs.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            jobs.recv_timeout(budget.retry_in(Instant::now()))
        };
        match job {
            Ok((kind, addresses)) if addresses.is_empty() => {
                // The start-up pass: at once, and not charged.
                if let Some(s) = seeder.snapshot(kind, Vec::new())
                    && replies.send(Reply::Snapshot(s)).is_err()
                {
                    return;
                }
            }
            Ok((kind, addresses)) => {
                let new = waiting.add(kind, addresses);
                if new && !budget.allows(Instant::now()) {
                    counters.seeder_deferred_rereads.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while !waiting.is_empty() && budget.allows(Instant::now()) {
            let Some((kind, addresses)) = waiting.next() else { break };
            let cpu = thread_cpu();
            let snapshot = seeder.snapshot(kind, addresses);
            budget.charge(Instant::now(), thread_cpu().saturating_sub(cpu));
            if let Some(s) = snapshot
                && replies.send(Reply::Snapshot(s)).is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(object: u64, pid: u32, handle: u64, type_index: u16) -> Entry {
        Entry { object, pid, handle, type_index }
    }

    const ME: u32 = 100;

    #[test]
    fn plan_groups_by_address_with_the_lowest_pid_first() {
        let t = [
            e(0xA, 900, 8, 1),
            e(0xA, 300, 4, 1),
            e(0xB, ME, 4, 1),
            e(0xC, ME, 8, 1),
            e(0xC, 50, 12, 1),
            e(0xD, 7, 4, 2), // another type
            e(0, 7, 8, 1),   // no address (unprivileged view)
        ];
        let (h, own) = plan(&t, 1, ME, &[]);
        assert_eq!(h.keys().copied().collect::<Vec<_>>(), [0xA, 0xC]);
        assert_eq!(h[&0xA].iter().map(|e| e.pid).collect::<Vec<_>>(), [300, 900]);
        assert_eq!(h[&0xC].iter().map(|e| e.pid).collect::<Vec<_>>(), [50], "the agent is never the owner");
        assert_eq!(own, [0xB], "held only by the agent");
        let (h, own) = plan(&t, 1, ME, &[0xC, 0xE]);
        assert_eq!(h.keys().copied().collect::<Vec<_>>(), [0xC]);
        assert!(own.is_empty());
    }

    #[test]
    fn verification_needs_the_handle_still_at_the_address() {
        let after = [e(0xA, ME, 0x40, 1), e(0xB, ME, 0x44, 1), e(0xC, 7, 0x48, 1), e(0xD, 9, 0x50, 1)];
        // Keys: the agent's duplicate. Files: the holder's handle. The PID counts:
        // 0xD is at handle 0x50 of process 9, not of process 7.
        let asked = [(0xA, ME, 0x40), (0xB, ME, 0x48), (0xC, 7, 0x48), (0xD, 7, 0x50), (0xE, 7, 0x4c)];
        assert_eq!(verified(&after, asked.into_iter()), HashSet::from([0xA, 0xC]));
    }

    /// R-M6: waiting re-reads are served oldest first, so a stream of key
    /// misses cannot starve file re-reads.
    #[test]
    fn waiting_rereads_are_served_oldest_first() {
        let mut w = Waiting::default();
        assert!(w.add(HandleKind::File, vec![1]));
        assert!(w.add(HandleKind::Key, vec![2]));
        assert!(!w.add(HandleKind::File, vec![3]), "merged into the waiting file re-read");
        assert_eq!(w.next(), Some((HandleKind::File, vec![1, 3])));
        assert!(w.add(HandleKind::File, vec![4]), "a new wait, behind the key re-read");
        assert_eq!(w.next(), Some((HandleKind::Key, vec![2])));
        assert_eq!(w.next(), Some((HandleKind::File, vec![4])));
        assert!(w.is_empty() && w.next().is_none());
    }

    #[test]
    fn budget_defers_until_charges_age_out() {
        let t = Instant::now();
        let mut b = Budget::new(Duration::from_millis(600), Duration::from_secs(60));
        assert!(b.allows(t));
        b.charge(t, Duration::from_millis(400));
        assert!(b.allows(t + Duration::from_secs(1)));
        b.charge(t + Duration::from_secs(1), Duration::from_millis(250));
        assert!(!b.allows(t + Duration::from_secs(2)), "650 ms spent of 600");
        assert_eq!(b.retry_in(t + Duration::from_secs(2)), Duration::from_secs(58));
        assert!(b.allows(t + Duration::from_secs(60)), "the first charge left the window");
        assert_eq!(Budget::new(Duration::ZERO, Duration::from_secs(1)).retry_in(t), Duration::from_millis(10));
    }

    /// Tests that need `SeDebugPrivilege`: `#[ignore]`d locally, run by CI's
    /// `agent-live` job and the elevated host script.
    mod elevated {
        use super::*;
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation};

        use super::super::super::util::{aligned, as_bytes};
        use super::super::super::value::tests::TestKey;

        fn seeder() -> Seeder {
            Seeder::new(&ServiceConfig::default(), Arc::new(ServiceCounters::default())).expect("SeDebugPrivilege")
        }

        fn object_of(h: HANDLE) -> u64 {
            let me = std::process::id();
            table().unwrap().iter().find(|e| e.pid == me && e.handle == h.0 as u64).expect("own handle").object
        }

        /// A child that inherits `handles` and sleeps; killed on drop.
        struct Holder(std::process::Child);

        impl Holder {
            fn new(handles: &[HANDLE]) -> Holder {
                for h in handles {
                    // SAFETY: test-only; marks our handle inheritable.
                    unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }.unwrap();
                }
                // One process, no grandchildren that would inherit the handles too.
                let child = std::process::Command::new("ping.exe")
                    .args(["-n", "60", "127.0.0.1"])
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                for h in handles {
                    // SAFETY: as above, undone.
                    unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }.unwrap();
                }
                std::thread::sleep(Duration::from_millis(300));
                Holder(child)
            }
        }

        impl Drop for Holder {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        #[test]
        #[ignore = "needs SeDebugPrivilege"]
        fn names_a_key_and_a_file_another_process_holds() {
            let k = TestKey::new("seeder");
            let path = std::env::temp_dir().join(format!("atlas-seeded-{}.txt", std::process::id()));
            struct Remove(std::path::PathBuf);
            impl Drop for Remove {
                fn drop(&mut self) {
                    let _ = std::fs::remove_file(&self.0);
                }
            }
            let _cleanup = Remove(path.clone()); // dropped after `f`, on success or failure
            let f = std::fs::File::create(&path).unwrap();
            let (kh, fh) = (HANDLE(k.hkey.0), HANDLE(f.as_raw_handle()));
            let holder = Holder::new(&[kh, fh]);
            let child = holder.0.id();
            let (ka, fa) = (object_of(kh), object_of(fh));
            let mut s = seeder();

            let snap = s.snapshot(HandleKind::Key, vec![ka, 0x1234]).unwrap();
            assert_eq!(snap.asked, [ka, 0x1234]);
            assert_eq!(snap.named, [Named { address: ka, owner_pid: child, name: k.path() }]);
            assert!(snap.unnamable.is_empty(), "0x1234 is in neither list: not in the table");

            let snap = s.snapshot(HandleKind::File, vec![fa]).unwrap();
            assert_eq!(snap.named.len(), 1, "{snap:?}");
            let n = &snap.named[0];
            assert_eq!((n.address, n.owner_pid), (fa, child));
            let file_name = path.file_name().unwrap().to_string_lossy().to_lowercase();
            assert!(n.name.starts_with(r"\Device\") && n.name.to_lowercase().ends_with(&file_name), "{}", n.name);
            assert!(snap.taken <= qpc_now());

            // Only the agent holds it once the child is gone.
            drop(holder);
            let snap = s.snapshot(HandleKind::Key, vec![ka]).unwrap();
            assert!(snap.named.is_empty());
            assert_eq!(snap.unnamable, [(ka, std::process::id())]);
            drop(f);
        }

        #[test]
        #[ignore = "needs SeDebugPrivilege"]
        fn start_up_pass_covers_the_table() {
            let mut s = seeder();
            let mut problems = Vec::new();
            for kind in [HandleKind::Key, HandleKind::File] {
                let t = Instant::now();
                let snap = s.snapshot(kind, Vec::new()).unwrap();
                let elapsed = t.elapsed();
                let mut seen = HashSet::new();
                for a in snap.named.iter().map(|n| n.address).chain(snap.unnamable.iter().map(|u| u.0)) {
                    assert!(seen.insert(a), "each address once: {a:#x}");
                }
                println!(
                    "{kind:?}: {} named, {} unnamable in {elapsed:?}; stuck helpers {}",
                    snap.named.len(),
                    snap.unnamable.len(),
                    s.counters.seeder_stuck_helpers.load(Ordering::Relaxed)
                );
                if snap.named.len() <= 100 {
                    problems.push(format!("{kind:?}: only {} named; a live system holds many", snap.named.len()));
                }
                // Keys: `\REGISTRY\…` or the root; files: `\Device\…`. Kernel names
                // keep their case: some keys are `\Registry\Machine\…` (plan 1b-3b,
                // finding F4), which `paths::registry` already matches.
                let well_formed = |n: &str| {
                    let n = n.to_ascii_uppercase();
                    match kind {
                        HandleKind::Key => n == r"\REGISTRY" || n.starts_with(r"\REGISTRY\"),
                        HandleKind::File => n.starts_with(r"\DEVICE\"),
                    }
                };
                let odd: Vec<&str> = snap.named.iter().map(|n| n.name.as_str()).filter(|n| !well_formed(n)).collect();
                if !odd.is_empty() {
                    problems.push(format!("{kind:?}: {} odd names, e.g. {:?}", odd.len(), &odd[..odd.len().min(5)]));
                }
            }
            assert!(problems.is_empty(), "{problems:#?}");
            assert_eq!(s.counters.seeder_table_reads.load(Ordering::Relaxed), 4, "two reads per snapshot");
        }

        /// D2: on a no-access duplicate, `ObjectNameInformation` gives the name
        /// `NtQueryKey(KeyNameInformation)` gives on a handle with access.
        #[test]
        #[ignore = "needs SeDebugPrivilege"]
        fn object_name_matches_key_name_information() {
            use windows::Wdk::System::Registry::{KeyNameInformation, NtQueryKey};
            use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
            use windows::Win32::System::Threading::GetCurrentProcess;
            let s = seeder();
            let t = table().unwrap();
            let me = std::process::id();
            let (mut compared, mut differ) = (0, Vec::new());
            for e in t.iter().filter(|e| e.type_index == s.key_type && e.pid != me && e.pid > 4).take(2000) {
                let Some(p) = open_process(e.pid) else { continue };
                let Some(zero) = duplicate(&p, e.handle) else { continue };
                let mut same = HANDLE::default();
                // SAFETY: test-only; a full-access copy for the reference query.
                let dup = unsafe {
                    DuplicateHandle(
                        p.raw(),
                        HANDLE(e.handle as *mut _),
                        GetCurrentProcess(),
                        &mut same,
                        0,
                        false,
                        DUPLICATE_SAME_ACCESS,
                    )
                };
                if dup.is_err() {
                    continue;
                }
                let same = Owned(same);
                let mut buf = aligned(4096);
                let mut ret = 0u32;
                let len = (buf.len() * 8) as u32;
                // SAFETY: test-only; aligned writable buffer.
                let st =
                    unsafe { NtQueryKey(same.raw(), KeyNameInformation, Some(buf.as_mut_ptr().cast()), len, &mut ret) };
                if st.is_err() {
                    continue;
                }
                let b = as_bytes(&buf);
                let n = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
                let units: Vec<u16> = b[4..4 + n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let reference = String::from_utf16_lossy(&units);
                compared += 1;
                let got = object_name(&zero);
                if got.as_deref() != Some(reference.as_str()) {
                    differ.push((reference, got));
                }
            }
            println!("compared {compared} key handles");
            assert!(compared > 100);
            assert!(differ.is_empty(), "{differ:?}");
        }
    }
}
