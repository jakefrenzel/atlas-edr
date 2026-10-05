//! Every validation rule in spec 6.1 has a negative case here, asserting the
//! exact field path and error kind. Boundary cases (exactly at a limit) must pass.

mod common;

use atlas_schema::limits::*;
use atlas_schema::wire::{self, event::Kind};
use atlas_schema::{Event, SchemaError, SchemaErrorKind as K, decode_event};
use prost::Message;

fn wire_sample(name: &str) -> wire::Event {
    let (_, event) = common::samples().into_iter().find(|(n, _)| *n == name).expect("sample exists");
    wire::Event::from(event)
}

fn check(w: wire::Event) -> Result<Event, SchemaError> {
    Event::try_from(w)
}

// ---- accessors into the wire tree (panic if the sample has another shape) ----

fn launch(w: &mut wire::Event) -> &mut wire::ProcessLaunch {
    use wire::process_activity::Activity;
    match w.kind.as_mut() {
        Some(Kind::Process(wire::ProcessActivity { activity: Some(Activity::Launch(l)) })) => l,
        _ => panic!("not a launch"),
    }
}

fn launch_process(w: &mut wire::Event) -> &mut wire::Process {
    launch(w).process.as_mut().expect("process")
}

fn launch_file(w: &mut wire::Event) -> &mut wire::File {
    launch_process(w).file.as_mut().expect("file")
}

fn network(w: &mut wire::Event) -> &mut wire::NetworkActivity {
    match w.kind.as_mut() {
        Some(Kind::Network(n)) => n,
        _ => panic!("not network"),
    }
}

fn file_activity(w: &mut wire::Event) -> &mut wire::FileSystemActivity {
    match w.kind.as_mut() {
        Some(Kind::File(f)) => f,
        _ => panic!("not file"),
    }
}

fn reg_key(w: &mut wire::Event) -> &mut wire::RegistryKeyActivity {
    match w.kind.as_mut() {
        Some(Kind::RegistryKey(k)) => k,
        _ => panic!("not registry key"),
    }
}

fn reg_value(w: &mut wire::Event) -> &mut wire::RegistryValueActivity {
    match w.kind.as_mut() {
        Some(Kind::RegistryValue(v)) => v,
        _ => panic!("not registry value"),
    }
}

fn reg_value_set(w: &mut wire::Event) -> &mut wire::RegistryValueSet {
    use wire::registry_value_activity::Activity;
    match reg_value(w).activity.as_mut() {
        Some(Activity::Set(s)) => s,
        _ => panic!("not a set"),
    }
}

fn dns_activity(w: &mut wire::Event) -> &mut wire::DnsActivity {
    match w.kind.as_mut() {
        Some(Kind::Dns(d)) => d,
        _ => panic!("not dns"),
    }
}

fn dns_response(w: &mut wire::Event) -> &mut wire::DnsResponse {
    use wire::dns_activity::Activity;
    match dns_activity(w).activity.as_mut() {
        Some(Activity::Response(r)) => r,
        None => panic!("no response"),
    }
}

fn event_log(w: &mut wire::Event) -> &mut wire::EventLogActivity {
    match w.kind.as_mut() {
        Some(Kind::EventLog(e)) => e,
        _ => panic!("not event log"),
    }
}

fn health_report(w: &mut wire::Event) -> &mut wire::SensorHealthReport {
    use wire::sensor_health::Activity;
    match w.kind.as_mut() {
        Some(Kind::SensorHealth(wire::SensorHealth { activity: Some(Activity::Report(r)), .. })) => r,
        _ => panic!("not a sensor health report"),
    }
}

fn long(n: usize) -> String {
    "a".repeat(n)
}

// ---- the rule table ----

type Mutation = fn(&mut wire::Event);

