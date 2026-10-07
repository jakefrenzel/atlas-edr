//! Pure event parsers: an event's identity and payload bytes in, a typed
//! [`RawEvent`] out (sensor spec §4.3). No `unsafe`, no Windows dependency, so
//! the parsers are unit-tested and fuzzed on Linux.
//!
//! Each parser follows the matching table in [`crate::layout`]. Pointer-sized
//! fields take their size from the event's own header ([`EventMeta::pointer_size`]).
//! Strings stay UTF-16 ([`WStr`]) until the agent emits an event.
//! A payload longer than the layout is accepted (trailing bytes are ignored);
//! one shorter than the layout is an error.
#![forbid(unsafe_code)]

mod dns;
mod reader;

use reader::Reader;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub use dns::{DnsAnswer, DnsAnswers, parse_query_results};
pub use reader::Sid;

use crate::{Provider, layout};

/// Pointer size of the process that logged the event (from its header flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerSize {
    P32,
    P64,
}

/// What the event header says about an event, all a parser needs besides the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventMeta {
    pub provider: Provider,
    /// The event ID; for classic events (Session B), the opcode.
    pub id: u16,
    pub version: u8,
    pub pointer_size: PointerSize,
}

/// Why a payload did not parse. Parsers never panic; the agent counts these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Not an event we consume.
    UnknownEvent,
    /// A version we have no layout for and that is not newer than our newest:
    /// counted as `unknown_version` and dropped.
    UnsupportedVersion { version: u8 },
    /// A version newer than our newest layout. The session layer decides, once
    /// per (provider, event, version), whether to parse it with `newest` via
    /// [`parse_as`] (sensor spec §4.3).
    NewerVersion { version: u8, newest: u8 },
    /// The payload ended inside a field.
    Truncated { field: &'static str, offset: usize },
    /// A string field without its NUL terminator.
    Unterminated { field: &'static str, offset: usize },
    /// A field whose content is impossible (for example a SID with a bad revision).
    Malformed { field: &'static str, offset: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownEvent => write!(f, "not an event we parse"),
            ParseError::UnsupportedVersion { version } => write!(f, "unsupported version {version}"),
            ParseError::NewerVersion { version, newest } => write!(f, "version {version} is newer than {newest}"),
            ParseError::Truncated { field, offset } => write!(f, "{field}: payload ends at offset {offset}"),
            ParseError::Unterminated { field, offset } => write!(f, "{field}: no terminator after offset {offset}"),
            ParseError::Malformed { field, offset } => write!(f, "{field}: malformed at offset {offset}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// A UTF-16 string as logged, without its terminator. Converted to UTF-8 only
/// when an event is emitted (lossily: unpaired surrogates become U+FFFD, 0a §6.1).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct WStr(Box<[u16]>);

impl WStr {
    pub(crate) fn from_le_bytes(b: &[u8]) -> Self {
        WStr(b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
    }

    pub fn from_units(units: &[u16]) -> Self {
        WStr(units.into())
    }

    pub fn as_units(&self) -> &[u16] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
}

impl std::fmt::Debug for WStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_string_lossy())
    }
}

impl From<&str> for WStr {
    fn from(s: &str) -> Self {
        WStr(s.encode_utf16().collect())
    }
}

/// Kernel-Process 1 (v3+): a new process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStart {
    pub pid: u32,
    pub sequence_number: u64,
    /// FILETIME (100 ns since 1601, UTC).
    pub create_time: u64,
    pub parent_pid: u32,
    pub parent_sequence_number: u64,
    pub session_id: u32,
    pub flags: u32,
    pub token_elevation_type: u32,
    pub token_is_elevated: u32,
    /// `S-1-16-X`, the integrity level (sensor spec §5.2).
    pub mandatory_label: Sid,
    /// NT path of the image.
    pub image_name: WStr,
    pub image_checksum: u32,
    pub time_date_stamp: u32,
    pub package_full_name: WStr,
    pub package_relative_app_id: WStr,
    /// v4 and later.
    pub security_mitigations: Option<u32>,
}

/// Kernel-Process 2 (v2+): a process exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStop {
    pub pid: u32,
    pub sequence_number: u64,
    pub create_time: u64,
    pub exit_time: u64,
    pub exit_code: u32,
    /// The image's file name, 8-bit (manifest `win:AnsiString`).
    pub image_name: Box<[u8]>,
}

