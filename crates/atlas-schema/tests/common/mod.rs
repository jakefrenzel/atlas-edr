//! Shared test data: one named sample per class/activity, and proptest
//! strategies that generate arbitrary *valid* domain events.

#![allow(dead_code)] // each test binary uses a different subset

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use atlas_schema::classes::dns::{DnsAction, DnsActivity, DnsAnswer};
use atlas_schema::classes::event_log::{EventLogAction, EventLogActivity};
use atlas_schema::classes::file::{FileAction, FileSystemActivity};
use atlas_schema::classes::module::{ModuleAction, ModuleActivity};
use atlas_schema::classes::network::{NetworkAction, NetworkActivity, NetworkDirection, NetworkProtocol};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{
    RegType, RegValueType, RegistryKeyAction, RegistryKeyActivity, RegistryValueAction, RegistryValueActivity,
};
use atlas_schema::classes::sensor_health::{
    ClassCount, HealthReport, SensorBuffer, SensorGap, SensorHealthAction, SensorHealthActivity, SensorHousekeeping,
    SensorLoss, SensorQuality, SensorResources,
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
        path_unresolved: false,
        action,
    }))
}

fn reg_value_event(action: RegistryValueAction) -> Event {
    event(EventKind::RegistryValue(RegistryValueActivity {
        actor: actor(),
        key_path: "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
        name: "Updater".into(),
        path_unresolved: false,
        action,
    }))
}

fn event_log_event(action: EventLogAction, log_provider: &str) -> Event {
    event(EventKind::EventLog(EventLogActivity {
        actor: None,
        log_name: "Atlas-Sensor".into(),
        log_provider: log_provider.into(),
        status_code: None,
        action,
    }))
}

