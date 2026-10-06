//! The providers the sensor consumes and how Session A enables them (sensor spec §4.2).
#![forbid(unsafe_code)]

/// A provider whose events we parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    KernelProcess,
    KernelFile,
    KernelRegistry,
    KernelNetwork,
    DnsClient,
    /// The classic kernel `Process` event class (Session B, system logger).
    ClassicProcess,
}

impl Provider {
    pub const ALL: [Provider; 6] = [
        Provider::KernelProcess,
        Provider::KernelFile,
        Provider::KernelRegistry,
        Provider::KernelNetwork,
        Provider::DnsClient,
        Provider::ClassicProcess,
    ];

    /// The provider GUID (for the classic process events, the event class GUID).
    pub const fn guid(self) -> u128 {
        match self {
            Provider::KernelProcess => 0x22fb2cd6_0e7b_422b_a0c7_2fad1fd0e716,
            Provider::KernelFile => 0xedd08927_9cc4_4e65_b970_c2560fb5c289,
            Provider::KernelRegistry => 0x70eb4f03_c1de_4f73_a051_33d13d5413bd,
            Provider::KernelNetwork => 0x7dd42a49_5329_4832_8dfd_43d979153a88,
            Provider::DnsClient => 0x1c95126e_7eea_49a9_a3fe_a378b03ddb4d,
            Provider::ClassicProcess => 0x3d6fa8d0_fe05_11d0_9dda_00c04fd7ba7c,
        }
    }

    pub fn from_guid(guid: u128) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.guid() == guid)
    }

    /// The registered provider name (for logs and Event Log Activity's `log_provider`).
    pub const fn name(self) -> &'static str {
        match self {
            Provider::KernelProcess => "Microsoft-Windows-Kernel-Process",
            Provider::KernelFile => "Microsoft-Windows-Kernel-File",
            Provider::KernelRegistry => "Microsoft-Windows-Kernel-Registry",
            Provider::KernelNetwork => "Microsoft-Windows-Kernel-Network",
            Provider::DnsClient => "Microsoft-Windows-DNS-Client",
            Provider::ClassicProcess => "Windows Kernel Trace (Process)",
        }
    }

    /// Logged from user mode, so any process can forge it (§4.4).
    pub const fn is_user_mode(self) -> bool {
        matches!(self, Provider::DnsClient)
    }
}

/// How one provider is enabled in Session A.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enable {
    pub provider: Provider,
    /// `MatchAnyKeyword`.
    pub keywords: u64,
    /// The `EVENT_FILTER_TYPE_EVENT_ID` allow-list.
    pub event_ids: Vec<u16>,
}

/// `TRACE_LEVEL_VERBOSE`: every level; the keywords and IDs do the selecting.
pub const LEVEL: u8 = 5;

/// Session A's providers, keywords and event IDs (sensor spec §4.2).
/// `udp`: the `network.udp` setting (§7.3), on by default.
pub fn session_a(udp: bool) -> Vec<Enable> {
    let mut net = vec![12, 13, 15, 28, 29, 31];
    if udp {
        net.extend([42, 43, 58, 59]);
    }
    vec![
        // WINEVENT_KEYWORD_PROCESS 0x10, WINEVENT_KEYWORD_IMAGE 0x40.
        Enable { provider: Provider::KernelProcess, keywords: 0x50, event_ids: vec![1, 2, 5] },
        // FILEIO 0x20, OP_END 0x40, CREATE 0x80, WRITE 0x200, DELETE_PATH 0x400,
        // RENAME_SETLINK_PATH 0x800, CREATE_NEW_FILE 0x1000.
        Enable {
            provider: Provider::KernelFile,
            keywords: 0x1EE0,
            event_ids: vec![12, 13, 14, 16, 17, 24, 26, 27, 30],
        },
        // CloseKey 0x1, SetValueKey 0x100, DeleteValueKey 0x200, CreateKey 0x1000,
        // OpenKey 0x2000, DeleteKey 0x4000.
        Enable { provider: Provider::KernelRegistry, keywords: 0x7301, event_ids: vec![1, 2, 3, 5, 6, 13] },
        // IPV4 0x10, IPV6 0x20.
        Enable { provider: Provider::KernelNetwork, keywords: 0x30, event_ids: net },
        // The Operational channel keyword, the only one that delivers 3008 (S3).
        Enable { provider: Provider::DnsClient, keywords: 0x8000_0000_0000_0000, event_ids: vec![3008] },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;

    #[test]
    fn guids_round_trip() {
        for p in Provider::ALL {
            assert_eq!(Provider::from_guid(p.guid()), Some(p));
        }
        assert_eq!(Provider::from_guid(0), None);
    }

    #[test]
    fn every_enabled_event_has_a_parser() {
        for e in session_a(true) {
            for id in e.event_ids {
                assert!(layout::newest(e.provider, id).is_some(), "{:?} {id}", e.provider);
            }
        }
    }

    #[test]
    fn udp_off_drops_only_the_datagram_events() {
        let on = session_a(true);
        let off = session_a(false);
        let net = |v: &[Enable]| v.iter().find(|e| e.provider == Provider::KernelNetwork).unwrap().event_ids.clone();
        assert_eq!(net(&on), [12, 13, 15, 28, 29, 31, 42, 43, 58, 59]);
        assert_eq!(net(&off), [12, 13, 15, 28, 29, 31]);
    }
}
