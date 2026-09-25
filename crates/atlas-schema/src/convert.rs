//! Shared helpers for wire → domain validation.
//!
//! Field paths are built only when an error is produced, so the happy path
//! does not allocate for them.

use crate::error::{SchemaError, SchemaErrorKind};

pub(crate) type Result<T> = std::result::Result<T, SchemaError>;

/// `parent.field`, or just `field` at the root.
pub(crate) fn join(parent: &str, field: &str) -> String {
    if parent.is_empty() { field.to_owned() } else { format!("{parent}.{field}") }
}

pub(crate) fn err<T>(parent: &str, field: &str, kind: SchemaErrorKind) -> Result<T> {
    Err(SchemaError::new(join(parent, field), kind))
}

/// A required sub-message or oneof.
pub(crate) fn require<T>(value: Option<T>, parent: &str, field: &str) -> Result<T> {
    match value {
        Some(v) => Ok(v),
        None => err(parent, field, SchemaErrorKind::Missing),
    }
}

/// A string with a byte-length limit.
pub(crate) fn bounded(value: String, max: usize, parent: &str, field: &str) -> Result<String> {
    if value.len() > max { err(parent, field, SchemaErrorKind::TooLarge) } else { Ok(value) }
}

/// A fixed-size byte field.
pub(crate) fn fixed<const N: usize>(value: Vec<u8>, parent: &str, field: &str) -> Result<[u8; N]> {
    match value.try_into() {
        Ok(v) => Ok(v),
        Err(_) => err(parent, field, SchemaErrorKind::Malformed),
    }
}

/// A `uint32` wire field that must fit in `u16`.
pub(crate) fn u16_field(value: u32, parent: &str, field: &str) -> Result<u16> {
    match u16::try_from(value) {
        Ok(v) => Ok(v),
        Err(_) => err(parent, field, SchemaErrorKind::Malformed),
    }
}

/// A proto enum field: 0 (`*_UNSPECIFIED`) is `Missing`, an unrecognized
/// number is `UnknownEnum`. `map` returns `None` for the unspecified variant.
pub(crate) fn wire_enum<W, D>(raw: i32, map: impl FnOnce(W) -> Option<D>, parent: &str, field: &str) -> Result<D>
where
    W: TryFrom<i32>,
{
    match W::try_from(raw) {
        Ok(w) => match map(w) {
            Some(d) => Ok(d),
            None => err(parent, field, SchemaErrorKind::Missing),
        },
        Err(_) => err(parent, field, SchemaErrorKind::UnknownEnum),
    }
}
