//! Shared objects (spec section 5.1) and their wire conversions.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use atlas_proto::v1 as wire;

use crate::convert::{Result, bounded, err, fixed, join, require, u16_field, wire_enum};
use crate::error::SchemaErrorKind;
use crate::ids::ProcessUid;
use crate::limits::{CMD_LINE_MAX, PATH_MAX, SIGNER_MAX, USER_NAME_MAX, USER_UID_MAX};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Windows SID string.
    pub uid: String,
    /// `DOMAIN\user`.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashes {
    pub sha256: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    Valid,
    Invalid,
    Unsigned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub signer: Option<String>,
    pub status: SignatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub name: String,
    pub hashes: Option<Hashes>,
    pub signature: Option<Signature>,
}

/// The actor core carried by every event (spec D5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRef {
    pub uid: ProcessUid,
    pub pid: u32,
    pub file: File,
    pub user: Option<User>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integrity {
    Untrusted,
    Low,
    Medium,
    High,
    System,
    Protected,
}

/// Full process detail; only carried by Process Launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub uid: ProcessUid,
    pub pid: u32,
    pub file: File,
    pub user: Option<User>,
    pub cmd_line: String,
    pub cmd_line_truncated: bool,
    /// Nanoseconds since the Unix epoch, UTC.
    pub created_time: i64,
    pub integrity: Option<Integrity>,
    /// The parent recorded by the OS (can be spoofed). `None` when the OS
    /// reports no parent (e.g. early boot processes).
    pub parent_process: Option<ProcessRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkEndpoint {
    pub ip: IpAddr,
    pub port: u16,
}

// ---- domain → wire (infallible) ----

impl From<User> for wire::User {
    fn from(v: User) -> Self {
        Self { uid: v.uid, name: v.name }
    }
}

impl From<Hashes> for wire::Hashes {
    fn from(v: Hashes) -> Self {
        Self { sha256: v.sha256.map(|h| h.to_vec()) }
    }
}

impl From<SignatureStatus> for wire::SignatureStatus {
    fn from(v: SignatureStatus) -> Self {
        match v {
            SignatureStatus::Valid => Self::Valid,
            SignatureStatus::Invalid => Self::Invalid,
            SignatureStatus::Unsigned => Self::Unsigned,
        }
    }
}

impl From<Signature> for wire::Signature {
    fn from(v: Signature) -> Self {
        Self { signer: v.signer, status: wire::SignatureStatus::from(v.status) as i32 }
    }
}

impl From<File> for wire::File {
    fn from(v: File) -> Self {
        Self { path: v.path, name: v.name, hashes: v.hashes.map(Into::into), signature: v.signature.map(Into::into) }
    }
}

impl From<ProcessRef> for wire::ProcessRef {
    fn from(v: ProcessRef) -> Self {
        Self { uid: v.uid.as_bytes().to_vec(), pid: v.pid, file: Some(v.file.into()), user: v.user.map(Into::into) }
    }
}

impl From<Integrity> for wire::Integrity {
    fn from(v: Integrity) -> Self {
        match v {
            Integrity::Untrusted => Self::Untrusted,
            Integrity::Low => Self::Low,
            Integrity::Medium => Self::Medium,
            Integrity::High => Self::High,
            Integrity::System => Self::System,
            Integrity::Protected => Self::Protected,
        }
    }
}

impl From<Process> for wire::Process {
    fn from(v: Process) -> Self {
        Self {
            uid: v.uid.as_bytes().to_vec(),
            pid: v.pid,
            file: Some(v.file.into()),
            user: v.user.map(Into::into),
            cmd_line: v.cmd_line,
            cmd_line_truncated: v.cmd_line_truncated,
            created_time: v.created_time,
            integrity: v.integrity.map(|i| wire::Integrity::from(i) as i32),
            parent_process: v.parent_process.map(Into::into),
        }
    }
}

impl From<NetworkEndpoint> for wire::NetworkEndpoint {
    fn from(v: NetworkEndpoint) -> Self {
        let ip = match v.ip {
            IpAddr::V4(a) => a.octets().to_vec(),
            IpAddr::V6(a) => a.octets().to_vec(),
        };
        Self { ip, port: u32::from(v.port) }
    }
}

// ---- wire → domain (validating) ----

impl User {
    pub(crate) fn from_wire(w: wire::User, path: &str) -> Result<Self> {
        Ok(Self {
            uid: bounded(w.uid, USER_UID_MAX, path, "uid")?,
            name: bounded(w.name, USER_NAME_MAX, path, "name")?,
        })
    }

