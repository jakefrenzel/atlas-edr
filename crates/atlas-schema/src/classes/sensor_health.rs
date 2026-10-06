//! Sensor Health (Atlas extension class, sensor spec §10.3): the agent's own
//! loss, quality, housekeeping and resource figures.
//!
//! Counters count occurrences during `[interval_start, meta.time]`; gauges are
//! sampled at the end of the interval. `None` means not measured; `Some(0)`
//! means measured and none occurred.

use atlas_proto::v1 as wire;
use atlas_proto::v1::sensor_health::Activity as W;

use crate::convert::{Result, err, require};
use crate::error::SchemaErrorKind;
use crate::limits::CLASS_COUNTS_MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorHealthActivity {
    /// Start of the interval: nanoseconds since the Unix epoch, UTC.
    pub interval_start: i64,
    pub action: SensorHealthAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SensorHealthAction {
    /// Boxed: a report is much larger than any other event body.
    Report(Box<HealthReport>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthReport {
    pub loss: SensorLoss,
    pub quality: SensorQuality,
    pub housekeeping: SensorHousekeeping,
    pub resources: SensorResources,
    /// Set when buffered events were deleted before delivery (overflow).
    pub gap: Option<SensorGap>,
    pub buffer: SensorBuffer,
}

/// A count for one event class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassCount {
    /// OCSF `class_uid`.
    pub class_uid: u32,
    pub count: u64,
}

/// Buffered events deleted before delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorGap {
    /// Event-time range of the deleted events (ns since the Unix epoch, UTC).
    /// Both `None` when none of the deleted records could be decoded.
    pub first_time: Option<i64>,
    pub last_time: Option<i64>,
    pub events: u64,
}

/// Converts one field of a counter group. Identity for scalars; validated for lists.
trait GroupField: Sized {
    type Wire;
    fn into_wire(self) -> Self::Wire;
    fn from_wire(w: Self::Wire, path: &str, field: &str) -> Result<Self>;
}

impl GroupField for Option<u64> {
    type Wire = Self;
    fn into_wire(self) -> Self {
        self
    }
    fn from_wire(w: Self, _: &str, _: &str) -> Result<Self> {
        Ok(w)
    }
}

impl GroupField for Option<bool> {
    type Wire = Self;
    fn into_wire(self) -> Self {
        self
    }
    fn from_wire(w: Self, _: &str, _: &str) -> Result<Self> {
        Ok(w)
    }
}

impl GroupField for Vec<ClassCount> {
    type Wire = Vec<wire::ClassCount>;
    fn into_wire(self) -> Self::Wire {
        self.into_iter().map(|c| wire::ClassCount { class_uid: c.class_uid, count: c.count }).collect()
    }
    fn from_wire(w: Self::Wire, path: &str, field: &str) -> Result<Self> {
        if w.len() > CLASS_COUNTS_MAX {
            return err(path, field, SchemaErrorKind::TooLarge);
        }
        Ok(w.into_iter().map(|c| ClassCount { class_uid: c.class_uid, count: c.count }).collect())
    }
}

/// A group of counters: the domain struct, and conversions that copy (or, for
/// lists, validate) each field. `path` is the group's field path for errors.
macro_rules! counter_group {
    ($(#[$doc:meta])* $name:ident, $path:literal { $($(#[$fdoc:meta])* $field:ident: $ty:ty,)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub struct $name {
            $($(#[$fdoc])* pub $field: $ty,)*
        }

        impl From<$name> for wire::$name {
            fn from(v: $name) -> Self {
                Self { $($field: GroupField::into_wire(v.$field),)* }
            }
        }

        impl $name {
            fn from_wire(w: Option<wire::$name>) -> Result<Self> {
                let Some(w) = w else { return Ok(Self::default()) };
                Ok(Self { $($field: GroupField::from_wire(w.$field, $path, stringify!($field))?,)* })
            }

            /// `None` for an all-absent group, so empty groups cost nothing on the wire.
            fn into_wire(self) -> Option<wire::$name> {
                (self != Self::default()).then(|| self.into())
            }
        }
    };
}

counter_group!(
    /// Events that never reached the buffer.
    SensorLoss, "loss" {
        /// ETW events lost by the `Atlas-Sensor` session.
        sensor_session_events_lost: Option<u64>,
        /// ETW events lost by the `Atlas-Process` session.
        process_session_events_lost: Option<u64>,
        sensor_session_buffers_lost: Option<u64>,
        process_session_buffers_lost: Option<u64>,
        kernel_queue_drops: Option<u64>,
        user_queue_drops: Option<u64>,
        dns_rate_limit_drops: Option<u64>,
        /// Dropped because no process uid could be computed, per class.
        actor_dropped: Vec<ClassCount>,
        /// Dropped because the buffer's in-memory backlog was full.
        buffer_backlog_drops: Option<u64>,
        /// Skipped because the ETW callback panicked on them.
        callback_panics: Option<u64>,
    }
);

counter_group!(
    /// Events emitted with less than full information, or handled outside the usual path.
    SensorQuality, "quality" {
        late_arrivals: Option<u64>,
        parse_errors: Option<u64>,
        unknown_version: Option<u64>,
        launch_join_miss: Option<u64>,
        /// Emitted with an unresolved actor (empty path and name), per class.
        actor_unresolved: Vec<ClassCount>,
        unknown_file_object: Option<u64>,
        registry_unresolved: Option<u64>,
        value_read_failed: Option<u64>,
        early_read_redone: Option<u64>,
        reg_type_unusual: Option<u64>,
        file_op_late_failure: Option<u64>,
        writes_after_cleanup: Option<u64>,
        file_object_replaced: Option<u64>,
        /// Invalid records found when the buffer was replayed.
        buffer_invalid_records: Option<u64>,
        enrichment_misses: Option<u64>,
        enrichment_errors: Option<u64>,
        /// Registry value names with more than one possible end (embedded NULs).
        reg_name_ambiguous: Option<u64>,
    }
);

counter_group!(
    /// Evictions from bounded structures, and expected drops.
    SensorHousekeeping, "housekeeping" {
        process_cache_evictions: Option<u64>,
        file_map_evictions: Option<u64>,
        key_map_evictions: Option<u64>,
        early_key_map_evictions: Option<u64>,
        flow_table_evictions: Option<u64>,
        hash_cache_evictions: Option<u64>,
        /// Whole buffer segments deleted by rolling retention (expected without a transport).
        retention_evictions: Option<u64>,
        /// Failed file creates, deletes and renames that were dropped.
        file_op_failed: Option<u64>,
        pending_overflow: Option<u64>,
        seeding_enabled: Option<bool>,
        seeder_handles_named: Option<u64>,
        seeder_handles_failed: Option<u64>,
        seeder_handles_timed_out: Option<u64>,
        seeder_table_reads: Option<u64>,
        seeder_deferred_rereads: Option<u64>,
        /// Gauge.
        seeder_stuck_helpers: Option<u64>,
        /// Gauge.
        seeder_negative_cache_size: Option<u64>,
    }
);

counter_group!(
    /// The agent's own resource use.
    SensorResources, "resources" {
        /// Process CPU time (user + kernel) during the interval, in nanoseconds.
        cpu_time: Option<u64>,
        /// Working set at the end of the interval, in bytes (a gauge).
        working_set: Option<u64>,
    }
);

counter_group!(
    /// The agent's on-disk buffer (sensor spec §8).
    SensorBuffer, "buffer" {
        write_errors: Option<u64>,
        recoveries: Option<u64>,
        /// Gauge: writes were failing at the end of the interval.
        failing: Option<bool>,
        /// Records refused: empty or over 256 KiB (an agent defect).
        rejected: Option<u64>,
        /// Segments another process held open, so they could not be deleted.
        delete_failures: Option<u64>,
        /// Found at startup; reported once, in the first report.
        truncated_bytes: Option<u64>,
        cursor_reset: Option<bool>,
        foreign_segments: Option<u64>,
        /// Sealed segments whose remainder a reader skipped as corrupt.
        corrupt_segments: Option<u64>,
        /// Gauge: bytes in all segments.
        disk_bytes: Option<u64>,
    }
);

impl From<SensorGap> for wire::SensorGap {
    fn from(v: SensorGap) -> Self {
        Self { first_time: v.first_time, last_time: v.last_time, events: v.events }
    }
}

impl SensorGap {
    fn from_wire(w: wire::SensorGap) -> Result<Self> {
        match (w.first_time, w.last_time) {
            (Some(first), Some(last)) if first > last => return err("gap", "last_time", SchemaErrorKind::Malformed),
            (Some(_), None) | (None, Some(_)) => return err("", "gap", SchemaErrorKind::Malformed),
            _ => {}
        }
        Ok(Self { first_time: w.first_time, last_time: w.last_time, events: w.events })
    }
}

impl From<SensorHealthActivity> for wire::SensorHealth {
    fn from(v: SensorHealthActivity) -> Self {
        let activity = match v.action {
            SensorHealthAction::Report(r) => W::Report(wire::SensorHealthReport {
                loss: r.loss.into_wire(),
                quality: r.quality.into_wire(),
                housekeeping: r.housekeeping.into_wire(),
                resources: r.resources.into_wire(),
                gap: r.gap.map(Into::into),
                buffer: r.buffer.into_wire(),
            }),
        };
        Self { interval_start: v.interval_start, activity: Some(activity) }
    }
}

impl SensorHealthActivity {
    pub(crate) fn from_wire(w: wire::SensorHealth) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let action = match require(w.activity, "", "activity")? {
            W::Report(r) => SensorHealthAction::Report(Box::new(HealthReport {
                loss: SensorLoss::from_wire(r.loss)?,
                quality: SensorQuality::from_wire(r.quality)?,
                housekeeping: SensorHousekeeping::from_wire(r.housekeeping)?,
                resources: SensorResources::from_wire(r.resources)?,
                gap: match r.gap {
                    Some(g) => Some(SensorGap::from_wire(g)?),
                    None => None,
                },
                buffer: SensorBuffer::from_wire(r.buffer)?,
            })),
        };
        Ok(Self { interval_start: w.interval_start, action })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(r: HealthReport) -> SensorHealthActivity {
        SensorHealthActivity {
            interval_start: 1_790_000_000_000_000_000,
            action: SensorHealthAction::Report(Box::new(r)),
        }
    }

    fn rejected(w: wire::SensorHealth) -> (String, SchemaErrorKind) {
        let e = SensorHealthActivity::from_wire(w).unwrap_err();
        (e.field_path, e.kind)
    }

    fn wire_report(a: SensorHealthActivity) -> (wire::SensorHealth, wire::SensorHealthReport) {
        let w = wire::SensorHealth::from(a);
        let Some(W::Report(r)) = w.activity.clone() else { unreachable!() };
        (w, r)
    }

    #[test]
    fn empty_report_round_trips_and_sends_no_groups() {
        let a = report(HealthReport::default());
        assert_eq!(SensorHealthActivity::from_wire(a.clone().into()).unwrap(), a);
        let (_, r) = wire_report(a);
        assert_eq!(r, wire::SensorHealthReport::default());
    }

    #[test]
    fn full_report_round_trips() {
        let a = report(HealthReport {
            loss: SensorLoss {
                kernel_queue_drops: Some(0),
                actor_dropped: vec![ClassCount { class_uid: 4001, count: 3 }],
                ..Default::default()
            },
            quality: SensorQuality {
                late_arrivals: Some(12),
                actor_unresolved: vec![ClassCount { class_uid: 1001, count: 1 }],
                ..Default::default()
            },
            housekeeping: SensorHousekeeping {
                seeding_enabled: Some(false),
                retention_evictions: Some(2),
                ..Default::default()
            },
            resources: SensorResources { cpu_time: Some(250_000_000), working_set: Some(40 << 20) },
            gap: Some(SensorGap { first_time: Some(1), last_time: Some(2), events: 9 }),
            buffer: SensorBuffer { failing: Some(true), write_errors: Some(3), ..Default::default() },
        });
        assert_eq!(SensorHealthActivity::from_wire(a.clone().into()).unwrap(), a);
    }

    #[test]
    fn class_count_lists_are_bounded() {
        let many = vec![ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX + 1];
        let a = report(HealthReport {
            quality: SensorQuality { actor_unresolved: many, ..Default::default() },
            ..Default::default()
        });
        assert_eq!(rejected(a.into()), ("quality.actor_unresolved".into(), SchemaErrorKind::TooLarge));

        let at_limit = vec![ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX];
        let a = report(HealthReport {
            loss: SensorLoss { actor_dropped: at_limit, ..Default::default() },
            ..Default::default()
        });
        assert_eq!(SensorHealthActivity::from_wire(a.clone().into()).unwrap(), a);
    }

    #[test]
    fn gap_times_are_both_or_neither_and_ordered() {
        let gap = |first_time, last_time| {
            report(HealthReport { gap: Some(SensorGap { first_time, last_time, events: 1 }), ..Default::default() })
        };
        assert_eq!(rejected(gap(Some(5), Some(4)).into()), ("gap.last_time".into(), SchemaErrorKind::Malformed));
        assert_eq!(rejected(gap(Some(5), None).into()), ("gap".into(), SchemaErrorKind::Malformed));
        assert_eq!(rejected(gap(None, Some(5)).into()), ("gap".into(), SchemaErrorKind::Malformed));
        for ok in [gap(None, None), gap(Some(5), Some(5))] {
            assert_eq!(SensorHealthActivity::from_wire(ok.clone().into()).unwrap(), ok);
        }
    }
}
