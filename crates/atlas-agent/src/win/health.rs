//! The health thread (sensor spec §3.2 [7], §9.3; plan 1b-4a, D1): every 5 s
//! it takes a [`Snapshot`] of every counter; every 60 s, or at once when a
//! loss counter rose, it sends a Sensor Health report to the buffer writer and
//! logs its figures. Plan 1b-4b adds the watchdog's checks to this thread.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use atlas_etw::session::{SessionInfo, query_by_name};
use atlas_schema::{Device, EventId};
use windows::Win32::Foundation::{FILETIME, HLOCAL, LocalFree};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessIdOfThread, GetProcessTimes, GetThreadDescription,
    GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};

use super::agent::{HealthSources, Stopped};
use super::identity;
use super::util::Owned;
use crate::counters::Counters;
use crate::health::{self, Diagnostics, SessionLoss, Sizes, Snapshot, ThreadList};
use crate::store::Record;

#[derive(Debug, Clone, Copy)]
pub struct HealthConfig {
    pub check_every: Duration,
    pub report_every: Duration,
    /// How long the pipeline may take to answer a counters request.
    pub pipeline_timeout: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        HealthConfig {
            check_every: Duration::from_secs(5),
            report_every: Duration::from_secs(60),
            pipeline_timeout: Duration::from_secs(2),
        }
    }
}

/// Where the next report starts: its base snapshot and its start time (Unix ns).
#[derive(Debug, Clone)]
pub struct Interval {
    pub from: Snapshot,
    pub start: i64,
}

/// The running health thread.
pub struct HealthThread {
    stop: Sender<()>,
    thread: JoinHandle<Interval>,
}

/// Everything a report needs besides the sources.
#[derive(Clone)]
pub struct Reporting {
    pub to: crate::queue::QueueSender<Record>,
    pub device: Device,
    /// `loss.buffer_backlog_drops` (`Store::drops`).
    pub buffer_drops: Arc<AtomicU64>,
}

impl HealthThread {
    pub fn start(src: HealthSources, out: Reporting, first: Interval, cfg: HealthConfig) -> HealthThread {
        let (stop, stop_rx) = channel();
        let thread = std::thread::Builder::new()
            .name("atlas-health".into())
            .spawn(move || run(&src, &out, first, cfg, &stop_rx))
            .expect("spawn the health thread");
        HealthThread { stop, thread }
    }

    /// Stops the thread; returns where the final report starts.
    pub fn stop(self) -> Interval {
        drop(self.stop);
        self.thread.join().expect("the health thread")
    }
}

fn run(src: &HealthSources, out: &Reporting, first: Interval, cfg: HealthConfig, stop: &Receiver<()>) -> Interval {
    let mut at = first;
    let mut next_report = Instant::now() + cfg.report_every;
    let mut next_listing = next_report;
    loop {
        match stop.recv_timeout(cfg.check_every) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return at,
        }
        let mut now = snapshot(src, &out.buffer_drops, pipeline_counters(src, cfg.pipeline_timeout));
        if Instant::now() >= next_report || health::loss_rose(&at.from, &now) {
            // Listing the threads costs ~45 ms (Q7): at most once a period,
            // for a report of either kind (review m3). A report without them
            // leaves them to the next listing, which covers the time since.
            if Instant::now() >= next_listing {
                now.threads = (src.threads)();
                next_listing = Instant::now() + cfg.report_every;
            }
            at = send(out, at, &now);
            next_report = Instant::now() + cfg.report_every;
        }
    }
}

/// Sends the report for `at` up to `now`; returns where the next one starts.
pub fn send(out: &Reporting, at: Interval, now: &Snapshot) -> Interval {
    let time = identity::anchor().unix_ns;
    let (report, diag) = health::report(&at.from, now);
    log(&report, &diag, time - at.start);
    let e = health::event(EventId::new_v7(), time, out.device, at.start, report);
    if out.to.send(Record::Health(e)).is_err() {
        tracing::warn!("health: the buffer writer is gone; report not written");
    }
    Interval { from: at.from.advance(now), start: time }
}

