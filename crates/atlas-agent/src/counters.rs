//! The pipeline's counters (sensor spec §9.3). Each one is a Sensor Health
//! field (atlas-schema `SensorLoss`, `SensorQuality`, `SensorHousekeeping`);
//! plan 1b-4 copies them into the periodic report and resets the deltas.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// An OCSF event class, for the per-class counts (`ClassCount`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    File,
    Module,
    Process,
    Network,
    Dns,
    RegistryKey,
    RegistryValue,
}

impl Class {
    /// OCSF 1.9.0 `class_uid`, as atlas-schema derives it.
    pub const fn uid(self) -> u32 {
        match self {
            Class::File => 1001,
            Class::Module => 1005,
            Class::Process => 1007,
            Class::Network => 4001,
            Class::Dns => 4003,
            Class::RegistryKey => 201001,
            Class::RegistryValue => 201002,
        }
    }
}

/// Counters kept by the pipeline thread. Counters count occurrences; the
/// report takes the difference between two snapshots (1b-1, D1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    // Loss.
    pub actor_dropped: BTreeMap<Class, u64>,
    // Quality.
    pub late_arrivals: u64,
    pub launch_join_miss: u64,
    pub actor_unresolved: BTreeMap<Class, u64>,
    pub unknown_file_object: u64,
    pub registry_unresolved: u64,
    pub value_read_failed: u64,
    pub early_read_redone: u64,
    pub reg_type_unusual: u64,
    /// A value name with more than one possible end (plan 1b-2, clarification 5).
    pub reg_name_ambiguous: u64,
    pub file_op_late_failure: u64,
    pub writes_after_cleanup: u64,
    pub file_object_replaced: u64,
    pub enrichment_misses: u64,
    pub enrichment_errors: u64,
    /// Deletes reported from the request alone: the Cleanup's outcome was unknown (plan 1b-3c).
    pub file_delete_outcome_unknown: u64,
    // Housekeeping.
    pub process_cache_evictions: u64,
    pub file_map_evictions: u64,
    pub key_map_evictions: u64,
    pub flow_table_evictions: u64,
    pub file_op_failed: u64,
    pub pending_overflow: u64,
    /// Events dropped at emission because their actor is the agent (§5.5).
    pub self_filtered: u64,
}

impl Counters {
    pub fn add_class(map: &mut BTreeMap<Class, u64>, class: Class) {
        *map.entry(class).or_default() += 1;
    }
}

/// Counters kept by the ETW callbacks, which run on the consumer threads.
#[derive(Debug, Default)]
pub struct IntakeCounters {
    pub kernel_queue_drops: AtomicU64,
    pub user_queue_drops: AtomicU64,
    pub dns_rate_limit_drops: AtomicU64,
    pub parse_errors: AtomicU64,
    pub unknown_version: AtomicU64,
    pub early_key_map_evictions: AtomicU64,
    /// Successful OperationEnds discarded in the callback (§3.2): not a loss.
    pub op_end_discarded: AtomicU64,
    /// Value reads sent on the fast path (§7.5).
    pub fast_reads: AtomicU64,
    /// Cleanups whose outcome could not be paired with them, because another
    /// operation on their Irp came first. With `cleanup_outcome_late` (an
    /// outcome past the window), `quality.file_cleanup_unpaired` (plan 1b-3c,
    /// review R-M4).
    pub cleanup_unpaired: AtomicU64,
    pub cleanup_outcome_late: AtomicU64,
    /// Gauges: the early key map's entries and capacity (R6).
    pub early_keys: AtomicU64,
    pub early_keys_capacity: AtomicU64,
}

impl IntakeCounters {
    pub fn bump(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }

    /// The early key map's (entries, capacity), gauges (R6).
    pub fn set_early_keys(&self, (len, cap): (usize, usize)) {
        self.early_keys.store(len as u64, Ordering::Relaxed);
        self.early_keys_capacity.store(cap as u64, Ordering::Relaxed);
    }

