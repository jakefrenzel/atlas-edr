//! Atlas event schema: the typed domain model every Atlas component uses.
//!
//! - Domain → wire is infallible (`From`); see [`encode_event`].
//! - Wire → domain validates untrusted input (`TryFrom`); see [`decode_event`].
//!
//! Design: `docs/specs/2026-09-24-event-schema-design.md`.

pub mod classes;
mod codec;
mod convert;
mod error;
mod event;
mod ids;
pub mod limits;
mod objects;
mod ocsf;

pub use codec::{decode_event, encode_event};
pub use error::{SchemaError, SchemaErrorKind};
pub use event::{Device, Event, EventKind, EventMeta, Sensor};
pub use ids::{BootId, DeviceUid, EventId, ProcessUid, process_uid};
pub use objects::{File, Hashes, Integrity, NetworkEndpoint, Process, ProcessRef, Signature, SignatureStatus, User};
pub use ocsf::OcsfIds;

/// Generated wire types, re-exported for transport code.
pub use atlas_proto::v1 as wire;