/// The figures of each report, on one line, for the host run's summary.
fn log(r: &atlas_schema::classes::sensor_health::HealthReport, d: &Diagnostics, interval_ns: i64) {
    let n = |v: Option<u64>| v.map_or(-1, |v| v as i64);
    let classes = |v: &[atlas_schema::classes::sensor_health::ClassCount]| v.iter().map(|c| c.count).sum::<u64>();
    tracing::info!(
        interval_ms = interval_ns / 1_000_000,
        sensor_events_lost = n(r.loss.sensor_session_events_lost),
        process_events_lost = n(r.loss.process_session_events_lost),
        sensor_buffers_lost = n(r.loss.sensor_session_buffers_lost),
        process_buffers_lost = n(r.loss.process_session_buffers_lost),
        kernel_queue_drops = n(r.loss.kernel_queue_drops),
        user_queue_drops = n(r.loss.user_queue_drops),
        buffer_drops = n(r.loss.buffer_backlog_drops),
        callback_panics = n(r.loss.callback_panics),
        actor_dropped = classes(&r.loss.actor_dropped),
        dns_rate_limit_drops = n(r.loss.dns_rate_limit_drops),
        cleanup_unpaired = d.cleanup_unpaired,
        cleanup_outcome_late = d.cleanup_outcome_late,
        file_delete_outcome_unknown = n(r.quality.file_delete_outcome_unknown),
        late_arrivals = n(r.quality.late_arrivals),
        service_queue_drops = n(r.housekeeping.service_queue_drops),
        pending_overflow = n(r.housekeeping.pending_overflow),
        self_filtered = n(d.self_filtered),
        sensor_events_received = n(r.throughput.sensor_session_events_received),
        process_events_received = n(r.throughput.process_session_events_received),
        cpu_ms = n(r.resources.cpu_time.map(|ns| ns / 1_000_000)),
        working_set_mb = n(r.resources.working_set.map(|b| b >> 20)),
        threads_ms = d.threads_interval.map_or(-1, |ns| ns / 1_000_000),
        threads = d.threads.as_deref().map_or_else(|| "-".to_owned(), thread_line),
        sizes = sizes_line(&d.sizes),
        "health report"
    );
}

/// `name=ms` for each thread name, most first, as the log line carries them.
fn thread_line(threads: &[(String, u64)]) -> String {
    let parts: Vec<String> = threads.iter().map(|(name, ns)| format!("{name}={}", ns / 1_000_000)).collect();
    parts.join(" ")
}

/// `name=entries/capacity` for each structure, as the log line carries them (R6).
fn sizes_line(sizes: &[(&str, u64, u64)]) -> String {
    let parts: Vec<String> = sizes.iter().map(|(name, len, cap)| format!("{name}={len}/{cap}")).collect();
    parts.join(" ")
}

/// The pipeline's counters and structure sizes, read on its thread between
/// two passes; `None` if it does not answer in time (it ended, or is stuck).
fn pipeline_counters(src: &HealthSources, timeout: Duration) -> Option<(Counters, Sizes)> {
    let (tx, rx) = channel();
    src.control.send(Box::new(move |p| drop(tx.send((p.counters(), p.sizes()))))).ok()?;
    rx.recv_timeout(timeout).ok()
}

fn session_loss(info: Option<SessionInfo>) -> Option<SessionLoss> {
    info.map(|i| SessionLoss { events_lost: i.events_lost, buffers_lost: i.realtime_buffers_lost })
}

/// Every source now but the threads (the caller lists them when due).
/// `pipeline` is read by the caller: through the control channel while
/// running, from `Stopped` at the end.
pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<(Counters, Sizes)>) -> Snapshot {
    let query = |name: &str| session_loss(query_by_name(name).ok().flatten());
    let (cpu_time, working_set) = resources();
    let (pipeline, pipeline_sizes) = pipeline.map_or((None, Sizes::new()), |(c, s)| (Some(c), s));
    Snapshot {
        threads: None,
        pipeline,
        pipeline_sizes,
        intake: src.intake.totals(),
        services: src.services.totals(),
        callback_panics: src.counters.iter().map(|c| c.panics.load(Ordering::Relaxed)).sum(),
        received: [0, 1].map(|i| src.counters[i].received.load(Ordering::Relaxed)),
        sensor_session: query(&src.sessions[0]),
        process_session: query(&src.sessions[1]),
        buffer_drops: buffer_drops.load(Ordering::Relaxed),
        cpu_time,
        working_set,
        seeding: src.seeding,
    }
}