const CASES: &[(&str, Mutation, &str, K)] = &[
    // envelope
    ("process_launch", |w| w.event_id.truncate(15), "event_id", K::Malformed),
    ("process_launch", |w| w.event_id[6] = 0x40, "event_id", K::Malformed), // version 4, not 7
    ("process_launch", |w| w.sensor = 0, "sensor", K::Missing),
    ("process_launch", |w| w.sensor = 99, "sensor", K::UnknownEnum),
    ("process_launch", |w| w.device = None, "device", K::Missing),
    ("process_launch", |w| w.device.as_mut().unwrap().uid.truncate(15), "device.uid", K::Malformed),
    ("process_launch", |w| w.device.as_mut().unwrap().boot_id.clear(), "device.boot_id", K::Malformed),
    ("process_launch", |w| w.kind = None, "kind", K::Missing),
    // process
    (
        "process_launch",
        |w| match w.kind.as_mut() {
            Some(Kind::Process(p)) => p.activity = None,
            _ => unreachable!(),
        },
        "activity",
        K::Missing,
    ),
    ("process_launch", |w| launch(w).actor = None, "actor.process", K::Missing),
    ("process_launch", |w| launch(w).actor.as_mut().unwrap().file = None, "actor.process.file", K::Missing),
    ("process_launch", |w| launch(w).process = None, "process", K::Missing),
    ("process_launch", |w| launch_process(w).file = None, "process.file", K::Missing),
    ("process_launch", |w| launch_file(w).path = long(PATH_MAX + 1), "process.file.path", K::TooLarge),
    ("process_launch", |w| launch_file(w).name = long(PATH_MAX + 1), "process.file.name", K::TooLarge),
    ("process_launch", |w| launch_process(w).cmd_line = long(CMD_LINE_MAX + 1), "process.cmd_line", K::TooLarge),
    ("process_launch", |w| launch_process(w).uid.push(0), "process.uid", K::Malformed),
    ("process_launch", |w| launch_process(w).integrity = Some(0), "process.integrity", K::Missing),
    ("process_launch", |w| launch_process(w).integrity = Some(42), "process.integrity", K::UnknownEnum),
    (
        "process_launch",
        |w| launch_process(w).parent_process.as_mut().unwrap().uid.truncate(3),
        "process.parent_process.uid",
        K::Malformed,
    ),
    (
        "process_launch",
        |w| launch_file(w).hashes.as_mut().unwrap().sha256.as_mut().unwrap().truncate(31),
        "process.file.hashes.sha256",
        K::Malformed,
    ),
    (
        "process_launch",
        |w| launch_file(w).signature.as_mut().unwrap().status = 0,
        "process.file.signature.status",
        K::Missing,
    ),
    (
        "process_terminate",
        |w| match w.kind.as_mut() {
            Some(Kind::Process(wire::ProcessActivity {
                activity: Some(wire::process_activity::Activity::Terminate(t)),
            })) => t.process = None,
            _ => unreachable!(),
        },
        "process",
        K::Missing,
    ),
    // module
    (
        "module_load",
        |w| match w.kind.as_mut() {
            Some(Kind::Module(wire::ModuleActivity {
                activity: Some(wire::module_activity::Activity::Load(l)),
                ..
            })) => l.file = None,
            _ => unreachable!(),
        },
        "module.file",
        K::Missing,
    ),
    (
        "module_load",
        |w| match w.kind.as_mut() {
            Some(Kind::Module(m)) => m.actor = None,
            _ => unreachable!(),
        },
        "actor.process",
        K::Missing,
    ),
    // network
    ("network_open", |w| network(w).src_endpoint = None, "src_endpoint", K::Missing),
    (
        "network_open",
        |w| network(w).dst_endpoint.as_mut().unwrap().ip = vec![1, 2, 3, 4, 5],
        "dst_endpoint.ip",
        K::Malformed,
    ),
    ("network_open", |w| network(w).src_endpoint.as_mut().unwrap().port = 70_000, "src_endpoint.port", K::Malformed),
    ("network_open", |w| network(w).protocol = 0, "protocol", K::Missing),
    ("network_open", |w| network(w).direction = 9, "direction", K::UnknownEnum),
    ("network_open", |w| network(w).activity = None, "activity", K::Missing),
    // file
    ("file_create", |w| file_activity(w).file = None, "file", K::Missing),
    (
        "file_rename",
        |w| match file_activity(w).activity.as_mut() {
            Some(wire::file_system_activity::Activity::Rename(r)) => r.file_result = None,
            _ => unreachable!(),
        },
        "file_result",
        K::Missing,
    ),
    (
        "file_rename",
        |w| match file_activity(w).activity.as_mut() {
            Some(wire::file_system_activity::Activity::Rename(r)) => {
                r.file_result.as_mut().unwrap().path = long(PATH_MAX + 1)
            }
            _ => unreachable!(),
        },
        "file_result.path",
        K::TooLarge,
    ),
    // registry key
    ("registry_key_create", |w| reg_key(w).path = long(PATH_MAX + 1), "reg_key.path", K::TooLarge),
    (
        "registry_key_rename",
        |w| match reg_key(w).activity.as_mut() {
            Some(wire::registry_key_activity::Activity::Rename(r)) => r.prev_path = long(PATH_MAX + 1),
            _ => unreachable!(),
        },
        "prev_reg_key.path",
        K::TooLarge,
    ),
    // registry value
    ("registry_value_set", |w| reg_value(w).key_path = long(PATH_MAX + 1), "reg_value.path", K::TooLarge),
    ("registry_value_set", |w| reg_value(w).name = long(PATH_MAX + 1), "reg_value.name", K::TooLarge),
    // user / signer strings (bounded so attacker-controlled actor fields stay small)
    (
        "process_launch",
        |w| launch(w).actor.as_mut().unwrap().user.as_mut().unwrap().uid = long(USER_UID_MAX + 1),
        "actor.process.user.uid",
        K::TooLarge,
    ),
    (
        "process_launch",
        |w| launch_process(w).user.as_mut().unwrap().name = long(USER_NAME_MAX + 1),
        "process.user.name",
        K::TooLarge,
    ),
    (
        "process_launch",
        |w| launch_process(w).parent_process.as_mut().unwrap().user.as_mut().unwrap().uid = long(USER_UID_MAX + 1),
        "process.parent_process.user.uid",
        K::TooLarge,
    ),
    (
        "process_launch",
        |w| launch_file(w).signature.as_mut().unwrap().signer = Some(long(SIGNER_MAX + 1)),
        "process.file.signature.signer",
        K::TooLarge,
    ),
    ("registry_value_set", |w| reg_value_set(w).r#type = Some(12), "reg_value.type", K::UnknownEnum),
    ("registry_value_set", |w| reg_value_set(w).data = vec![0; REG_DATA_MAX + 1], "reg_value.data", K::TooLarge),
    // registry value: sub-project 1 fields (sensor spec §10.4)
    ("registry_value_set", |w| reg_value_set(w).raw_type = Some(12), "reg_value.raw_type", K::Malformed),
    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = Some(11), "reg_value.raw_type", K::Malformed),
    ("registry_value_set", |w| reg_value_set(w).data_unavailable = true, "reg_value.data_unavailable", K::Malformed),
    (
        "registry_value_set_unavailable",
        |w| reg_value_set(w).data_truncated = true,
        "reg_value.data_unavailable",
        K::Malformed,
    ),
    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = None, "reg_value.type", K::Missing),
    // event log
    ("event_log_stop", |w| event_log(w).activity = None, "activity", K::Missing),
    ("event_log_stop", |w| event_log(w).log_name.clear(), "log_name", K::Missing),
    ("event_log_stop", |w| event_log(w).log_name = long(EVENT_LOG_NAME_MAX + 1), "log_name", K::TooLarge),
    ("event_log_disable", |w| event_log(w).log_provider.clear(), "log_provider", K::Missing),
    ("event_log_restart", |w| event_log(w).log_provider = long(EVENT_LOG_NAME_MAX + 1), "log_provider", K::TooLarge),
    (
        "event_log_stop",
        |w| event_log(w).actor = Some(wire::ProcessRef { uid: vec![0; 3], ..Default::default() }),
        "actor.process.uid",
        K::Malformed,
    ),
    // sensor health
    (
        "sensor_health_report",
        |w| match w.kind.as_mut() {
            Some(Kind::SensorHealth(h)) => h.activity = None,
            _ => unreachable!(),
        },
        "activity",
        K::Missing,
    ),
    (
        "sensor_health_report",
        |w| {
            let q = health_report(w).quality.as_mut().unwrap();
            q.actor_unresolved = vec![wire::ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX + 1];
        },
        "quality.actor_unresolved",
        K::TooLarge,
    ),
    (
        "sensor_health_report",
        |w| {
            let l = health_report(w).loss.as_mut().unwrap();
            l.actor_dropped = vec![wire::ClassCount { class_uid: 4001, count: 1 }; CLASS_COUNTS_MAX + 1];
        },
        "loss.actor_dropped",
        K::TooLarge,
    ),
    (
        "sensor_health_report",
        |w| health_report(w).gap.as_mut().unwrap().first_time = Some(i64::MAX),
        "gap.last_time",
        K::Malformed,
    ),
    ("sensor_health_report", |w| health_report(w).gap.as_mut().unwrap().last_time = None, "gap", K::Malformed),
    // dns
    ("dns_response", |w| dns_activity(w).hostname = long(DNS_HOSTNAME_MAX + 1), "query.hostname", K::TooLarge),
    ("dns_response", |w| dns_activity(w).query_type = 65_536, "query.type", K::Malformed),
    ("dns_response", |w| dns_response(w).rcode = Some(70_000), "rcode", K::Malformed),
    (
        "dns_response",
        |w| {
            let a = dns_response(w).answers[0].clone();
            dns_response(w).answers = vec![a; DNS_ANSWERS_MAX + 1];
        },
        "answers",
        K::TooLarge,
    ),
    (
        "dns_response",
        |w| {
            let r = dns_response(w);
            let a = r.answers[0].clone();
            r.answers = vec![a.clone(), a.clone(), wire::DnsAnswer { data: long(DNS_ANSWER_DATA_MAX + 1), ..a }];
        },
        "answers[2].data",
        K::TooLarge,
    ),
    ("dns_response", |w| dns_response(w).answers[0].r#type = 70_000, "answers[0].type", K::Malformed),
];

