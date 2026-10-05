//! OCSF 1.9.0 numeric ids, derived from the domain types (spec section 5.0).
//! They are never stored, so they cannot disagree with the event.

use crate::classes::dns::DnsAction;
use crate::classes::event_log::EventLogAction;
use crate::classes::file::FileAction;
use crate::classes::module::ModuleAction;
use crate::classes::network::NetworkAction;
use crate::classes::process::ProcessActivity;
use crate::classes::registry::{RegistryKeyAction, RegistryValueAction};
use crate::classes::sensor_health::SensorHealthAction;
use crate::event::EventKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcsfIds {
    pub category_uid: u32,
    pub class_uid: u32,
    pub activity_id: u32,
}

impl OcsfIds {
    /// `class_uid * 100 + activity_id`, as OCSF requires.
    pub const fn type_uid(&self) -> u64 {
        self.class_uid as u64 * 100 + self.activity_id as u64
    }
}

const SYSTEM: u32 = 1;
const NETWORK: u32 = 4;
const APPLICATION: u32 = 6;

/// Atlas's OCSF extension uid. Outside OCSF's registered extensions (1–3 and
/// 985–999 in the registry at github.com/ocsf/ocsf-schema `extensions.md`, 2026-10-04).
pub const ATLAS_EXTENSION_UID: u32 = 500;

/// Sensor Health: `extension_uid × 100000 + category_uid × 1000 + 1` (OCSF's extension class rule).
pub const SENSOR_HEALTH_CLASS_UID: u32 = ATLAS_EXTENSION_UID * 100_000 + APPLICATION * 1000 + 1;

impl EventKind {
    pub fn ocsf_ids(&self) -> OcsfIds {
        let (category_uid, class_uid, activity_id) = match self {
            EventKind::Process(a) => (
                SYSTEM,
                1007,
                match a {
                    ProcessActivity::Launch { .. } => 1,
                    ProcessActivity::Terminate { .. } => 2,
                },
            ),
            EventKind::Module(a) => (
                SYSTEM,
                1005,
                match a.action {
                    ModuleAction::Load { .. } => 1,
                },
            ),
            EventKind::Network(a) => (
                NETWORK,
                4001,
                match a.action {
                    NetworkAction::Open => 1,
                    NetworkAction::Close { .. } => 2,
                },
            ),
            EventKind::File(a) => (
                SYSTEM,
                1001,
                match a.action {
                    FileAction::Create => 1,
                    FileAction::Read => 2,
                    FileAction::Update => 3,
                    FileAction::Delete => 4,
                    FileAction::Rename { .. } => 5,
                    FileAction::SetAttributes => 6,
                    FileAction::Open => 14,
                },
            ),
            EventKind::RegistryKey(a) => (
                SYSTEM,
                201001,
                match a.action {
                    RegistryKeyAction::Create => 1,
                    RegistryKeyAction::Delete => 4,
                    RegistryKeyAction::Rename { .. } => 5,
                },
            ),
            EventKind::RegistryValue(a) => (
                SYSTEM,
                201002,
                match a.action {
                    RegistryValueAction::Set { .. } => 2,
                    RegistryValueAction::Delete => 4,
                },
            ),
            EventKind::Dns(a) => (
                NETWORK,
                4003,
                match a.action {
                    DnsAction::Response { .. } => 2,
                },
            ),
            EventKind::EventLog(a) => (
                SYSTEM,
                1008,
                match a.action {
                    EventLogAction::Stop => 7,
                    EventLogAction::Restart => 8,
                    EventLogAction::Disable => 10,
                },
            ),
            EventKind::SensorHealth(a) => (
                APPLICATION,
                SENSOR_HEALTH_CLASS_UID,
                match a.action {
                    SensorHealthAction::Report(_) => 1,
                },
            ),
        };
        OcsfIds { category_uid, class_uid, activity_id }
    }
}
