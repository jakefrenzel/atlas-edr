//! The pipeline thread's loop (sensor spec §3.2; plan 1b-3c, decision D1).
//!
//! Every `cadence` (10 ms) one pass:
//! 1. drains the kernel and user-mode queues into [`Pipeline::push`], at most
//!    a queue's capacity each, so a flood cannot starve `tick` (review R-m8);
//! 2. feeds the services' replies into [`Pipeline::reply`], and runs any
//!    [`Control`] another thread sent (review R-M5);
//! 3. calls [`Pipeline::tick`] with the current QPC;
//! 4. hands the pipeline's requests to the services;
//! 5. passes the emitted events, in order, to the sink.
//!
//! The anchor is re-taken every `anchor_every` (60 s, §3.3). The loop ends when
//! both queues are closed (every sender gone): it applies the replies already
//! in, then [`Pipeline::stop`] sends what is pending as at its deadlines
//! (§11.4). The driver owns the services, so they are dropped after the
//! callbacks' senders (and their `FastRead`), the order 1b-3b's R-m11 needs.
//!
//! Portable: Windows supplies the clock, the anchor and the services
//! (`win::agent`); the tests use fakes.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use atlas_schema::Event;

use crate::counters::Counters;
use crate::input::Incoming;
use crate::pipeline::Pipeline;
use crate::services::{Lookups, Reply, Request};
use crate::time::Anchor;

/// What the loop needs from the services: hand a request to its lane, and
/// collect the replies so far. Neither may block (§3.2).
pub trait Lanes {
    fn submit(&self, r: Request);
    fn replies(&self) -> Vec<Reply>;
}

/// The loop's settings (sensor spec §3.2, §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverConfig {
    /// One pass every this long; `tick` must come at least every 50 ms.
    pub cadence: Duration,
    /// The anchor pair is re-taken this often (§3.3).
    pub anchor_every: Duration,
    /// Queue capacities (§3.2): kernel providers and Session B, and the
    /// user-mode providers.
    pub kernel_queue_cap: usize,
    pub user_queue_cap: usize,
}

impl Default for DriverConfig {
    fn default() -> Self {
        DriverConfig {
            cadence: Duration::from_millis(10),
            anchor_every: Duration::from_secs(60),
            kernel_queue_cap: 65_536,
            user_queue_cap: 8_192,
        }
    }
}

/// The current QPC.
pub type Clock = Box<dyn FnMut() -> i64 + Send>;
/// A fresh anchor pair (QPC, Unix time).
pub type AnchorSource = Box<dyn FnMut() -> Anchor + Send>;
/// Work another thread wants done on the pipeline thread, between two passes:
/// plan 1b-4's canary self key (`Pipeline::add_self_key`), its Sensor Health
/// reads of `Pipeline::counters`.
pub type Control<L> = Box<dyn FnOnce(&mut Pipeline<L>) + Send>;

pub struct Driver<L, S> {
    pipeline: Pipeline<L>,
    lanes: S,
    kernel: Receiver<Incoming>,
    user: Receiver<Incoming>,
    control: Receiver<Control<L>>,
    clock: Clock,
    anchor: AnchorSource,
    cfg: DriverConfig,
}

impl<L: Lookups, S: Lanes> Driver<L, S> {
    pub fn new(
        pipeline: Pipeline<L>,
        lanes: S,
        kernel: Receiver<Incoming>,
        user: Receiver<Incoming>,
        clock: Clock,
        anchor: AnchorSource,
        cfg: DriverConfig,
    ) -> Self {
        // No control until `with_control`: a receiver whose sender is gone.
        let control = channel().1;
        Driver { pipeline, lanes, kernel, user, control, clock, anchor, cfg }
    }

    /// Runs what arrives on `control` on the pipeline thread, once per pass.
    pub fn with_control(mut self, control: Receiver<Control<L>>) -> Self {
        self.control = control;
        self
    }

    /// Runs until both queues are closed, then stops the pipeline. Returns its
    /// counters.
    pub fn run(mut self, mut sink: impl FnMut(Event)) -> Counters {
        let mut next_anchor = Instant::now() + self.cfg.anchor_every;
        loop {
            let started = Instant::now();
            let open = self.pass(&mut sink);
            if !open {
                break;
            }
            if started >= next_anchor {
                self.pipeline.set_anchor((self.anchor)());
                next_anchor = started + self.cfg.anchor_every;
            }
            std::thread::sleep(self.cfg.cadence.saturating_sub(started.elapsed()));
        }
        for r in self.lanes.replies() {
            self.pipeline.reply(r);
        }
        for e in self.pipeline.stop() {
            sink(e);
        }
        self.pipeline.counters()
    }