#[test]
fn every_rule_rejects_with_exact_path_and_kind() {
    for (i, (sample, mutate, path, kind)) in CASES.iter().enumerate() {
        let mut w = wire_sample(sample);
        mutate(&mut w);
        let err = check(w).expect_err(&format!("case {i} ({path}) should be rejected"));
        assert_eq!((err.field_path.as_str(), err.kind), (*path, *kind), "case {i}");
    }
}

// ---- boundaries: exactly at the limit is accepted ----

const AT_LIMIT: &[(&str, Mutation)] = &[
    ("process_launch", |w| launch_file(w).path = long(PATH_MAX)),
    ("process_launch", |w| launch_process(w).cmd_line = long(CMD_LINE_MAX)),
    ("registry_value_set", |w| reg_value_set(w).data = vec![0; REG_DATA_MAX]),
    ("dns_response", |w| dns_activity(w).hostname = long(DNS_HOSTNAME_MAX)),
    ("dns_response", |w| {
        let a = dns_response(w).answers[0].clone();
        dns_response(w).answers = vec![a; DNS_ANSWERS_MAX];
    }),
    ("network_open", |w| network(w).src_endpoint.as_mut().unwrap().port = 65_535),
    ("event_log_disable", |w| {
        let e = event_log(w);
        (e.log_name, e.log_provider) = (long(EVENT_LOG_NAME_MAX), long(EVENT_LOG_NAME_MAX));
    }),
    ("sensor_health_report", |w| {
        let q = health_report(w).quality.as_mut().unwrap();
        q.actor_unresolved = vec![wire::ClassCount { class_uid: 1001, count: 1 }; CLASS_COUNTS_MAX];
    }),
    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = Some(12)),
    ("registry_value_set_unavailable", |w| reg_value_set(w).raw_type = Some(u32::MAX)),
];

