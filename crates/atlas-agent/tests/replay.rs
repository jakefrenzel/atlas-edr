//! Full-pipeline replay (sensor spec §12.2; plan 1b-3 decision Q3): the CI
//! runner's recording (`atlas-etw/tests/fixtures/scenario.jsonl`) goes through
//! the parsers, the intake and the pipeline with fake Windows services, and the
//! output is checked two ways:
//! - scenario assertions: what the live scenario must produce;
//! - a golden snapshot of every emitted event (`tests/snapshots/scenario.jsonl`,
//!   protobuf-JSON, one event per line). Regenerate it with
//!   `ATLAS_UPDATE_SNAPSHOT=1 cargo test -p atlas-agent --test replay` and review
//!   the diff against the assertions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::sync_channel;

use atlas_agent::config::{Config, Ticks};
use atlas_agent::counters::IntakeCounters;
use atlas_agent::fakes::{FakeLookups, sequential_ids};
use atlas_agent::input::{Header, Session};
use atlas_agent::intake::{Intake, Queues};
use atlas_agent::pipeline::{Pipeline, Setup};
use atlas_agent::process::Identity;
use atlas_agent::services::{LiveProcess, Reply, Request, ValueData};
use atlas_agent::time::Anchor;
use atlas_etw::Provider;
use atlas_etw::parse::{EventMeta, PointerSize, RawEvent, parse};
use atlas_schema::classes::dns::DnsAction;
use atlas_schema::classes::file::FileAction;
use atlas_schema::classes::network::{NetworkAction, NetworkProtocol};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{RegistryKeyAction, RegistryValueAction};
use atlas_schema::{BootId, DeviceUid, Event, EventKind, Hashes, decode_event, encode_event};
use prost_reflect::{DescriptorPool, DynamicMessage};
use serde_json::Value;

/// The runner's QPC frequency (10 MHz on every current Windows).
const FREQ: i64 = 10_000_000;

struct Line {
    header: Header,
    meta: EventMeta,
    payload: Vec<u8>,
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn provider(s: &str) -> Provider {
    match s {
        "kernel-process" => Provider::KernelProcess,
        "kernel-file" => Provider::KernelFile,
        "kernel-registry" => Provider::KernelRegistry,
        "kernel-network" => Provider::KernelNetwork,
        "dns-client" => Provider::DnsClient,
        "process-classic" => Provider::ClassicProcess,
        other => panic!("unknown provider {other}"),
    }
}

fn fixture() -> Vec<Line> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../atlas-etw/tests/fixtures/scenario.jsonl");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            let provider = provider(v["provider"].as_str().unwrap());
            let classic = provider == Provider::ClassicProcess;
            let flags = u16::from_str_radix(v["flags"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
            Line {
                header: Header {
                    session: if classic { Session::Process } else { Session::Sensor },
                    pid: v["pid"].as_u64().unwrap() as u32,
                    tid: v["tid"].as_u64().unwrap() as u32,
                    ts: v["ts"].as_i64().unwrap(),
                    start_key: v["start_key"]
                        .as_str()
                        .map(|k| u64::from_str_radix(k.trim_start_matches("0x"), 16).unwrap()),
                },
                meta: EventMeta {
                    provider,
                    id: (if classic { v["opcode"].as_u64() } else { v["id"].as_u64() }).unwrap() as u16,
                    version: v["version"].as_u64().unwrap() as u8,
                    pointer_size: if flags & 0x20 != 0 { PointerSize::P32 } else { PointerSize::P64 },
                },
                payload: hex(v["raw"].as_str().unwrap()),
            }
        })
        .collect()
}

/// What 1b-3b's telemetry query would say about the processes already running
/// when the sessions started: their start keys, from the events they logged.
fn live_processes(lines: &[Line]) -> HashMap<u32, LiveProcess> {
    let mut keys: HashMap<u32, u64> = HashMap::new();
    for l in lines {
        if let Some(k) = l.header.start_key {
            keys.entry(l.header.pid).or_insert(k);
        }
    }
    let mut out = HashMap::new();
    for l in lines {
        if let Ok(RawEvent::ClassicProcess(c)) = parse(&l.meta, &l.payload)
            && let Some(k) = keys.get(&c.pid)
        {
            out.insert(c.pid, LiveProcess { start_key: *k, image_path: String::new(), command_line: None });
        }
    }
    out
}

