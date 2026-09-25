//! Validation errors produced by wire → domain conversion.

use std::fmt;

/// Why a wire event was rejected, and where.
///
/// `field_path` uses the domain field names, e.g. `process.file.path`,
/// `answers[3].data`, `device.uid`. Errors about the whole encoded event use
/// the path `event`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field_path}: {kind}")]
pub struct SchemaError {
    pub field_path: String,
    pub kind: SchemaErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaErrorKind {
    /// A required field, message, oneof, or enum value is absent
    /// (an enum set to its `*_UNSPECIFIED` zero value counts as absent).
    Missing,
    /// An enum or oneof carries a value this build does not know.
    UnknownEnum,
    /// A field or the whole event exceeds its size limit (see `limits`).
    TooLarge,
    /// The value is present but structurally invalid (wrong byte length,
    /// out-of-range number, undecodable protobuf).
    Malformed,
}

impl fmt::Display for SchemaErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl SchemaError {
    pub fn new(field_path: impl Into<String>, kind: SchemaErrorKind) -> Self {
        Self { field_path: field_path.into(), kind }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_path_colon_kind() {
        let e = SchemaError::new("process.file.path", SchemaErrorKind::TooLarge);
        assert_eq!(e.to_string(), "process.file.path: TooLarge");
    }
}
