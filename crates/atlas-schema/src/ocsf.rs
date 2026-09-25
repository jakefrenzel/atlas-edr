//! OCSF 1.9.0 numeric ids, derived from the domain types (spec section 5.0).
//! They are never stored, so they cannot disagree with the event.

use crate::classes::dns::DnsAction;
use crate::classes::file::FileAction;
use crate::classes::module::ModuleAction;
use crate::classes::network::NetworkAction;
use crate::classes::process::ProcessActivity;
use crate::classes::registry::{RegistryKeyAction, RegistryValueAction};
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
        };
        OcsfIds { category_uid, class_uid, activity_id }
    }
}
