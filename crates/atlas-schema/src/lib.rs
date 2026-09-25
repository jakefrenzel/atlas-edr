//! Atlas event schema: the typed domain model every Atlas component uses.
//!
//! Design: `docs/specs/2026-09-24-event-schema-design.md`.

mod error;
mod ids;
pub mod limits;

pub use error::{SchemaError, SchemaErrorKind};
pub use ids::{BootId, DeviceUid, EventId, ProcessUid, process_uid};