/// Kernel-Process 5: an image mapped into a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLoad {
    pub image_base: u64,
    pub image_size: u64,
    /// The process the image was mapped into (authoritative, §5.3).
    pub pid: u32,
    pub image_checksum: u32,
    pub time_date_stamp: u32,
    pub default_base: u64,
    pub image_name: WStr,
}

/// Kernel-File 12 `Create` and 30 `CreateNewFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCreate {
    pub irp: u64,
    pub file_object: u64,
    pub issuing_tid: u32,
    /// `CreateOptions` from `NtCreateFile`, with the create disposition in the high byte.
    pub create_options: u32,
    pub create_attributes: u32,
    pub share_access: u32,
    /// NT path as opened (may contain 8.3 components, §7.2).
    pub file_name: WStr,
}

impl FileCreate {
    /// `FILE_DELETE_ON_CLOSE` (sensor spec §7.1).
    pub const DELETE_ON_CLOSE: u32 = 0x1000;

    pub fn delete_on_close(&self) -> bool {
        self.create_options & Self::DELETE_ON_CLOSE != 0
    }
}

/// Kernel-File 13 `Cleanup` and 14 `Close`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHandle {
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    pub issuing_tid: u32,
}

/// Kernel-File 16 `Write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub byte_offset: u64,
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    pub issuing_tid: u32,
    pub io_size: u32,
    pub io_flags: u32,
    pub extra_flags: u32,
}

/// Kernel-File 17 `SetInformation` and 18 `SetDelete`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSetInfo {
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    /// For 18, 1 when the delete disposition is set and 0 when it is cleared
    /// (plan 1b-3c, verified on the host for both disposition classes).
    pub extra_information: u64,
    pub issuing_tid: u32,
    /// `FILE_INFORMATION_CLASS`: 4 basic (timestamps, attributes), 19 end of file;
    /// for 18, 13 `FileDispositionInformation` or 64 `FileDispositionInformationEx`.
    pub info_class: u32,
}

/// Kernel-File 24 `OperationEnd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpEnd {
    pub irp: u64,
    pub extra_information: u64,
    /// The operation's NTSTATUS.
    pub status: u32,
}

impl FileOpEnd {
    /// The status has **error** severity (its top two bits are set; sensor spec
    /// §5.5). Success, informational (`STATUS_REPARSE`) and warning
    /// (`STATUS_BUFFER_OVERFLOW`) codes are not failures. This is narrower than
    /// `!NT_SUCCESS`, which also counts warnings.
    pub fn failed(&self) -> bool {
        self.status >> 30 == 3
    }
}

/// Kernel-File 26 `DeletePath` and 27 `RenamePath`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePath {
    pub irp: u64,
    pub file_object: u64,
    pub file_key: u64,
    pub extra_information: u64,
    pub issuing_tid: u32,
    pub info_class: u32,
    /// For a delete, the file's path; for a rename, the **new** path (S6).
    pub file_path: WStr,
}

/// Kernel-Registry 1 `CreateKey` and 2 `OpenKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegOpen {
    pub base_object: u64,
    pub key_object: u64,
    pub status: u32,
    /// CreateKey: 1 created, 2 opened.
    pub disposition: u32,
    /// Always empty in practice (S7).
    pub base_name: WStr,
    /// Relative to `base_object` unless it starts with `\REGISTRY\`.
    pub relative_name: WStr,
}

/// Kernel-Registry 3 `DeleteKey` and 13 `CloseKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegKey {
    pub key_object: u64,
    pub status: u32,
    /// Always empty in practice (S7).
    pub key_name: WStr,
}

