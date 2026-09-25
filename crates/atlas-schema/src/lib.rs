//! Atlas event schema: the typed domain model every Atlas component uses.
//!
//! Design: `docs/specs/2026-09-24-event-schema-design.md`.

mod convert;
mod error;
mod ids;
pub mod limits;
mod objects;

pub use error::{SchemaError, SchemaErrorKind};
pub use ids::{BootId, DeviceUid, EventId, ProcessUid, process_uid};
pub use objects::{File, Hashes, Integrity, NetworkEndpoint, Process, ProcessRef, Signature, SignatureStatus, User};
