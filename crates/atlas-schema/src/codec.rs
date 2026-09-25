//! Byte-level encode/decode: the entry points agents and the server use.

use atlas_proto::v1 as wire;
use prost::Message;

use crate::error::{SchemaError, SchemaErrorKind};
use crate::event::Event;
use crate::limits::EVENT_MAX;

/// Encodes a domain event to protobuf bytes. Infallible.
pub fn encode_event(event: Event) -> Vec<u8> {
    wire::Event::from(event).encode_to_vec()
}

/// Decodes and validates untrusted bytes. The size limit is checked before
/// any protobuf parsing.
pub fn decode_event(bytes: &[u8]) -> Result<Event, SchemaError> {
    if bytes.len() > EVENT_MAX {
        return Err(SchemaError::new("event", SchemaErrorKind::TooLarge));
    }
    let wire = wire::Event::decode(bytes).map_err(|_| SchemaError::new("event", SchemaErrorKind::Malformed))?;
    Event::try_from(wire)
}