/// Kernel-Registry 5 `SetValueKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegSetValue {
    pub key_object: u64,
    pub status: u32,
    pub value_type: u32,
    pub data_size: u32,
    pub key_name: WStr,
    /// Counted: it may contain NULs (sensor spec §7.5).
    pub value_name: WStr,
    /// More than one terminator fitted (only possible if the captured buffers
    /// are not empty, which S4 never saw); `value_name` used the last one. The
    /// agent should treat the name, and a value read by it, as unreliable.
    pub value_name_ambiguous: bool,
    /// Never filled in practice (S4).
    pub captured_data: Box<[u8]>,
    pub previous_data_type: u32,
    pub previous_data_size: u32,
    pub previous_data: Box<[u8]>,
}

/// Kernel-Registry 6 `DeleteValueKey`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegDeleteValue {
    pub key_object: u64,
    pub status: u32,
    pub key_name: WStr,
    pub value_name: WStr,
}

/// A Kernel-Network event. Field names follow the manifest; the live tests pin
/// which end each address is (sensor spec §7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetEvent {
    /// The owning process (the header PID is not the owner, §5.3).
    pub pid: u32,
    /// Not a byte total (S5).
    pub size: u32,
    pub daddr: IpAddr,
    pub saddr: IpAddr,
    pub dport: u16,
    pub sport: u16,
    pub seqnum: u32,
    pub connid: u32,
}

/// DNS-Client 3008: a query completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsQuery {
    pub query_name: WStr,
    pub query_type: u32,
    pub query_options: u64,
    /// Win32 / DNS status; 0 on success.
    pub query_status: u32,
    /// `;`-separated answers; see [`parse_query_results`].
    pub query_results: WStr,
}

/// Which classic process event (Session B), by opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicKind {
    Start = 1,
    End = 2,
    /// Rundown of a process running when the session started.
    DcStart = 3,
    DcEnd = 4,
}

/// Session B's classic `Process` event, version 4 (S8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicProcess {
    pub kind: ClassicKind,
    pub unique_process_key: u64,
    pub pid: u32,
    pub parent_pid: u32,
    pub session_id: u32,
    /// Signed in the MOF class (`Int32`); 259 (`STILL_ACTIVE`) for a running process.
    pub exit_status: i32,
    pub directory_table_base: u64,
    pub flags: u32,
    /// The process's user. `None` when the event carries a null SID.
    pub user_sid: Option<Sid>,
    pub image_file_name: Box<[u8]>,
    pub command_line: WStr,
    pub package_full_name: WStr,
    pub application_id: WStr,
}

/// One parsed event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawEvent {
    ProcessStart(ProcessStart),
    ProcessStop(ProcessStop),
    ImageLoad(ImageLoad),
    FileCreate(FileCreate),
    FileCreateNew(FileCreate),
    FileCleanup(FileHandle),
    FileClose(FileHandle),
    FileWrite(FileWrite),
    FileSetInfo(FileSetInfo),
    /// Kernel-File 18 `SetDelete`: a delete disposition set or cleared.
    FileSetDelete(FileSetInfo),
    FileOpEnd(FileOpEnd),
    FileDeletePath(FilePath),
    FileRenamePath(FilePath),
    RegCreateKey(RegOpen),
    RegOpenKey(RegOpen),
    RegDeleteKey(RegKey),
    RegSetValue(RegSetValue),
    RegDeleteValue(RegDeleteValue),
    RegCloseKey(RegKey),
    TcpConnect(NetEvent),
    TcpAccept(NetEvent),
    TcpDisconnect(NetEvent),
    UdpSend(NetEvent),
    UdpRecv(NetEvent),
    DnsQuery(DnsQuery),
    ClassicProcess(ClassicProcess),
}

/// Parses one event. A version newer than our newest layout returns
/// [`ParseError::NewerVersion`]; the caller may then use [`parse_as`].
pub fn parse(meta: &EventMeta, payload: &[u8]) -> Result<RawEvent, ParseError> {
    if layout::find(meta.provider, meta.id, meta.version).is_some() {
        return parse_known(meta, payload, true);
    }
    match layout::newest(meta.provider, meta.id) {
        None => Err(ParseError::UnknownEvent),
        Some(n) if meta.version > n.version => {
            Err(ParseError::NewerVersion { version: meta.version, newest: n.version })
        }
        Some(_) => Err(ParseError::UnsupportedVersion { version: meta.version }),
    }
}