fn run() -> (Vec<Event>, atlas_agent::counters::Counters) {
    let lines = fixture();
    let first = lines.first().unwrap().header.ts;
    let last = lines.iter().map(|l| l.header.ts).max().unwrap();
    let kernel_boot_id = lines.iter().find_map(|l| l.header.start_key).unwrap() >> 48;
    let lookups = FakeLookups { live: live_processes(&lines), ..FakeLookups::default() }
        .with_device(r"\Device\HarddiskVolume4", "C:")
        .with_device(r"\Device\HarddiskVolume5", "D:");
    let config = Config::default();
    let ticks = Ticks::new(FREQ);
    let setup = Setup {
        config: config.clone(),
        ticks,
        anchor: Anchor { qpc: first, unix_ns: 1_790_000_000_000_000_000 },
        identity: Identity {
            device: DeviceUid::from_bytes([0xd0; 16]),
            boot: BootId::from_bytes([0xb0; 16]),
            kernel_boot_id: kernel_boot_id as u16,
        },
        current_control_set: 1,
        self_keys: vec![],
        started: first,
    };
    let mut p = Pipeline::new(setup, lookups, sequential_ids()).unwrap();
    let (ktx, krx) = sync_channel(65_536);
    let (utx, urx) = sync_channel(8_192);
    let counters = Arc::new(IntakeCounters::default());
    let mut intake =
        Intake::new(Session::Sensor, &config, ticks, Queues { kernel: ktx, user: utx }, counters.clone(), None, &[]);
    for l in &lines {
        intake.on_event(l.header, parse(&l.meta, &l.payload));
    }
    let mut out = Vec::new();
    let step = FREQ / 10; // 100 ms ticks
    let mut now = first;
    loop {
        for inc in krx.try_iter().chain(urx.try_iter()) {
            p.push(inc);
        }
        out.extend(p.tick(now));
        for r in p.take_requests() {
            answer(&mut p, r);
        }
        if now > last + 70 * FREQ {
            break;
        }
        now += step;
    }
    out.extend(p.stop());
    assert_eq!(IntakeCounters::get(&counters.parse_errors), 0);
    (out, p.counters())
}

/// The fake workers: a fixed hash for every file, the scenario's DWORD for the
/// value it sets, the runner's long user name for 8.3 names, nothing from the seeder.
fn answer(p: &mut Pipeline<FakeLookups>, r: Request) {
    match r {
        Request::Enrich { id, .. } => p.reply(Reply::Enriched {
            id,
            hashes: Some(Hashes { sha256: Some([0xaa; 32]) }),
            signature: None,
            error: false,
        }),
        Request::ReadValue { id, read } => {
            let result = (read.value_name == [u16::from(b'v')]).then(|| ValueData {
                value_type: 4,
                size: 4,
                data: 7u32.to_le_bytes().to_vec(),
            });
            p.reply(Reply::ValueRead { id, result });
        }
        // The runner's user is `runneradmin`, which 8.3 shortens to `RUNNER~1`.
        Request::Expand { id, slot, nt_path } => {
            let long_path = nt_path.contains("RUNNER~1").then(|| nt_path.replace("RUNNER~1", "runneradmin"));
            p.reply(Reply::Expanded { id, slot, long_path });
        }
        Request::Seed { .. } | Request::InvalidateHash { .. } => {}
    }
}

fn snapshot_line(e: &Event) -> String {
    let pool = DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET).unwrap();
    let desc = pool.get_message_by_name("atlas.events.v1.Event").unwrap();
    let msg = DynamicMessage::decode(desc, encode_event(e.clone()).as_slice()).unwrap();
    serde_json::to_string(&msg).unwrap()
}