/// The snapshot for the final report, after the stop: the sessions' last
/// counters come from their stop; the pipeline's are absent if its thread
/// panicked. `threads` was listed just before the stop, while the sensor's
/// threads still ran (review m2).
pub fn final_snapshot(
    stopped: &Stopped,
    buffer_drops: &AtomicU64,
    seeding: bool,
    threads: Option<ThreadList>,
) -> Snapshot {
    let (cpu_time, working_set) = resources();
    Snapshot {
        threads,
        pipeline: stopped.pipeline.as_ref().ok().cloned(),
        // The pipeline has stopped: its structures were emptied.
        pipeline_sizes: Sizes::new(),
        intake: stopped.intake.totals(),
        services: stopped.services.totals(),
        callback_panics: stopped.callback_panics,
        received: stopped.received,
        sensor_session: session_loss(stopped.sensor_session),
        process_session: session_loss(stopped.process_session),
        buffer_drops: buffer_drops.load(Ordering::Relaxed),
        cpu_time,
        working_set,
        seeding,
    }
}

/// The process's CPU time (user + kernel, ns) and working set (bytes).
fn resources() -> (Option<u64>, Option<u64>) {
    let ft = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) * 100;
    let (mut c, mut e, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the current-process pseudo-handle and four out-structures.
    let cpu =
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) }.ok().map(|()| ft(k) + ft(u));
    let mut m = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    // SAFETY: the current-process pseudo-handle and a structure of the size given.
    let ws = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) }.ok().map(|()| m.WorkingSetSize as u64);
    (cpu, ws)
}

/// This process's threads: each one's name (`(unnamed)` when empty) and CPU
/// time (user + kernel, ns), by thread id and creation time. `None` if the
/// threads could not be listed; a thread that cannot be opened, or whose id
/// now belongs to another process, is left out. It takes a snapshot of every
/// thread in the system: ~45 ms on the host.
pub(crate) fn threads() -> Option<ThreadList> {
    let at = identity::anchor().unix_ns;
    // SAFETY: a snapshot of every thread in the system; closed by `Owned`.
    let snap = Owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.ok()?);
    // SAFETY: reads the calling process's id.
    let me = unsafe { GetCurrentProcessId() };
    let mut threads = BTreeMap::new();
    let mut te = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
    // SAFETY: `te` is a THREADENTRY32 with its size set.
    let mut more = unsafe { Thread32First(snap.raw(), &mut te) }.is_ok();
    while more {
        if te.th32OwnerProcessID == me
            && let Some((key, entry)) = thread(te.th32ThreadID, me)
        {
            threads.insert(key, entry);
        }
        // SAFETY: as for Thread32First.
        more = unsafe { Thread32Next(snap.raw(), &mut te) }.is_ok();
    }
    Some(ThreadList { at, threads })
}

/// One of `me`'s threads: (id, creation time) and (name, CPU time); `None` if
/// it cannot be opened or its id now belongs to another process's thread.
fn thread(tid: u32, me: u32) -> Option<((u32, u64), (String, u64))> {
    let ft = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) * 100;
    // SAFETY: opens a thread for querying; closed by `Owned`.
    let h = Owned(unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) }.ok()?);
    // SAFETY: an open thread handle. The id may have been reused since the snapshot.
    if unsafe { GetProcessIdOfThread(h.raw()) } != me {
        return None;
    }
    // SAFETY: the returned string is ours to free with LocalFree.
    let name = unsafe { GetThreadDescription(h.raw()) }.ok().map(|p| {
        // SAFETY: a NUL-terminated string from GetThreadDescription.
        let s = unsafe { p.to_string() }.unwrap_or_default();
        // SAFETY: frees the string GetThreadDescription allocated.
        unsafe { LocalFree(Some(HLOCAL(p.0.cast()))) };
        s
    });
    let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| "(unnamed)".to_owned());
    let (mut c, mut e, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: an open thread handle and four out-structures.
    unsafe { GetThreadTimes(h.raw(), &mut c, &mut e, &mut k, &mut u) }.ok()?;
    Some(((tid, ft(c)), (name, ft(k) + ft(u))))
}

#[cfg(test)]
mod tests {
    use crate::queue::QueueReceiver;

    use atlas_schema::classes::sensor_health::SensorHealthAction;
    use atlas_schema::{BootId, DeviceUid, EventKind};

    use super::*;
    use windows::Win32::System::Threading::GetCurrentThread;

    use crate::counters::{IntakeCounters, ServiceCounters};

