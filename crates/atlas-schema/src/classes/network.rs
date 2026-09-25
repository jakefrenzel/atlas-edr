//! Network Activity (OCSF 4001), spec section 5.4.

use atlas_proto::v1 as wire;
use atlas_proto::v1::network_activity::Activity as W;

use crate::convert::{Result, require, wire_enum};
use crate::objects::{NetworkEndpoint, ProcessRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkActivity {
    pub actor: ProcessRef,
    pub src_endpoint: NetworkEndpoint,
    pub dst_endpoint: NetworkEndpoint,
    pub protocol: NetworkProtocol,
    pub direction: NetworkDirection,
    pub action: NetworkAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAction {
    Open,
    Close { bytes_in: Option<u64>, bytes_out: Option<u64> },
}

impl From<NetworkActivity> for wire::NetworkActivity {
    fn from(v: NetworkActivity) -> Self {
        let protocol = match v.protocol {
            NetworkProtocol::Tcp => wire::NetworkProtocol::Tcp,
            NetworkProtocol::Udp => wire::NetworkProtocol::Udp,
        };
        let direction = match v.direction {
            NetworkDirection::Inbound => wire::NetworkDirection::Inbound,
            NetworkDirection::Outbound => wire::NetworkDirection::Outbound,
        };
        let activity = match v.action {
            NetworkAction::Open => W::Open(wire::NetworkOpen {}),
            NetworkAction::Close { bytes_in, bytes_out } => W::Close(wire::NetworkClose { bytes_in, bytes_out }),
        };
        Self {
            actor: Some(v.actor.into()),
            src_endpoint: Some(v.src_endpoint.into()),
            dst_endpoint: Some(v.dst_endpoint.into()),
            protocol: protocol as i32,
            direction: direction as i32,
            activity: Some(activity),
        }
    }
}

impl NetworkActivity {
    pub(crate) fn from_wire(w: wire::NetworkActivity) -> Result<Self> {
        Ok(Self {
            actor: ProcessRef::required(w.actor, "", "actor.process")?,
            src_endpoint: NetworkEndpoint::required(w.src_endpoint, "", "src_endpoint")?,
            dst_endpoint: NetworkEndpoint::required(w.dst_endpoint, "", "dst_endpoint")?,
            protocol: wire_enum(
                w.protocol,
                |p: wire::NetworkProtocol| match p {
                    wire::NetworkProtocol::Unspecified => None,
                    wire::NetworkProtocol::Tcp => Some(NetworkProtocol::Tcp),
                    wire::NetworkProtocol::Udp => Some(NetworkProtocol::Udp),
                },
                "",
                "protocol",
            )?,
            direction: wire_enum(
                w.direction,
                |d: wire::NetworkDirection| match d {
                    wire::NetworkDirection::Unspecified => None,
                    wire::NetworkDirection::Inbound => Some(NetworkDirection::Inbound),
                    wire::NetworkDirection::Outbound => Some(NetworkDirection::Outbound),
                },
                "",
                "direction",
            )?,
            action: match require(w.activity, "", "activity")? {
                W::Open(_) => NetworkAction::Open,
                W::Close(c) => NetworkAction::Close { bytes_in: c.bytes_in, bytes_out: c.bytes_out },
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::error::SchemaErrorKind;
    use crate::objects::test_support::proc_ref;

    fn activity(action: NetworkAction) -> NetworkActivity {
        let ep = |port| NetworkEndpoint { ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), port };
        NetworkActivity {
            actor: proc_ref(),
            src_endpoint: ep(50000),
            dst_endpoint: ep(443),
            protocol: NetworkProtocol::Udp,
            direction: NetworkDirection::Inbound,
            action,
        }
    }

    #[test]
    fn open_and_close_round_trip() {
        for a in [activity(NetworkAction::Open), activity(NetworkAction::Close { bytes_in: Some(1), bytes_out: None })]
        {
            assert_eq!(NetworkActivity::from_wire(a.clone().into()).unwrap(), a);
        }
    }

    #[test]
    fn unspecified_protocol_is_missing_and_unknown_direction_is_unknown() {
        let w = wire::NetworkActivity { protocol: 0, ..activity(NetworkAction::Open).into() };
        let e = NetworkActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("protocol", SchemaErrorKind::Missing));

        let w = wire::NetworkActivity { direction: 9, ..activity(NetworkAction::Open).into() };
        let e = NetworkActivity::from_wire(w).unwrap_err();
        assert_eq!((e.field_path.as_str(), e.kind), ("direction", SchemaErrorKind::UnknownEnum));
    }
}
