//! Shared test data: one named sample per class/activity, and proptest
//! strategies that generate arbitrary *valid* domain events.

#![allow(dead_code)] // each test binary uses a different subset

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use atlas_schema::classes::dns::{DnsAction, DnsActivity, DnsAnswer};
use atlas_schema::classes::file::{FileAction, FileSystemActivity};
use atlas_schema::classes::module::{ModuleAction, ModuleActivity};
use atlas_schema::classes::network::{NetworkAction, NetworkActivity, NetworkDirection, NetworkProtocol};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{
    RegValueType, RegistryKeyAction, RegistryKeyActivity, RegistryValueAction, RegistryValueActivity,
};
use atlas_schema::*;

// ---------------------------------------------------------------- samples

pub fn fixed_event_id() -> EventId {
    // 0192... is a valid UUIDv7 (version nibble 7, RFC 9562 variant).
    EventId::from_uuid(uuid::Uuid::from_bytes([
        0x01, 0x92, 0x2a, 0x6e, 0x5b, 0x00, 0x70, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    ]))
    .expect("valid v7")
}

pub fn device() -> Device {
    Device { uid: DeviceUid::from_bytes([0xd0; 16]), boot_id: BootId::from_bytes([0xb0; 16]) }
}

pub fn file(path: &str) -> File {
    let name = path.rsplit('\\').next().unwrap_or(path).to_owned();
    File { path: path.to_owned(), name, hashes: None, signature: None }
}

pub fn user() -> User {
    User { uid: "S-1-5-21-1000-1000-1000-1001".into(), name: "DESK-01\\jake".into() }
}

pub fn proc_ref(start_key: u64, path: &str, pid: u32) -> ProcessRef {
    let d = device();
    ProcessRef { uid: process_uid(&d.uid, &d.boot_id, start_key), pid, file: file(path), user: Some(user()) }
}

pub fn actor() -> ProcessRef {
    proc_ref(7788, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", 7788)
}

pub fn event(kind: EventKind) -> Event {
    Event {
        meta: EventMeta { event_id: fixed_event_id(), time: 1_790_000_000_123_456_789, sensor: Sensor::Etw },
        device: device(),
        kind,
    }
}

fn file_event(action: FileAction) -> Event {
    event(EventKind::File(FileSystemActivity { actor: actor(), file: file("C:\\Users\\jake\\a.txt"), action }))
}

fn net_event(action: NetworkAction) -> Event {
    event(EventKind::Network(NetworkActivity {
        actor: actor(),
        src_endpoint: NetworkEndpoint { ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), port: 50123 },
        dst_endpoint: NetworkEndpoint { ip: IpAddr::V6(Ipv6Addr::LOCALHOST), port: 443 },
        protocol: NetworkProtocol::Tcp,
        direction: NetworkDirection::Outbound,
        action,
    }))
}

fn reg_key_event(action: RegistryKeyAction) -> Event {
    event(EventKind::RegistryKey(RegistryKeyActivity {
        actor: actor(),
        path: "HKLM\\SOFTWARE\\Atlas\\New".into(),
        action,
    }))
}

fn reg_value_event(action: RegistryValueAction) -> Event {
    event(EventKind::RegistryValue(RegistryValueActivity {
        actor: actor(),
        key_path: "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
        name: "Updater".into(),
        action,
    }))
}

/// One valid event per (class, activity), named `<class>_<activity>`.
pub fn samples() -> Vec<(&'static str, Event)> {
    let launch = ProcessActivity::Launch {
        actor: proc_ref(4120, "C:\\Program Files\\Microsoft Office\\root\\Office16\\WINWORD.EXE", 4120),
        process: Process {
            uid: actor().uid,
            pid: 7788,
            file: File {
                hashes: Some(Hashes { sha256: Some([0xab; 32]) }),
                signature: Some(Signature { signer: Some("Microsoft Windows".into()), status: SignatureStatus::Valid }),
                ..file("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
            },
            user: Some(user()),
            cmd_line: "powershell.exe -enc SQBFAFgA".into(),
            cmd_line_truncated: false,
            created_time: 1_790_000_000_123_000_000,
            integrity: Some(Integrity::Medium),
            parent_process: Some(proc_ref(
                4120,
                "C:\\Program Files\\Microsoft Office\\root\\Office16\\WINWORD.EXE",
                4120,
            )),
        },
    };
    vec![
        ("process_launch", event(EventKind::Process(launch))),
        (
            "process_terminate",
            event(EventKind::Process(ProcessActivity::Terminate { process: actor(), exit_code: Some(0) })),
        ),
        (
            "module_load",
            event(EventKind::Module(ModuleActivity {
                actor: actor(),
                action: ModuleAction::Load {
                    file: file("C:\\Windows\\System32\\amsi.dll"),
                    base_address: 0x7ffb_1234_0000,
                },
            })),
        ),
        ("network_open", net_event(NetworkAction::Open)),
        ("network_close", net_event(NetworkAction::Close { bytes_in: Some(5120), bytes_out: Some(812) })),
        ("file_create", file_event(FileAction::Create)),
        ("file_read", file_event(FileAction::Read)),
        ("file_update", file_event(FileAction::Update)),
        ("file_delete", file_event(FileAction::Delete)),
        ("file_rename", file_event(FileAction::Rename { file_result: file("C:\\Users\\jake\\a.txt.locked") })),
        ("file_set_attributes", file_event(FileAction::SetAttributes)),
        ("registry_key_create", reg_key_event(RegistryKeyAction::Create)),
        ("registry_key_delete", reg_key_event(RegistryKeyAction::Delete)),
        (
            "registry_key_rename",
            reg_key_event(RegistryKeyAction::Rename { prev_path: "HKLM\\SOFTWARE\\Atlas\\Old".into() }),
        ),
        (
            "registry_value_set",
            reg_value_event(RegistryValueAction::Set {
                value_type: RegValueType::Sz,
                data: "C:\\Users\\Public\\u.exe\0".encode_utf16().flat_map(u16::to_le_bytes).collect(),
                data_truncated: false,
            }),
        ),
        ("registry_value_delete", reg_value_event(RegistryValueAction::Delete)),
        (
            "dns_response",
            event(EventKind::Dns(DnsActivity {
                actor: actor(),
                hostname: "example.com".into(),
                query_type: 28,
                action: DnsAction::Response {
                    rcode: Some(0),
                    platform_status: Some(0),
                    answers: vec![DnsAnswer { rr_type: 28, data: "2606:2800:21f:cb07:6820:80da:af6b:8b2c".into() }],
                },
            })),
        ),
    ]
}
