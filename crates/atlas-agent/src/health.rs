//! Sensor Health reports (sensor spec §9.3, §10.3; plan 1b-4a, D1): the
//! counters of every thread, read into a [`Snapshot`], and a report made of
//! the differences between two snapshots.
//!
//! The health thread takes the snapshots: the pipeline's [`Counters`] through
//! the driver's control channel, the intake's and the services' atomics, the
//! sessions' losses, the process's CPU time and working set. It reports every
//! 60 s, sooner when a loss counter rose ([`loss_rose`]). The buffer writer
//! fills in the report's `buffer` group, `housekeeping.retention_evictions` and
//! the gap as it appends the report (`store`).

use std::collections::BTreeMap;

use atlas_schema::classes::sensor_health::{
    ClassCount, HealthReport, SensorHealthAction, SensorHealthActivity, SensorHousekeeping, SensorLoss, SensorQuality,
    SensorResources, SensorThroughput,
};
use atlas_schema::{Device, Event, EventId, EventKind, EventMeta, Sensor};

use crate::counters::{Class, Counters, IntakeTotals, ServiceTotals};

/// Bounded structures' entries and allocated capacity, by name, for the
/// diagnostic log (plan 1b-4a-p2, R6): gauges, not differences. Capacity is
/// what holds memory: a map keeps its largest allocation after entries leave.
pub type Sizes = Vec<(&'static str, u64, u64)>;

/// One ETW session's loss counters (`EVENT_TRACE_PROPERTIES`): 32-bit and
/// cumulative, so differences wrap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionLoss {
    pub events_lost: u32,
    pub buffers_lost: u32,
}

/// Every cumulative figure at one instant. `None` is "not read this time"
/// (the pipeline did not answer, a query failed); such fields are absent from
/// the report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub pipeline: Option<Counters>,
    pub intake: IntakeTotals,
    pub services: ServiceTotals,
    pub callback_panics: u64,
    /// ETW events each consumer was delivered, Session A's then Session B's
    /// (`throughput`, plan 1b-4a-p1).
    pub received: [u64; 2],
    pub sensor_session: Option<SessionLoss>,
    pub process_session: Option<SessionLoss>,
    /// Events the buffer could not take: the sink's channel was full, or the
    /// writer's backlog was. It decides an early report and the log line; the
    /// writer thread fills the report's `loss.buffer_backlog_drops` itself.
    pub buffer_drops: u64,
    /// Process CPU time (user + kernel), ns.
    pub cpu_time: Option<u64>,
    /// A gauge, bytes.
    pub working_set: Option<u64>,
    /// The last listing of the agent's threads, for the diagnostic log (plan
    /// 1b-4a-p1); `None` when not listed this time.
    pub threads: Option<ThreadList>,
    pub seeding: bool,
    /// The pipeline's structures (R6), read with its counters; empty when not
    /// read.
    pub pipeline_sizes: Sizes,
    /// The kernel, user and sink queues' fill and cap (review 7).
    pub queues: Sizes,
}

/// One listing of the agent's threads (plan 1b-4a-p1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadList {
    /// When it was taken, Unix ns.
    pub at: i64,
    /// Each thread's name and CPU time (user + kernel, ns), by thread id and
    /// creation time (a thread id is reused once its thread ends).
    pub threads: BTreeMap<(u32, u64), (String, u64)>,
}

impl Snapshot {
    /// The state at start (`start`, Unix ns): everything zero, the sessions
    /// just created, no thread listed yet (each thread then counts in full).
    pub fn at_start(seeding: bool, start: i64) -> Snapshot {
        Snapshot {
            pipeline: Some(Counters::default()),
            sensor_session: Some(SessionLoss::default()),
            process_session: Some(SessionLoss::default()),
            cpu_time: Some(0),
            threads: Some(ThreadList { at: start, threads: BTreeMap::new() }),
            seeding,
            ..Snapshot::default()
        }
    }

    /// What the next report is measured from: `now`, keeping `self`'s values
    /// for what `now` could not read, so nothing is counted twice or lost.
    pub fn advance(&self, now: &Snapshot) -> Snapshot {
        Snapshot {
            pipeline: now.pipeline.clone().or_else(|| self.pipeline.clone()),
            sensor_session: now.sensor_session.or(self.sensor_session),
            process_session: now.process_session.or(self.process_session),
            cpu_time: now.cpu_time.or(self.cpu_time),
            threads: now.threads.clone().or_else(|| self.threads.clone()),
            ..now.clone()
        }
    }
}

