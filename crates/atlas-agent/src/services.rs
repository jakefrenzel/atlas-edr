//! The pipeline's view of Windows (implemented in plan 1b-3b; faked in tests).
//!
//! - [`Lookups`]: cheap, cached queries the pipeline thread makes directly.
//! - [`Request`] / [`Reply`]: slow work done elsewhere, the workers and the
//!   reader lane ([4] in §3.2) and the seeder ([8]). The pipeline only queues
//!   requests; whoever drives it sends them and feeds the replies back.

use atlas_schema::{Hashes, Signature};

use crate::completion::PendingId;

/// A running process as `PROCESS_TELEMETRY_ID_INFORMATION` describes it (§5.2, §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveProcess {
    pub start_key: u64,
    /// NT path of the image.
    pub image_path: String,
    pub command_line: Option<String>,
}

/// Cheap lookups made on the pipeline thread. Implementations cache.
pub trait Lookups {
    /// The drive form of an NT path (`\Device\HarddiskVolume3\x` → `C:\x`),
    /// or `None` to keep the NT path (§5.5).
    fn dos_path(&mut self, nt_path: &str) -> Option<String>;
    /// The live process with this PID, if any (§5.3 rule 2; §5.2's Launch fallback).
    fn live_process(&mut self, pid: u32) -> Option<LiveProcess>;
    /// `DOMAIN\name` for a SID string (`LookupAccountSid`, cached).
    fn account_name(&mut self, sid: &str) -> Option<String>;
}

/// Which file an enrichment result belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrichTarget {
    /// `process.file` of a Launch.
    LaunchImage,
    /// `module.file` of a Module Load.
    Module,
}

/// Seeder questions are about one of the two handle types (§7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandleKind {
    Key,
    File,
}

/// Work the pipeline asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// SHA-256 and signature of a file (§6.3).
    Enrich { id: PendingId, target: EnrichTarget, nt_path: String },
    /// Forget cached results for a path that changed (§6.3).
    InvalidateHash { nt_path: String },
    /// Read a registry value after its SetValueKey (§7.5), on the ordered path.
    ReadValue { id: PendingId, read: ValueRead },
    /// Expand 8.3 components of an NT path (§7.2; plan 1b-3a decision D4). `slot` tells
    /// apart the paths of one event (a Rename has two); the reply repeats it.
    Expand { id: PendingId, slot: u8, nt_path: String },
    /// Read the handle table for these object addresses (§7.4). Empty: the
    /// start-up pass over every handle.
    Seed { kind: HandleKind, addresses: Vec<u64> },
}

/// A registry value read (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueRead {
    /// The raw NT key path as logged (`\REGISTRY\MACHINE\…`, `ControlSet00N`).
    pub key_path: String,
    /// The value name as counted in the event (may contain NULs).
    pub value_name: Vec<u16>,
}

/// What a value read found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueData {
    pub value_type: u32,
    /// The full length of the value.
    pub size: u32,
    /// At most 4 KiB of it.
    pub data: Vec<u8>,
}

/// A seeded name: one handle-table entry the seeder could name (§7.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub address: u64,
    pub owner_pid: u32,
    /// NT name (`\REGISTRY\…` for keys, `\Device\…` for files).
    pub name: String,
}

/// One read of the handle table (§7.4), answering one `Request::Seed`.
///
/// Every handle of `kind` that the read covers appears in `named` or in
/// `unnamable` (a file handle that is not a disk file is unnamable). The read
/// covers the addresses asked about, or the whole table for the start-up pass.
/// A covered address that is in neither list was not in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub kind: HandleKind,
    /// QPC when the table was read (T_snap).
    pub taken: i64,
    /// The request's addresses; empty for the start-up pass.
    pub asked: Vec<u64>,
    pub named: Vec<Named>,
    /// Addresses in the table that could not be named (protected processes,
    /// failed or timed-out queries, non-disk files), with their owner's PID:
    /// the negative cache.
    pub unnamable: Vec<(u64, u32)>,
}

/// Results coming back to the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// `error`: an operational error (§6.3: signature absent, counted).
    Enriched {
        id: PendingId,
        hashes: Option<Hashes>,
        signature: Option<Signature>,
        error: bool,
    },
    ValueRead {
        id: PendingId,
        result: Option<ValueData>,
    },
    /// A read made on the fast path (§7.5), keyed by the event it answers.
    EarlyRead {
        event: EarlyKey,
        read: ValueRead,
        result: Option<ValueData>,
    },
    /// The long NT path, or `None` if it could not be expanded.
    Expanded {
        id: PendingId,
        slot: u8,
        long_path: Option<String>,
    },
    Snapshot(Snapshot),
}

/// Identifies a SetValueKey across the fast path and the ordered path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EarlyKey {
    pub ts: i64,
    pub tid: u32,
    pub key_object: u64,
}