#[test]
fn values_exactly_at_limits_are_accepted() {
    for (i, (sample, mutate)) in AT_LIMIT.iter().enumerate() {
        let mut w = wire_sample(sample);
        mutate(&mut w);
        check(w).unwrap_or_else(|e| panic!("case {i}: {e}"));
    }
}

// ---- optional fields may be absent ----

#[test]
fn optional_fields_may_be_absent() {
    let mut w = wire_sample("process_launch");
    let p = launch_process(&mut w);
    p.user = None;
    p.integrity = None;
    p.parent_process = None;
    let f = p.file.as_mut().unwrap();
    f.hashes = None;
    f.signature = None;
    check(w).expect("optional fields are optional");

    // Event Log Activity: no actor and no status code (the samples already omit both).
    check(wire_sample("event_log_stop")).expect("actor and status_code are optional");

    // Sensor Health: every group and the gap may be absent.
    let mut w = wire_sample("sensor_health_report");
    *health_report(&mut w) = wire::SensorHealthReport::default();
    check(w).expect("every Sensor Health field is optional");
}

#[test]
fn empty_strings_are_allowed() {
    // proto3 cannot tell "absent" from "empty"; e.g. the System process has no image path.
    let mut w = wire_sample("process_launch");
    launch_file(&mut w).path.clear();
    launch_process(&mut w).cmd_line.clear();
    check(w).expect("empty strings are valid");
}

// ---- byte-level checks in decode_event ----

#[test]
fn oversized_input_is_rejected_before_decoding() {
    let err = decode_event(&vec![0u8; EVENT_MAX + 1]).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("event", K::TooLarge));
}

#[test]
fn undecodable_bytes_are_malformed() {
    let err = decode_event(&[0xff, 0xff, 0xff]).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("event", K::Malformed));
}

#[test]
fn invalid_utf8_in_a_string_is_malformed() {
    let mut bytes = wire_sample("registry_key_create").encode_to_vec();
    let at = bytes.windows(4).position(|w| w == b"HKLM").expect("path bytes present");
    bytes[at] = 0xff;
    let err = decode_event(&bytes).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("event", K::Malformed));
}

