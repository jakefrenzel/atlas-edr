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
}

impl IntakeCounters {
    pub fn bump(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }
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
}
