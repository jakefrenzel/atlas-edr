//! Process Activity (OCSF 1007), spec section 5.2.

use atlas_proto::v1 as wire;
use atlas_proto::v1::process_activity::Activity as W;

use crate::convert::{Result, require};
use crate::objects::{Process, ProcessRef};

// Launch is much larger than Terminate by design (full process detail).
// Events are moved, not stored in bulk arrays, so boxing buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessActivity {
    Launch {
        /// The process that actually issued the creation.
        actor: ProcessRef,
        /// The new process, full detail.
        process: Process,
    },
    Terminate {
        process: ProcessRef,
        exit_code: Option<i32>,
    },
}

impl From<ProcessActivity> for wire::ProcessActivity {
    fn from(v: ProcessActivity) -> Self {
        let activity = match v {
            ProcessActivity::Launch { actor, process } => {
                W::Launch(wire::ProcessLaunch { actor: Some(actor.into()), process: Some(process.into()) })
            }
            ProcessActivity::Terminate { process, exit_code } => {
                W::Terminate(wire::ProcessTerminate { process: Some(process.into()), exit_code })
            }
        };
        Self { activity: Some(activity) }
    }
}

impl ProcessActivity {
    pub(crate) fn from_wire(w: wire::ProcessActivity) -> Result<Self> {
        Ok(match require(w.activity, "", "activity")? {
            W::Launch(l) => Self::Launch {
                actor: ProcessRef::required(l.actor, "", "actor.process")?,
                process: Process::from_wire(require(l.process, "", "process")?, "process")?,
            },
            W::Terminate(t) => {
                Self::Terminate { process: ProcessRef::required(t.process, "", "process")?, exit_code: t.exit_code }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::ids::ProcessUid;
    use crate::limits::CMD_LINE_MAX;
    use crate::objects::Integrity;
    use crate::objects::test_support::{file, proc_ref};

    fn launch() -> ProcessActivity {
        ProcessActivity::Launch {
            actor: proc_ref(),
            process: Process {
                uid: ProcessUid::from_bytes([9; 16]),
                pid: 100,
                file: file("C:\\child.exe"),
                user: None,
                cmd_line: "child.exe /x".into(),
                cmd_line_truncated: false,
                created_time: 5,
                integrity: Some(Integrity::High),
                parent_process: Some(proc_ref()),
            },
        }
    }

    #[test]
    fn launch_and_terminate_round_trip() {
        for a in [launch(), ProcessActivity::Terminate { process: proc_ref(), exit_code: Some(-1) }] {
            assert_eq!(ProcessActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn launch_without_actor_is_missing_actor_process() {
        let mut w = wire::ProcessActivity::from(launch());
        let Some(W::Launch(l)) = w.activity.as_mut() else { unreachable!() };
        l.actor = None;
        let e = ProcessActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("actor.process", SchemaErrorKind::Missing));
    }

    #[test]
    fn cmd_line_over_limit_is_too_large() {
        let mut w = wire::ProcessActivity::from(launch());
        let Some(W::Launch(l)) = w.activity.as_mut() else { unreachable!() };
        l.process.as_mut().unwrap().cmd_line = "a".repeat(CMD_LINE_MAX + 1);
        let e = ProcessActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("process.cmd_line", SchemaErrorKind::TooLarge));
    }

    #[test]
    fn missing_activity_is_missing() {
        let e = ProcessActivity::from_wire(wire::ProcessActivity { activity: None }).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("activity", SchemaErrorKind::Missing));
    }
}