    /// The values now, for a Sensor Health snapshot (plan 1b-4a).
    pub fn totals(&self) -> IntakeTotals {
        let g = Self::get;
        IntakeTotals {
            kernel_queue_drops: g(&self.kernel_queue_drops),
            user_queue_drops: g(&self.user_queue_drops),
            dns_rate_limit_drops: g(&self.dns_rate_limit_drops),
            parse_errors: g(&self.parse_errors),
            unknown_version: g(&self.unknown_version),
            early_key_map_evictions: g(&self.early_key_map_evictions),
            op_end_discarded: g(&self.op_end_discarded),
            fast_reads: g(&self.fast_reads),
            cleanup_unpaired: g(&self.cleanup_unpaired),
            cleanup_outcome_late: g(&self.cleanup_outcome_late),
            early_keys: g(&self.early_keys),
            early_keys_capacity: g(&self.early_keys_capacity),
        }
    }
}

/// [`IntakeCounters`] read at one instant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntakeTotals {
    pub kernel_queue_drops: u64,
    pub user_queue_drops: u64,
    pub dns_rate_limit_drops: u64,
    pub parse_errors: u64,
    pub unknown_version: u64,
    pub early_key_map_evictions: u64,
    pub op_end_discarded: u64,
    pub fast_reads: u64,
    pub cleanup_unpaired: u64,
    pub cleanup_outcome_late: u64,
    /// Gauges: the early key map's entries and capacity (R6).
    pub early_keys: u64,
    pub early_keys_capacity: u64,
}

/// Counters kept by the Windows services (plan 1b-3b), shared with their
/// threads. Each one is a Sensor Health field; `seeder_stuck_helpers` is a
/// gauge, the rest count occurrences.
#[derive(Debug, Default)]
pub struct ServiceCounters {
    /// Requests dropped because their lane was full (`housekeeping.service_queue_drops`).
    /// A dropped `Enrich`, `ReadValue`, `Expand` or `Seed` costs its event's
    /// deadline; a dropped invalidation clears the expansion cache (R-M3).
    pub service_queue_drops: AtomicU64,
    /// Hash results dropped when the cache started over (`housekeeping.hash_cache_evictions`).
    pub hash_cache_evictions: AtomicU64,
    pub seeder_handles_named: AtomicU64,
    /// Covered addresses answered as unnamable, including timed-out ones.
    pub seeder_handles_failed: AtomicU64,
    pub seeder_handles_timed_out: AtomicU64,
    /// Two per snapshot: the read, and the verifying read (F2).
    pub seeder_table_reads: AtomicU64,
    /// Re-reads that had to wait for the CPU budget, once each (§7.4).
    pub seeder_deferred_rereads: AtomicU64,
    /// Gauge, updated at the end of each snapshot.
    pub seeder_stuck_helpers: AtomicU64,
    /// Gauges for the diagnostic log (R6): the hash cache's entries and
    /// capacity after each hash job, the expansion cache's entries after each
    /// expansion.
    pub hash_cache_entries: AtomicU64,
    pub hash_cache_capacity: AtomicU64,
    pub expand_cache_entries: AtomicU64,
}

impl ServiceCounters {
    /// The values now, for a Sensor Health snapshot (plan 1b-4a).
    pub fn totals(&self) -> ServiceTotals {
        let g = IntakeCounters::get;
        ServiceTotals {
            service_queue_drops: g(&self.service_queue_drops),
            hash_cache_evictions: g(&self.hash_cache_evictions),
            seeder_handles_named: g(&self.seeder_handles_named),
            seeder_handles_failed: g(&self.seeder_handles_failed),
            seeder_handles_timed_out: g(&self.seeder_handles_timed_out),
            seeder_table_reads: g(&self.seeder_table_reads),
            seeder_deferred_rereads: g(&self.seeder_deferred_rereads),
            seeder_stuck_helpers: g(&self.seeder_stuck_helpers),
            hash_cache_entries: g(&self.hash_cache_entries),
            hash_cache_capacity: g(&self.hash_cache_capacity),
            expand_cache_entries: g(&self.expand_cache_entries),
        }
    }
}

/// [`ServiceCounters`] read at one instant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServiceTotals {
    pub service_queue_drops: u64,
    pub hash_cache_evictions: u64,
    pub seeder_handles_named: u64,
    pub seeder_handles_failed: u64,
    pub seeder_handles_timed_out: u64,
    pub seeder_table_reads: u64,
    pub seeder_deferred_rereads: u64,
    /// A gauge.
    pub seeder_stuck_helpers: u64,
    /// Gauges (R6).
    pub hash_cache_entries: u64,
    pub hash_cache_capacity: u64,
    pub expand_cache_entries: u64,
}
