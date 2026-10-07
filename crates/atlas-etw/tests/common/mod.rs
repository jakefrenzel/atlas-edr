//! Shared by the replay and live tests: the fixture line format and the TDH
//! oracle comparison (sensor spec §4.3, §12.2).
#![allow(dead_code)] // each test binary uses a different part

use atlas_etw::Provider;
use atlas_etw::parse::{EventMeta, PointerSize, RawEvent, WStr};
use serde_json::{Map, Value};
use std::net::IpAddr;

/// `EVENT_HEADER_FLAG_32_BIT_HEADER`.
pub const FLAG_32_BIT: u16 = 0x0020;

/// Short provider names used in fixture lines (the spike probe's names, so its
/// recordings replay too).
pub fn short_name(p: Provider) -> &'static str {
    match p {
        Provider::KernelProcess => "kernel-process",
        Provider::KernelFile => "kernel-file",
        Provider::KernelRegistry => "kernel-registry",
        Provider::KernelNetwork => "kernel-network",
        Provider::DnsClient => "dns-client",
        Provider::ClassicProcess => "process-classic",
    }
}

pub fn from_short_name(s: &str) -> Option<Provider> {
    Provider::ALL.into_iter().find(|p| short_name(*p) == s)
}

/// One fixture line, as written by the recorder (`tests/live.rs`).
#[derive(Debug)]
pub struct Fixture {
    pub meta: EventMeta,
    pub pid: u32,
    pub tid: u32,
    pub timestamp: i64,
    pub start_key: Option<u64>,
    pub payload: Vec<u8>,
    /// TDH's decoding on the machine that recorded the event.
    pub tdh: Map<String, Value>,
}

fn hex_u64(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
}

pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Fixture {
    /// Reads one line. `None` for lines that are not parseable events (no raw
    /// payload, a provider or event we don't parse).
    pub fn from_json(line: &str) -> Option<Fixture> {
        let v: Value = serde_json::from_str(line).ok()?;
        let provider = from_short_name(v["provider"].as_str()?)?;
        let flags = u16::try_from(hex_u64(v["flags"].as_str()?)?).ok()?;
        // Classic events have ID 0 and are told apart by opcode.
        let id = if provider == Provider::ClassicProcess { v["opcode"].as_u64()? } else { v["id"].as_u64()? };
        let meta = EventMeta {
            provider,
            id: u16::try_from(id).ok()?,
            version: u8::try_from(v["version"].as_u64()?).ok()?,
            pointer_size: if flags & FLAG_32_BIT != 0 { PointerSize::P32 } else { PointerSize::P64 },
        };
        Some(Fixture {
            meta,
            pid: u32::try_from(v["pid"].as_u64()?).ok()?,
            tid: u32::try_from(v["tid"].as_u64()?).ok()?,
            timestamp: v["ts"].as_i64()?,
            start_key: v["start_key"].as_str().and_then(hex_u64),
            payload: decode_hex(v["raw"].as_str()?)?,
            tdh: v["fields"].as_object()?.clone(),
        })
    }
}

/// How one parsed field is compared with TDH's text.
#[derive(Debug)]
pub enum Expect {
    /// An integer; TDH prints it in decimal or `0x` hex depending on its out-type.
    Num(u64),
    /// A signed integer, printed in decimal.
    Signed(i64),
    Str(String),
    /// A FILETIME; TDH prints an ISO 8601 UTC time (with direction marks).
    Time(u64),
    Addr(IpAddr),
    /// A binary blob; TDH prints `0x` + uppercase hex, or nothing when empty.
    Bytes(Vec<u8>),
    /// Not compared here: a field we skip over, or one TDH renders in a form
    /// that cannot be checked offline (a SID shown as an account name).
    Skip,
}

fn s(w: &WStr) -> Expect {
    Expect::Str(w.to_string_lossy())
}

fn a(b: &[u8]) -> Expect {
    Expect::Str(String::from_utf8_lossy(b).into_owned())
}