    /// An optional `User` sub-message at `parent.user`.
    pub(crate) fn optional(w: Option<wire::User>, parent: &str) -> Result<Option<Self>> {
        match w {
            Some(u) => Ok(Some(Self::from_wire(u, &join(parent, "user"))?)),
            None => Ok(None),
        }
    }
}

impl Hashes {
    pub(crate) fn from_wire(w: wire::Hashes, path: &str) -> Result<Self> {
        let sha256 = match w.sha256 {
            Some(b) => Some(fixed::<32>(b, path, "sha256")?),
            None => None,
        };
        Ok(Self { sha256 })
    }
}

impl Signature {
    pub(crate) fn from_wire(w: wire::Signature, path: &str) -> Result<Self> {
        let status = wire_enum(
            w.status,
            |s: wire::SignatureStatus| match s {
                wire::SignatureStatus::Unspecified => None,
                wire::SignatureStatus::Valid => Some(SignatureStatus::Valid),
                wire::SignatureStatus::Invalid => Some(SignatureStatus::Invalid),
                wire::SignatureStatus::Unsigned => Some(SignatureStatus::Unsigned),
            },
            path,
            "status",
        )?;
        let signer = match w.signer {
            Some(s) => Some(bounded(s, SIGNER_MAX, path, "signer")?),
            None => None,
        };
        Ok(Self { signer, status })
    }
}

impl File {
    pub(crate) fn from_wire(w: wire::File, path: &str) -> Result<Self> {
        Ok(Self {
            path: bounded(w.path, PATH_MAX, path, "path")?,
            name: bounded(w.name, PATH_MAX, path, "name")?,
            hashes: match w.hashes {
                Some(h) => Some(Hashes::from_wire(h, &join(path, "hashes"))?),
                None => None,
            },
            signature: match w.signature {
                Some(s) => Some(Signature::from_wire(s, &join(path, "signature"))?),
                None => None,
            },
        })
    }

    /// A required `File` sub-message at `parent.field`.
    pub(crate) fn required(w: Option<wire::File>, parent: &str, field: &str) -> Result<Self> {
        let w = require(w, parent, field)?;
        Self::from_wire(w, &join(parent, field))
    }
}

impl ProcessRef {
    pub(crate) fn from_wire(w: wire::ProcessRef, path: &str) -> Result<Self> {
        Ok(Self {
            uid: ProcessUid::from_bytes(fixed::<16>(w.uid, path, "uid")?),
            pid: w.pid,
            file: File::required(w.file, path, "file")?,
            user: User::optional(w.user, path)?,
        })
    }

    /// A required `ProcessRef` sub-message at `parent.field`.
    pub(crate) fn required(w: Option<wire::ProcessRef>, parent: &str, field: &str) -> Result<Self> {
        let w = require(w, parent, field)?;
        Self::from_wire(w, &join(parent, field))
    }
}

fn integrity_from_wire(raw: i32, path: &str) -> Result<Integrity> {
    wire_enum(
        raw,
        |i: wire::Integrity| match i {
            wire::Integrity::Unspecified => None,
            wire::Integrity::Untrusted => Some(Integrity::Untrusted),
            wire::Integrity::Low => Some(Integrity::Low),
            wire::Integrity::Medium => Some(Integrity::Medium),
            wire::Integrity::High => Some(Integrity::High),
            wire::Integrity::System => Some(Integrity::System),
            wire::Integrity::Protected => Some(Integrity::Protected),
        },
        path,
        "integrity",
    )
}

impl Process {
    pub(crate) fn from_wire(w: wire::Process, path: &str) -> Result<Self> {
        Ok(Self {
            uid: ProcessUid::from_bytes(fixed::<16>(w.uid, path, "uid")?),
            pid: w.pid,
            file: File::required(w.file, path, "file")?,
            user: User::optional(w.user, path)?,
            cmd_line: bounded(w.cmd_line, CMD_LINE_MAX, path, "cmd_line")?,
            cmd_line_truncated: w.cmd_line_truncated,
            created_time: w.created_time,
            integrity: match w.integrity {
                Some(raw) => Some(integrity_from_wire(raw, path)?),
                None => None,
            },
            parent_process: match w.parent_process {
                Some(p) => Some(ProcessRef::from_wire(p, &join(path, "parent_process"))?),
                None => None,
            },
        })
    }
}

