//! The services' threads and the routing of [`Request`]s to them (sensor spec
//! §3.2 [4] and [8]; plan 1b-3b, D5).
//!
//! - **Hash workers** (2, below normal): `Enrich`, and the hash cache's
//!   invalidations, so the pipeline thread never takes the cache's lock.
//! - **Reader lane** (1): value reads, on the ordered path and the fast path.
//!   It does no file I/O, so a slow directory never delays a value read (R-M2).
//! - **Expander lane** (1): 8.3 expansion, and the expansion cache's
//!   invalidations in arrival order (D4).
//! - **Seeder** (1, below normal): `Seed`, when `SeDebugPrivilege` is available.
//!
//! Lanes are bounded and never block the caller. A full lane drops the request
//! and counts it: a dropped `Enrich`, `ReadValue`, `Expand` or `Seed` costs its
//! event's deadline (clarification 25). A dropped invalidation of the
//! expansion cache sets a flag, and the expander clears its whole cache before
//! its next job (R-M3). The hash cache needs no such care: its key holds the USN.
//! Replies come back on one channel, which the driver (plan 1b-3c) feeds to
//! `Pipeline::reply`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex};
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

enum HashJob {
    Enrich(PendingId, String),
    Invalidate(String),
}

enum ReaderJob {
    Read(PendingId, ValueRead),
    Early(EarlyKey, ValueRead),
}

enum ExpandJob {
    Expand(PendingId, u8, String),
    Invalidate(String),
}

pub struct Services {
    hash: SyncSender<HashJob>,
    reader: SyncSender<ReaderJob>,
    expander: SyncSender<ExpandJob>,
    /// Set when an expansion-cache invalidation was dropped (R-M3).
    invalidation_lost: Arc<AtomicBool>,
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
        for i in 0..cfg.hash_workers.max(1) {
            let (rx, tx, cache, counters, cap) =
                (hash_rx.clone(), reply_tx.clone(), cache.clone(), counters.clone(), cfg.hash_size_cap);
            std::thread::Builder::new()
                .name(format!("atlas-hash-{i}"))
                .spawn(move || hash_worker(&rx, &tx, &cache, &counters, cap))
                .expect("spawn a hash worker");
        }

        let backup = privilege::enable(privilege::BACKUP);
        let (reader, reader_rx) = sync_channel::<ReaderJob>(cfg.lane_cap);
        let tx = reply_tx.clone();
        std::thread::Builder::new()
            .name("atlas-reader".into())
            .spawn(move || reader_lane(reader_rx, &tx, backup))
            .expect("spawn the reader lane");

        let invalidation_lost = Arc::new(AtomicBool::new(false));
        let (expander, expand_rx) = sync_channel::<ExpandJob>(cfg.lane_cap);
        let (tx, lost) = (reply_tx.clone(), invalidation_lost.clone());
        std::thread::Builder::new()
            .name("atlas-expander".into())
            .spawn(move || expander_lane(expand_rx, &tx, Expander::new(dir), &lost))
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

        Services { hash, reader, expander, invalidation_lost, seeder, replies, counters }
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

    /// Hands a request to its lane. Never blocks, and takes no lock.
    pub fn submit(&self, r: Request) {
        match r {
            Request::Enrich { id, nt_path, .. } => self.count(self.hash.try_send(HashJob::Enrich(id, nt_path))),
            Request::InvalidateHash { nt_path } => {
                self.count(self.hash.try_send(HashJob::Invalidate(nt_path.clone())));
                if self.expander.try_send(ExpandJob::Invalidate(nt_path)).is_err() {
                    self.invalidation_lost.store(true, Ordering::Release);
                    self.counters.service_queue_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
            Request::ReadValue { id, read } => self.count(self.reader.try_send(ReaderJob::Read(id, read))),
            Request::Expand { id, slot, nt_path } => {
                self.count(self.expander.try_send(ExpandJob::Expand(id, slot, nt_path)))
            }
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
    counters: &ServiceCounters,
    size_cap: u64,
) {
    // SAFETY: lowers this thread's priority.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    let enricher = Enricher::new(size_cap);
    loop {
        let job = match jobs.lock().expect("hash lane").recv() {
            Ok(j) => j,
            Err(_) => return,
        };
        let (id, nt_path) = match job {
            HashJob::Enrich(id, p) => (id, p),
            HashJob::Invalidate(p) => {
                cache.lock().expect("hash cache").invalidate(&p);
                continue;
            }
        };
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
    lost: &AtomicBool,
) {
    for job in jobs {
        if lost.swap(false, Ordering::Acquire) {
            expander.clear();
        }
        match job {
            ExpandJob::Expand(id, slot, nt) => {
                let reply = Reply::Expanded { id, slot, long_path: expander.expand(&nt, Instant::now()) };
                if replies.send(reply).is_err() {
                    return;
                }
            }
            ExpandJob::Invalidate(nt) => expander.invalidate(&nt),
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

    /// R-M3: when the expander lane is full, a dropped invalidation clears the
    /// expansion cache before the next job, so a reused short name is not
    /// answered from the cache.
    #[test]
    fn a_lost_invalidation_clears_the_expansion_cache() {
        let dir = GateDir::default();
        dir.set("SECRET~1", "SecretStuffAAA");
        let cfg = ServiceConfig { lane_cap: 1, ..ServiceConfig::default() };
        let s = Services::start_with(&cfg, dir.clone());
        s.submit(Request::Expand { id: 1, slot: 0, nt_path: format!(r"{V}\SECRET~1") });
        assert_eq!(expanded(&wait(&s, 1), 1), Some(Some(format!(r"{V}\SecretStuffAAA"))));

        dir.close();
        s.submit(Request::Expand { id: 2, slot: 0, nt_path: format!(r"{V}\OTHER~1") }); // the lane blocks on it
        std::thread::sleep(Duration::from_millis(100));
        s.submit(Request::Expand { id: 3, slot: 0, nt_path: format!(r"{V}\SECRET~1") }); // fills the queue
        s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\SecretStuffAAA") }); // dropped
        assert!(s.counters().service_queue_drops.load(Ordering::Relaxed) >= 1);
        dir.set("SECRET~1", "SecretStuffBBB"); // the short name now belongs to another directory
        dir.open();
        let got = wait(&s, 2);
        assert_eq!(expanded(&got, 3), Some(Some(format!(r"{V}\SecretStuffBBB"))), "{got:?}");
    }
}