#[test]
fn class_from_a_newer_schema_is_rejected_as_missing_kind() {
    // A newer agent might send oneof field 19 (a class this build does not know;
    // 17 and 18 are Event Log Activity and Sensor Health since sub-project 1).
    // prost keeps it as an unknown field, so `kind` is absent.
    let mut w = wire_sample("process_launch");
    w.kind = None;
    let mut bytes = w.encode_to_vec();
    bytes.extend(field(19, &[]));
    let err = decode_event(&bytes).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("kind", K::Missing));
}

#[test]
fn activity_from_a_newer_schema_is_rejected_as_missing_activity() {
    // A newer agent might send ProcessActivity oneof field 3 (e.g. Inject).
    let mut w = wire_sample("process_launch");
    w.kind = None;
    let mut bytes = w.encode_to_vec();
    let process_activity = [0x1a, 0x00]; // field 3, wire type 2, length 0
    bytes.extend_from_slice(&[0x52, process_activity.len() as u8]); // Event field 10 (process)
    bytes.extend_from_slice(&process_activity);
    let err = decode_event(&bytes).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("activity", K::Missing));
}

/// A length-delimited protobuf field: key (`tag`, wire type 2), length, bytes.
fn field(tag: u32, inner: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    prost::encoding::encode_key(tag, prost::encoding::WireType::LengthDelimited, &mut out);
    prost::encoding::encode_varint(inner.len() as u64, &mut out);
    out.extend_from_slice(inner);
    out
}

#[test]
fn unknown_activity_is_reported_before_class_fields() {
    // A newer activity may omit fields today's activities require (e.g. a future
    // Network Listen has no dst_endpoint). Version skew must read as `activity: Missing`,
    // not as a missing class field. (sample, Event oneof tag, first unused field tag)
    let cases: &[(&str, u32, u32)] = &[
        ("module_load", 11, 3),
        ("network_open", 12, 8),
        ("file_create", 13, 10),
        ("registry_key_create", 14, 7),
        ("registry_value_set", 15, 7),
        ("dns_response", 16, 5),
        ("event_log_stop", 17, 8),
        ("sensor_health_report", 18, 3),
    ];
    for &(sample, event_tag, unknown_activity_tag) in cases {
        let mut w = wire_sample(sample);
        let mut inner = match w.kind.take().expect("sample has a kind") {
            Kind::Module(mut c) => {
                (c.activity, c.actor) = (None, None);
                c.encode_to_vec()
            }
            Kind::Network(mut c) => {
                (c.activity, c.actor, c.dst_endpoint) = (None, None, None);
                c.encode_to_vec()
            }
            Kind::File(mut c) => {
                (c.activity, c.actor, c.file) = (None, None, None);
                c.encode_to_vec()
            }
            Kind::RegistryKey(mut c) => {
                (c.activity, c.actor) = (None, None);
                c.encode_to_vec()
            }
            Kind::RegistryValue(mut c) => {
                (c.activity, c.actor) = (None, None);
                c.encode_to_vec()
            }
            Kind::Dns(mut c) => {
                (c.activity, c.actor) = (None, None);
                c.encode_to_vec()
            }
            Kind::EventLog(mut c) => {
                // `log_name` is required: the unknown activity must still be reported first.
                (c.activity, c.log_name) = (None, String::new());
                c.encode_to_vec()
            }
            Kind::SensorHealth(mut c) => {
                c.activity = None;
                c.encode_to_vec()
            }
            Kind::Process(_) => unreachable!("process has no class-level fields"),
        };
        inner.extend(field(unknown_activity_tag, &[]));
        let mut bytes = w.encode_to_vec();
        bytes.extend(field(event_tag, &inner));
        let err = decode_event(&bytes).unwrap_err();
        assert_eq!((err.field_path.as_str(), err.kind), ("activity", K::Missing), "{sample}");
    }
}

#[test]
fn registry_value_set_without_type_is_missing_not_reg_none() {
    // proto3 cannot tell an absent uint32 from 0, so `type` needs explicit presence;
    // otherwise an unset type silently becomes REG_NONE (spec 6.1).
    let mut w = wire_sample("registry_value_set");
    reg_value_set(&mut w).r#type = None; // field absent on the wire
    let err = check(w).unwrap_err();
    assert_eq!((err.field_path.as_str(), err.kind), ("reg_value.type", K::Missing));
}

#[test]
fn explicit_reg_none_is_a_valid_value_type() {
    let mut w = wire_sample("registry_value_set");
    reg_value_set(&mut w).r#type = Some(0); // REG_NONE, present on the wire
    check(w).expect("REG_NONE is a real Windows value type");
}
