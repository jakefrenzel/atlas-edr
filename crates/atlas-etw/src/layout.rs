//! The payload layout of every (provider, event, version) we parse, as the
//! provider's manifest declares it: field names and TDH in-types, in order.
//!
//! The hand-written parsers in [`crate::parse`] follow these tables. Three
//! checks tie the tables to Windows:
//! - a Windows test compares each manifest entry with the installed manifest
//!   (`TdhGetManifestEventInformation`, no elevation needed);
//! - at run time, an event with a higher version than we know is accepted only
//!   if our newest layout is a prefix of its TDH layout (sensor spec §4.3);
//! - the replay fixtures compare parsed values with TDH's decoding.
//!
//! Session B's classic process events have no manifest; their layout comes
//! from the kernel's MOF class and spike S8 (§15.3), and the version check reads
//! it through TDH the same way.
#![forbid(unsafe_code)]

use crate::Provider;

/// A TDH in-type (`TDH_INTYPE_*` in tdh.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum InType {
    UnicodeString = 1,
    AnsiString = 2,
    UInt16 = 6,
    Int32 = 7,
    UInt32 = 8,
    UInt64 = 10,
    Binary = 14,
    Pointer = 16,
    FileTime = 17,
    Sid = 19,
    HexInt32 = 20,
    HexInt64 = 21,
    /// A `TOKEN_USER` followed by a SID (classic MOF events).
    WbemSid = 310,
}

impl InType {
    pub fn from_raw(v: u16) -> Option<Self> {
        use InType::*;
        [
            UnicodeString,
            AnsiString,
            UInt16,
            Int32,
            UInt32,
            UInt64,
            Binary,
            Pointer,
            FileTime,
            Sid,
            HexInt32,
            HexInt64,
            WbemSid,
        ]
        .into_iter()
        .find(|t| *t as u16 == v)
    }
}

