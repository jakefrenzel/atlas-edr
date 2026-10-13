//! The health thread (sensor spec §3.2 [7], §9.3; plan 1b-4a, D1): every 5 s
//! it takes a [`Snapshot`] of every counter; every 60 s, or at once when a
//! loss counter rose, it sends a Sensor Health report to the buffer writer and
//! logs its figures. Plan 1b-4b adds the watchdog's checks to this thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use atlas_etw::session::{SessionInfo, query_by_name};
use atlas_schema::{Device, EventId};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use super::agent::{HealthSources, Stopped};
use super::identity;
use crate::counters::Counters;
use crate::health::{self, Diagnostics, SessionLoss, Snapshot};
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
    pub to: SyncSender<Record>,
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
    loop {
        match stop.recv_timeout(cfg.check_every) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return at,
        }
        let now = snapshot(src, &out.buffer_drops, pipeline_counters(src, cfg.pipeline_timeout));
        if Instant::now() >= next_report || health::loss_rose(&at.from, &now) {
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
        cpu_ms = n(r.resources.cpu_time.map(|ns| ns / 1_000_000)),
        working_set_mb = n(r.resources.working_set.map(|b| b >> 20)),
        "health report"
    );
}

/// The pipeline's counters, read on its thread between two passes; `None` if
/// it does not answer in time (it ended, or is stuck).
fn pipeline_counters(src: &HealthSources, timeout: Duration) -> Option<Counters> {
    let (tx, rx) = channel();
    src.control.send(Box::new(move |p| drop(tx.send(p.counters())))).ok()?;
    rx.recv_timeout(timeout).ok()
}

fn session_loss(info: Option<SessionInfo>) -> Option<SessionLoss> {
    info.map(|i| SessionLoss { events_lost: i.events_lost, buffers_lost: i.realtime_buffers_lost })
}

/// Every source now. `pipeline` is read by the caller: through the control
/// channel while running, from `Stopped` at the end.
pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<Counters>) -> Snapshot {
    let query = |name: &str| session_loss(query_by_name(name).ok().flatten());
    let (cpu_time, working_set) = resources();
    Snapshot {
        pipeline,
        intake: src.intake.totals(),
        services: src.services.totals(),
        callback_panics: src.panics.iter().map(|p| p.load(Ordering::Relaxed)).sum(),
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
/// panicked.
pub fn final_snapshot(stopped: &Stopped, buffer_drops: &AtomicU64, seeding: bool) -> Snapshot {
    let (cpu_time, working_set) = resources();
    Snapshot {
        pipeline: stopped.pipeline.as_ref().ok().cloned(),
        intake: stopped.intake.totals(),
        services: stopped.services.totals(),
        callback_panics: stopped.callback_panics,
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use atlas_schema::classes::sensor_health::SensorHealthAction;
    use atlas_schema::{BootId, DeviceUid, EventKind};

    use super::*;
    use crate::counters::{IntakeCounters, ServiceCounters};

    /// Sources with no pipeline (its channel is closed) and no sessions.
    fn sources(intake: &Arc<IntakeCounters>) -> HealthSources {
        let (control, _) = channel();
        let pid = std::process::id();
        HealthSources {
            control,
            intake: intake.clone(),
            services: Arc::new(ServiceCounters::default()),
            panics: Vec::new(),
            sessions: [format!("Atlas-Test-None-A-{pid}"), format!("Atlas-Test-None-B-{pid}")],
            seeding: false,
        }
    }

    /// The next report's time and `loss.kernel_queue_drops`.
    fn next_report(rx: &Receiver<Record>, within: Duration) -> Option<(i64, Option<u64>)> {
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
        let intake = Arc::new(IntakeCounters::default());
        let (to, rx) = sync_channel(64);
        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
        let out = Reporting { to, device, buffer_drops: Arc::new(AtomicU64::new(0)) };
        let period = Duration::from_millis(1_000);
        let cfg = HealthConfig {
            check_every: Duration::from_millis(20),
            report_every: period,
            pipeline_timeout: Duration::from_millis(1),
        };
        let first = Interval { from: Snapshot::at_start(false), start: identity::anchor().unix_ns };
        let health = HealthThread::start(sources(&intake), out, first, cfg);

        std::thread::sleep(Duration::from_millis(400));
        assert!(rx.try_recv().is_err(), "no report before the period without a loss");
        intake.kernel_queue_drops.fetch_add(1, Ordering::Relaxed);
        let (t1, drops) = next_report(&rx, Duration::from_millis(500)).expect("the loss reported at once");
        assert_eq!(drops, Some(1));
        // Times are taken as each report is sent: a slow test thread does not
        // shorten the gap.
        let (t2, drops) = next_report(&rx, Duration::from_secs(5)).expect("the periodic report");
        let gap = Duration::from_nanos((t2 - t1) as u64);
        assert!(gap + Duration::from_millis(50) >= period, "{gap:?}: the period did not restart at the loss");
        assert_eq!(drops, Some(0), "counted since the report before");
        let at = health.stop();
        assert_eq!(at.start, t2, "the final report starts where the last one ended");
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
