//! Event Log Activity (OCSF 1008), sensor spec §10.2: tampering the watchdog
//! sees on the agent's own ETW sessions.

use atlas_proto::v1 as wire;
use atlas_proto::v1::event_log_activity::Activity as W;

use crate::convert::{Result, bounded, err, require};
use crate::error::SchemaErrorKind;
use crate::limits::EVENT_LOG_NAME_MAX;
use crate::objects::ProcessRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogActivity {
    /// Optional: the watchdog sees the effect, not who caused it.
    pub actor: Option<ProcessRef>,
    /// `log_name`: the ETW session name. Never empty.
    pub log_name: String,
    /// `log_provider`: the ETW provider name. Never empty for `Disable`.
    pub log_provider: String,
    /// `status_code`: the Win32 error or NTSTATUS that revealed the change.
    pub status_code: Option<u32>,
    pub action: EventLogAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLogAction {
    /// The session was found stopped, or replaced by another session with its name.
    Stop,
    /// The agent recreated the session.
    Restart,
    /// A provider was disabled or changed in the session, or its canary went silent.
    Disable,
}

impl From<EventLogActivity> for wire::EventLogActivity {
    fn from(v: EventLogActivity) -> Self {
        let activity = match v.action {
            EventLogAction::Stop => W::Stop(wire::EventLogStop {}),
            EventLogAction::Restart => W::Restart(wire::EventLogRestart {}),
            EventLogAction::Disable => W::Disable(wire::EventLogDisable {}),
        };
        Self {
            actor: v.actor.map(Into::into),
            log_name: v.log_name,
            log_provider: v.log_provider,
            status_code: v.status_code,
            activity: Some(activity),
        }
    }
}

impl EventLogActivity {
    pub(crate) fn from_wire(w: wire::EventLogActivity) -> Result<Self> {
        // Activity first: an unknown (newer) activity must read as `activity: Missing`.
        let activity = require(w.activity, "", "activity")?;
        let actor = match w.actor {
            Some(a) => Some(ProcessRef::from_wire(a, "actor.process")?),
            None => None,
        };
        let log_name = bounded(w.log_name, EVENT_LOG_NAME_MAX, "", "log_name")?;
        if log_name.is_empty() {
            return err("", "log_name", SchemaErrorKind::Missing);
        }
        let log_provider = bounded(w.log_provider, EVENT_LOG_NAME_MAX, "", "log_provider")?;
        let action = match activity {
            W::Stop(_) => EventLogAction::Stop,
            W::Restart(_) => EventLogAction::Restart,
            W::Disable(_) => EventLogAction::Disable,
        };
        if action == EventLogAction::Disable && log_provider.is_empty() {
            return err("", "log_provider", SchemaErrorKind::Missing);
        }
        Ok(Self { actor, log_name, log_provider, status_code: w.status_code, action })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::test_support::proc_ref;

    fn activity(action: EventLogAction) -> EventLogActivity {
        EventLogActivity {
            actor: None,
            log_name: "Atlas-Sensor".into(),
            log_provider: "Microsoft-Windows-Kernel-File".into(),
            status_code: Some(4201),
            action,
        }
    }

    fn rejected(w: wire::EventLogActivity) -> (String, SchemaErrorKind) {
        let e = EventLogActivity::from_wire(w).unwrap_err();
        (e.field_path, e.kind)
    }

    #[test]
    fn every_action_round_trips_with_and_without_actor() {
        for action in [EventLogAction::Stop, EventLogAction::Restart, EventLogAction::Disable] {
            let a = activity(action);
            assert_eq!(EventLogActivity::from_wire(a.clone().into()).unwrap(), a);
            let a = EventLogActivity { actor: Some(proc_ref()), status_code: None, ..a };
            assert_eq!(EventLogActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn log_name_is_required_and_bounded() {
        let w = wire::EventLogActivity { log_name: String::new(), ..activity(EventLogAction::Stop).into() };
        assert_eq!(rejected(w), ("log_name".into(), SchemaErrorKind::Missing));
        let w = wire::EventLogActivity {
            log_name: "a".repeat(EVENT_LOG_NAME_MAX + 1),
            ..activity(EventLogAction::Stop).into()
        };
        assert_eq!(rejected(w), ("log_name".into(), SchemaErrorKind::TooLarge));
    }

    #[test]
    fn provider_is_required_only_for_disable() {
        let w = wire::EventLogActivity { log_provider: String::new(), ..activity(EventLogAction::Disable).into() };
        assert_eq!(rejected(w), ("log_provider".into(), SchemaErrorKind::Missing));
        for action in [EventLogAction::Stop, EventLogAction::Restart] {
            let a = EventLogActivity { log_provider: String::new(), ..activity(action) };
            assert_eq!(EventLogActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn a_present_actor_is_validated() {
        let mut w = wire::EventLogActivity::from(EventLogActivity {
            actor: Some(proc_ref()),
            ..activity(EventLogAction::Stop)
        });
        w.actor.as_mut().unwrap().uid.pop();
        assert_eq!(rejected(w), ("actor.process.uid".into(), SchemaErrorKind::Malformed));
    }
}
