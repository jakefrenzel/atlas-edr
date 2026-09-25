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
use proptest::prelude::*;

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

// ------------------------------------------------------------- strategies

pub fn arb_string(max_chars: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=max_chars).prop_map(String::from_iter)
}

fn arb_uid<T: 'static + std::fmt::Debug>(f: fn([u8; 16]) -> T) -> impl Strategy<Value = T> {
    any::<[u8; 16]>().prop_map(f)
}

pub fn arb_event_id() -> impl Strategy<Value = EventId> {
    (0u64..(1 << 48), any::<[u8; 10]>()).prop_map(|(ms, rand)| {
        EventId::from_uuid(uuid::Builder::from_unix_timestamp_millis(ms, &rand).into_uuid()).expect("v7")
    })
}

pub fn arb_user() -> BoxedStrategy<User> {
    (arb_string(20), arb_string(20)).prop_map(|(uid, name)| User { uid, name }).boxed()
}

pub fn arb_file() -> BoxedStrategy<File> {
    let status =
        prop_oneof![Just(SignatureStatus::Valid), Just(SignatureStatus::Invalid), Just(SignatureStatus::Unsigned)];
    (
        arb_string(40),
        arb_string(20),
        prop::option::of(prop::option::of(any::<[u8; 32]>()).prop_map(|sha256| Hashes { sha256 })),
        prop::option::of(
            (prop::option::of(arb_string(20)), status).prop_map(|(signer, status)| Signature { signer, status }),
        ),
    )
        .prop_map(|(path, name, hashes, signature)| File { path, name, hashes, signature })
        .boxed()
}

pub fn arb_process_ref() -> BoxedStrategy<ProcessRef> {
    (arb_uid(ProcessUid::from_bytes), any::<u32>(), arb_file(), prop::option::of(arb_user()))
        .prop_map(|(uid, pid, file, user)| ProcessRef { uid, pid, file, user })
        .boxed()
}

pub fn arb_integrity() -> impl Strategy<Value = Integrity> {
    prop_oneof![
        Just(Integrity::Untrusted),
        Just(Integrity::Low),
        Just(Integrity::Medium),
        Just(Integrity::High),
        Just(Integrity::System),
        Just(Integrity::Protected),
    ]
}

pub fn arb_process() -> BoxedStrategy<Process> {
    (
        arb_process_ref(),
        arb_string(60),
        any::<bool>(),
        any::<i64>(),
        prop::option::of(arb_integrity()),
        prop::option::of(arb_process_ref()),
    )
        .prop_map(|(r, cmd_line, cmd_line_truncated, created_time, integrity, parent_process)| Process {
            uid: r.uid,
            pid: r.pid,
            file: r.file,
            user: r.user,
            cmd_line,
            cmd_line_truncated,
            created_time,
            integrity,
            parent_process,
        })
        .boxed()
}

pub fn arb_endpoint() -> BoxedStrategy<NetworkEndpoint> {
    let ip = prop_oneof![
        any::<[u8; 4]>().prop_map(|o| IpAddr::V4(Ipv4Addr::from(o))),
        any::<[u8; 16]>().prop_map(|o| IpAddr::V6(Ipv6Addr::from(o))),
    ];
    (ip, any::<u16>()).prop_map(|(ip, port)| NetworkEndpoint { ip, port }).boxed()
}

fn arb_reg_value_type() -> impl Strategy<Value = RegValueType> {
    (0u32..=11).prop_map(|raw| RegValueType::from_raw(raw).expect("0..=11 are valid"))
}