/// Parses an event with the layout of version `as_version`, which must be one we
/// know. Used for newer versions that passed the prefix check (§4.3). `exact`:
/// the newer version's layout **equals** ours (only the number changed), so a
/// name that ends the layout may run to the end of the payload. When the newer
/// layout only starts with ours, fields may follow, and such names stop at
/// their first NUL instead.
pub fn parse_as(meta: &EventMeta, as_version: u8, payload: &[u8], exact: bool) -> Result<RawEvent, ParseError> {
    if layout::find(meta.provider, meta.id, as_version).is_none() {
        return Err(ParseError::UnsupportedVersion { version: as_version });
    }
    parse_known(&EventMeta { version: as_version, ..*meta }, payload, exact)
}

/// `exact`: the payload is exactly this layout's version, so a name can be read
/// to the end of the payload. A newer version may append fields after it.
fn parse_known(meta: &EventMeta, payload: &[u8], exact: bool) -> Result<RawEvent, ParseError> {
    let r = &mut Reader::new(payload);
    let p = meta.pointer_size;
    Ok(match (meta.provider, meta.id) {
        (Provider::KernelProcess, 1) => RawEvent::ProcessStart(process_start(r, meta.version)?),
        (Provider::KernelProcess, 2) => RawEvent::ProcessStop(process_stop(r)?),
        (Provider::KernelProcess, 5) => RawEvent::ImageLoad(image_load(r, p)?),
        (Provider::KernelFile, 12) => RawEvent::FileCreate(file_create(r, p)?),
        (Provider::KernelFile, 30) => RawEvent::FileCreateNew(file_create(r, p)?),
        (Provider::KernelFile, 13) => RawEvent::FileCleanup(file_handle(r, p)?),
        (Provider::KernelFile, 14) => RawEvent::FileClose(file_handle(r, p)?),
        (Provider::KernelFile, 16) => RawEvent::FileWrite(file_write(r, p)?),
        (Provider::KernelFile, 17) => RawEvent::FileSetInfo(file_set_info(r, p)?),
        (Provider::KernelFile, 18) => RawEvent::FileSetDelete(file_set_info(r, p)?),
        (Provider::KernelFile, 24) => RawEvent::FileOpEnd(file_op_end(r, p)?),
        (Provider::KernelFile, 26) => RawEvent::FileDeletePath(file_path(r, p)?),
        (Provider::KernelFile, 27) => RawEvent::FileRenamePath(file_path(r, p)?),
        (Provider::KernelRegistry, 1) => RawEvent::RegCreateKey(reg_open(r, p, exact)?),
        (Provider::KernelRegistry, 2) => RawEvent::RegOpenKey(reg_open(r, p, exact)?),
        (Provider::KernelRegistry, 3) => RawEvent::RegDeleteKey(reg_key(r, p)?),
        (Provider::KernelRegistry, 5) => RawEvent::RegSetValue(reg_set_value(r, p, exact)?),
        (Provider::KernelRegistry, 6) => RawEvent::RegDeleteValue(reg_delete_value(r, p, exact)?),
        (Provider::KernelRegistry, 13) => RawEvent::RegCloseKey(reg_key(r, p)?),
        (Provider::KernelNetwork, 12) => RawEvent::TcpConnect(net(r, false, true)?),
        (Provider::KernelNetwork, 28) => RawEvent::TcpConnect(net(r, true, true)?),
        (Provider::KernelNetwork, 15) => RawEvent::TcpAccept(net(r, false, true)?),
        (Provider::KernelNetwork, 31) => RawEvent::TcpAccept(net(r, true, true)?),
        (Provider::KernelNetwork, 13) => RawEvent::TcpDisconnect(net(r, false, false)?),
        (Provider::KernelNetwork, 29) => RawEvent::TcpDisconnect(net(r, true, false)?),
        (Provider::KernelNetwork, 42) => RawEvent::UdpSend(net(r, false, false)?),
        (Provider::KernelNetwork, 58) => RawEvent::UdpSend(net(r, true, false)?),
        (Provider::KernelNetwork, 43) => RawEvent::UdpRecv(net(r, false, false)?),
        (Provider::KernelNetwork, 59) => RawEvent::UdpRecv(net(r, true, false)?),
        (Provider::DnsClient, 3008) => RawEvent::DnsQuery(dns_query(r)?),
        (Provider::ClassicProcess, op @ 1..=4) => RawEvent::ClassicProcess(classic_process(r, p, op)?),
        _ => return Err(ParseError::UnknownEvent),
    })
}

