//! The services' threads and the routing of [`Request`]s to them (sensor spec
//! §3.2 [4] and [8]; plan 1b-3b, D5).
//!
//! - **Hash workers** (2, below normal): `Enrich`.
//! - **Reader lane** (1): value reads, on the ordered path and the fast path.
//!   It does no file I/O, so a slow directory never delays a value read (R-M2).
//! - **Expander lane** (1): 8.3 expansion (D4).
//! - **Seeder** (1, below normal): `Seed`, when `SeDebugPrivilege` is available.
//!
//! Lanes are bounded and never block the caller. A full lane drops the request
//! and counts it: a dropped `Enrich`, `ReadValue`, `Expand` or `Seed` costs its
//! event's deadline (clarification 25).
//!
//! **Invalidations wake no thread** (plan 1b-4a-p2, R3a): handing a job to a
//! parked worker costs about 22 µs of CPU, and every file Update, Rename and
//! Delete invalidates both caches. The hash cache's invalidations go on a
//! channel the workers drain before their next lookup: the caller (the
//! pipeline thread) sends without taking a lock a below-normal worker could
//! hold (review 4). Its key holds the USN, so they only free memory, and past
//! the lane's capacity the newest is dropped. The expansion cache's wait in a
//! list the expander applies before its next expansion, so none is applied
//! late; if the list fills first, the expander clears its whole cache instead
//! (R-M3), and the list keeps nothing more until then (review 3). Neither is a
//! counted drop. Replies come back on one channel,
//! which the driver (plan 1b-3c) feeds to `Pipeline::reply`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};

use super::expand::{Dir, Expander, NtDir};
use super::hash::{Enricher, HashCache};
use super::seeder::{self, Seeder};
use super::{privilege, value};
use crate::completion::PendingId;
use crate::config::ServiceConfig;
use crate::counters::ServiceCounters;
use crate::intake::FastRead;
use crate::services::{EarlyKey, HandleKind, Reply, Request, ValueRead};

/// An `Enrich`: the event waiting for it, and the file's NT path.
type HashJob = (PendingId, String);

enum ReaderJob {
    Read(PendingId, ValueRead),
    Early(EarlyKey, ValueRead),
}

/// An `Expand`: the event, its path slot, and the NT path.
type ExpandJob = (PendingId, u8, String);

/// The hash cache's invalidations, waiting for a worker's next lookup.
struct Forgets {
    /// Shared by the workers only.
    rx: Mutex<Receiver<String>>,
    /// Sent and not yet applied.
    waiting: AtomicUsize,
}

impl Forgets {
    /// Applies every waiting invalidation to `cache`.
    fn apply(&self, cache: &Mutex<HashCache>) {
        let paths: Vec<String> = self.rx.lock().expect("hash invalidations").try_iter().collect();
        if paths.is_empty() {
            return;
        }
        self.waiting.fetch_sub(paths.len(), Ordering::Relaxed);
        let mut c = cache.lock().expect("hash cache");
        for p in &paths {
            c.invalidate(p);
        }
    }
}

/// The expansion cache's invalidations, waiting for the expander's next job.
#[derive(Default)]
struct Invalidations {
    paths: Vec<String>,
    /// Too many waited: the expander clears its whole cache instead.
    lost: bool,
}

pub struct Services {
    hash: SyncSender<HashJob>,
    reader: SyncSender<ReaderJob>,
    expander: SyncSender<ExpandJob>,
    /// The workers' cache, for the tests to look into.
    #[cfg(test)]
    cache: Arc<Mutex<HashCache>>,
    forget: Sender<String>,
    forgets: Arc<Forgets>,
    invalidations: Arc<Mutex<Invalidations>>,
    /// The most invalidations that wait for either cache (the lane's capacity).
    invalidations_cap: usize,
    seeder: Option<SyncSender<(HandleKind, Vec<u64>)>>,
    replies: Receiver<Reply>,
    counters: Arc<ServiceCounters>,
}

/// The driver's view (plan 1b-3c): the same two calls.
impl crate::driver::Lanes for Services {
    fn submit(&self, r: Request) {
        Services::submit(self, r);
    }

    fn replies(&self) -> Vec<Reply> {
        Services::replies(self).collect()
    }
}

impl Services {
    /// Starts the threads. Enables `SeBackupPrivilege` for value reads and
    /// `SeDebugPrivilege` for the seeder; without them reads use a normal open
    /// and seeding is off ([`Services::seeding`]).
    pub fn start(cfg: &ServiceConfig) -> Services {
        Self::start_with(cfg, NtDir)
    }

