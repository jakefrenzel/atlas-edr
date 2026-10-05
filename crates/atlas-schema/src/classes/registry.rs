//! Registry Key Activity (OCSF 201001) and Registry Value Activity (OCSF 201002),
//! spec sections 5.6–5.7, plus the sub-project 1 fields (sensor spec §10.4).

use atlas_proto::v1 as wire;
use atlas_proto::v1::registry_key_activity::Activity as WK;
use atlas_proto::v1::registry_value_activity::Activity as WV;

use crate::convert::{Result, bounded, err, require};
use crate::error::SchemaErrorKind;
use crate::limits::{PATH_MAX, REG_DATA_MAX};
use crate::objects::ProcessRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryKeyActivity {
    pub actor: ProcessRef,
    /// `reg_key.path`. For `Rename`: the new path.
    pub path: String,
    /// `path` holds only what the sensor saw (a relative name, or empty), not a full path.
    pub path_unresolved: bool,
    pub action: RegistryKeyAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryKeyAction {
    Create,
    Delete,
    /// `prev_path` is `prev_reg_key.path`, the original path.
    Rename {
        prev_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryValueActivity {
    pub actor: ProcessRef,
    /// `reg_value.path`: the containing key.
    pub key_path: String,
    /// `reg_value.name`; empty means the default value of the key.
    pub name: String,
    /// `key_path` holds only what the sensor saw (a relative name, or empty), not a full path.
    pub path_unresolved: bool,
    pub action: RegistryValueAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryValueAction {
    Set {
        value_type: RegType,
        data: Vec<u8>,
        data_truncated: bool,
        /// `data` was read from the registry after the event, not captured with it.
        data_read_after: bool,
        /// No data was obtained. Requires empty `data` and `data_truncated == false`.
        data_unavailable: bool,
    },
    Delete,
}

/// The type of a set value: a `REG_*` constant 0–11, or a raw number above
/// `REG_QWORD` (sensor spec §7.5), which travels as `raw_type` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegType {
    Known(RegValueType),
    /// Must be greater than 11; anything else is rejected on decode.
    Raw(u32),
}

/// Windows `REG_*` value types. Discriminants are the Windows constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RegValueType {
    None = 0,
    Sz = 1,
    ExpandSz = 2,
    Binary = 3,
    Dword = 4,
    DwordBigEndian = 5,
    Link = 6,
    MultiSz = 7,
    ResourceList = 8,
    FullResourceDescriptor = 9,
    ResourceRequirementsList = 10,
    Qword = 11,
}

impl RegValueType {
    pub fn from_raw(raw: u32) -> Option<Self> {
        use RegValueType::*;
        Some(match raw {
            0 => None,
            1 => Sz,
            2 => ExpandSz,
            3 => Binary,
            4 => Dword,
            5 => DwordBigEndian,
            6 => Link,
            7 => MultiSz,
            8 => ResourceList,
            9 => FullResourceDescriptor,
            10 => ResourceRequirementsList,
            11 => Qword,
            _ => return Option::None,
        })
    }
}

impl RegType {
    /// The sensor's mapping from the raw type in an event.
    pub fn from_raw(raw: u32) -> Self {
        match RegValueType::from_raw(raw) {
            Some(t) => Self::Known(t),
            None => Self::Raw(raw),
        }
    }
}

impl From<RegistryKeyActivity> for wire::RegistryKeyActivity {
    fn from(v: RegistryKeyActivity) -> Self {
        let activity = match v.action {
            RegistryKeyAction::Create => WK::Create(wire::RegistryKeyCreate {}),
            RegistryKeyAction::Delete => WK::Delete(wire::RegistryKeyDelete {}),
            RegistryKeyAction::Rename { prev_path } => WK::Rename(wire::RegistryKeyRename { prev_path }),
        };
        Self { actor: Some(v.actor.into()), path: v.path, path_unresolved: v.path_unresolved, activity: Some(activity) }
    }
}

impl From<RegistryValueActivity> for wire::RegistryValueActivity {
    fn from(v: RegistryValueActivity) -> Self {
        let activity = match v.action {
            RegistryValueAction::Set { value_type, data, data_truncated, data_read_after, data_unavailable } => {
                let (r#type, raw_type) = match value_type {
                    RegType::Known(t) => (Some(t as u32), None),
                    RegType::Raw(raw) => (None, Some(raw)),
                };
                WV::Set(wire::RegistryValueSet {
                    r#type,
                    data,
                    data_truncated,
                    data_read_after,
                    data_unavailable,
                    raw_type,
                })
            }
            RegistryValueAction::Delete => WV::Delete(wire::RegistryValueDelete {}),
        };
        Self {
            actor: Some(v.actor.into()),
            key_path: v.key_path,
            name: v.name,
            path_unresolved: v.path_unresolved,
            activity: Some(activity),
        }
    }
}

impl RegistryKeyActivity {
    pub(crate) fn from_wire(w: wire::RegistryKeyActivity) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let activity = require(w.activity, "", "activity")?;
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            path: bounded(w.path, PATH_MAX, "", "reg_key.path")?,
            path_unresolved: w.path_unresolved,
            action: match activity {
                WK::Create(_) => RegistryKeyAction::Create,
                WK::Delete(_) => RegistryKeyAction::Delete,
                WK::Rename(r) => {
                    RegistryKeyAction::Rename { prev_path: bounded(r.prev_path, PATH_MAX, "", "prev_reg_key.path")? }
                }
            },
        })
    }
}