    /// Sources with no pipeline (its channel is closed) and no sessions; the
    /// threads' lister counts its calls in `lists`.
    fn sources(intake: &Arc<IntakeCounters>, lists: &Arc<AtomicU64>) -> HealthSources {
        let (control, _) = channel();
        let pid = std::process::id();
        let lists = lists.clone();
        HealthSources {
            control,
            intake: intake.clone(),
            services: Arc::new(ServiceCounters::default()),
            counters: Default::default(),
            sessions: [format!("Atlas-Test-None-A-{pid}"), format!("Atlas-Test-None-B-{pid}")],
            seeding: false,
            threads: Arc::new(move || {
                lists.fetch_add(1, Ordering::Relaxed);
                Some(ThreadList::default())
            }),
        }
    }

    fn reporting() -> (Reporting, QueueReceiver<Record>) {
        let (to, rx) = crate::queue::bounded(4_096);
        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
        (Reporting { to, device, buffer_drops: Arc::new(AtomicU64::new(0)) }, rx)
    }

    /// The next report's time and `loss.kernel_queue_drops`.
    fn next_report(rx: &QueueReceiver<Record>, within: Duration) -> Option<(i64, Option<u64>)> {
        let Record::Health(e) = rx.recv_timeout(within).ok()? else { panic!("not a report") };
        let EventKind::SensorHealth(a) = &e.kind else { panic!("not Sensor Health") };
        let SensorHealthAction::Report(r) = &a.action;
        Some((e.meta.time, r.loss.kernel_queue_drops))
    }

    /// Review finding 8: a loss is reported at the next check, the period
    /// starts again from that report, and `stop` returns where the last one
    /// ended.
    #[test]
    fn a_loss_is_reported_at_once_and_restarts_the_period() {
        let (intake, lists) = (Arc::new(IntakeCounters::default()), Arc::new(AtomicU64::new(0)));
        let (out, rx) = reporting();
        let period = Duration::from_millis(1_000);
        let cfg = HealthConfig {
            check_every: Duration::from_millis(20),
            report_every: period,
            pipeline_timeout: Duration::from_millis(1),
        };
        let first =
            Interval { from: Snapshot::at_start(false, identity::anchor().unix_ns), start: identity::anchor().unix_ns };
        let health = HealthThread::start(sources(&intake, &lists), out, first, cfg);

        std::thread::sleep(Duration::from_millis(400));
        assert!(rx.try_recv().is_err(), "no report before the period without a loss");
        intake.kernel_queue_drops.fetch_add(1, Ordering::Relaxed);
        let (t1, drops) = next_report(&rx, Duration::from_millis(500)).expect("the loss reported at once");
        assert_eq!(drops, Some(1));
        // Threads are listed at most once a period, for a report (plan
        // 1b-4a-p1, Q7): none for the ~20 checks and the early loss report.
        assert_eq!(lists.load(Ordering::Relaxed), 0, "threads listed at a check or too soon");
        // Times are taken as each report is sent: a slow test thread does not
        // shorten the gap.
        let (t2, drops) = next_report(&rx, Duration::from_secs(5)).expect("the periodic report");
        let gap = Duration::from_nanos((t2 - t1) as u64);
        assert!(gap + Duration::from_millis(50) >= period, "{gap:?}: the period did not restart at the loss");
        assert_eq!(drops, Some(0), "counted since the report before");
        assert_eq!(lists.load(Ordering::Relaxed), 1, "listed once, for the periodic report");
        let at = health.stop();
        assert_eq!(at.start, t2, "the final report starts where the last one ended");
    }