fn process_start(r: &mut Reader, version: u8) -> Result<ProcessStart, ParseError> {
    Ok(ProcessStart {
        pid: r.field("ProcessID").u32()?,
        sequence_number: r.field("ProcessSequenceNumber").u64()?,
        create_time: r.field("CreateTime").u64()?,
        parent_pid: r.field("ParentProcessID").u32()?,
        parent_sequence_number: r.field("ParentProcessSequenceNumber").u64()?,
        session_id: r.field("SessionID").u32()?,
        flags: r.field("Flags").u32()?,
        token_elevation_type: r.field("ProcessTokenElevationType").u32()?,
        token_is_elevated: r.field("ProcessTokenIsElevated").u32()?,
        mandatory_label: r.field("MandatoryLabel").sid()?,
        image_name: r.field("ImageName").wstr()?,
        image_checksum: r.field("ImageChecksum").u32()?,
        time_date_stamp: r.field("TimeDateStamp").u32()?,
        package_full_name: r.field("PackageFullName").wstr()?,
        package_relative_app_id: r.field("PackageRelativeAppId").wstr()?,
        security_mitigations: if version >= 4 { Some(r.field("SecurityMitigations").u32()?) } else { None },
    })
}

fn process_stop(r: &mut Reader) -> Result<ProcessStop, ParseError> {
    let pid = r.field("ProcessID").u32()?;
    let sequence_number = r.field("ProcessSequenceNumber").u64()?;
    let create_time = r.field("CreateTime").u64()?;
    let exit_time = r.field("ExitTime").u64()?;
    let exit_code = r.field("ExitCode").u32()?;
    // TokenElevationType .. HardFaultCount: 2 × u32, 3 × u64, 5 × u32.
    r.field("TokenElevationType..HardFaultCount").skip(4 * 2 + 8 * 3 + 4 * 5)?;
    let image_name = r.field("ImageName").astr()?;
    Ok(ProcessStop { pid, sequence_number, create_time, exit_time, exit_code, image_name })
}

fn image_load(r: &mut Reader, p: PointerSize) -> Result<ImageLoad, ParseError> {
    Ok(ImageLoad {
        image_base: r.field("ImageBase").ptr(p)?,
        image_size: r.field("ImageSize").ptr(p)?,
        pid: r.field("ProcessID").u32()?,
        image_checksum: r.field("ImageCheckSum").u32()?,
        time_date_stamp: r.field("TimeDateStamp").u32()?,
        default_base: r.field("DefaultBase").ptr(p)?,
        image_name: r.field("ImageName").wstr()?,
    })
}

fn file_create(r: &mut Reader, p: PointerSize) -> Result<FileCreate, ParseError> {
    Ok(FileCreate {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        create_options: r.field("CreateOptions").u32()?,
        create_attributes: r.field("CreateAttributes").u32()?,
        share_access: r.field("ShareAccess").u32()?,
        file_name: r.field("FileName").wstr()?,
    })
}

fn file_handle(r: &mut Reader, p: PointerSize) -> Result<FileHandle, ParseError> {
    Ok(FileHandle {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
    })
}

fn file_write(r: &mut Reader, p: PointerSize) -> Result<FileWrite, ParseError> {
    Ok(FileWrite {
        byte_offset: r.field("ByteOffset").u64()?,
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        io_size: r.field("IOSize").u32()?,
        io_flags: r.field("IOFlags").u32()?,
        extra_flags: r.field("ExtraFlags").u32()?,
    })
}

fn file_set_info(r: &mut Reader, p: PointerSize) -> Result<FileSetInfo, ParseError> {
    Ok(FileSetInfo {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        extra_information: r.field("ExtraInformation").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        info_class: r.field("InfoClass").u32()?,
    })
}