/// Every TDH field of the event, with what our parser produced for it.
pub fn expected_fields(e: &RawEvent) -> Vec<(&'static str, Expect)> {
    use Expect::*;
    match e {
        RawEvent::ProcessStart(p) => {
            let mut v = vec![
                ("ProcessID", Num(p.pid.into())),
                ("ProcessSequenceNumber", Num(p.sequence_number)),
                ("CreateTime", Time(p.create_time)),
                ("ParentProcessID", Num(p.parent_pid.into())),
                ("ParentProcessSequenceNumber", Num(p.parent_sequence_number)),
                ("SessionID", Num(p.session_id.into())),
                ("Flags", Num(p.flags.into())),
                ("ProcessTokenElevationType", Num(p.token_elevation_type.into())),
                ("ProcessTokenIsElevated", Num(p.token_is_elevated.into())),
                ("MandatoryLabel", Str(p.mandatory_label.to_string())),
                ("ImageName", s(&p.image_name)),
                ("ImageChecksum", Num(p.image_checksum.into())),
                ("TimeDateStamp", Num(p.time_date_stamp.into())),
                ("PackageFullName", s(&p.package_full_name)),
                ("PackageRelativeAppId", s(&p.package_relative_app_id)),
            ];
            if let Some(m) = p.security_mitigations {
                v.push(("SecurityMitigations", Num(m.into())));
            }
            v
        }
        RawEvent::ProcessStop(p) => {
            let mut v = vec![
                ("ProcessID", Num(p.pid.into())),
                ("ProcessSequenceNumber", Num(p.sequence_number)),
                ("CreateTime", Time(p.create_time)),
                ("ExitTime", Time(p.exit_time)),
                ("ExitCode", Num(p.exit_code.into())),
                ("ImageName", a(&p.image_name)),
            ];
            for skipped in [
                "TokenElevationType",
                "HandleCount",
                "CommitCharge",
                "CommitPeak",
                "CPUCycleCount",
                "ReadOperationCount",
                "WriteOperationCount",
                "ReadTransferKiloBytes",
                "WriteTransferKiloBytes",
                "HardFaultCount",
            ] {
                v.push((skipped, Skip));
            }
            v
        }
        RawEvent::ImageLoad(i) => vec![
            ("ImageBase", Num(i.image_base)),
            ("ImageSize", Num(i.image_size)),
            ("ProcessID", Num(i.pid.into())),
            ("ImageCheckSum", Num(i.image_checksum.into())),
            ("TimeDateStamp", Num(i.time_date_stamp.into())),
            ("DefaultBase", Num(i.default_base)),
            ("ImageName", s(&i.image_name)),
        ],
        RawEvent::FileCreate(c) | RawEvent::FileCreateNew(c) => vec![
            ("Irp", Num(c.irp)),
            ("FileObject", Num(c.file_object)),
            ("IssuingThreadId", Num(c.issuing_tid.into())),
            ("CreateOptions", Num(c.create_options.into())),
            ("CreateAttributes", Num(c.create_attributes.into())),
            ("ShareAccess", Num(c.share_access.into())),
            ("FileName", s(&c.file_name)),
        ],
        RawEvent::FileCleanup(h) | RawEvent::FileClose(h) => vec![
            ("Irp", Num(h.irp)),
            ("FileObject", Num(h.file_object)),
            ("FileKey", Num(h.file_key)),
            ("IssuingThreadId", Num(h.issuing_tid.into())),
        ],
        RawEvent::FileWrite(w) => vec![
            ("ByteOffset", Num(w.byte_offset)),
            ("Irp", Num(w.irp)),
            ("FileObject", Num(w.file_object)),
            ("FileKey", Num(w.file_key)),
            ("IssuingThreadId", Num(w.issuing_tid.into())),
            ("IOSize", Num(w.io_size.into())),
            ("IOFlags", Num(w.io_flags.into())),
            ("ExtraFlags", Num(w.extra_flags.into())),
        ],
        RawEvent::FileSetInfo(i) | RawEvent::FileSetDelete(i) => vec![
            ("Irp", Num(i.irp)),
            ("FileObject", Num(i.file_object)),
            ("FileKey", Num(i.file_key)),
            ("ExtraInformation", Num(i.extra_information)),
            ("IssuingThreadId", Num(i.issuing_tid.into())),
            ("InfoClass", Num(i.info_class.into())),
        ],
        RawEvent::FileOpEnd(o) => {
            vec![("Irp", Num(o.irp)), ("ExtraInformation", Num(o.extra_information)), ("Status", Num(o.status.into()))]
        }
        RawEvent::FileDeletePath(p) | RawEvent::FileRenamePath(p) => vec![
            ("Irp", Num(p.irp)),
            ("FileObject", Num(p.file_object)),
            ("FileKey", Num(p.file_key)),
            ("ExtraInformation", Num(p.extra_information)),
            ("IssuingThreadId", Num(p.issuing_tid.into())),
            ("InfoClass", Num(p.info_class.into())),
            ("FilePath", s(&p.file_path)),
        ],
        RawEvent::RegCreateKey(o) | RawEvent::RegOpenKey(o) => vec![
            ("BaseObject", Num(o.base_object)),
            ("KeyObject", Num(o.key_object)),
            ("Status", Num(o.status.into())),
            ("Disposition", Num(o.disposition.into())),
            ("BaseName", s(&o.base_name)),
            ("RelativeName", s(&o.relative_name)),
        ],
        RawEvent::RegDeleteKey(k) | RawEvent::RegCloseKey(k) => {
            vec![("KeyObject", Num(k.key_object)), ("Status", Num(k.status.into())), ("KeyName", s(&k.key_name))]
        }
        RawEvent::RegSetValue(v) => vec![
            ("KeyObject", Num(v.key_object)),
            ("Status", Num(v.status.into())),
            ("Type", Num(v.value_type.into())),
            ("DataSize", Num(v.data_size.into())),
            ("KeyName", s(&v.key_name)),
            ("ValueName", s(&v.value_name)),
            ("CapturedDataSize", Num(v.captured_data.len() as u64)),
            ("CapturedData", Bytes(v.captured_data.to_vec())),
            ("PreviousDataType", Num(v.previous_data_type.into())),
            ("PreviousDataSize", Num(v.previous_data_size.into())),
            ("PreviousDataCapturedSize", Num(v.previous_data.len() as u64)),
            ("PreviousData", Bytes(v.previous_data.to_vec())),
        ],
        RawEvent::RegDeleteValue(v) => vec![
            ("KeyObject", Num(v.key_object)),
            ("Status", Num(v.status.into())),
            ("KeyName", s(&v.key_name)),
            ("ValueName", s(&v.value_name)),
        ],
        RawEvent::TcpConnect(n)
        | RawEvent::TcpAccept(n)
        | RawEvent::TcpDisconnect(n)
        | RawEvent::UdpSend(n)
        | RawEvent::UdpRecv(n) => {
            let mut v = vec![
                ("PID", Num(n.pid.into())),
                ("size", Num(n.size.into())),
                ("daddr", Addr(n.daddr)),
                ("saddr", Addr(n.saddr)),
                ("dport", Num(n.dport.into())),
                ("sport", Num(n.sport.into())),
                ("seqnum", Num(n.seqnum.into())),
                ("connid", Num(n.connid.into())),
            ];
            if matches!(e, RawEvent::TcpConnect(_) | RawEvent::TcpAccept(_)) {
                for skipped in ["mss", "sackopt", "tsopt", "wsopt", "rcvwin", "rcvwinscale", "sndwinscale"] {
                    v.push((skipped, Skip));
                }
            }
            v
        }
        RawEvent::DnsQuery(q) => vec![
            ("QueryName", s(&q.query_name)),
            ("QueryType", Num(q.query_type.into())),
            ("QueryOptions", Num(q.query_options)),
            ("QueryStatus", Num(q.query_status.into())),
            ("QueryResults", s(&q.query_results)),
        ],
        RawEvent::ClassicProcess(c) => vec![
            ("UniqueProcessKey", Num(c.unique_process_key)),
            ("ProcessId", Num(c.pid.into())),
            ("ParentId", Num(c.parent_pid.into())),
            ("SessionId", Num(c.session_id.into())),
            ("ExitStatus", Signed(c.exit_status.into())),
            ("DirectoryTableBase", Num(c.directory_table_base)),
            ("Flags", Num(c.flags.into())),
            // TDH shows the account name; the live test checks it via LookupAccountSid.
            ("UserSID", Skip),
            ("ImageFileName", a(&c.image_file_name)),
            ("CommandLine", s(&c.command_line)),
            ("PackageFullName", s(&c.package_full_name)),
            ("ApplicationId", s(&c.application_id)),
        ],
    }
}

