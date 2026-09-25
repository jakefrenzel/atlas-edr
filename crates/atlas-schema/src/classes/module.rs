//! Module Activity (OCSF 1005), spec section 5.3.

use atlas_proto::v1 as wire;
use atlas_proto::v1::module_activity::Activity as W;

use crate::convert::{Result, require};
use crate::objects::{File, ProcessRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleActivity {
    pub actor: ProcessRef,
    pub action: ModuleAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleAction {
    Load { file: File, base_address: u64 },
}

impl From<ModuleActivity> for wire::ModuleActivity {
    fn from(v: ModuleActivity) -> Self {
        let activity = match v.action {
            ModuleAction::Load { file, base_address } => {
                W::Load(wire::ModuleLoad { file: Some(file.into()), base_address })
            }
        };
        Self { actor: Some(v.actor.into()), activity: Some(activity) }
    }
}

impl ModuleActivity {
    pub(crate) fn from_wire(w: wire::ModuleActivity) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let activity = require(w.activity, "", "activity")?;
        let actor = ProcessRef::required(w.actor, "", "actor.process")?;
        let action = match activity {
            W::Load(l) => {
                ModuleAction::Load { file: File::required(l.file, "", "module.file")?, base_address: l.base_address }
            }
        };
        Ok(Self { actor, action })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::objects::test_support::{file, proc_ref};

    fn load() -> ModuleActivity {
        ModuleActivity {
            actor: proc_ref(),
            action: ModuleAction::Load { file: file("C:\\amsi.dll"), base_address: 0x7ff0_0000 },
        }
    }

    #[test]
    fn load_round_trips() {
        assert_eq!(ModuleActivity::from_wire(load().into()).unwrap(), load());
    }

    #[test]
    fn load_without_file_is_missing_module_file() {
        let mut w = wire::ModuleActivity::from(load());
        let Some(W::Load(l)) = w.activity.as_mut() else { unreachable!() };
        l.file = None;
        let e = ModuleActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("module.file", SchemaErrorKind::Missing));
    }
}
