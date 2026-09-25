//! Registry Key Activity (OCSF 201001) and Registry Value Activity (OCSF 201002),
//! spec sections 5.6–5.7.

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
    pub action: RegistryValueAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryValueAction {
    Set { value_type: RegValueType, data: Vec<u8>, data_truncated: bool },
    Delete,
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

impl From<RegistryKeyActivity> for wire::RegistryKeyActivity {
    fn from(v: RegistryKeyActivity) -> Self {
        let activity = match v.action {
            RegistryKeyAction::Create => WK::Create(wire::RegistryKeyCreate {}),
            RegistryKeyAction::Delete => WK::Delete(wire::RegistryKeyDelete {}),
            RegistryKeyAction::Rename { prev_path } => WK::Rename(wire::RegistryKeyRename { prev_path }),
        };
        Self { actor: Some(v.actor.into()), path: v.path, activity: Some(activity) }
    }
}

impl From<RegistryValueActivity> for wire::RegistryValueActivity {
    fn from(v: RegistryValueActivity) -> Self {
        let activity = match v.action {
            RegistryValueAction::Set { value_type, data, data_truncated } => {
                WV::Set(wire::RegistryValueSet { r#type: value_type as u32, data, data_truncated })
            }
            RegistryValueAction::Delete => WV::Delete(wire::RegistryValueDelete {}),
        };
        Self { actor: Some(v.actor.into()), key_path: v.key_path, name: v.name, activity: Some(activity) }
    }
}

impl RegistryKeyActivity {
    pub(crate) fn from_wire(w: wire::RegistryKeyActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            path: bounded(w.path, PATH_MAX, "", "reg_key.path")?,
            action: match require(w.activity, "", "activity")? {
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
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            key_path: bounded(w.key_path, PATH_MAX, "", "reg_value.path")?,
            name: bounded(w.name, PATH_MAX, "", "reg_value.name")?,
            action: match require(w.activity, "", "activity")? {
                WV::Set(s) => {
                    let Some(value_type) = RegValueType::from_raw(s.r#type) else {
                        return err("", "reg_value.type", SchemaErrorKind::UnknownEnum);
                    };
                    if s.data.len() > REG_DATA_MAX {
                        return err("", "reg_value.data", SchemaErrorKind::TooLarge);
                    }
                    RegistryValueAction::Set { value_type, data: s.data, data_truncated: s.data_truncated }
                }
                WV::Delete(_) => RegistryValueAction::Delete,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn key(action: RegistryKeyAction) -> RegistryKeyActivity {
        RegistryKeyActivity { actor: proc_ref(), path: "HKLM\\SOFTWARE\\New".into(), action }
    }

    fn set(data: Vec<u8>) -> RegistryValueActivity {
        RegistryValueActivity {
            actor: proc_ref(),
            key_path: "HKCU\\Software\\Run".into(),
            name: "x".into(),
            action: RegistryValueAction::Set { value_type: RegValueType::Dword, data, data_truncated: false },
        }
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
        let delete = RegistryValueActivity { action: RegistryValueAction::Delete, ..set(vec![]) };
        for a in [set(vec![1, 0, 0, 0]), delete] {
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
    fn unknown_value_type_and_oversized_data_are_rejected() {
        let mut w = wire::RegistryValueActivity::from(set(vec![]));
        let Some(WV::Set(s)) = w.activity.as_mut() else { unreachable!() };
        s.r#type = 12;
        let e = RegistryValueActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("reg_value.type", SchemaErrorKind::UnknownEnum));

        let e = RegistryValueActivity::from_wire(set(vec![0; REG_DATA_MAX + 1]).into()).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("reg_value.data", SchemaErrorKind::TooLarge));
    }
}