/// Figures for the diagnostic log only: no schema field (plan 1b-4a, D5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// The two parts of `quality.file_cleanup_unpaired` (plan 1b-3c, R-M4).
    pub cleanup_unpaired: u64,
    pub cleanup_outcome_late: u64,
    /// Events the self-filter dropped, successful OperationEnds discarded in
    /// the callback, fast-path value reads sent.
    pub self_filtered: Option<u64>,
    pub op_end_discarded: u64,
    pub fast_reads: u64,
    /// CPU time (ns) each thread name used since the last listing, threads
    /// sharing a name summed, most first, those that used none left out (plan
    /// 1b-4a-p1). A thread that ended since loses its time since the last
    /// listing; one that started counts in full.
    pub threads: Option<Vec<(String, u64)>>,
    /// The time between the two listings, ns: it differs from the report's
    /// interval when the threads were last listed before the last report.
    pub threads_interval: Option<i64>,
    /// Every bounded structure's entries and capacity at `now` (R6): the
    /// pipeline's, then the early key map, the hash and expansion caches.
    pub sizes: Sizes,
}

fn d(prev: u64, now: u64) -> u64 {
    now.saturating_sub(prev)
}

fn d32(prev: u32, now: u32) -> u64 {
    u64::from(now.wrapping_sub(prev))
}

/// Per-class differences, zero counts left out.
fn d_classes(prev: &BTreeMap<Class, u64>, now: &BTreeMap<Class, u64>) -> Vec<ClassCount> {
    now.iter()
        .map(|(class, &n)| ClassCount { class_uid: class.uid(), count: d(prev.get(class).copied().unwrap_or(0), n) })
        .filter(|c| c.count > 0)
        .collect()
}