#[test]
fn the_ci_recording_produces_the_scenario() {
    let (out, counters) = run();
    for e in &out {
        let back = decode_event(&encode_event(e.clone())).unwrap_or_else(|err| panic!("{err}: {e:?}"));
        assert_eq!(&back, e, "every event passes the schema's validation unchanged");
    }

    // Process: the actor started cmd.exe /c "exit 7", which exited with 7.
    let launch = out
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::Process(ProcessActivity::Launch { process, actor }) if process.file.name == "cmd.exe" => {
                Some((process.clone(), actor.clone()))
            }
            _ => None,
        })
        .expect("the cmd.exe Launch");
    assert_eq!(launch.0.cmd_line, r#""C:\Windows\System32\cmd.exe" /c "exit 7""#);
    assert_eq!(launch.0.file.path, r"C:\Windows\System32\cmd.exe");
    assert!(launch.0.user.as_ref().is_some_and(|u| u.uid.starts_with("S-1-5-21-")));
    assert_eq!(launch.0.file.hashes, Some(Hashes { sha256: Some([0xaa; 32]) }));
    assert!(launch.1.file.name.starts_with("live-"), "actor: {:?}", launch.1);
    assert_eq!(launch.0.parent_process.as_ref().map(|p| p.uid), Some(launch.1.uid));
    assert!(
        out.iter().any(|e| matches!(&e.kind,
        EventKind::Process(ProcessActivity::Terminate { process, exit_code: Some(7) }) if process.uid == launch.0.uid))
    );
    assert!(out.iter().any(|e| matches!(&e.kind, EventKind::Module(m) if m.actor.uid == launch.0.uid)));

    // Files: the scenario's creates, updates, attribute change, rename, deletes.
    let files: Vec<(String, String)> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::File(f) if f.file.path.contains("atlas-etw-live-") || f.file.path.contains("ATLAS-") => {
                let name = f.file.path.rsplit('\\').next().unwrap().to_string();
                let action = match &f.action {
                    FileAction::Rename { file_result } => format!("rename→{}", file_result.name),
                    a => format!("{a:?}").to_lowercase(),
                };
                Some((action, name))
            }
            _ => None,
        })
        .collect();
    let has = |a: &str, n: &str| files.iter().any(|(x, y)| x == a && y == n);
    assert!(has("create", "a.txt"), "{files:?}");
    assert!(files.iter().filter(|(a, n)| a == "update" && n == "a.txt").count() >= 2, "{files:?}"); // write, then overwrite
    assert!(has("setattributes", "a.txt"), "{files:?}");
    assert!(has("rename→b.txt", "a.txt"), "{files:?}");
    assert!(has("delete", "b.txt"), "{files:?}");
    assert!(has("delete", "d.txt"), "delete-on-close: {files:?}");
    // c.txt: the failed CREATE_NEW and the refused delete leave no event; the
    // final delete (after clearing read-only) does.
    assert_eq!(files.iter().filter(|(a, n)| a == "delete" && n == "c.txt").count(), 1, "{files:?}");
    assert!(counters.file_op_failed >= 2, "{counters:?}");
    // Deletes come from the Cleanup outcome (plan 1b-3c): the undelete gives
    // none (u.txt's one Delete is the directory's removal at the end), the
    // hard link and the stream each give their own.
    let deletes = |n: &str| files.iter().filter(|(a, x)| a == "delete" && x == n).count();
    assert_eq!(deletes("u.txt"), 1, "{files:?}");
    assert_eq!(deletes("l2.txt"), 1, "{files:?}");
    assert_eq!(deletes("s.txt:x"), 1, "{files:?}");
    assert_eq!(counters.file_delete_outcome_unknown, 0, "{counters:?}");
    // One spelling: no emitted path keeps the runner's 8.3 user name (plan 1b-3a Q4).
    for e in &out {
        if let EventKind::File(f) = &e.kind {
            assert!(!f.file.path.contains("RUNNER~1"), "{}", f.file.path);
        }
    }

    // Registry: the test key, its value (read after the event), and the
    // embedded-NUL names kept whole.
    let key = out
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::RegistryKey(k) if k.action == RegistryKeyAction::Create && k.path.contains("AtlasEtwLive-") => {
                Some(k.path.clone())
            }
            _ => None,
        })
        .expect("the test key's Create");
    let (head, pid) = key.rsplit_once('-').unwrap();
    assert!(key.starts_with(r"HKU\S-1-5-21-") && head.ends_with(r"\Software\AtlasEtwLive"), "{key}");
    assert!(pid.parse::<u32>().is_ok(), "the actor's PID: {key}");
    let set = out.iter().find_map(|e| match &e.kind {
        EventKind::RegistryValue(v) if v.name == "v" => match &v.action {
            RegistryValueAction::Set { data, data_read_after, data_unavailable, .. } => {
                Some((v.key_path.clone(), data.clone(), *data_read_after, *data_unavailable))
            }
            RegistryValueAction::Delete => None,
        },
        _ => None,
    });
    assert_eq!(set, Some((key.clone(), 7u32.to_le_bytes().to_vec(), true, false)));
    assert!(out.iter().any(|e| matches!(&e.kind, EventKind::RegistryValue(v)
        if v.name == "a\0b" && v.action == RegistryValueAction::Delete && v.key_path == key)));
    assert!(out.iter().any(|e| matches!(&e.kind, EventKind::RegistryKey(k)
        if k.action == RegistryKeyAction::Create && k.path == format!("{key}\\k\0x"))));
    assert!(out.iter().any(|e| matches!(&e.kind, EventKind::RegistryKey(k)
        if k.action == RegistryKeyAction::Delete && k.path == key)));

    // Network: TCP open and close over IPv4 and IPv6, and one UDP flow each.
    let net = |proto, v6: bool, open: bool| {
        out.iter()
            .filter(|e| matches!(&e.kind, EventKind::Network(n)
                if n.protocol == proto && n.src_endpoint.ip.is_ipv6() == v6 && matches!(n.action, NetworkAction::Open) == open))
            .count()
    };
    for v6 in [false, true] {
        assert_eq!(net(NetworkProtocol::Tcp, v6, true), 2, "connect and accept, v6: {v6}");
        assert_eq!(net(NetworkProtocol::Tcp, v6, false), 2, "both disconnects, v6: {v6}");
        assert_eq!((net(NetworkProtocol::Udp, v6, true), net(NetworkProtocol::Udp, v6, false)), (2, 2), "v6: {v6}");
    }

    // DNS: the three lookups, the .invalid one as NXDOMAIN.
    let dns: Vec<(String, Option<u16>)> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Dns(d) => {
                let DnsAction::Response { rcode, .. } = &d.action;
                Some((d.hostname.clone(), *rcode))
            }
            _ => None,
        })
        .collect();
    assert_eq!(dns.len(), 3, "{dns:?}");
    assert!(dns.contains(&("atlas-etw-live.invalid".into(), Some(3))), "{dns:?}");
}

#[test]
fn the_output_matches_the_snapshot() {
    let (out, _) = run();
    let text: String = out.iter().map(|e| format!("{}\n", snapshot_line(e))).collect();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/scenario.jsonl");
    if std::env::var_os("ATLAS_UPDATE_SNAPSHOT").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {}; run with ATLAS_UPDATE_SNAPSHOT=1", path.display()))
        .replace("\r\n", "\n");
    let (got, want): (Vec<&str>, Vec<&str>) = (text.lines().collect(), want.lines().collect());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "event {i} differs");
    }
    assert_eq!(got.len(), want.len(), "event count differs");
}

#[test]
fn the_replay_is_deterministic() {
    let a: Vec<Vec<u8>> = run().0.into_iter().map(encode_event).collect();
    let b: Vec<Vec<u8>> = run().0.into_iter().map(encode_event).collect();
    assert_eq!(a, b);
}