    /// As [`Services::start`], with the expander's directory lookups given (tests).
    pub(crate) fn start_with<D: Dir + Send + 'static>(cfg: &ServiceConfig, dir: D) -> Services {
        let counters = Arc::new(ServiceCounters::default());
        let (reply_tx, replies) = channel();

        let cache = Arc::new(Mutex::new(HashCache::new(cfg.hash_cache_cap)));
        let (hash, hash_rx) = sync_channel::<HashJob>(cfg.lane_cap);
        let hash_rx = Arc::new(Mutex::new(hash_rx));
        let (forget, forget_rx) = channel();
        let forgets = Arc::new(Forgets { rx: Mutex::new(forget_rx), waiting: AtomicUsize::new(0) });
        for i in 0..cfg.hash_workers.max(1) {
            let (rx, tx, cache, counters, cap) =
                (hash_rx.clone(), reply_tx.clone(), cache.clone(), counters.clone(), cfg.hash_size_cap);
            let forgets = forgets.clone();
            std::thread::Builder::new()
                .name(format!("atlas-hash-{i}"))
                .spawn(move || hash_worker(&rx, &tx, &cache, &forgets, &counters, cap))
                .expect("spawn a hash worker");
        }

        let backup = privilege::enable(privilege::BACKUP);
        let (reader, reader_rx) = sync_channel::<ReaderJob>(cfg.lane_cap);
        let tx = reply_tx.clone();
        std::thread::Builder::new()
            .name("atlas-reader".into())
            .spawn(move || reader_lane(reader_rx, &tx, backup))
            .expect("spawn the reader lane");

        let invalidations = Arc::new(Mutex::new(Invalidations::default()));
        let (expander, expand_rx) = sync_channel::<ExpandJob>(cfg.lane_cap);
        let (tx, pending) = (reply_tx.clone(), invalidations.clone());
        std::thread::Builder::new()
            .name("atlas-expander".into())
            .spawn(move || expander_lane(expand_rx, &tx, Expander::new(dir), &pending))
            .expect("spawn the expander lane");

        let seeder = Seeder::new(cfg, counters.clone()).map(|s| {
            let (seed_tx, seed_rx) = sync_channel(cfg.lane_cap);
            let (tx, cfg) = (reply_tx.clone(), cfg.clone());
            std::thread::Builder::new()
                .name("atlas-seeder".into())
                .spawn(move || seeder::run(s, seed_rx, tx, &cfg))
                .expect("spawn the seeder");
            seed_tx
        });