/// Each thread's difference, then summed by name, most first, zero left out.
fn d_threads(prev: &ThreadList, now: &ThreadList) -> Vec<(String, u64)> {
    let mut by_name: BTreeMap<&str, u64> = BTreeMap::new();
    for (key, (name, cpu)) in &now.threads {
        let base = prev.threads.get(key).map_or(0, |(_, c)| *c);
        *by_name.entry(name).or_default() += d(base, *cpu);
    }
    let mut v: Vec<_> = by_name.into_iter().filter(|(_, n)| *n > 0).map(|(k, n)| (k.to_owned(), n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// The report for the interval from `prev` to `now`, without the buffer's
/// figures (the writer adds them), and the figures for the log.
pub fn report(prev: &Snapshot, now: &Snapshot) -> (HealthReport, Diagnostics) {
    let (i0, i1) = (&prev.intake, &now.intake);
    let (s0, s1) = (&prev.services, &now.services);
    let session = |p: Option<SessionLoss>, n: Option<SessionLoss>| match (p, n) {
        (Some(p), Some(n)) => (Some(d32(p.events_lost, n.events_lost)), Some(d32(p.buffers_lost, n.buffers_lost))),
        _ => (None, None),
    };
    let (sensor_events, sensor_buffers) = session(prev.sensor_session, now.sensor_session);
    let (process_events, process_buffers) = session(prev.process_session, now.process_session);
    let pipe = match (&prev.pipeline, &now.pipeline) {
        (Some(p), Some(n)) => Some((p, n)),
        _ => None,
    };
    // A pipeline counter's difference, absent when the pipeline was not read.
    let p = |f: fn(&Counters) -> u64| pipe.map(|(a, b)| d(f(a), f(b)));

    let loss = SensorLoss {
        sensor_session_events_lost: sensor_events,
        process_session_events_lost: process_events,
        sensor_session_buffers_lost: sensor_buffers,
        process_session_buffers_lost: process_buffers,
        kernel_queue_drops: Some(d(i0.kernel_queue_drops, i1.kernel_queue_drops)),
        user_queue_drops: Some(d(i0.user_queue_drops, i1.user_queue_drops)),
        dns_rate_limit_drops: Some(d(i0.dns_rate_limit_drops, i1.dns_rate_limit_drops)),
        actor_dropped: pipe.map(|(a, b)| d_classes(&a.actor_dropped, &b.actor_dropped)).unwrap_or_default(),
        buffer_backlog_drops: Some(d(prev.buffer_drops, now.buffer_drops)),
        callback_panics: Some(d(prev.callback_panics, now.callback_panics)),
    };
    let quality = SensorQuality {
        late_arrivals: p(|c| c.late_arrivals),
        parse_errors: Some(d(i0.parse_errors, i1.parse_errors)),
        unknown_version: Some(d(i0.unknown_version, i1.unknown_version)),
        launch_join_miss: p(|c| c.launch_join_miss),
        actor_unresolved: pipe.map(|(a, b)| d_classes(&a.actor_unresolved, &b.actor_unresolved)).unwrap_or_default(),
        unknown_file_object: p(|c| c.unknown_file_object),
        registry_unresolved: p(|c| c.registry_unresolved),
        value_read_failed: p(|c| c.value_read_failed),
        early_read_redone: p(|c| c.early_read_redone),
        reg_type_unusual: p(|c| c.reg_type_unusual),
        file_op_late_failure: p(|c| c.file_op_late_failure),
        writes_after_cleanup: p(|c| c.writes_after_cleanup),
        file_object_replaced: p(|c| c.file_object_replaced),
        // The agent never replays its buffer; `dump` and the transport do.
        buffer_invalid_records: None,
        enrichment_misses: p(|c| c.enrichment_misses),
        enrichment_errors: p(|c| c.enrichment_errors),
        reg_name_ambiguous: p(|c| c.reg_name_ambiguous),
        file_delete_outcome_unknown: p(|c| c.file_delete_outcome_unknown),
        file_cleanup_unpaired: Some(
            d(i0.cleanup_unpaired, i1.cleanup_unpaired) + d(i0.cleanup_outcome_late, i1.cleanup_outcome_late),
        ),
    };
    let housekeeping = SensorHousekeeping {
        process_cache_evictions: p(|c| c.process_cache_evictions),
        file_map_evictions: p(|c| c.file_map_evictions),
        key_map_evictions: p(|c| c.key_map_evictions),
        early_key_map_evictions: Some(d(i0.early_key_map_evictions, i1.early_key_map_evictions)),
        flow_table_evictions: p(|c| c.flow_table_evictions),
        hash_cache_evictions: Some(d(s0.hash_cache_evictions, s1.hash_cache_evictions)),
        // The writer's figure.
        retention_evictions: None,
        file_op_failed: p(|c| c.file_op_failed),
        pending_overflow: p(|c| c.pending_overflow),
        seeding_enabled: Some(now.seeding),
        seeder_handles_named: Some(d(s0.seeder_handles_named, s1.seeder_handles_named)),
        seeder_handles_failed: Some(d(s0.seeder_handles_failed, s1.seeder_handles_failed)),
        seeder_handles_timed_out: Some(d(s0.seeder_handles_timed_out, s1.seeder_handles_timed_out)),
        seeder_table_reads: Some(d(s0.seeder_table_reads, s1.seeder_table_reads)),
        seeder_deferred_rereads: Some(d(s0.seeder_deferred_rereads, s1.seeder_deferred_rereads)),
        seeder_stuck_helpers: Some(s1.seeder_stuck_helpers),
        // No source yet: the seeder keeps no count of its negative cache.
        seeder_negative_cache_size: None,
        service_queue_drops: Some(d(s0.service_queue_drops, s1.service_queue_drops)),
    };
    let resources = SensorResources {
        cpu_time: prev.cpu_time.zip(now.cpu_time).map(|(a, b)| d(a, b)),
        working_set: now.working_set,
    };
    let throughput = SensorThroughput {
        sensor_session_events_received: Some(d(prev.received[0], now.received[0])),
        process_session_events_received: Some(d(prev.received[1], now.received[1])),
    };
    let diagnostics = Diagnostics {
        cleanup_unpaired: d(i0.cleanup_unpaired, i1.cleanup_unpaired),
        cleanup_outcome_late: d(i0.cleanup_outcome_late, i1.cleanup_outcome_late),
        self_filtered: p(|c| c.self_filtered),
        op_end_discarded: d(i0.op_end_discarded, i1.op_end_discarded),
        fast_reads: d(i0.fast_reads, i1.fast_reads),
        threads: prev.threads.as_ref().zip(now.threads.as_ref()).map(|(a, b)| d_threads(a, b)),
        threads_interval: prev.threads.as_ref().zip(now.threads.as_ref()).map(|(a, b)| b.at - a.at),
        sizes: sizes(now),
    };
    let report = HealthReport { loss, quality, housekeeping, resources, throughput, ..HealthReport::default() };
    (report, diagnostics)
}

/// The structures' gauges in `now`: the pipeline's when it was read, then the
/// intake's and the services'.
fn sizes(now: &Snapshot) -> Sizes {
    let (i, s) = (&now.intake, &now.services);
    let mut v = now.pipeline_sizes.clone();
    v.push(("early_key_map", i.early_keys, i.early_keys_capacity));
    v.push(("hash_cache", s.hash_cache_entries, s.hash_cache_capacity));
    // A BTreeMap allocates per entry, with no spare room: its capacity is its
    // entries (review 7).
    v.push(("expand_cache", s.expand_cache_entries, s.expand_cache_entries));
    v.extend(now.queues.iter().copied());
    v
}

/// Whether any loss counter rose between the two snapshots: a report is then
/// due at once (§9.3).
pub fn loss_rose(prev: &Snapshot, now: &Snapshot) -> bool {
    let l = report(prev, now).0.loss;
    let counts = [
        l.sensor_session_events_lost,
        l.process_session_events_lost,
        l.sensor_session_buffers_lost,
        l.process_session_buffers_lost,
        l.kernel_queue_drops,
        l.user_queue_drops,
        l.dns_rate_limit_drops,
        l.buffer_backlog_drops,
        l.callback_panics,
    ];
    counts.iter().any(|c| c.is_some_and(|n| n > 0)) || l.actor_dropped.iter().any(|c| c.count > 0)
}

/// The Sensor Health event: `interval_start` and `time` in Unix ns.
pub fn event(id: EventId, time: i64, device: Device, interval_start: i64, report: HealthReport) -> Event {
    Event {
        meta: EventMeta { event_id: id, time, sensor: Sensor::Etw },
        device,
        kind: EventKind::SensorHealth(SensorHealthActivity {
            interval_start,
            action: SensorHealthAction::Report(Box::new(report)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_schema::{BootId, DeviceUid};

    fn busy() -> Snapshot {
        let mut c = Counters { late_arrivals: 5, file_op_failed: 2, self_filtered: 40, ..Counters::default() };
        c.actor_dropped.insert(Class::File, 3);
        c.actor_unresolved.insert(Class::Network, 1);
        Snapshot {
            pipeline: Some(c),
            intake: IntakeTotals {
                kernel_queue_drops: 7,
                cleanup_unpaired: 2,
                cleanup_outcome_late: 1,
                ..Default::default()
            },
            services: ServiceTotals { seeder_handles_named: 9, seeder_stuck_helpers: 1, ..Default::default() },
            callback_panics: 0,
            received: [120_000, 300],
            sensor_session: Some(SessionLoss { events_lost: 4, buffers_lost: 0 }),
            process_session: Some(SessionLoss::default()),
            buffer_drops: 6,
            cpu_time: Some(1_000),
            working_set: Some(50 << 20),
            threads: Some(listing(1_000, &[(1, 10, "atlas-hash-0", 900), (2, 10, "atlas-pipeline", 100)])),
            seeding: true,
            pipeline_sizes: vec![("file_map", 3, 16)],
            queues: vec![("kernel_queue", 4, 65_536)],
        }
    }

    /// R6: the structures' gauges come from `now` alone, the pipeline's first,
    /// then the early key map, the hash and expansion caches.
    #[test]
    fn a_report_carries_every_structures_size_from_now() {
        let mut now = busy();
        now.intake.early_keys = 7;
        now.intake.early_keys_capacity = 14;
        now.services.hash_cache_entries = 5;
        now.services.hash_cache_capacity = 28;
        now.services.expand_cache_entries = 2;
        let mut prev = busy();
        prev.pipeline_sizes = vec![("file_map", 99, 99)];
        prev.queues = vec![("kernel_queue", 99, 99)];
        let (_, diag) = report(&prev, &now);
        assert_eq!(
            diag.sizes,
            vec![
                ("file_map", 3, 16),
                ("early_key_map", 7, 14),
                ("hash_cache", 5, 28),
                ("expand_cache", 2, 2),
                ("kernel_queue", 4, 65_536)
            ]
        );
        // The pipeline not read: its part is absent, the rest still there.
        now.pipeline_sizes = vec![];
        assert_eq!(report(&prev, &now).1.sizes.len(), 4);
    }

    /// A thread listing: (thread id, creation time, name, CPU ns).
    fn listing(at: i64, threads: &[(u32, u64, &str, u64)]) -> ThreadList {
        ThreadList { at, threads: threads.iter().map(|&(t, c, n, cpu)| ((t, c), (n.to_owned(), cpu))).collect() }
    }

    /// Per-thread CPU (plan 1b-4a-p1, review m1): each thread's difference,
    /// summed by name, most first, zero left out; a thread that ended loses
    /// only its time since the last listing, even when another thread shares
    /// its name; a new thread (or a reused id) counts in full; an unlisted
    /// interval is absent, then covered; the listings' own interval.
    #[test]
    fn thread_cpu_is_differenced_by_thread_then_summed_by_name() {
        let (_, diag) = report(&Snapshot::at_start(true, 0), &busy());
        assert_eq!(diag.threads, Some(vec![("atlas-hash-0".into(), 900), ("atlas-pipeline".into(), 100)]));
        assert_eq!(diag.threads_interval, Some(1_000));

        // Name X: T7 (10 s so far) ends, T8 (1 s) runs on; a reader starts;
        // thread 2's id is reused by a new pipeline thread (creation 20).
        let mut prev = busy();
        prev.threads = Some(listing(
            1_000,
            &[(1, 10, "atlas-hash-0", 900), (2, 10, "atlas-pipeline", 100), (7, 1, "X", 10_000), (8, 2, "X", 1_000)],
        ));
        let mut now = busy();
        now.threads = Some(listing(
            61_000,
            &[
                (1, 10, "atlas-hash-0", 950),
                (2, 20, "atlas-pipeline", 40),
                (8, 2, "X", 1_500),
                (9, 30, "atlas-reader", 7),
            ],
        ));
        let (_, diag) = report(&prev, &now);
        assert_eq!(
            diag.threads,
            Some(vec![
                ("X".into(), 500),
                ("atlas-hash-0".into(), 50),
                ("atlas-pipeline".into(), 40),
                ("atlas-reader".into(), 7),
            ])
        );
        assert_eq!(diag.threads_interval, Some(60_000));
        let mut idle = prev.clone();
        idle.threads.as_mut().unwrap().at = 2_000;
        assert_eq!(report(&prev, &idle).1.threads, Some(vec![]), "nothing used, nothing listed");

        let mut blind = now.clone();
        blind.threads = None;
        assert_eq!(report(&prev, &blind).1.threads, None);
        assert_eq!(report(&prev, &blind).1.threads_interval, None);
        let from = prev.advance(&blind);
        assert_eq!(report(&from, &now).1.threads.unwrap()[0], ("X".into(), 500));
    }

    #[test]
    fn a_report_is_the_difference_of_two_snapshots() {
        let (r, diag) = report(&Snapshot::at_start(true, 0), &busy());
        assert_eq!(r.loss.kernel_queue_drops, Some(7));
        assert_eq!(r.loss.sensor_session_events_lost, Some(4));
        assert_eq!(r.loss.process_session_events_lost, Some(0));
        assert_eq!(r.loss.buffer_backlog_drops, Some(6));
        assert_eq!(r.loss.actor_dropped, [ClassCount { class_uid: 1001, count: 3 }]);
        assert_eq!(r.quality.late_arrivals, Some(5));
        assert_eq!(r.quality.actor_unresolved, [ClassCount { class_uid: 4001, count: 1 }]);
        assert_eq!(r.quality.file_cleanup_unpaired, Some(3));
        assert_eq!(r.housekeeping.file_op_failed, Some(2));
        assert_eq!(r.housekeeping.seeder_handles_named, Some(9));
        assert_eq!(r.housekeeping.seeding_enabled, Some(true));
        assert_eq!(r.resources.cpu_time, Some(1_000));
        assert_eq!(r.resources.working_set, Some(50 << 20));
        assert_eq!(r.throughput.sensor_session_events_received, Some(120_000));
        assert_eq!(r.throughput.process_session_events_received, Some(300));
        assert_eq!((diag.cleanup_unpaired, diag.cleanup_outcome_late, diag.self_filtered), (2, 1, Some(40)));

        // The next interval counts only what is new; gauges are taken as they are.
        let mut later = busy();
        later.intake.kernel_queue_drops = 10;
        later.pipeline.as_mut().unwrap().actor_dropped.insert(Class::File, 3);
        later.received[0] += 5_000;
        let (r, _) = report(&busy(), &later);
        assert_eq!(r.throughput.sensor_session_events_received, Some(5_000));
        assert_eq!(r.throughput.process_session_events_received, Some(0));
        assert_eq!(r.loss.kernel_queue_drops, Some(3));
        assert_eq!(r.loss.sensor_session_events_lost, Some(0));
        assert!(r.loss.actor_dropped.is_empty(), "zero counts are left out");
        assert_eq!(r.housekeeping.seeder_stuck_helpers, Some(1));
        assert_eq!(r.resources.working_set, Some(50 << 20));
    }

    #[test]
    fn session_counters_wrap() {
        let mut a = busy();
        a.sensor_session = Some(SessionLoss { events_lost: u32::MAX - 1, buffers_lost: u32::MAX });
        let mut b = busy();
        b.sensor_session = Some(SessionLoss { events_lost: 2, buffers_lost: 0 });
        let (r, _) = report(&a, &b);
        assert_eq!(r.loss.sensor_session_events_lost, Some(4));
        assert_eq!(r.loss.sensor_session_buffers_lost, Some(1));
    }

    #[test]
    fn what_was_not_read_is_absent_and_counted_later() {
        let start = Snapshot::at_start(true, 0);
        let mut blind = busy();
        blind.pipeline = None;
        blind.sensor_session = None;
        let (r, diag) = report(&start, &blind);
        assert_eq!(r.quality.late_arrivals, None);
        assert!(r.loss.actor_dropped.is_empty());
        assert_eq!(r.loss.sensor_session_events_lost, None);
        assert_eq!(diag.self_filtered, None);
        assert_eq!(r.loss.kernel_queue_drops, Some(7), "the atomics are read regardless");

        // The next report covers both intervals for what was missed.
        let from = start.advance(&blind);
        let (r, _) = report(&from, &busy());
        assert_eq!(r.quality.late_arrivals, Some(5));
        assert_eq!(r.loss.sensor_session_events_lost, Some(4));
        assert_eq!(r.loss.kernel_queue_drops, Some(0));
    }

    #[test]
    fn a_rising_loss_counter_is_seen_and_other_counters_are_not() {
        let base = busy();
        assert!(!loss_rose(&base, &base));
        let mut quality_only = busy();
        quality_only.pipeline.as_mut().unwrap().late_arrivals += 100;
        quality_only.intake.cleanup_unpaired += 1;
        assert!(!loss_rose(&base, &quality_only));
        let rises: [fn(&mut Snapshot); 6] = [
            |s| s.intake.kernel_queue_drops += 1,
            |s| s.intake.dns_rate_limit_drops += 1,
            |s| s.buffer_drops += 1,
            |s| s.callback_panics += 1,
            |s| s.process_session.as_mut().unwrap().buffers_lost += 1,
            |s| *s.pipeline.as_mut().unwrap().actor_dropped.entry(Class::Dns).or_default() += 1,
        ];
        for (i, rise) in rises.iter().enumerate() {
            let mut s = busy();
            rise(&mut s);
            assert!(loss_rose(&base, &s), "rise {i}");
        }
    }

    #[test]
    fn the_event_carries_the_interval_and_validates() {
        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
        let (r, _) = report(&Snapshot::at_start(false, 0), &busy());
        let e = event(EventId::new_v7(), 2_000, device, 1_000, r.clone());
        let back = atlas_schema::decode_event(&atlas_schema::encode_event(e.clone())).unwrap();
        assert_eq!(back, e);
        let EventKind::SensorHealth(a) = back.kind else { panic!("not a Sensor Health event") };
        assert_eq!(a.interval_start, 1_000);
        assert_eq!(a.action, SensorHealthAction::Report(Box::new(r)));
    }
}