fn file_op_end(r: &mut Reader, p: PointerSize) -> Result<FileOpEnd, ParseError> {
    Ok(FileOpEnd {
        irp: r.field("Irp").ptr(p)?,
        extra_information: r.field("ExtraInformation").ptr(p)?,
        status: r.field("Status").u32()?,
    })
}

fn file_path(r: &mut Reader, p: PointerSize) -> Result<FilePath, ParseError> {
    Ok(FilePath {
        irp: r.field("Irp").ptr(p)?,
        file_object: r.field("FileObject").ptr(p)?,
        file_key: r.field("FileKey").ptr(p)?,
        extra_information: r.field("ExtraInformation").ptr(p)?,
        issuing_tid: r.field("IssuingThreadId").u32()?,
        info_class: r.field("InfoClass").u32()?,
        file_path: r.field("FilePath").wstr()?,
    })
}

/// A registry name that is the event's last field: for the exact version it
/// runs to the final terminator, embedded NULs included (sensor spec §7.5).
fn last_name(r: &mut Reader, field: &'static str, exact: bool) -> Result<WStr, ParseError> {
    // Only the final NUL can leave an empty trailer, so this is never ambiguous.
    if exact {
        r.field(field).counted_wstr(|t| t.is_empty().then_some(0)).map(|(s, _)| s)
    } else {
        r.field(field).wstr()
    }
}

fn reg_open(r: &mut Reader, p: PointerSize, exact: bool) -> Result<RegOpen, ParseError> {
    Ok(RegOpen {
        base_object: r.field("BaseObject").ptr(p)?,
        key_object: r.field("KeyObject").ptr(p)?,
        status: r.field("Status").u32()?,
        disposition: r.field("Disposition").u32()?,
        base_name: r.field("BaseName").wstr()?,
        relative_name: last_name(r, "RelativeName", exact)?,
    })
}

fn reg_key(r: &mut Reader, p: PointerSize) -> Result<RegKey, ParseError> {
    Ok(RegKey {
        key_object: r.field("KeyObject").ptr(p)?,
        status: r.field("Status").u32()?,
        key_name: r.field("KeyName").wstr()?,
    })
}

/// Bytes taken by SetValueKey v0's fields after `ValueName`, if `b` holds them:
/// CapturedDataSize + data, PreviousDataType, PreviousDataSize,
/// PreviousDataCapturedSize + data.
fn set_value_trailer_len(b: &[u8]) -> Option<usize> {
    let size_at = |at: usize| Some(usize::from(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?)));
    let previous_at = 2 + size_at(0)? + 8;
    let total = previous_at + 2 + size_at(previous_at)?;
    (total <= b.len()).then_some(total)
}

fn reg_set_value(r: &mut Reader, p: PointerSize, exact: bool) -> Result<RegSetValue, ParseError> {
    let key_object = r.field("KeyObject").ptr(p)?;
    let status = r.field("Status").u32()?;
    let value_type = r.field("Type").u32()?;
    let data_size = r.field("DataSize").u32()?;
    let key_name = r.field("KeyName").wstr()?;
    // A value name may embed NULs, a known way to hide Run values (§7.5).
    let (value_name, value_name_ambiguous) = if exact {
        r.field("ValueName").counted_wstr(set_value_trailer_len)?
    } else {
        (r.field("ValueName").wstr()?, false)
    };
    let captured_data = captured(r, "CapturedDataSize", "CapturedData")?;
    let previous_data_type = r.field("PreviousDataType").u32()?;
    let previous_data_size = r.field("PreviousDataSize").u32()?;
    let previous_data = captured(r, "PreviousDataCapturedSize", "PreviousData")?;
    Ok(RegSetValue {
        key_object,
        status,
        value_type,
        data_size,
        key_name,
        value_name,
        value_name_ambiguous,
        captured_data,
        previous_data_type,
        previous_data_size,
        previous_data,
    })
}

