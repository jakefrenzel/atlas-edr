//! The event envelope (spec sections 3.1, 4.1, 4.2).

use atlas_proto::v1 as wire;
use atlas_proto::v1::event::Kind as W;
use uuid::Uuid;

use crate::classes::dns::DnsActivity;
use crate::classes::file::FileSystemActivity;
use crate::classes::module::ModuleActivity;
use crate::classes::network::NetworkActivity;
use crate::classes::process::ProcessActivity;
use crate::classes::registry::{RegistryKeyActivity, RegistryValueActivity};
use crate::convert::{Result, err, fixed, require, wire_enum};
use crate::error::{SchemaError, SchemaErrorKind};
use crate::ids::{BootId, DeviceUid, EventId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub meta: EventMeta,
    pub device: Device,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMeta {
    pub event_id: EventId,
    /// When the event occurred: nanoseconds since the Unix epoch, UTC.
    pub time: i64,
    pub sensor: Sensor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {
    Etw,
    Driver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    /// Must be checked against the agent's authenticated identity by the server.
    pub uid: DeviceUid,
    pub boot_id: BootId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    Process(ProcessActivity),
    Module(ModuleActivity),
    Network(NetworkActivity),
    File(FileSystemActivity),
    RegistryKey(RegistryKeyActivity),
    RegistryValue(RegistryValueActivity),
    Dns(DnsActivity),
}

impl From<Event> for wire::Event {
    fn from(v: Event) -> Self {
        let sensor = match v.meta.sensor {
            Sensor::Etw => wire::Sensor::Etw,
            Sensor::Driver => wire::Sensor::Driver,
        };
        let kind = match v.kind {
            EventKind::Process(a) => W::Process(a.into()),
            EventKind::Module(a) => W::Module(a.into()),
            EventKind::Network(a) => W::Network(a.into()),
            EventKind::File(a) => W::File(a.into()),
            EventKind::RegistryKey(a) => W::RegistryKey(a.into()),
            EventKind::RegistryValue(a) => W::RegistryValue(a.into()),
            EventKind::Dns(a) => W::Dns(a.into()),
        };
        Self {
            event_id: v.meta.event_id.as_bytes().to_vec(),
            time: v.meta.time,
            sensor: sensor as i32,
            device: Some(wire::Device {
                uid: v.device.uid.as_bytes().to_vec(),
                boot_id: v.device.boot_id.as_bytes().to_vec(),
            }),
            kind: Some(kind),
        }
    }
}

/// The validation gate for untrusted wire data (spec section 6).
impl TryFrom<wire::Event> for Event {
    type Error = SchemaError;

    fn try_from(w: wire::Event) -> Result<Self> {
        let Some(event_id) = EventId::from_uuid(Uuid::from_bytes(fixed::<16>(w.event_id, "", "event_id")?)) else {
            return err("", "event_id", SchemaErrorKind::Malformed);
        };
        let sensor = wire_enum(
            w.sensor,
            |s: wire::Sensor| match s {
                wire::Sensor::Unspecified => None,
                wire::Sensor::Etw => Some(Sensor::Etw),
                wire::Sensor::Driver => Some(Sensor::Driver),
            },
            "",
            "sensor",
        )?;
        let device = require(w.device, "", "device")?;
        let device = Device {
            uid: DeviceUid::from_bytes(fixed::<16>(device.uid, "device", "uid")?),
            boot_id: BootId::from_bytes(fixed::<16>(device.boot_id, "device", "boot_id")?),
        };
        let kind = match require(w.kind, "", "kind")? {
            W::Process(a) => EventKind::Process(ProcessActivity::from_wire(a)?),
            W::Module(a) => EventKind::Module(ModuleActivity::from_wire(a)?),
            W::Network(a) => EventKind::Network(NetworkActivity::from_wire(a)?),
            W::File(a) => EventKind::File(FileSystemActivity::from_wire(a)?),
            W::RegistryKey(a) => EventKind::RegistryKey(RegistryKeyActivity::from_wire(a)?),
            W::RegistryValue(a) => EventKind::RegistryValue(RegistryValueActivity::from_wire(a)?),
            W::Dns(a) => EventKind::Dns(DnsActivity::from_wire(a)?),
        };
        Ok(Self { meta: EventMeta { event_id, time: w.time, sensor }, device, kind })
    }
}