impl NetworkEndpoint {
    pub(crate) fn required(w: Option<wire::NetworkEndpoint>, parent: &str, field: &str) -> Result<Self> {
        let w = require(w, parent, field)?;
        let path = join(parent, field);
        let ip = match w.ip.len() {
            4 => IpAddr::V4(Ipv4Addr::from(fixed::<4>(w.ip, &path, "ip")?)),
            16 => IpAddr::V6(Ipv6Addr::from(fixed::<16>(w.ip, &path, "ip")?)),
            _ => return err(&path, "ip", SchemaErrorKind::Malformed),
        };
        let port = u16_field(w.port, &path, "port")?;
        Ok(Self { ip, port })
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn file(path: &str) -> File {
        File { path: path.into(), name: "x.exe".into(), hashes: None, signature: None }
    }

    pub fn proc_ref() -> ProcessRef {
        ProcessRef { uid: ProcessUid::from_bytes([7; 16]), pid: 42, file: file("C:\\x.exe"), user: None }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn kind_at<T: std::fmt::Debug>(r: Result<T>) -> (String, SchemaErrorKind) {
        let e = r.unwrap_err();
        (e.field_path, e.kind)
    }

    #[test]
    fn file_round_trips_with_enrichment() {
        let f = File {
            hashes: Some(Hashes { sha256: Some([1; 32]) }),
            signature: Some(Signature { signer: None, status: SignatureStatus::Unsigned }),
            ..file("C:\\a.dll")
        };
        assert_eq!(File::from_wire(f.clone().into(), "file").unwrap(), f);
    }

    #[test]
    fn file_path_over_limit_is_too_large() {
        let w = wire::File { path: "a".repeat(PATH_MAX + 1), ..file("x").into() };
        assert_eq!(kind_at(File::from_wire(w, "file")), ("file.path".into(), SchemaErrorKind::TooLarge));
    }

    #[test]
    fn sha256_must_be_32_bytes() {
        let w = wire::File { hashes: Some(wire::Hashes { sha256: Some(vec![0; 31]) }), ..file("x").into() };
        assert_eq!(kind_at(File::from_wire(w, "file")), ("file.hashes.sha256".into(), SchemaErrorKind::Malformed));
    }

    #[test]
    fn signature_status_unspecified_is_missing_and_unknown_is_unknown() {
        let w = |status| wire::File { signature: Some(wire::Signature { signer: None, status }), ..file("x").into() };
        assert_eq!(kind_at(File::from_wire(w(0), "f")), ("f.signature.status".into(), SchemaErrorKind::Missing));
        assert_eq!(kind_at(File::from_wire(w(99), "f")), ("f.signature.status".into(), SchemaErrorKind::UnknownEnum));
    }

    #[test]
    fn process_ref_uid_must_be_16_bytes() {
        let w = wire::ProcessRef { uid: vec![0; 15], ..proc_ref().into() };
        assert_eq!(
            kind_at(ProcessRef::from_wire(w, "actor.process")),
            ("actor.process.uid".into(), SchemaErrorKind::Malformed)
        );
    }

    #[test]
    fn process_round_trips_and_integrity_zero_is_missing() {
        let p = Process {
            uid: ProcessUid::from_bytes([1; 16]),
            pid: 1,
            file: file("C:\\p.exe"),
            user: None,
            cmd_line: String::new(),
            cmd_line_truncated: false,
            created_time: 0,
            integrity: None,
            parent_process: Some(proc_ref()),
        };
        assert_eq!(Process::from_wire(p.clone().into(), "process").unwrap(), p);
        let w = wire::Process { integrity: Some(0), ..p.into() };
        assert_eq!(kind_at(Process::from_wire(w, "process")), ("process.integrity".into(), SchemaErrorKind::Missing));
    }

    #[test]
    fn endpoints_round_trip_v4_and_v6() {
        for ip in [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), IpAddr::V6(Ipv6Addr::LOCALHOST)] {
            let e = NetworkEndpoint { ip, port: 443 };
            assert_eq!(NetworkEndpoint::required(Some(e.into()), "", "src_endpoint").unwrap(), e);
        }
    }

    #[test]
    fn endpoint_rejects_bad_ip_length_bad_port_and_absence() {
        let bad_ip = wire::NetworkEndpoint { ip: vec![1, 2, 3, 4, 5], port: 1 };
        assert_eq!(
            kind_at(NetworkEndpoint::required(Some(bad_ip), "", "dst_endpoint")),
            ("dst_endpoint.ip".into(), SchemaErrorKind::Malformed)
        );
        let bad_port = wire::NetworkEndpoint { ip: vec![1, 2, 3, 4], port: 70_000 };
        assert_eq!(
            kind_at(NetworkEndpoint::required(Some(bad_port), "", "src_endpoint")),
            ("src_endpoint.port".into(), SchemaErrorKind::Malformed)
        );
        assert_eq!(
            kind_at(NetworkEndpoint::required(None, "", "src_endpoint")),
            ("src_endpoint".into(), SchemaErrorKind::Missing)
        );
    }
}