    /// One pass. Returns whether a queue is still open.
    fn pass(&mut self, sink: &mut impl FnMut(Event)) -> bool {
        let kernel = drain(&self.kernel, &mut self.pipeline, self.cfg.kernel_queue_cap);
        let user = drain(&self.user, &mut self.pipeline, self.cfg.user_queue_cap);
        for r in self.lanes.replies() {
            self.pipeline.reply(r);
        }
        while let Ok(c) = self.control.try_recv() {
            c(&mut self.pipeline);
        }
        let out = self.pipeline.tick((self.clock)());
        for r in self.pipeline.take_requests() {
            self.lanes.submit(r);
        }
        for e in out {
            sink(e);
        }
        kernel || user
    }
}

/// Pushes what is queued, at most `max` events. Returns whether the queue is
/// still open.
fn drain<L: Lookups>(rx: &Receiver<Incoming>, p: &mut Pipeline<L>, max: usize) -> bool {
    for _ in 0..max.max(1) {
        match rx.try_recv() {
            Ok(inc) => p.push(inc),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{SyncSender, channel, sync_channel};
    use std::sync::{Arc, Mutex};

    use atlas_etw::parse::{FileCreate, RawEvent};
    use atlas_schema::{BootId, DeviceUid};

    use super::*;
    use crate::config::{Config, Ticks};
    use crate::fakes::{FakeLanes, FakeLookups, sequential_ids};
    use crate::input::{Header, Session};
    use crate::pipeline::Setup;
    use crate::process::Identity;

    const FREQ: i64 = 10_000_000;

    /// A driver on its own thread with a real clock; the sink sends events back.
    struct Running {
        kernel: Option<SyncSender<Incoming>>,
        user: Option<SyncSender<Incoming>>,
        events: std::sync::mpsc::Receiver<Event>,
        thread: std::thread::JoinHandle<Counters>,
        t0: Instant,
    }

    fn qpc(t0: Instant) -> i64 {
        (t0.elapsed().as_nanos() / 100) as i64
    }

    fn start(cfg: DriverConfig, anchor: AnchorSource) -> Running {
        let t0 = Instant::now();
        let setup = Setup {
            config: Config::default(),
            ticks: Ticks::new(FREQ),
            anchor: Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 },
            identity: Identity {
                device: DeviceUid::from_bytes([0xd0; 16]),
                boot: BootId::from_bytes([0xb0; 16]),
                kernel_boot_id: 7,
            },
            current_control_set: 1,
            self_keys: vec![],
            started: 0,
        };
        let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
        let (ktx, krx) = sync_channel(cfg.kernel_queue_cap);
        let (utx, urx) = sync_channel(cfg.user_queue_cap);
        let (etx, events) = channel();
        let driver = Driver::new(p, FakeLanes::new(|_| None), krx, urx, Box::new(move || qpc(t0)), anchor, cfg);
        let thread = std::thread::spawn(move || driver.run(|e| etx.send(e).unwrap()));
        Running { kernel: Some(ktx), user: Some(utx), events, thread, t0 }
    }

    /// A file created by the Idle process (a built-in actor), at QPC `ts`.
    fn created(ts: i64, name: &str) -> Incoming {
        Incoming {
            header: Header { session: Session::Sensor, pid: 0, tid: 1, ts, start_key: None },
            event: RawEvent::FileCreateNew(FileCreate {
                irp: 1,
                file_object: 0xA,
                issuing_tid: 1,
                create_options: 0,
                create_attributes: 0,
                share_access: 7,
                file_name: name.into(),
            }),
        }
    }

    fn no_anchor() -> AnchorSource {
        Box::new(|| Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 })
    }

    #[test]
    fn events_come_out_after_the_hold_while_idle_and_the_loop_ends_when_the_queues_close() {
        let r = start(DriverConfig::default(), no_anchor());
        let send = Instant::now();
        r.kernel.as_ref().unwrap().send(created(qpc(r.t0), r"\Device\HarddiskVolume3\a.txt")).unwrap();
        // No other event comes: idle ticks alone release it, after the 750 ms hold.
        let e = r.events.recv_timeout(Duration::from_secs(5)).expect("released while idle");
        let waited = send.elapsed();
        // At least the hold; the upper bound is loose for a loaded CI runner (review R-m10).
        assert!(waited >= Duration::from_millis(700) && waited < Duration::from_secs(5), "{waited:?}");
        assert!(matches!(&e.kind, atlas_schema::EventKind::File(_)));
        drop(r.kernel);
        drop(r.user);
        r.thread.join().unwrap();
    }

    #[test]
    fn closing_the_queues_drains_what_is_held() {
        let r = start(DriverConfig::default(), no_anchor());
        let k = r.kernel.unwrap();
        // The user-mode queue first: the loop runs on while one queue is open.
        drop(r.user);
        for i in 0..3 {
            k.send(created(qpc(r.t0), &format!(r"\Device\HarddiskVolume3\{i}.txt"))).unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));
        drop(k); // well inside the hold: the stop sends them
        r.thread.join().unwrap();
        assert_eq!(r.events.try_iter().count(), 3);
    }

    #[test]
    fn the_anchor_is_retaken_on_its_period() {
        let taken = Arc::new(AtomicUsize::new(0));
        let n = taken.clone();
        let cfg = DriverConfig { anchor_every: Duration::from_millis(20), ..DriverConfig::default() };
        let r = start(
            cfg,
            Box::new(move || {
                n.fetch_add(1, Ordering::Relaxed);
                Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 }
            }),
        );
        std::thread::sleep(Duration::from_millis(250));
        drop(r.kernel);
        drop(r.user);
        r.thread.join().unwrap();
        let n = taken.load(Ordering::Relaxed);
        assert!((1..=13).contains(&n), "{n} refreshes in 250 ms");
    }

    #[test]
    fn the_loop_hands_requests_to_the_lanes() {
        // The replay test (`tests/replay.rs`) checks the whole loop against the
        // synchronous replay; here, only that the lanes are used from the loop.
        let submitted = Arc::new(Mutex::new(0usize));
        let s = submitted.clone();
        struct Counting(Arc<Mutex<usize>>);
        impl Lanes for Counting {
            fn submit(&self, _: Request) {
                *self.0.lock().unwrap() += 1;
            }
            fn replies(&self) -> Vec<Reply> {
                Vec::new()
            }
        }
        let setup = Setup {
            config: Config::default(),
            ticks: Ticks::new(FREQ),
            anchor: Anchor { qpc: 0, unix_ns: 0 },
            identity: Identity {
                device: DeviceUid::from_bytes([1; 16]),
                boot: BootId::from_bytes([2; 16]),
                kernel_boot_id: 7,
            },
            current_control_set: 1,
            self_keys: vec![],
            started: 0,
        };
        let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
        let (ktx, krx) = sync_channel(4);
        let (utx, urx) = sync_channel(4);
        drop((ktx, utx));
        let cfg = DriverConfig { cadence: Duration::ZERO, ..DriverConfig::default() };
        Driver::new(p, Counting(s), krx, urx, Box::new(|| 0), no_anchor(), cfg).run(|_| {});
        // The start-up seeding pass (`seed_on_start`) is asked for on the first pass.
        assert_eq!(*submitted.lock().unwrap(), 2, "a Seed per handle kind");
    }

    #[test]
    fn control_runs_on_the_pipeline_thread_between_passes() {
        let setup = Setup {
            config: Config::default(),
            ticks: Ticks::new(FREQ),
            anchor: Anchor { qpc: 0, unix_ns: 0 },
            identity: Identity {
                device: DeviceUid::from_bytes([1; 16]),
                boot: BootId::from_bytes([2; 16]),
                kernel_boot_id: 7,
            },
            current_control_set: 1,
            self_keys: vec![],
            started: 0,
        };
        let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
        let (ktx, krx) = sync_channel(4);
        let (_utx, urx) = sync_channel(4);
        let (ctx, crx) = channel::<Control<FakeLookups>>();
        let t0 = Instant::now();
        let driver = Driver::new(
            p,
            FakeLanes::new(|_| None),
            krx,
            urx,
            Box::new(move || qpc(t0)),
            no_anchor(),
            DriverConfig::default(),
        )
        .with_control(crx);
        let thread = std::thread::spawn(move || driver.run(|_| {}));
        // A Sensor Health read while the loop runs (plan 1b-4).
        let (tx, rx) = channel();
        ctx.send(Box::new(move |p: &mut Pipeline<FakeLookups>| tx.send(p.counters()).unwrap())).unwrap();
        let counters = rx.recv_timeout(Duration::from_secs(5)).expect("the control ran");
        assert_eq!(counters.late_arrivals, 0);
        drop((ktx, _utx));
        thread.join().unwrap();
    }

    #[test]
    fn a_capped_drain_loses_nothing() {
        // One event per queue per pass (review R-m8): the rest waits for the
        // next pass, and closing the queue still drains it all.
        let cfg = DriverConfig { kernel_queue_cap: 1, user_queue_cap: 1, ..DriverConfig::default() };
        let r = start(cfg, no_anchor());
        let k = r.kernel.unwrap();
        drop(r.user);
        let now = qpc(r.t0);
        // The queue holds one: each send waits for a pass to make room.
        for i in 0..5 {
            k.send(created(now, &format!(r"\Device\HarddiskVolume3\{i}.txt"))).unwrap();
        }
        drop(k);
        r.thread.join().unwrap();
        assert_eq!(r.events.try_iter().count(), 5);
    }
}