        Services {
            hash,
            reader,
            expander,
            #[cfg(test)]
            cache,
            forget,
            forgets,
            invalidations,
            invalidations_cap: cfg.lane_cap,
            seeder,
            replies,
            counters,
        }
    }

    /// Whether the seeder runs. If not, the driver sets `Config::seed_on_start`
    /// and `seed_on_miss` to false (§7.4) and Sensor Health reports it.
    pub fn seeding(&self) -> bool {
        self.seeder.is_some()
    }

    pub fn counters(&self) -> Arc<ServiceCounters> {
        self.counters.clone()
    }

    /// The replies so far, without waiting.
    pub fn replies(&self) -> impl Iterator<Item = Reply> + '_ {
        self.replies.try_iter()
    }

    /// Hands a request to its lane. Never blocks; an invalidation takes one
    /// short lock, the expander's list's, and wakes no thread (R3a).
    pub fn submit(&self, r: Request) {
        match r {
            Request::Enrich { id, nt_path, .. } => self.count(self.hash.try_send((id, nt_path))),
            Request::InvalidateHash { nt_path } => {
                if self.forgets.waiting.fetch_add(1, Ordering::Relaxed) < self.invalidations_cap {
                    let _ = self.forget.send(nt_path.clone());
                } else {
                    self.forgets.waiting.fetch_sub(1, Ordering::Relaxed);
                }
                // A worker or the expander that panicked holding a lock must
                // not take the pipeline thread with it (review 9).
                let mut q = self.invalidations.lock().unwrap_or_else(PoisonError::into_inner);
                if q.lost {
                    // The whole cache is cleared at the next job anyway.
                } else if q.paths.len() >= self.invalidations_cap {
                    q.paths.clear();
                    q.lost = true;
                } else {
                    q.paths.push(nt_path);
                }
            }
            Request::ReadValue { id, read } => self.count(self.reader.try_send(ReaderJob::Read(id, read))),
            Request::Expand { id, slot, nt_path } => self.count(self.expander.try_send((id, slot, nt_path))),
            Request::Seed { kind, addresses } => {
                if let Some(s) = &self.seeder {
                    self.count(s.try_send((kind, addresses)));
                }
            }
        }
    }

    fn count<T>(&self, sent: Result<(), TrySendError<T>>) {
        if sent.is_err() {
            self.counters.service_queue_drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The fast path's value reads (§7.5), for Session A's `Intake`. Called
    /// from the ETW callback: never blocks.
    pub fn fast_read(&self) -> FastRead {
        let (tx, counters) = (self.reader.clone(), self.counters.clone());
        Box::new(move |key, read| {
            if tx.try_send(ReaderJob::Early(key, read)).is_err() {
                counters.service_queue_drops.fetch_add(1, Ordering::Relaxed);
            }
        })
    }
}

fn hash_worker(
    jobs: &Mutex<Receiver<HashJob>>,
    replies: &Sender<Reply>,
    cache: &Mutex<HashCache>,
    forgets: &Forgets,
    counters: &ServiceCounters,
    size_cap: u64,
) {
    // SAFETY: lowers this thread's priority.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let enricher = Enricher::new(size_cap);
    loop {
        let (id, nt_path) = match jobs.lock().expect("hash lane").recv() {
            Ok(j) => j,
            Err(_) => return,
        };
        forgets.apply(cache);
        let r = enricher.enrich(&nt_path, cache);
        counters.hash_cache_evictions.store(cache.lock().expect("hash cache").evictions, Ordering::Relaxed);
        let reply = Reply::Enriched { id, hashes: r.hashes, signature: r.signature, error: r.error };
        if replies.send(reply).is_err() {
            return;
        }
    }
}

fn reader_lane(jobs: Receiver<ReaderJob>, replies: &Sender<Reply>, backup: bool) {
    for job in jobs {
        let reply = match job {
            ReaderJob::Read(id, read) => Reply::ValueRead { id, result: value::read(&read, backup) },
            ReaderJob::Early(event, read) => {
                let result = value::read(&read, backup);
                Reply::EarlyRead { event, read, result }
            }
        };
        if replies.send(reply).is_err() {
            return;
        }
    }
}

fn expander_lane<D: Dir>(
    jobs: Receiver<ExpandJob>,
    replies: &Sender<Reply>,
    mut expander: Expander<D>,
    pending: &Mutex<Invalidations>,
) {
    for (id, slot, nt) in jobs {
        let waiting = std::mem::take(&mut *pending.lock().expect("invalidations"));
        if waiting.lost {
            expander.clear();
        }
        for p in &waiting.paths {
            expander.invalidate(p);
        }
        let reply = Reply::Expanded { id, slot, long_path: expander.expand(&nt, Instant::now()) };
        if replies.send(reply).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::EnrichTarget;
    use std::collections::HashMap;
    use std::sync::Condvar;
    use std::time::Duration;
    use windows::Win32::System::Registry::REG_DWORD;

    use super::super::value::tests::{TestKey, units};

    fn wait(s: &Services, n: usize) -> Vec<Reply> {
        let until = Instant::now() + Duration::from_secs(10);
        let mut out = Vec::new();
        while out.len() < n && Instant::now() < until {
            out.extend(s.replies());
            std::thread::sleep(Duration::from_millis(5));
        }
        out
    }

    fn nt(dos: &str) -> String {
        let (dev, drive) =
            super::super::lookups::query_drives().into_iter().find(|(_, d)| dos[..2].eq_ignore_ascii_case(d)).unwrap();
        format!("{dev}{}", &dos[drive.len()..])
    }

    #[test]
    fn every_request_kind_is_answered() {
        let s = Services::start(&ServiceConfig::default());
        let k = TestKey::new("services");
        k.set(k.hkey, &units("v"), REG_DWORD.0, &9u32.to_le_bytes());
        let read = ValueRead { key_path: k.path(), value_name: units("v") };
        let notepad = nt(&format!(r"{}\System32\notepad.exe", std::env::var("SystemRoot").unwrap()));
        s.submit(Request::Enrich { id: 1, target: EnrichTarget::LaunchImage, nt_path: notepad.clone() });
        s.submit(Request::ReadValue { id: 2, read: read.clone() });
        s.submit(Request::Expand { id: 3, slot: 1, nt_path: notepad.to_uppercase() });
        let key = EarlyKey { ts: 5, tid: 6, key_object: 7 };
        (s.fast_read())(key, read.clone());
        let mut replies = wait(&s, 4);
        replies.sort_by_key(|r| format!("{r:?}").chars().take(8).collect::<String>());
        assert_eq!(replies.len(), 4, "{replies:?}");
        let nine =
            Some(crate::services::ValueData { value_type: REG_DWORD.0, size: 4, data: 9u32.to_le_bytes().to_vec() });
        for r in replies {
            match r {
                Reply::Enriched { id, hashes, signature, error } => {
                    assert_eq!(id, 1);
                    assert!(hashes.is_some() && !error);
                    assert_eq!(signature.map(|s| s.status), Some(atlas_schema::SignatureStatus::Valid));
                }
                Reply::ValueRead { id, result } => assert_eq!((id, result), (2, nine.clone())),
                Reply::Expanded { id, slot, long_path } => {
                    assert_eq!((id, slot), (3, 1));
                    assert_eq!(long_path, Some(notepad.to_uppercase()), "no short components: unchanged");
                }
                Reply::EarlyRead { event, read: r, result } => {
                    assert_eq!((event, r, result), (key, read.clone(), nine.clone()));
                }
                Reply::Snapshot(_) => panic!("no seed was asked"),
            }
        }
        assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn full_lanes_drop_and_count() {
        let cfg = ServiceConfig { lane_cap: 1, hash_workers: 1, ..ServiceConfig::default() };
        let s = Services::start(&cfg);
        let big = nt(&format!(r"{}\System32\ntoskrnl.exe", std::env::var("SystemRoot").unwrap()));
        for id in 0..50 {
            s.submit(Request::Enrich { id, target: EnrichTarget::Module, nt_path: big.clone() });
        }
        let drops = s.counters().service_queue_drops.load(Ordering::Relaxed);
        assert!(drops > 0, "a lane of one cannot take 50 at once");
        let answered = wait(&s, (50 - drops) as usize).len() as u64;
        assert_eq!(answered + drops, 50, "each request is answered or counted, never both");
    }

    #[test]
    fn seeding_needs_the_debug_privilege() {
        let s = Services::start(&ServiceConfig::default());
        assert_eq!(s.seeding(), privilege::enable(privilege::DEBUG));
        if !s.seeding() {
            s.submit(Request::Seed { kind: HandleKind::Key, addresses: vec![] });
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(s.replies().count(), 0);
            assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0);
        }
    }

    /// Directory lookups from a table, which wait while the gate is closed:
    /// a stand-in for a directory whose query stalls (a Cloud Files
    /// placeholder, a redirector).
    /// (closed, short → long), and the gate's condition.
    type Gate = (Mutex<(bool, HashMap<String, String>)>, Condvar);

    #[derive(Clone, Default)]
    struct GateDir(Arc<Gate>);

    impl GateDir {
        fn set(&self, short: &str, long: &str) {
            self.0.0.lock().unwrap().1.insert(short.to_uppercase(), long.into());
        }
        fn close(&self) {
            self.0.0.lock().unwrap().0 = true;
        }
        fn open(&self) {
            self.0.0.lock().unwrap().0 = false;
            self.0.1.notify_all();
        }
    }

    impl Dir for GateDir {
        fn long_name(&mut self, _dir: &str, component: &str) -> Option<String> {
            let (lock, cv) = &*self.0;
            let g = cv.wait_while(lock.lock().unwrap(), |s| s.0).unwrap();
            g.1.get(&component.to_uppercase()).cloned()
        }
    }

    const V: &str = r"\Device\HarddiskVolume3";

    fn expanded(replies: &[Reply], want: u64) -> Option<Option<String>> {
        replies.iter().find_map(|r| match r {
            Reply::Expanded { id, long_path, .. } if *id == want => Some(long_path.clone()),
            _ => None,
        })
    }

    /// R-M2: a value read is answered while a directory lookup is stalled.
    #[test]
    fn value_reads_never_wait_behind_directory_lookups() {
        let dir = GateDir::default();
        dir.close();
        let s = Services::start_with(&ServiceConfig::default(), dir.clone());
        let k = TestKey::new("lanes");
        k.set(k.hkey, &units("v"), REG_DWORD.0, &1u32.to_le_bytes());
        s.submit(Request::Expand { id: 1, slot: 0, nt_path: format!(r"{V}\STALLE~1\x") });
        std::thread::sleep(Duration::from_millis(50));
        s.submit(Request::ReadValue { id: 2, read: ValueRead { key_path: k.path(), value_name: units("v") } });
        let got = wait(&s, 1);
        assert!(matches!(got.as_slice(), [Reply::ValueRead { id: 2, result: Some(_) }]), "{got:?}");
        dir.open();
        assert!(expanded(&wait(&s, 1), 1).is_some(), "the expansion still completes");
    }

    /// The expander's cache answers SECRET~1 with SecretStuffAAA, then its
    /// lane blocks on an expansion of OTHER~1 and a second SECRET~1 waits.
    fn blocked_after_caching_secret(lane_cap: usize) -> (GateDir, Services) {
        let dir = GateDir::default();
        dir.set("SECRET~1", "SecretStuffAAA");
        let cfg = ServiceConfig { lane_cap, ..ServiceConfig::default() };
        let s = Services::start_with(&cfg, dir.clone());
        s.submit(Request::Expand { id: 1, slot: 0, nt_path: format!(r"{V}\SECRET~1") });
        assert_eq!(expanded(&wait(&s, 1), 1), Some(Some(format!(r"{V}\SecretStuffAAA"))));
        dir.close();
        s.submit(Request::Expand { id: 2, slot: 0, nt_path: format!(r"{V}\OTHER~1") }); // the lane blocks on it
        std::thread::sleep(Duration::from_millis(100));
        s.submit(Request::Expand { id: 3, slot: 0, nt_path: format!(r"{V}\SECRET~1") });
        (dir, s)
    }

    /// R3a: an invalidation reaches the expander without a job of its own,
    /// before the next expansion, even while its lane is full.
    #[test]
    fn an_invalidation_waits_for_the_expanders_next_job() {
        let (dir, s) = blocked_after_caching_secret(1);
        // The lane (one job) is full; the list holds one path: it fits there.
        s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\SecretStuffAAA") });
        dir.set("SECRET~1", "SecretStuffBBB"); // the short name now belongs to another directory
        dir.open();
        let got = wait(&s, 2);
        assert_eq!(expanded(&got, 3), Some(Some(format!(r"{V}\SecretStuffBBB"))), "{got:?}");
        assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0, "nothing was dropped");
    }

    /// R-M3, R3a: when more invalidations wait than the list holds, the
    /// expander clears its whole cache before its next job, so a reused short
    /// name is not answered from the cache.
    #[test]
    fn too_many_waiting_invalidations_clear_the_expansion_cache() {
        let (dir, s) = blocked_after_caching_secret(2);
        for i in 0..2 {
            s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\Unrelated{i}") });
        }
        // The one that matters overflows the list (review 2).
        s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\SecretStuffAAA") });
        dir.set("SECRET~1", "SecretStuffBBB");
        dir.open();
        let got = wait(&s, 2);
        assert_eq!(expanded(&got, 3), Some(Some(format!(r"{V}\SecretStuffBBB"))), "{got:?}");
        assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0);
    }

    /// R3a, review 4: the caller only sends; a worker applies the invalidation
    /// before its next lookup.
    #[test]
    fn an_invalidation_reaches_the_hash_cache_before_the_next_lookup() {
        let s = Services::start(&ServiceConfig::default());
        let system32 = format!(r"{}\System32", std::env::var("SystemRoot").unwrap());
        let (notepad, cmd) = (nt(&format!(r"{system32}\notepad.exe")), nt(&format!(r"{system32}\cmd.exe")));
        s.submit(Request::Enrich { id: 1, target: EnrichTarget::LaunchImage, nt_path: notepad.clone() });
        assert_eq!(wait(&s, 1).len(), 1);
        assert_eq!(s.cache.lock().unwrap().len(), 1, "notepad.exe is cached");
        s.submit(Request::InvalidateHash { nt_path: notepad.to_uppercase() });
        assert_eq!(s.cache.lock().unwrap().len(), 1, "the caller touched no cache");
        s.submit(Request::Enrich { id: 2, target: EnrichTarget::LaunchImage, nt_path: cmd });
        assert_eq!(wait(&s, 1).len(), 1);
        assert_eq!(s.cache.lock().unwrap().len(), 1, "notepad.exe gone before cmd.exe's lookup, cmd.exe cached");
    }

    /// Review 3: once the expander's list has overflowed it keeps nothing more,
    /// and the hash cache's invalidations stop at the lane's capacity.
    #[test]
    fn invalidations_past_the_cap_are_not_kept() {
        let cfg = ServiceConfig { lane_cap: 2, ..ServiceConfig::default() };
        let s = Services::start(&cfg);
        for i in 0..5 {
            s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\x{i}") });
        }
        let q = s.invalidations.lock().unwrap();
        assert!(q.lost && q.paths.is_empty(), "lost {}, {} kept", q.lost, q.paths.len());
        assert_eq!(s.forgets.waiting.load(Ordering::Relaxed), 2, "no hash job ran to apply them");
    }
}