/// A `u16` byte count followed by that many bytes.
fn captured(r: &mut Reader, size_field: &'static str, data_field: &'static str) -> Result<Box<[u8]>, ParseError> {
    let n = usize::from(r.field(size_field).u16()?);
    let start = r.pos();
    r.field(data_field).skip(n)?;
    Ok(r.consumed_since(start).into())
}

fn reg_delete_value(r: &mut Reader, p: PointerSize, exact: bool) -> Result<RegDeleteValue, ParseError> {
    Ok(RegDeleteValue {
        key_object: r.field("KeyObject").ptr(p)?,
        status: r.field("Status").u32()?,
        key_name: r.field("KeyName").wstr()?,
        value_name: last_name(r, "ValueName", exact)?,
    })
}

fn addr(r: &mut Reader, v6: bool, name: &'static str) -> Result<IpAddr, ParseError> {
    r.field(name);
    Ok(if v6 { IpAddr::V6(Ipv6Addr::from(r.bytes::<16>()?)) } else { IpAddr::V4(Ipv4Addr::from(r.bytes::<4>()?)) })
}

/// `with_tcp_options`: connect and accept carry mss .. sndwinscale between the
/// ports and `seqnum` (16 bytes: 4 × u16, u32, 2 × u16).
fn net(r: &mut Reader, v6: bool, with_tcp_options: bool) -> Result<NetEvent, ParseError> {
    let pid = r.field("PID").u32()?;
    let size = r.field("size").u32()?;
    let daddr = addr(r, v6, "daddr")?;
    let saddr = addr(r, v6, "saddr")?;
    let dport = r.field("dport").u16_be()?;
    let sport = r.field("sport").u16_be()?;
    if with_tcp_options {
        r.field("mss..sndwinscale").skip(16)?;
    }
    let seqnum = r.field("seqnum").u32()?;
    let connid = r.field("connid").u32()?;
    Ok(NetEvent { pid, size, daddr, saddr, dport, sport, seqnum, connid })
}

fn dns_query(r: &mut Reader) -> Result<DnsQuery, ParseError> {
    Ok(DnsQuery {
        query_name: r.field("QueryName").wstr()?,
        query_type: r.field("QueryType").u32()?,
        query_options: r.field("QueryOptions").u64()?,
        query_status: r.field("QueryStatus").u32()?,
        query_results: r.field("QueryResults").wstr()?,
    })
}

/// `UserSID` (MOF `WbemSid`): a null SID is 4 zero bytes; otherwise a
/// `TOKEN_USER` (pointer + attributes, padded to two pointers) then the SID (S8).
fn wbem_sid(r: &mut Reader, p: PointerSize) -> Result<Option<Sid>, ParseError> {
    r.field("UserSID");
    let first = r.rest().get(..4).ok_or(ParseError::Truncated { field: "UserSID", offset: r.pos() })?;
    if first == [0, 0, 0, 0] {
        r.skip(4)?;
        return Ok(None);
    }
    r.skip(match p {
        PointerSize::P32 => 8,
        PointerSize::P64 => 16,
    })?;
    r.sid().map(Some)
}

fn classic_process(r: &mut Reader, p: PointerSize, opcode: u16) -> Result<ClassicProcess, ParseError> {
    let kind = match opcode {
        1 => ClassicKind::Start,
        2 => ClassicKind::End,
        3 => ClassicKind::DcStart,
        _ => ClassicKind::DcEnd,
    };
    Ok(ClassicProcess {
        kind,
        unique_process_key: r.field("UniqueProcessKey").ptr(p)?,
        pid: r.field("ProcessId").u32()?,
        parent_pid: r.field("ParentId").u32()?,
        session_id: r.field("SessionId").u32()?,
        exit_status: r.field("ExitStatus").u32()? as i32,
        directory_table_base: r.field("DirectoryTableBase").ptr(p)?,
        flags: r.field("Flags").u32()?,
        user_sid: wbem_sid(r, p)?,
        image_file_name: r.field("ImageFileName").astr()?,
        command_line: r.field("CommandLine").wstr()?,
        package_full_name: r.field("PackageFullName").wstr()?,
        application_id: r.field("ApplicationId").wstr()?,
    })
}

#[cfg(test)]
mod tests;