    /// Review m3: under a loss at every check (a report each time), the
    /// threads are still listed about once a period, not at every report.
    #[test]
    fn threads_are_listed_under_sustained_loss() {
        let (intake, lists) = (Arc::new(IntakeCounters::default()), Arc::new(AtomicU64::new(0)));
        let (out, rx) = reporting();
        let cfg = HealthConfig {
            check_every: Duration::from_millis(20),
            report_every: Duration::from_millis(300),
            pipeline_timeout: Duration::from_millis(1),
        };
        let start = identity::anchor().unix_ns;
        let first = Interval { from: Snapshot::at_start(false, start), start };
        let health = HealthThread::start(sources(&intake, &lists), out, first, cfg);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1_500) {
            intake.kernel_queue_drops.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(5));
        }
        health.stop();
        let reports = rx.try_iter().count() as u64;
        let listed = lists.load(Ordering::Relaxed);
        assert!(listed >= 2, "{listed} listings in 1.5 s of loss ({reports} reports)");
        assert!(reports >= 2 * listed, "{listed} listings for {reports} reports: listed at every report");
    }

    /// Each session's delivered count goes to its own place (plan 1b-4a-p1).
    #[test]
    fn each_sessions_received_count_is_read_into_its_place() {
        let intake = Arc::new(IntakeCounters::default());
        let src = sources(&intake, &Arc::new(AtomicU64::new(0)));
        src.counters[0].received.store(5, Ordering::Relaxed);
        src.counters[1].received.store(7, Ordering::Relaxed);
        assert_eq!(snapshot(&src, &AtomicU64::new(0), None).received, [5, 7]);
    }

    /// The calling thread's CPU time (user + kernel), ns.
    fn own_cpu() -> u64 {
        let ft = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) * 100;
        let (mut c, mut e, mut k, mut u) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: the current-thread pseudo-handle and four out-structures.
        unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) }.expect("GetThreadTimes");
        ft(k) + ft(u)
    }

    /// Burns CPU until this thread used `ns` more: CPU time, not wall time,
    /// so a busy machine only makes it slower.
    fn burn(ns: u64) {
        let end = own_cpu() + ns;
        let mut x = 0u64;
        while own_cpu() < end {
            for _ in 0..10_000 {
                x = std::hint::black_box(x.wrapping_add(1));
            }
        }
    }

    /// Each thread is listed on its own (two sharing a name are two entries),
    /// only this process's threads are listed, and ended ones are gone (plan
    /// 1b-4a-p1, review m1).
    #[test]
    fn threads_are_listed_one_by_one() {
        let name = format!("atlas-test-burn-{}", std::process::id());
        let (done, burnt) = channel::<()>();
        let (mut go, mut handles) = (Vec::new(), Vec::new());
        for _ in 0..2 {
            let (g, wait) = channel::<()>();
            let done = done.clone();
            go.push(g);
            let t = std::thread::Builder::new().name(name.clone()).spawn(move || {
                burn(60_000_000);
                done.send(()).unwrap();
                wait.recv().unwrap();
                burn(60_000_000);
                done.send(()).unwrap();
                wait.recv().unwrap(); // stays until read
            });
            handles.push(t.unwrap());
        }
        let step = |go: &[Sender<()>]| {
            for _ in 0..2 {
                burnt.recv().unwrap();
            }
            let all = threads().expect("listed");
            let process = resources().0.expect("the process's CPU");
            assert!(all.threads.values().map(|(_, c)| c).sum::<u64>() <= process, "only this process's threads");
            for g in go {
                g.send(()).unwrap();
            }
            let ours: Vec<u64> = all.threads.values().filter(|(n, _)| *n == name).map(|(_, c)| *c).collect();
            assert_eq!(ours.len(), 2, "two threads, two entries");
            assert!(ours.iter().all(|&c| c >= 60_000_000), "{ours:?}");
            ours.iter().sum::<u64>()
        };
        let first = step(&go);
        assert!(first >= 120_000_000, "{first} ns: both threads");
        let second = step(&go);
        assert!(second >= first + 120_000_000, "{first} then {second} ns");
        for h in handles {
            h.join().unwrap();
        }
        assert!(!threads().expect("listed").threads.values().any(|(n, _)| *n == name), "ended threads are gone");
    }

    #[test]
    fn the_thread_line_is_name_equals_ms() {
        let line = thread_line(&[("atlas-hash-0".into(), 1_234_000_000), ("atlas-pipeline".into(), 999_999)]);
        assert_eq!(line, "atlas-hash-0=1234 atlas-pipeline=0");
    }

    #[test]
    fn the_sizes_line_is_name_equals_entries_over_capacity() {
        assert_eq!(sizes_line(&[("file_map", 3, 16), ("hash_cache", 0, 0)]), "file_map=3/16 hash_cache=0/0");
    }

    #[test]
    fn resources_are_read() {
        let (cpu, ws) = resources();
        assert!(cpu.is_some() && ws.is_some_and(|b| b > 0));
        // Burn a little CPU: the time moves.
        let t0 = Instant::now();
        let mut x = 0u64;
        while t0.elapsed() < Duration::from_millis(50) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        assert!(resources().0 > cpu);
    }
}