pub fn arb_kind() -> BoxedStrategy<EventKind> {
    let process = prop_oneof![
        (arb_process_ref(), arb_process()).prop_map(|(actor, process)| ProcessActivity::Launch { actor, process }),
        (arb_process_ref(), any::<Option<i32>>())
            .prop_map(|(process, exit_code)| ProcessActivity::Terminate { process, exit_code }),
    ]
    .prop_map(EventKind::Process);

    let module = (arb_process_ref(), arb_file(), any::<u64>()).prop_map(|(actor, file, base_address)| {
        EventKind::Module(ModuleActivity { actor, action: ModuleAction::Load { file, base_address } })
    });

    let net_action = prop_oneof![
        Just(NetworkAction::Open),
        (any::<Option<u64>>(), any::<Option<u64>>())
            .prop_map(|(bytes_in, bytes_out)| NetworkAction::Close { bytes_in, bytes_out }),
    ];
    let protocol = prop_oneof![Just(NetworkProtocol::Tcp), Just(NetworkProtocol::Udp)];
    let direction = prop_oneof![Just(NetworkDirection::Inbound), Just(NetworkDirection::Outbound)];
    let network = (arb_process_ref(), arb_endpoint(), arb_endpoint(), protocol, direction, net_action).prop_map(
        |(actor, src_endpoint, dst_endpoint, protocol, direction, action)| {
            EventKind::Network(NetworkActivity { actor, src_endpoint, dst_endpoint, protocol, direction, action })
        },
    );

    let file_action = prop_oneof![
        Just(FileAction::Create),
        Just(FileAction::Read),
        Just(FileAction::Update),
        Just(FileAction::Delete),
        arb_file().prop_map(|file_result| FileAction::Rename { file_result }),
        Just(FileAction::SetAttributes),
    ];
    let file = (arb_process_ref(), arb_file(), file_action)
        .prop_map(|(actor, file, action)| EventKind::File(FileSystemActivity { actor, file, action }));

    let key_action = prop_oneof![
        Just(RegistryKeyAction::Create),
        Just(RegistryKeyAction::Delete),
        arb_string(40).prop_map(|prev_path| RegistryKeyAction::Rename { prev_path }),
    ];
    let reg_key = (arb_process_ref(), arb_string(40), key_action)
        .prop_map(|(actor, path, action)| EventKind::RegistryKey(RegistryKeyActivity { actor, path, action }));

    let value_action = prop_oneof![
        (arb_reg_value_type(), prop::collection::vec(any::<u8>(), 0..64), any::<bool>()).prop_map(
            |(value_type, data, data_truncated)| RegistryValueAction::Set { value_type, data, data_truncated }
        ),
        Just(RegistryValueAction::Delete),
    ];
    let reg_value = (arb_process_ref(), arb_string(40), arb_string(20), value_action).prop_map(
        |(actor, key_path, name, action)| {
            EventKind::RegistryValue(RegistryValueActivity { actor, key_path, name, action })
        },
    );

    let answer = (any::<u16>(), arb_string(30)).prop_map(|(rr_type, data)| DnsAnswer { rr_type, data });
    let dns = (
        arb_process_ref(),
        arb_string(30),
        any::<u16>(),
        any::<Option<u16>>(),
        any::<Option<u32>>(),
        prop::collection::vec(answer, 0..5),
    )
        .prop_map(|(actor, hostname, query_type, rcode, platform_status, answers)| {
            EventKind::Dns(DnsActivity {
                actor,
                hostname,
                query_type,
                action: DnsAction::Response { rcode, platform_status, answers },
            })
        });

    prop_oneof![
        process.boxed(),
        module.boxed(),
        network.boxed(),
        file.boxed(),
        reg_key.boxed(),
        reg_value.boxed(),
        dns.boxed()
    ]
    .boxed()
}

pub fn arb_event() -> BoxedStrategy<Event> {
    let sensor = prop_oneof![Just(Sensor::Etw), Just(Sensor::Driver)];
    (arb_event_id(), any::<i64>(), sensor, arb_uid(DeviceUid::from_bytes), arb_uid(BootId::from_bytes), arb_kind())
        .prop_map(|(event_id, time, sensor, uid, boot_id, kind)| Event {
            meta: EventMeta { event_id, time, sensor },
            device: Device { uid, boot_id },
            kind,
        })
        .boxed()
}