/// One field: its manifest name and in-type.
pub type Field = (&'static str, InType);

/// The layout of one event version.
#[derive(Debug)]
pub struct Layout {
    pub provider: Provider,
    /// The event ID; for classic events (Session B), the opcode.
    pub id: u16,
    pub version: u8,
    pub fields: &'static [Field],
}

use InType::*;

const PROCESS_START_V3: &[Field] = &[
    ("ProcessID", UInt32),
    ("ProcessSequenceNumber", UInt64),
    ("CreateTime", FileTime),
    ("ParentProcessID", UInt32),
    ("ParentProcessSequenceNumber", UInt64),
    ("SessionID", UInt32),
    ("Flags", UInt32),
    ("ProcessTokenElevationType", UInt32),
    ("ProcessTokenIsElevated", UInt32),
    ("MandatoryLabel", Sid),
    ("ImageName", UnicodeString),
    ("ImageChecksum", UInt32),
    ("TimeDateStamp", UInt32),
    ("PackageFullName", UnicodeString),
    ("PackageRelativeAppId", UnicodeString),
];
const PROCESS_START_V4: &[Field] = &[
    ("ProcessID", UInt32),
    ("ProcessSequenceNumber", UInt64),
    ("CreateTime", FileTime),
    ("ParentProcessID", UInt32),
    ("ParentProcessSequenceNumber", UInt64),
    ("SessionID", UInt32),
    ("Flags", UInt32),
    ("ProcessTokenElevationType", UInt32),
    ("ProcessTokenIsElevated", UInt32),
    ("MandatoryLabel", Sid),
    ("ImageName", UnicodeString),
    ("ImageChecksum", UInt32),
    ("TimeDateStamp", UInt32),
    ("PackageFullName", UnicodeString),
    ("PackageRelativeAppId", UnicodeString),
    ("SecurityMitigations", UInt32),
];
const PROCESS_STOP_V2: &[Field] = &[
    ("ProcessID", UInt32),
    ("ProcessSequenceNumber", UInt64),
    ("CreateTime", FileTime),
    ("ExitTime", FileTime),
    ("ExitCode", UInt32),
    ("TokenElevationType", UInt32),
    ("HandleCount", UInt32),
    ("CommitCharge", UInt64),
    ("CommitPeak", UInt64),
    ("CPUCycleCount", UInt64),
    ("ReadOperationCount", UInt32),
    ("WriteOperationCount", UInt32),
    ("ReadTransferKiloBytes", UInt32),
    ("WriteTransferKiloBytes", UInt32),
    ("HardFaultCount", UInt32),
    ("ImageName", AnsiString),
];
const IMAGE_LOAD_V0: &[Field] = &[
    ("ImageBase", Pointer),
    ("ImageSize", Pointer),
    ("ProcessID", UInt32),
    ("ImageCheckSum", UInt32),
    ("TimeDateStamp", UInt32),
    ("DefaultBase", Pointer),
    ("ImageName", UnicodeString),
];

const FILE_CREATE_V1: &[Field] = &[
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("IssuingThreadId", UInt32),
    ("CreateOptions", UInt32),
    ("CreateAttributes", UInt32),
    ("ShareAccess", UInt32),
    ("FileName", UnicodeString),
];
const FILE_HANDLE_V1: &[Field] =
    &[("Irp", Pointer), ("FileObject", Pointer), ("FileKey", Pointer), ("IssuingThreadId", UInt32)];
const FILE_WRITE_V1: &[Field] = &[
    ("ByteOffset", UInt64),
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("FileKey", Pointer),
    ("IssuingThreadId", UInt32),
    ("IOSize", UInt32),
    ("IOFlags", UInt32),
    ("ExtraFlags", UInt32),
];
const FILE_SET_INFO_V1: &[Field] = &[
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("FileKey", Pointer),
    ("ExtraInformation", Pointer),
    ("IssuingThreadId", UInt32),
    ("InfoClass", UInt32),
];
const FILE_OP_END_V0: &[Field] = &[("Irp", Pointer), ("ExtraInformation", Pointer), ("Status", UInt32)];
const FILE_PATH_V1: &[Field] = &[
    ("Irp", Pointer),
    ("FileObject", Pointer),
    ("FileKey", Pointer),
    ("ExtraInformation", Pointer),
    ("IssuingThreadId", UInt32),
    ("InfoClass", UInt32),
    ("FilePath", UnicodeString),
];

const REG_OPEN_V0: &[Field] = &[
    ("BaseObject", Pointer),
    ("KeyObject", Pointer),
    ("Status", UInt32),
    ("Disposition", UInt32),
    ("BaseName", UnicodeString),
    ("RelativeName", UnicodeString),
];
const REG_KEY_V0: &[Field] = &[("KeyObject", Pointer), ("Status", UInt32), ("KeyName", UnicodeString)];
const REG_SET_VALUE_V0: &[Field] = &[
    ("KeyObject", Pointer),
    ("Status", UInt32),
    ("Type", UInt32),
    ("DataSize", UInt32),
    ("KeyName", UnicodeString),
    ("ValueName", UnicodeString),
    ("CapturedDataSize", UInt16),
    ("CapturedData", Binary),
    ("PreviousDataType", UInt32),
    ("PreviousDataSize", UInt32),
    ("PreviousDataCapturedSize", UInt16),
    ("PreviousData", Binary),
];
const REG_DELETE_VALUE_V0: &[Field] =
    &[("KeyObject", Pointer), ("Status", UInt32), ("KeyName", UnicodeString), ("ValueName", UnicodeString)];

const TCP4_OPEN: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", UInt32),
    ("saddr", UInt32),
    ("dport", UInt16),
    ("sport", UInt16),
    ("mss", UInt16),
    ("sackopt", UInt16),
    ("tsopt", UInt16),
    ("wsopt", UInt16),
    ("rcvwin", UInt32),
    ("rcvwinscale", UInt16),
    ("sndwinscale", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];
const TCP6_OPEN: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", Binary),
    ("saddr", Binary),
    ("dport", UInt16),
    ("sport", UInt16),
    ("mss", UInt16),
    ("sackopt", UInt16),
    ("tsopt", UInt16),
    ("wsopt", UInt16),
    ("rcvwin", UInt32),
    ("rcvwinscale", UInt16),
    ("sndwinscale", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];
/// TCP disconnect and all UDP datagram events (IPv4).
const NET4_SIMPLE: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", UInt32),
    ("saddr", UInt32),
    ("dport", UInt16),
    ("sport", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];
const NET6_SIMPLE: &[Field] = &[
    ("PID", UInt32),
    ("size", UInt32),
    ("daddr", Binary),
    ("saddr", Binary),
    ("dport", UInt16),
    ("sport", UInt16),
    ("seqnum", UInt32),
    ("connid", UInt32),
];

const DNS_QUERY_V0: &[Field] = &[
    ("QueryName", UnicodeString),
    ("QueryType", UInt32),
    ("QueryOptions", UInt64),
    ("QueryStatus", UInt32),
    ("QueryResults", UnicodeString),
];

/// Classic `Process` event class, version 4 (Start, End, DCStart, DCEnd).
const CLASSIC_PROCESS_V4: &[Field] = &[
    ("UniqueProcessKey", Pointer),
    ("ProcessId", UInt32),
    ("ParentId", UInt32),
    ("SessionId", UInt32),
    ("ExitStatus", Int32),
    ("DirectoryTableBase", Pointer),
    ("Flags", UInt32),
    ("UserSID", WbemSid),
    ("ImageFileName", AnsiString),
    ("CommandLine", UnicodeString),
    ("PackageFullName", UnicodeString),
    ("ApplicationId", UnicodeString),
];

const fn l(provider: Provider, id: u16, version: u8, fields: &'static [Field]) -> Layout {
    Layout { provider, id, version, fields }
}

/// Every layout we parse, ordered by (provider, id, version).
pub const LAYOUTS: &[Layout] = &[
    l(Provider::KernelProcess, 1, 3, PROCESS_START_V3),
    l(Provider::KernelProcess, 1, 4, PROCESS_START_V4),
    l(Provider::KernelProcess, 2, 2, PROCESS_STOP_V2),
    l(Provider::KernelProcess, 5, 0, IMAGE_LOAD_V0),
    l(Provider::KernelFile, 12, 1, FILE_CREATE_V1),
    l(Provider::KernelFile, 13, 1, FILE_HANDLE_V1),
    l(Provider::KernelFile, 14, 1, FILE_HANDLE_V1),
    l(Provider::KernelFile, 16, 1, FILE_WRITE_V1),
    l(Provider::KernelFile, 17, 1, FILE_SET_INFO_V1),
    l(Provider::KernelFile, 18, 1, FILE_SET_INFO_V1),
    l(Provider::KernelFile, 24, 0, FILE_OP_END_V0),
    l(Provider::KernelFile, 26, 1, FILE_PATH_V1),
    l(Provider::KernelFile, 27, 1, FILE_PATH_V1),
    l(Provider::KernelFile, 30, 1, FILE_CREATE_V1),
    l(Provider::KernelRegistry, 1, 0, REG_OPEN_V0),
    l(Provider::KernelRegistry, 2, 0, REG_OPEN_V0),
    l(Provider::KernelRegistry, 3, 0, REG_KEY_V0),
    l(Provider::KernelRegistry, 5, 0, REG_SET_VALUE_V0),
    l(Provider::KernelRegistry, 6, 0, REG_DELETE_VALUE_V0),
    l(Provider::KernelRegistry, 13, 0, REG_KEY_V0),
    l(Provider::KernelNetwork, 12, 0, TCP4_OPEN),
    l(Provider::KernelNetwork, 13, 0, NET4_SIMPLE),
    l(Provider::KernelNetwork, 15, 0, TCP4_OPEN),
    l(Provider::KernelNetwork, 28, 0, TCP6_OPEN),
    l(Provider::KernelNetwork, 29, 0, NET6_SIMPLE),
    l(Provider::KernelNetwork, 31, 0, TCP6_OPEN),
    l(Provider::KernelNetwork, 42, 0, NET4_SIMPLE),
    l(Provider::KernelNetwork, 43, 0, NET4_SIMPLE),
    l(Provider::KernelNetwork, 58, 0, NET6_SIMPLE),
    l(Provider::KernelNetwork, 59, 0, NET6_SIMPLE),
    l(Provider::DnsClient, 3008, 0, DNS_QUERY_V0),
    l(Provider::ClassicProcess, 1, 4, CLASSIC_PROCESS_V4),
    l(Provider::ClassicProcess, 2, 4, CLASSIC_PROCESS_V4),
    l(Provider::ClassicProcess, 3, 4, CLASSIC_PROCESS_V4),
    l(Provider::ClassicProcess, 4, 4, CLASSIC_PROCESS_V4),
];

/// The layout of exactly this version, if we parse it.
pub fn find(provider: Provider, id: u16, version: u8) -> Option<&'static Layout> {
    LAYOUTS.iter().find(|l| l.provider == provider && l.id == id && l.version == version)
}