/// TDH's text for an integer: decimal, or `0x` + hex.
fn tdh_num(t: &str) -> Option<u64> {
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => t.parse().ok(),
    }
}

/// FILETIME → `YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`, the form TDH prints (minus its
/// left-to-right marks).
pub fn filetime_iso(ft: u64) -> String {
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = (ft % 10_000_000) * 100;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{nanos:09}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Compares a parsed event with TDH's decoding. Every TDH field must be
/// accounted for, so a renamed or extra field is caught too. Returns the
/// mismatches, empty when the event agrees.
pub fn compare(e: &RawEvent, tdh: &Map<String, Value>) -> Vec<String> {
    let ours = expected_fields(e);
    let mut errs = Vec::new();
    for name in tdh.keys() {
        if !ours.iter().any(|(n, _)| n == name) {
            errs.push(format!("{name}: in TDH's decoding but not in ours"));
        }
    }
    for (name, want) in &ours {
        let Some(got) = tdh.get(*name).and_then(Value::as_str) else {
            errs.push(format!("{name}: missing from TDH's decoding"));
            continue;
        };
        let ok = match want {
            Expect::Num(n) => tdh_num(got) == Some(*n),
            Expect::Signed(n) => got.parse::<i64>().ok() == Some(*n),
            // TDH stops a string at its first NUL; ours keeps a counted registry
            // name whole (embedded NULs, sensor spec §7.5).
            Expect::Str(s) => got == s || (s.contains('\0') && s.split('\0').next() == Some(got)),
            Expect::Time(ft) => got.replace('\u{200e}', "") == filetime_iso(*ft),
            Expect::Addr(a) => got.parse::<IpAddr>().ok() == Some(*a),
            Expect::Bytes(b) if b.is_empty() => got.is_empty(),
            Expect::Bytes(b) => got.strip_prefix("0x").map(str::to_ascii_lowercase) == Some(encode_hex(b)),
            Expect::Skip => true,
        };
        if !ok {
            errs.push(format!("{name}: TDH {got:?}, ours {want:?}"));
        }
    }
    errs
}
