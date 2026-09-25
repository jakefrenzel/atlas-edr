//! Identifiers: event ids, device/boot ids, and deterministic process uids
//! (spec sections 4.1–4.3).

use std::fmt;

use uuid::{Uuid, Variant};

macro_rules! uid16 {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name([u8; 16]);

        impl $name {
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }

            pub const fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }
        }

        /// Lowercase hex, 32 characters.
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                for b in self.0 {
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }
    };
}

uid16!(
    /// Random id generated once at agent install (`device.uid`).
    DeviceUid
);
uid16!(
    /// Opaque per-boot id (`device.boot_id`). Derivation is the sensor's job.
    BootId
);
uid16!(
    /// Deterministic process id (`process.uid`); see [`process_uid`].
    ProcessUid
);

/// `meta.event_id`: always a UUIDv7 (RFC 9562 variant).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct EventId(Uuid);

impl EventId {
    /// A fresh time-ordered id for a newly created event.
    pub fn new_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// Accepts only UUIDv7 with the RFC 9562 variant.
    pub fn from_uuid(uuid: Uuid) -> Option<Self> {
        (uuid.get_version_num() == 7 && uuid.get_variant() == Variant::RFC4122).then_some(Self(uuid))
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Domain-separation tag for the v1 process uid formula. Changing the formula
/// means changing the tag, so ids from different formulas never collide.
const PROCESS_UID_TAG: &[u8] = b"atlas.process.v1";

/// `process.uid = BLAKE3("atlas.process.v1" ‖ device.uid ‖ boot_id ‖ start_key LE)[0..16]`.
///
/// `start_key` is the Windows process start key, treated as an opaque u64.
pub fn process_uid(device: &DeviceUid, boot: &BootId, start_key: u64) -> ProcessUid {
    let mut hasher = blake3::Hasher::new();
    hasher.update(PROCESS_UID_TAG);
    hasher.update(device.as_bytes());
    hasher.update(boot.as_bytes());
    hasher.update(&start_key.to_le_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    ProcessUid(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(start: u8) -> [u8; 16] {
        std::array::from_fn(|i| start + i as u8)
    }

    #[test]
    fn uid_display_is_lowercase_hex() {
        let id = DeviceUid::from_bytes(seq(0xf0));
        assert_eq!(id.to_string(), "f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        assert_eq!(format!("{id:?}"), "DeviceUid(f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff)");
    }

    #[test]
    fn event_id_new_is_v7() {
        let id = EventId::new_v7();
        assert!(EventId::from_uuid(*id.as_uuid()).is_some());
    }

    #[test]
    fn event_id_rejects_non_v7() {
        assert!(EventId::from_uuid(Uuid::nil()).is_none());
        let v4 = Uuid::from_bytes([
            0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x41, 0x11, 0x81, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
        ]);
        assert_eq!(v4.get_version_num(), 4);
        assert!(EventId::from_uuid(v4).is_none());
    }

    /// Conformance vectors: any other implementation of the formula must
    /// produce these exact outputs.
    #[test]
    fn process_uid_test_vectors() {
        let zero = process_uid(&DeviceUid::from_bytes([0; 16]), &BootId::from_bytes([0; 16]), 0);
        assert_eq!(zero.to_string(), "8a01e78b10f07bca76e121414f3c9f00");

        let v = process_uid(&DeviceUid::from_bytes(seq(0x00)), &BootId::from_bytes(seq(0x10)), 0x0001_0000_0000_002a);
        assert_eq!(v.to_string(), "b1e0e2e71592001f8cc1b00c7846432e");
    }

    #[test]
    fn process_uid_depends_on_every_input() {
        let d = DeviceUid::from_bytes(seq(0x00));
        let b = BootId::from_bytes(seq(0x10));
        let base = process_uid(&d, &b, 42);
        assert_ne!(base, process_uid(&DeviceUid::from_bytes(seq(0x01)), &b, 42));
        assert_ne!(base, process_uid(&d, &BootId::from_bytes(seq(0x11)), 42));
        assert_ne!(base, process_uid(&d, &b, 43));
    }
}