/// The newest version of this event that we parse.
pub fn newest(provider: Provider, id: u16) -> Option<&'static Layout> {
    LAYOUTS.iter().filter(|l| l.provider == provider && l.id == id).max_by_key(|l| l.version)
}

/// Sensor spec §4.3: a newer version is parsed with our newest layout only if
/// that layout is a prefix of the newer one (same names and in-types, in order).
pub fn is_prefix(known: &[Field], newer: &[(String, u16)]) -> bool {
    known.len() <= newer.len() && known.iter().zip(newer).all(|((n, t), (m, u))| *n == m && *t as u16 == *u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_sorted_and_unique() {
        let keys: Vec<_> = LAYOUTS.iter().map(|l| (l.provider as u8, l.id, l.version)).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn newest_picks_the_highest_version() {
        assert_eq!(newest(Provider::KernelProcess, 1).map(|l| l.version), Some(4));
        assert!(newest(Provider::KernelProcess, 99).is_none());
    }

    #[test]
    fn v4_process_start_extends_v3() {
        let v4: Vec<_> = PROCESS_START_V4.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        assert!(is_prefix(PROCESS_START_V3, &v4));
        assert!(!is_prefix(PROCESS_START_V4, &v4[..v4.len() - 1]));
    }

    #[test]
    fn a_renamed_or_retyped_field_is_not_a_prefix() {
        let mut newer: Vec<_> = REG_KEY_V0.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
        newer.push(("Extra".into(), UInt32 as u16));
        assert!(is_prefix(REG_KEY_V0, &newer));
        newer[1].0 = "NtStatus".into();
        assert!(!is_prefix(REG_KEY_V0, &newer));
        newer[1] = ("Status".into(), HexInt32 as u16);
        assert!(!is_prefix(REG_KEY_V0, &newer));
    }
}