/// One valid event per (class, activity), named `<class>_<activity>`, plus
/// named variants for fields that change validation (`*_read_after`, …).
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
        ("file_open", file_event(FileAction::Open)),
        ("registry_key_create", reg_key_event(RegistryKeyAction::Create)),
        ("registry_key_delete", reg_key_event(RegistryKeyAction::Delete)),
        (
            "registry_key_rename",
            reg_key_event(RegistryKeyAction::Rename { prev_path: "HKLM\\SOFTWARE\\Atlas\\Old".into() }),
        ),
        (
            "registry_key_create_unresolved",
            event(EventKind::RegistryKey(RegistryKeyActivity {
                actor: actor(),
                path: "Atlas\\New".into(),
                path_unresolved: true,
                action: RegistryKeyAction::Create,
            })),
        ),
        (
            "registry_value_set",
            reg_value_event(RegistryValueAction::Set {
                value_type: RegType::Known(RegValueType::Sz),
                data: "C:\\Users\\Public\\u.exe\0".encode_utf16().flat_map(u16::to_le_bytes).collect(),
                data_truncated: false,
                data_read_after: false,
                data_unavailable: false,
            }),
        ),
        (
            "registry_value_set_read_after",
            reg_value_event(RegistryValueAction::Set {
                value_type: RegType::Known(RegValueType::Dword),
                data: vec![1, 0, 0, 0],
                data_truncated: false,
                data_read_after: true,
                data_unavailable: false,
            }),
        ),
        (
            "registry_value_set_unavailable",
            event(EventKind::RegistryValue(RegistryValueActivity {
                actor: actor(),
                key_path: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
                name: "Updater".into(),
                path_unresolved: true,
                action: RegistryValueAction::Set {
                    value_type: RegType::Raw(0x0020_0000),
                    data: vec![],
                    data_truncated: false,
                    data_read_after: false,
                    data_unavailable: true,
                },
            })),
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
        ("event_log_stop", event_log_event(EventLogAction::Stop, "")),
        ("event_log_restart", event_log_event(EventLogAction::Restart, "")),
        ("event_log_disable", event_log_event(EventLogAction::Disable, "Microsoft-Windows-Kernel-Registry")),
        (
            "sensor_health_report",
            event(EventKind::SensorHealth(SensorHealthActivity {
                interval_start: 1_790_000_000_000_000_000,
                action: SensorHealthAction::Report(Box::new(HealthReport {
                    loss: SensorLoss {
                        sensor_session_events_lost: Some(0),
                        kernel_queue_drops: Some(0),
                        actor_dropped: vec![ClassCount { class_uid: 4001, count: 2 }],
                        ..Default::default()
                    },
                    quality: SensorQuality {
                        late_arrivals: Some(17),
                        actor_unresolved: vec![ClassCount { class_uid: 1001, count: 3 }],
                        ..Default::default()
                    },
                    housekeeping: SensorHousekeeping {
                        retention_evictions: Some(1),
                        seeding_enabled: Some(true),
                        seeder_handles_named: Some(14_139),
                        ..Default::default()
                    },
                    resources: SensorResources { cpu_time: Some(480_000_000), working_set: Some(41_943_040) },
                    gap: Some(SensorGap {
                        first_time: Some(1_789_999_000_000_000_000),
                        last_time: Some(1_789_999_100_000_000_000),
                        events: 52_000,
                    }),
                    buffer: SensorBuffer {
                        write_errors: Some(0),
                        failing: Some(false),
                        disk_bytes: Some(734_003_200),
                        ..Default::default()
                    },
                })),
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

fn arb_reg_type() -> impl Strategy<Value = RegType> {
    prop_oneof![
        (0u32..=11).prop_map(|raw| RegType::Known(RegValueType::from_raw(raw).expect("0..=11 are valid"))),
        (12u32..).prop_map(RegType::Raw),
    ]
}

fn arb_class_counts() -> impl Strategy<Value = Vec<ClassCount>> {
    let entry = (any::<u32>(), any::<u64>()).prop_map(|(class_uid, count)| ClassCount { class_uid, count });
    prop::collection::vec(entry, 0..4)
}

/// A counter: absent, or any value.
fn counter() -> impl Strategy<Value = Option<u64>> {
    any::<Option<u64>>()
}

fn arb_health_report() -> BoxedStrategy<HealthReport> {
    let loss =
        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), arb_class_counts(), counter())
            .prop_map(|f| SensorLoss {
                sensor_session_events_lost: f.0,
                process_session_events_lost: f.1,
                sensor_session_buffers_lost: f.2,
                process_session_buffers_lost: f.3,
                kernel_queue_drops: f.4,
                user_queue_drops: f.5,
                dns_rate_limit_drops: f.6,
                actor_dropped: f.7,
                buffer_backlog_drops: f.8,
            });
    let quality = (
        (counter(), counter(), counter(), counter(), arb_class_counts(), counter(), counter(), counter()),
        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
    )
        .prop_map(|(a, b)| SensorQuality {
            late_arrivals: a.0,
            parse_errors: a.1,
            unknown_version: a.2,
            launch_join_miss: a.3,
            actor_unresolved: a.4,
            unknown_file_object: a.5,
            registry_unresolved: a.6,
            value_read_failed: a.7,
            early_read_redone: b.0,
            reg_type_unusual: b.1,
            file_op_late_failure: b.2,
            writes_after_cleanup: b.3,
            file_object_replaced: b.4,
            buffer_invalid_records: b.5,
            enrichment_misses: b.6,
            enrichment_errors: b.7,
        });
    let housekeeping = (
        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
        (any::<Option<bool>>(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
    )
        .prop_map(|(a, b)| SensorHousekeeping {
            process_cache_evictions: a.0,
            file_map_evictions: a.1,
            key_map_evictions: a.2,
            early_key_map_evictions: a.3,
            flow_table_evictions: a.4,
            hash_cache_evictions: a.5,
            retention_evictions: a.6,
            file_op_failed: a.7,
            pending_overflow: a.8,
            seeding_enabled: b.0,
            seeder_handles_named: b.1,
            seeder_handles_failed: b.2,
            seeder_handles_timed_out: b.3,
            seeder_table_reads: b.4,
            seeder_deferred_rereads: b.5,
            seeder_stuck_helpers: b.6,
            seeder_negative_cache_size: b.7,
        });
    let resources =
        (counter(), counter()).prop_map(|(cpu_time, working_set)| SensorResources { cpu_time, working_set });
    // Gap times are both present (and ordered) or both absent.
    let times = prop::option::of((any::<i64>(), any::<i64>())).prop_map(|t| match t {
        Some((a, b)) => (Some(a.min(b)), Some(a.max(b))),
        None => (None, None),
    });
    let gap = prop::option::of((times, any::<u64>()).prop_map(|((first_time, last_time), events)| SensorGap {
        first_time,
        last_time,
        events,
    }));
    let buffer = (
        (counter(), counter(), any::<Option<bool>>(), counter(), counter()),
        (counter(), any::<Option<bool>>(), counter(), counter(), counter()),
    )
        .prop_map(|(a, b)| SensorBuffer {
            write_errors: a.0,
            recoveries: a.1,
            failing: a.2,
            rejected: a.3,
            delete_failures: a.4,
            truncated_bytes: b.0,
            cursor_reset: b.1,
            foreign_segments: b.2,
            corrupt_segments: b.3,
            disk_bytes: b.4,
        });
    (loss, quality, housekeeping, resources, gap, buffer)
        .prop_map(|(loss, quality, housekeeping, resources, gap, buffer)| HealthReport {
            loss,
            quality,
            housekeeping,
            resources,
            gap,
            buffer,
        })
        .boxed()
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
        Just(FileAction::Open),
    ];
    let file = (arb_process_ref(), arb_file(), file_action)
        .prop_map(|(actor, file, action)| EventKind::File(FileSystemActivity { actor, file, action }));

    let key_action = prop_oneof![
        Just(RegistryKeyAction::Create),
        Just(RegistryKeyAction::Delete),
        arb_string(40).prop_map(|prev_path| RegistryKeyAction::Rename { prev_path }),
    ];
    let reg_key = (arb_process_ref(), arb_string(40), any::<bool>(), key_action).prop_map(
        |(actor, path, path_unresolved, action)| {
            EventKind::RegistryKey(RegistryKeyActivity { actor, path, path_unresolved, action })
        },
    );

    // (data, data_truncated, data_unavailable): unavailable requires no data and no truncation.
    let value_data = prop_oneof![
        (prop::collection::vec(any::<u8>(), 0..64), any::<bool>())
            .prop_map(|(data, truncated)| (data, truncated, false)),
        Just((vec![], false, true)),
    ];
    let value_action = prop_oneof![
        (arb_reg_type(), value_data, any::<bool>()).prop_map(
            |(value_type, (data, data_truncated, data_unavailable), data_read_after)| RegistryValueAction::Set {
                value_type,
                data,
                data_truncated,
                data_read_after,
                data_unavailable,
            }
        ),
        Just(RegistryValueAction::Delete),
    ];
    let reg_value = (arb_process_ref(), arb_string(40), arb_string(20), any::<bool>(), value_action).prop_map(
        |(actor, key_path, name, path_unresolved, action)| {
            EventKind::RegistryValue(RegistryValueActivity { actor, key_path, name, path_unresolved, action })
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

    // `log_name` is never empty, and neither is `log_provider` for Disable.
    let log_action =
        prop_oneof![Just(EventLogAction::Stop), Just(EventLogAction::Restart), Just(EventLogAction::Disable)];
    let event_log =
        (prop::option::of(arb_process_ref()), "[A-Za-z-]{1,20}", "[A-Za-z-]{1,20}", any::<Option<u32>>(), log_action)
            .prop_map(|(actor, log_name, log_provider, status_code, action)| {
                EventKind::EventLog(EventLogActivity { actor, log_name, log_provider, status_code, action })
            });

    let sensor_health = (any::<i64>(), arb_health_report()).prop_map(|(interval_start, report)| {
        EventKind::SensorHealth(SensorHealthActivity {
            interval_start,
            action: SensorHealthAction::Report(Box::new(report)),
        })
    });

    prop_oneof![
        process.boxed(),
        module.boxed(),
        network.boxed(),
        file.boxed(),
        reg_key.boxed(),
        reg_value.boxed(),
        dns.boxed(),
        event_log.boxed(),
        sensor_health.boxed(),
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