impl RegistryValueActivity {
    pub(crate) fn from_wire(w: wire::RegistryValueActivity) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let activity = require(w.activity, "", "activity")?;
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            key_path: bounded(w.key_path, PATH_MAX, "", "reg_value.path")?,
            name: bounded(w.name, PATH_MAX, "", "reg_value.name")?,
            path_unresolved: w.path_unresolved,
            action: match activity {
                WV::Set(s) => set_from_wire(s)?,
                WV::Delete(_) => RegistryValueAction::Delete,
            },
        })
    }
}

fn set_from_wire(s: wire::RegistryValueSet) -> Result<RegistryValueAction> {
    // Exactly one of `type` (0–11) and `raw_type` (> 11). Both absent keeps 0a's
    // `reg_value.type: Missing`, so every message 0a accepts is still accepted.
    let value_type = match (s.r#type, s.raw_type) {
        (Some(raw), None) => match RegValueType::from_raw(raw) {
            Some(t) => RegType::Known(t),
            None => return err("", "reg_value.type", SchemaErrorKind::UnknownEnum),
        },
        (None, Some(raw)) if RegValueType::from_raw(raw).is_none() => RegType::Raw(raw),
        (None, None) => return err("", "reg_value.type", SchemaErrorKind::Missing),
        // A raw type in the known range, or both fields set.
        _ => return err("", "reg_value.raw_type", SchemaErrorKind::Malformed),
    };
    if s.data.len() > REG_DATA_MAX {
        return err("", "reg_value.data", SchemaErrorKind::TooLarge);
    }
    if s.data_unavailable && (!s.data.is_empty() || s.data_truncated) {
        return err("", "reg_value.data_unavailable", SchemaErrorKind::Malformed);
    }
    Ok(RegistryValueAction::Set {
        value_type,
        data: s.data,
        data_truncated: s.data_truncated,
        data_read_after: s.data_read_after,
        data_unavailable: s.data_unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn key(action: RegistryKeyAction) -> RegistryKeyActivity {
        RegistryKeyActivity { actor: proc_ref(), path: "HKLM\\SOFTWARE\\New".into(), path_unresolved: false, action }
    }

    fn set(value_type: RegType, data: Vec<u8>) -> RegistryValueActivity {
        RegistryValueActivity {
            actor: proc_ref(),
            key_path: "HKCU\\Software\\Run".into(),
            name: "x".into(),
            path_unresolved: false,
            action: RegistryValueAction::Set {
                value_type,
                data,
                data_truncated: false,
                data_read_after: false,
                data_unavailable: false,
            },
        }
    }

    fn dword(data: Vec<u8>) -> RegistryValueActivity {
        set(RegType::Known(RegValueType::Dword), data)
    }

    fn wire_set(w: &mut wire::RegistryValueActivity) -> &mut wire::RegistryValueSet {
        let Some(WV::Set(s)) = w.activity.as_mut() else { unreachable!() };
        s
    }

    fn rejected(w: wire::RegistryValueActivity) -> (String, SchemaErrorKind) {
        let e = RegistryValueActivity::from_wire(w).unwrap_err();
        (e.field_path, e.kind)
    }

    #[test]
    fn key_actions_round_trip() {
        for action in [
            RegistryKeyAction::Create,
            RegistryKeyAction::Delete,
            RegistryKeyAction::Rename { prev_path: "HKLM\\SOFTWARE\\Old".into() },
        ] {
            let a = key(action);
            assert_eq!(RegistryKeyActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn value_actions_round_trip() {
        let delete = RegistryValueActivity { action: RegistryValueAction::Delete, ..dword(vec![]) };
        for a in [dword(vec![1, 0, 0, 0]), delete] {
            assert_eq!(RegistryValueActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn reg_value_type_raw_values_match_windows_constants() {
        for raw in 0..=11 {
            assert_eq!(RegValueType::from_raw(raw).unwrap() as u32, raw);
        }
        assert_eq!(RegValueType::from_raw(12), None);
    }

    #[test]
    fn reg_type_from_raw_splits_at_reg_qword() {
        assert_eq!(RegType::from_raw(11), RegType::Known(RegValueType::Qword));
        assert_eq!(RegType::from_raw(12), RegType::Raw(12));
        assert_eq!(RegType::from_raw(u32::MAX), RegType::Raw(u32::MAX));
    }

    #[test]
    fn unknown_value_type_and_oversized_data_are_rejected() {
        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        wire_set(&mut w).r#type = Some(12);
        assert_eq!(rejected(w), ("reg_value.type".into(), SchemaErrorKind::UnknownEnum));

        let w = dword(vec![0; REG_DATA_MAX + 1]).into();
        assert_eq!(rejected(w), ("reg_value.data".into(), SchemaErrorKind::TooLarge));
    }

    #[test]
    fn sub_project_1_fields_round_trip() {
        let mut unresolved_key = key(RegistryKeyAction::Create);
        unresolved_key.path = "Software\\Relative".into();
        unresolved_key.path_unresolved = true;
        assert_eq!(RegistryKeyActivity::from_wire(unresolved_key.clone().into()).unwrap(), unresolved_key);

        let read_after = RegistryValueActivity {
            action: RegistryValueAction::Set {
                value_type: RegType::Known(RegValueType::Sz),
                data: vec![b'a', 0, 0, 0],
                data_truncated: false,
                data_read_after: true,
                data_unavailable: false,
            },
            ..dword(vec![])
        };
        let unavailable = RegistryValueActivity {
            path_unresolved: true,
            action: RegistryValueAction::Set {
                value_type: RegType::Raw(0x2000_0000),
                data: vec![],
                data_truncated: false,
                data_read_after: false,
                data_unavailable: true,
            },
            ..dword(vec![])
        };
        for a in [read_after, unavailable] {
            assert_eq!(RegistryValueActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn raw_type_travels_alone_on_the_wire() {
        let mut w = wire::RegistryValueActivity::from(set(RegType::Raw(12), vec![]));
        let s = wire_set(&mut w);
        assert_eq!((s.r#type, s.raw_type), (None, Some(12)));
    }

    #[test]
    fn raw_type_must_be_above_reg_qword_and_alone() {
        // In the known range: the sensor should have used `type`.
        let w = set(RegType::Raw(11), vec![]).into();
        assert_eq!(rejected(w), ("reg_value.raw_type".into(), SchemaErrorKind::Malformed));

        // Both set.
        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        wire_set(&mut w).raw_type = Some(12);
        assert_eq!(rejected(w), ("reg_value.raw_type".into(), SchemaErrorKind::Malformed));

        // Neither set: still 0a's Missing.
        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        wire_set(&mut w).r#type = None;
        assert_eq!(rejected(w), ("reg_value.type".into(), SchemaErrorKind::Missing));
    }

    #[test]
    fn data_unavailable_requires_no_data() {
        let mut w = wire::RegistryValueActivity::from(dword(vec![1, 0, 0, 0]));
        wire_set(&mut w).data_unavailable = true;
        assert_eq!(rejected(w), ("reg_value.data_unavailable".into(), SchemaErrorKind::Malformed));

        let mut w = wire::RegistryValueActivity::from(dword(vec![]));
        let s = wire_set(&mut w);
        (s.data_unavailable, s.data_truncated) = (true, true);
        assert_eq!(rejected(w), ("reg_value.data_unavailable".into(), SchemaErrorKind::Malformed));
    }
}
