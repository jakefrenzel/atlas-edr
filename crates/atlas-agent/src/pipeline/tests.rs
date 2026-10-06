//! Pipeline scenarios (sensor spec §12.1): synthetic events in, domain events
//! out, with a fake clock and fake Windows services.

use std::net::IpAddr;

use atlas_etw::parse::*;
use atlas_schema::classes::dns::DnsAction;
use atlas_schema::classes::file::FileAction;
use atlas_schema::classes::network::{NetworkAction, NetworkDirection, NetworkProtocol};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{RegType, RegValueType, RegistryKeyAction, RegistryValueAction};
use atlas_schema::{BootId, DeviceUid, Event, EventKind, Hashes, Integrity, decode_event, encode_event};

use super::{Pipeline, Setup};
use crate::config::{Config, Ticks};
use crate::fakes::{FakeLookups, sequential_ids};
use crate::input::{Header, Incoming, Session};
use crate::process::{Identity, start_key};
use crate::services::{
    EarlyKey, EnrichTarget, HandleKind, LiveProcess, Named, Reply, Request, Snapshot, ValueData, ValueRead,
};
use crate::time::Anchor;

const FREQ: i64 = 10_000_000;
const BOOT: u16 = 7;
const AGENT: u64 = 0x0007_0000_0000_0999;
const SID: &str = "S-1-5-21-1-2-3-1001";

fn ms(n: i64) -> i64 {
    n * FREQ / 1000
}

fn key(seq: u64) -> u64 {
    start_key(BOOT, seq)
}

fn identity() -> Identity {
    Identity { device: DeviceUid::from_bytes([0xd0; 16]), boot: BootId::from_bytes([0xb0; 16]), kernel_boot_id: BOOT }
}

struct T {
    p: Pipeline<FakeLookups>,
    out: Vec<Event>,
    reqs: Vec<Request>,
    now: i64,
}

impl T {
    fn new() -> T {
        T::with(Config::default(), FakeLookups::default())
    }

    fn with(config: Config, lookups: FakeLookups) -> T {
        let lookups = lookups.with_device(r"\Device\HarddiskVolume3", "C:");
        let mut accounts = lookups.accounts.clone();
        accounts.insert(SID.into(), r"DESK\jake".into());
        let lookups = FakeLookups { accounts, ..lookups };
        let setup = Setup {
            config,
            ticks: Ticks::new(FREQ),
            anchor: Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 },
            identity: identity(),
            current_control_set: 1,
            self_keys: vec![AGENT],
            started: 0,
        };
        let mut p = Pipeline::new(setup, lookups, sequential_ids()).unwrap();
        let reqs = p.take_requests();
        T { p, out: Vec::new(), reqs, now: 0 }
    }

    /// An event from Session A (or B for classic process events) at `at` ms.
    fn ev(&mut self, at: i64, pid: u32, start: Option<u64>, event: RawEvent) {
        let session = if matches!(event, RawEvent::ClassicProcess(_)) { Session::Process } else { Session::Sensor };
        let header = Header { session, pid, tid: 1, ts: ms(at), start_key: start };
        self.p.push(Incoming { header, event });
    }

    /// Advances the clock to `at` ms.
    fn at(&mut self, at: i64) -> &mut Self {
        self.now = ms(at);
        self.out.extend(self.p.tick(self.now));
        self.reqs.extend(self.p.take_requests());
        self
    }

    fn reply(&mut self, r: Reply) {
        self.p.reply(r);
    }

    /// Runs far ahead: every hold, window and deadline passes. Two ticks: the
    /// first releases what is held (deadlines start then), the second passes them.
    fn settle(&mut self) -> Vec<Event> {
        let t = self.now / (FREQ / 1000) + 60_000;
        self.at(t);
        self.at(t + 70_000);
        std::mem::take(&mut self.out)
    }

    fn requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.reqs)
    }
}

/// Every emitted event must pass the schema's validating decoder unchanged.
fn valid(events: &[Event]) {
    for e in events {
        let back = decode_event(&encode_event(e.clone())).unwrap_or_else(|err| panic!("{err}: {e:?}"));
        assert_eq!(&back, e);
    }
}

// ---- builders ----

fn wstr(s: &str) -> WStr {
    s.into()
}

/// `S-1-16-<rid>`: a mandatory label.
fn label(rid: u32) -> Sid {
    let mut b = vec![1u8, 1, 0, 0, 0, 0, 0, 16];
    b.extend(rid.to_le_bytes());
    Sid::from_bytes(&b).unwrap()
}

fn kp_start(pid: u32, seq: u64, ppid: u32, pseq: u64, image: &str) -> RawEvent {
    RawEvent::ProcessStart(ProcessStart {
        pid,
        sequence_number: seq,
        create_time: 134_354_398_476_102_067,
        parent_pid: ppid,
        parent_sequence_number: pseq,
        session_id: 1,
        flags: 0,
        token_elevation_type: 3,
        token_is_elevated: 0,
        mandatory_label: label(8192),
        image_name: wstr(image),
        image_checksum: 0,
        time_date_stamp: 0,
        package_full_name: WStr::default(),
        package_relative_app_id: WStr::default(),
        security_mitigations: Some(0),
    })
}

/// `SID` (S-1-5-21-1-2-3-1001).
fn sid() -> Sid {
    let mut b = vec![1u8, 5, 0, 0, 0, 0, 0, 5];
    for s in [21u32, 1, 2, 3, 1001] {
        b.extend(s.to_le_bytes());
    }
    Sid::from_bytes(&b).unwrap()
}

fn classic(kind: ClassicKind, pid: u32, ppid: u32, cmd: &str) -> RawEvent {
    RawEvent::ClassicProcess(ClassicProcess {
        kind,
        unique_process_key: 0,
        pid,
        parent_pid: ppid,
        session_id: 1,
        exit_status: 259,
        directory_table_base: 0,
        flags: 0,
        user_sid: Some(sid()),
        image_file_name: Box::from(&b"x.exe"[..]),
        command_line: wstr(cmd),
        package_full_name: WStr::default(),
        application_id: WStr::default(),
    })
}

fn create(irp: u64, fo: u64, name: &str, options: u32) -> RawEvent {
    RawEvent::FileCreate(FileCreate {
        irp,
        file_object: fo,
        issuing_tid: 1,
        create_options: options,
        create_attributes: 0,
        share_access: 7,
        file_name: wstr(name),
    })
}

fn handle(fo: u64) -> FileHandle {
    FileHandle { irp: 0, file_object: fo, file_key: 0, issuing_tid: 1 }
}

fn write(fo: u64) -> RawEvent {
    RawEvent::FileWrite(FileWrite {
        byte_offset: 0,
        irp: 0,
        file_object: fo,
        file_key: 0,
        issuing_tid: 1,
        io_size: 5,
        io_flags: 0,
        extra_flags: 0,
    })
}

fn set_info(fo: u64, class: u32) -> RawEvent {
    RawEvent::FileSetInfo(FileSetInfo {
        irp: 0,
        file_object: fo,
        file_key: 0,
        extra_information: 0,
        issuing_tid: 1,
        info_class: class,
    })
}

fn op_end(irp: u64, status: u32) -> RawEvent {
    RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: 0, status })
}

fn path_event(irp: u64, fo: u64, path: &str) -> FilePath {
    FilePath {
        irp,
        file_object: fo,
        file_key: 0,
        extra_information: 0,
        issuing_tid: 1,
        info_class: 64,
        file_path: wstr(path),
    }
}

fn reg_open(key: u64, base: u64, rel: &str, disposition: u32) -> RegOpen {
    RegOpen {
        base_object: base,
        key_object: key,
        status: 0,
        disposition,
        base_name: WStr::default(),
        relative_name: wstr(rel),
    }
}

fn reg_set(key: u64, name: &str, value_type: u32, size: u32) -> RawEvent {
    RawEvent::RegSetValue(RegSetValue {
        key_object: key,
        status: 0,
        value_type,
        data_size: size,
        key_name: WStr::default(),
        value_name: wstr(name),
        value_name_ambiguous: false,
        captured_data: Box::default(),
        previous_data_type: 0,
        previous_data_size: 0,
        previous_data: Box::default(),
    })
}

fn net(pid: u32, s: &str, sport: u16, d: &str, dport: u16) -> NetEvent {
    NetEvent { pid, size: 0, saddr: s.parse().unwrap(), sport, daddr: d.parse().unwrap(), dport, seqnum: 0, connid: 0 }
}

/// A process already running (as the rundown would seed it).
fn running(t: &mut T, pid: u32, seq: u64, image: &str) {
    t.p.lookups.live.insert(
        pid,
        LiveProcess { start_key: key(seq), image_path: image.into(), command_line: Some(format!("{image} --x")) },
    );
    t.ev(0, 0, None, classic(ClassicKind::DcStart, pid, 1, ""));
}

const EXPLORER: &str = r"\Device\HarddiskVolume3\Windows\explorer.exe";
const CMD: &str = r"\Device\HarddiskVolume3\Windows\System32\cmd.exe";

fn launches(events: &[Event]) -> Vec<&atlas_schema::Process> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Process(ProcessActivity::Launch { process, .. }) => Some(process),
            _ => None,
        })
        .collect()
}

fn files(events: &[Event]) -> Vec<(&str, String, u32)> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::File(f) => Some((
                match &f.action {
                    FileAction::Create => "create",
                    FileAction::Update => "update",
                    FileAction::Delete => "delete",
                    FileAction::Rename { .. } => "rename",
                    FileAction::SetAttributes => "setattr",
                    FileAction::Open => "open",
                    FileAction::Read => "read",
                },
                f.file.path.clone(),
                f.actor.pid,
            )),
            _ => None,
        })
        .collect()
}

// ---- processes ----

#[test]
fn a_launch_joins_both_halves_in_either_order() {
    for classic_first in [false, true] {
        let mut t = T::new();
        running(&mut t, 100, 10, EXPLORER);
        let (kp, cl) = (kp_start(200, 20, 100, 10, CMD), classic(ClassicKind::Start, 200, 100, "cmd /c dir"));
        if classic_first {
            t.ev(5, 100, None, cl);
            t.ev(5, 100, Some(key(10)), kp);
        } else {
            t.ev(5, 100, Some(key(10)), kp);
            t.ev(5, 100, None, cl);
        }
        t.at(1_000);
        let enrich = t.requests().into_iter().find_map(|r| match r {
            Request::Enrich { id, target: EnrichTarget::LaunchImage, nt_path } => Some((id, nt_path)),
            _ => None,
        });
        let (id, nt) = enrich.expect("an enrichment request");
        assert_eq!(nt, CMD);
        assert!(t.out.is_empty(), "waits for enrichment");
        t.reply(Reply::Enriched { id, hashes: Some(Hashes { sha256: Some([7; 32]) }), signature: None, error: false });
        let out = t.settle();
        let l = launches(&out);
        assert_eq!(l.len(), 1, "classic first: {classic_first}");
        let p = l[0];
        assert_eq!(
            (p.pid, p.file.path.as_str(), p.file.name.as_str()),
            (200, r"C:\Windows\System32\cmd.exe", "cmd.exe")
        );
        assert_eq!(p.cmd_line, "cmd /c dir");
        assert_eq!(p.user.as_ref().map(|u| (u.uid.as_str(), u.name.as_str())), Some((SID, r"DESK\jake")));
        assert_eq!(p.integrity, Some(Integrity::Medium));
        assert_eq!(p.uid, identity().uid(key(20)));
        assert_eq!(p.parent_process.as_ref().map(|r| r.file.name.as_str()), Some("explorer.exe"));
        assert_eq!(p.file.hashes, Some(Hashes { sha256: Some([7; 32]) }));
        assert_eq!(p.created_time, 1_790_966_247_610_206_700);
        if let EventKind::Process(ProcessActivity::Launch { actor, .. }) = &out[0].kind {
            assert_eq!(actor.file.name, "explorer.exe");
        }
        assert_eq!(t.p.counters().launch_join_miss, 0);
        valid(&out);
    }
}

#[test]
fn a_join_miss_takes_the_command_line_from_the_live_process() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.p.lookups.live.insert(
        200,
        LiveProcess { start_key: key(20), image_path: CMD.into(), command_line: Some("cmd /live".into()) },
    );
    t.ev(5, 100, Some(key(10)), kp_start(200, 20, 100, 10, CMD));
    let out = t.settle();
    let l = launches(&out);
    assert_eq!(l[0].cmd_line, "cmd /live");
    let c = t.p.counters();
    assert_eq!((c.launch_join_miss, c.enrichment_misses), (1, 1));
}

#[test]
fn a_classic_half_alone_uses_the_live_process_for_its_start_key() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.p.lookups.live.insert(300, LiveProcess { start_key: key(30), image_path: CMD.into(), command_line: None });
    t.ev(5, 100, None, classic(ClassicKind::Start, 300, 100, "cmd /only-classic"));
    let out = t.settle();
    let l = launches(&out);
    assert_eq!((l[0].uid, l[0].cmd_line.as_str()), (identity().uid(key(30)), "cmd /only-classic"));
    // Without a live process there is no start key, hence no uid: dropped.
    let mut t = T::new();
    t.ev(5, 100, None, classic(ClassicKind::Start, 301, 100, "gone"));
    assert!(launches(&t.settle()).is_empty());
    assert_eq!(t.p.counters().launch_join_miss, 1);
}

#[test]
fn terminate_from_the_cache_or_from_the_event() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let stop = |pid, seq| {
        RawEvent::ProcessStop(ProcessStop {
            pid,
            sequence_number: seq,
            create_time: 0,
            exit_time: 0,
            exit_code: 7,
            image_name: Box::from(&b"gone.exe"[..]),
        })
    };
    t.ev(10, 100, Some(key(10)), stop(100, 10));
    t.ev(11, 555, Some(key(55)), stop(555, 55));
    let out = t.settle();
    let terms: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Process(ProcessActivity::Terminate { process, exit_code }) => {
                Some((process.file.name.clone(), *exit_code))
            }
            _ => None,
        })
        .collect();
    assert_eq!(terms, [("explorer.exe".into(), Some(7)), ("gone.exe".into(), Some(7))]);
    valid(&out);
}

#[test]
fn actors_resolve_at_the_events_time_across_pid_reuse() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), kp_start(400, 40, 100, 10, CMD));
    t.ev(5, 100, None, classic(ClassicKind::Start, 400, 100, "first"));
    t.ev(
        20,
        400,
        Some(key(40)),
        RawEvent::ProcessStop(ProcessStop {
            pid: 400,
            sequence_number: 40,
            create_time: 0,
            exit_time: 0,
            exit_code: 0,
            image_name: Box::from(&b"cmd.exe"[..]),
        }),
    );
    let notepad = r"\Device\HarddiskVolume3\Windows\notepad.exe";
    t.ev(30, 100, Some(key(10)), kp_start(400, 41, 100, 10, notepad)); // PID 400 reused
    t.ev(30, 100, None, classic(ClassicKind::Start, 400, 100, "second"));
    let f = r"\Device\HarddiskVolume3\a.txt";
    t.ev(
        15,
        400,
        Some(key(40)),
        RawEvent::FileCreateNew(FileCreate {
            irp: 1,
            file_object: 1,
            issuing_tid: 1,
            create_options: 0,
            create_attributes: 0,
            share_access: 0,
            file_name: wstr(f),
        }),
    );
    t.ev(
        35,
        400,
        Some(key(41)),
        RawEvent::FileCreateNew(FileCreate {
            irp: 2,
            file_object: 2,
            issuing_tid: 1,
            create_options: 0,
            create_attributes: 0,
            share_access: 0,
            file_name: wstr(f),
        }),
    );
    let out = t.settle();
    let actors: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::File(x) => Some(x.actor.file.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(actors, ["cmd.exe", "notepad.exe"]);
}

#[test]
fn an_unknown_actor_with_a_start_key_is_kept_bare_and_counted() {
    let mut t = T::new();
    t.ev(
        5,
        777,
        Some(key(77)),
        RawEvent::FileCreateNew(FileCreate {
            irp: 1,
            file_object: 1,
            issuing_tid: 1,
            create_options: 0,
            create_attributes: 0,
            share_access: 0,
            file_name: wstr(r"\Device\HarddiskVolume3\x"),
        }),
    );
    let out = t.settle();
    let EventKind::File(f) = &out[0].kind else { panic!() };
    assert_eq!((f.actor.uid, f.actor.pid, f.actor.file.path.as_str()), (identity().uid(key(77)), 777, ""));
    assert_eq!(t.p.counters().actor_unresolved.values().sum::<u64>(), 1);
    valid(&out);
}

#[test]
fn a_network_event_for_an_unknown_pid_is_dropped_and_counted() {
    let mut t = T::new();
    t.ev(5, 4, None, RawEvent::TcpConnect(net(999, "10.0.0.5", 50000, "1.2.3.4", 443)));
    assert!(t.settle().is_empty());
    assert_eq!(t.p.counters().actor_dropped.values().sum::<u64>(), 1);
}

#[test]
fn events_are_ordered_before_the_state_sees_them() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    // The child's image load arrives before its ProcessStart, with a later time.
    t.ev(
        6,
        500,
        Some(key(50)),
        RawEvent::ImageLoad(ImageLoad {
            image_base: 0x1000,
            image_size: 0x10,
            pid: 500,
            image_checksum: 0,
            time_date_stamp: 0,
            default_base: 0,
            image_name: wstr(CMD),
        }),
    );
    t.ev(5, 100, Some(key(10)), kp_start(500, 50, 100, 10, CMD));
    let out = t.settle();
    let module_actor = out.iter().find_map(|e| match &e.kind {
        EventKind::Module(m) => Some(m.actor.file.name.clone()),
        _ => None,
    });
    assert_eq!(module_actor.as_deref(), Some("cmd.exe"));
    assert_eq!(t.p.counters().late_arrivals, 0);
    assert_eq!(t.p.counters().actor_unresolved.values().sum::<u64>(), 0);
}

// ---- files ----

#[test]
fn one_update_per_written_handle_with_the_opener_as_actor() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let f = r"\Device\HarddiskVolume3\Users\jake\a.txt";
    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, 0));
    t.ev(6, 100, Some(key(10)), write(0xA));
    t.ev(7, 100, Some(key(10)), write(0xA));
    // The cache manager closes it from System: the actor is still the opener.
    t.ev(8, 4, None, RawEvent::FileCleanup(handle(0xA)));
    t.ev(9, 4, None, write(0xA)); // a lazy-writer flush after Cleanup
    t.ev(10, 4, None, RawEvent::FileClose(handle(0xA)));
    // An overwrite: Create on an existing file plus end-of-file truncation.
    t.ev(20, 100, Some(key(10)), create(2, 0xB, f, 0));
    t.ev(21, 100, Some(key(10)), set_info(0xB, 19));
    t.ev(22, 100, Some(key(10)), RawEvent::FileCleanup(handle(0xB)));
    let out = t.settle();
    assert_eq!(
        files(&out),
        [("update", r"C:\Users\jake\a.txt".into(), 100), ("update", r"C:\Users\jake\a.txt".into(), 100)]
    );
    assert_eq!(t.p.counters().writes_after_cleanup, 1);
    valid(&out);
}

#[test]
fn delete_on_close_emits_a_delete_at_cleanup() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let f = r"\Device\HarddiskVolume3\tmp\d.txt";
    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, FileCreate::DELETE_ON_CLOSE));
    t.ev(6, 100, Some(key(10)), RawEvent::FileCleanup(handle(0xA)));
    assert_eq!(files(&t.settle()), [("delete", r"C:\tmp\d.txt".into(), 100)]);
}

#[test]
fn failed_operations_are_dropped_by_irp_within_the_window() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let p = |irp, path: &str| path_event(irp, 0xF, path);
    // A failed delete, a failed rename, a delete that stands.
    t.ev(5, 100, Some(key(10)), RawEvent::FileDeletePath(p(1, r"\Device\HarddiskVolume3\ro.txt")));
    t.ev(6, 100, Some(key(10)), op_end(1, 0xC000_0121));
    t.ev(7, 100, Some(key(10)), RawEvent::FileRenamePath(p(2, r"\Device\HarddiskVolume3\new.txt")));
    t.ev(8, 100, Some(key(10)), op_end(2, 0xC000_0035));
    t.ev(9, 100, Some(key(10)), RawEvent::FileDeletePath(p(3, r"\Device\HarddiskVolume3\ok.txt")));
    t.ev(10, 100, Some(key(10)), op_end(3, 0x104)); // informational, not a failure
    // A failure long after the window: the delete stands, counted as late.
    t.ev(30, 100, Some(key(10)), RawEvent::FileDeletePath(p(5, r"\Device\HarddiskVolume3\slow.txt")));
    t.ev(900, 100, Some(key(10)), op_end(5, 0xC000_0001));
    // The failure is processed first when its operation arrives late (after
    // the ordering stage released newer events): the ring of failures catches it.
    t.ev(1_000, 100, Some(key(10)), op_end(4, 0xC000_0043));
    t.at(2_000);
    t.ev(999, 100, Some(key(10)), RawEvent::FileDeletePath(p(4, r"\Device\HarddiskVolume3\late.txt")));
    let out = t.settle();
    let got: Vec<String> = files(&out).into_iter().map(|(_, p, _)| p).collect();
    assert_eq!(got, [r"C:\ok.txt", r"C:\slow.txt"]);
    let c = t.p.counters();
    assert_eq!((c.file_op_failed, c.file_op_late_failure, c.late_arrivals), (3, 1, 1));
}

#[test]
fn a_failed_create_takes_its_map_entry_and_open_with_it() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let sam = r"\Device\HarddiskVolume3\Windows\System32\config\SAM";
    t.ev(5, 100, Some(key(10)), create(1, 0xA, sam, 0));
    t.ev(6, 100, Some(key(10)), op_end(1, 0xC000_0022));
    t.ev(7, 100, Some(key(10)), write(0xA)); // unknown now: provisional
    assert!(files(&t.settle()).is_empty());
}

#[test]
fn a_confirmed_rename_renames_the_handle() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
    t.ev(6, 100, Some(key(10)), write(0xA));
    t.ev(7, 100, Some(key(10)), RawEvent::FileRenamePath(path_event(2, 0xA, r"\Device\HarddiskVolume3\b.txt")));
    t.ev(900, 100, Some(key(10)), RawEvent::FileCleanup(handle(0xA)));
    let out = t.settle();
    let EventKind::File(r) = &out[0].kind else { panic!() };
    match &r.action {
        FileAction::Rename { file_result } => {
            assert_eq!((r.file.path.as_str(), file_result.path.as_str()), (r"C:\a.txt", r"C:\b.txt"))
        }
        a => panic!("{a:?}"),
    }
    assert_eq!(files(&out)[1], ("update", r"C:\b.txt".into(), 100));
}

#[test]
fn watchlist_opens_wait_for_confirmation_and_coalesce() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let sam = r"\Device\HarddiskVolumeShadowCopy2\Windows\System32\config\SAM";
    t.ev(5, 100, Some(key(10)), create(1, 0xA, sam, 0));
    t.ev(6, 100, Some(key(10)), create(2, 0xB, sam, 0)); // same process, same path: coalesced
    t.ev(7, 100, Some(key(10)), create(3, 0xC, r"\Device\HarddiskVolume3\Windows\notes.txt", 0));
    t.at(900);
    let out = t.settle();
    assert_eq!(files(&out), [("open", sam.into(), 100)]); // shadow copies keep their NT path
}

#[test]
fn short_names_are_matched_after_expansion() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let short = r"\Device\HarddiskVolume3\Users\jake\.ssh\ID_ED2~1";
    let plain = r"\Device\HarddiskVolume3\Users\jake\DOCUME~1\x.txt";
    t.ev(5, 100, Some(key(10)), create(1, 0xA, short, 0));
    t.ev(6, 100, Some(key(10)), create(2, 0xB, plain, 0));
    t.at(1_000);
    let ids: Vec<_> = t
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Expand { id, slot, nt_path } => Some((id, slot, nt_path)),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    let long = |s: &str| Some(format!(r"\Device\HarddiskVolume3\Users\jake\{s}"));
    t.reply(Reply::Expanded { id: ids[0].0, slot: ids[0].1, long_path: long(r".ssh\id_ed25519") });
    t.reply(Reply::Expanded { id: ids[1].0, slot: ids[1].1, long_path: long(r"Documents\x.txt") });
    let out = t.settle();
    // The first matches as logged (.ssh\*) and is emitted with its long name;
    // the second matches neither form and is dropped.
    assert_eq!(files(&out), [("open", r"C:\Users\jake\.ssh\id_ed25519".into(), 100)]);
}

#[test]
fn short_names_in_emitted_paths_are_expanded() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let short = r"\Device\HarddiskVolume3\Users\JOHN~1.SMI\AppData\Local\Temp\INVOIC~1.EXE";
    let long = r"\Device\HarddiskVolume3\Users\john.smith\AppData\Local\Temp\invoice-2026.exe";
    t.ev(
        5,
        100,
        Some(key(10)),
        RawEvent::FileCreateNew(FileCreate {
            irp: 1,
            file_object: 0xA,
            issuing_tid: 1,
            create_options: 0,
            create_attributes: 0,
            share_access: 0,
            file_name: wstr(short),
        }),
    );
    t.ev(6, 100, Some(key(10)), write(0xA));
    t.ev(7, 100, Some(key(10)), RawEvent::FileCleanup(handle(0xA)));
    t.ev(
        8,
        100,
        Some(key(10)),
        RawEvent::FileRenamePath(path_event(2, 0xA, r"\Device\HarddiskVolume3\Users\JOHN~1.SMI\x.exe")),
    );
    t.at(1_000);
    let asks: Vec<_> = t
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Expand { id, slot, nt_path } => Some((id, slot, nt_path)),
            _ => None,
        })
        .collect();
    // The Create; the Rename's result and source (the Update reuses nothing yet:
    // its expansion was asked before the Create's answer came back).
    assert_eq!(asks.len(), 4, "{asks:?}");
    for (id, slot, nt) in asks {
        let l = if nt == short { long.to_string() } else { nt.replace("JOHN~1.SMI", "john.smith") };
        t.reply(Reply::Expanded { id, slot, long_path: Some(l) });
    }
    let out = t.settle();
    let paths: Vec<_> = files(&out).into_iter().map(|(a, p, _)| (a, p)).collect();
    let l = r"C:\Users\john.smith\AppData\Local\Temp\invoice-2026.exe".to_string();
    assert_eq!(paths, [("create", l.clone()), ("update", l.clone()), ("rename", l)]);
    let EventKind::File(r) = &out[2].kind else { panic!() };
    let FileAction::Rename { file_result } = &r.action else { panic!() };
    assert_eq!(file_result.path, r"C:\Users\john.smith\x.exe");
    // A failed expansion leaves the path as logged.
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::FileDeletePath(path_event(1, 0xB, short)));
    t.at(1_000);
    for r in t.requests() {
        if let Request::Expand { id, slot, .. } = r {
            t.reply(Reply::Expanded { id, slot, long_path: None });
        }
    }
    assert_eq!(files(&t.settle())[0].1, r"C:\Users\JOHN~1.SMI\AppData\Local\Temp\INVOIC~1.EXE");
}

#[test]
fn an_unknown_handle_is_named_by_the_seeder_or_dropped() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), write(0xA)); // opened before we watched
    t.ev(6, 4, None, RawEvent::FileCleanup(handle(0xA)));
    t.ev(7, 100, Some(key(10)), write(0xB));
    t.ev(8, 4, None, RawEvent::FileCleanup(handle(0xB)));
    t.at(1_000);
    let asked: Vec<_> = t
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Seed { kind: HandleKind::File, addresses } if !addresses.is_empty() => Some(addresses),
            _ => None,
        })
        .collect();
    assert_eq!(asked, [vec![0xA, 0xB]]);
    t.reply(Reply::Snapshot(Snapshot {
        kind: HandleKind::File,
        taken: ms(1_000),
        asked: vec![0xA, 0xB],
        named: vec![Named { address: 0xA, owner_pid: 100, name: r"\Device\HarddiskVolume3\held.txt".into() }],
        unnamable: vec![],
    }));
    let out = t.settle();
    assert_eq!(files(&out), [("update", r"C:\held.txt".into(), 100)]);
    assert_eq!(t.p.counters().unknown_file_object, 1); // 0xB was never named
}

// ---- registry ----

fn values(events: &[Event]) -> Vec<(String, String, bool, Vec<u8>, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::RegistryValue(v) => match &v.action {
                RegistryValueAction::Set { data, data_unavailable, .. } => {
                    Some((v.key_path.clone(), v.name.clone(), v.path_unresolved, data.clone(), *data_unavailable))
                }
                RegistryValueAction::Delete => {
                    Some((v.key_path.clone(), v.name.clone(), v.path_unresolved, vec![], false))
                }
            },
            _ => None,
        })
        .collect()
}

const RUN: &str = r"\REGISTRY\MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Run";

#[test]
fn keys_and_values_with_reads_after_the_event() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(1, 0, r"\REGISTRY\MACHINE\SOFTWARE", 0)));
    t.ev(6, 100, Some(key(10)), RawEvent::RegCreateKey(reg_open(2, 1, r"Microsoft\Windows\CurrentVersion\Run", 2)));
    t.ev(7, 100, Some(key(10)), RawEvent::RegCreateKey(reg_open(3, 2, "Atlas", 1)));
    t.ev(8, 100, Some(key(10)), reg_set(2, "evil", 1, 6));
    t.ev(9, 100, Some(key(10)), reg_set(2, "wrong", 4, 4));
    t.at(1_000);
    let reads: Vec<_> = t
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::ReadValue { id, read } => Some((id, read)),
            _ => None,
        })
        .collect();
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0].1.key_path, RUN); // the raw NT name, not the normalized one
    t.reply(Reply::ValueRead {
        id: reads[0].0,
        result: Some(ValueData { value_type: 1, size: 6, data: b"x\0y\0\0\0".to_vec() }),
    });
    // The value changed before the read: its length no longer matches.
    t.reply(Reply::ValueRead { id: reads[1].0, result: Some(ValueData { value_type: 4, size: 8, data: vec![0; 8] }) });
    let out = t.settle();
    let created: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::RegistryKey(k) if k.action == RegistryKeyAction::Create => Some(k.path.clone()),
            _ => None,
        })
        .collect();
    // Only a new key is a Create (disposition 1).
    assert_eq!(created, [r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run\Atlas"]);
    let hklm_run = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run".to_string();
    assert_eq!(
        values(&out),
        [
            (hklm_run.clone(), "evil".into(), false, b"x\0y\0\0\0".to_vec(), false),
            (hklm_run, "wrong".into(), false, vec![], true)
        ]
    );
    assert_eq!(t.p.counters().value_read_failed, 1);
    valid(&out);
}

#[test]
fn a_fast_path_read_is_used_when_it_names_the_same_key() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(1, 0, RUN, 0)));
    t.ev(6, 100, Some(key(10)), reg_set(1, "v", 4, 4));
    t.ev(7, 100, Some(key(10)), reg_set(1, "w", 4, 4));
    let read = |name: &str, path: &str| ValueRead { key_path: path.into(), value_name: name.encode_utf16().collect() };
    let data = Some(ValueData { value_type: 4, size: 4, data: vec![1, 0, 0, 0] });
    t.reply(Reply::EarlyRead {
        event: EarlyKey { ts: ms(6), tid: 1, key_object: 1 },
        read: read("v", RUN),
        result: data.clone(),
    });
    // The early map named the key differently: redone on the ordered path.
    t.reply(Reply::EarlyRead {
        event: EarlyKey { ts: ms(7), tid: 1, key_object: 1 },
        read: read("w", r"\REGISTRY\MACHINE\Other"),
        result: data,
    });
    t.at(1_000);
    let ordered = t.requests().into_iter().filter(|r| matches!(r, Request::ReadValue { .. })).count();
    assert_eq!(ordered, 1);
    assert_eq!(t.p.counters().early_read_redone, 1);
    let out = t.settle();
    assert_eq!(values(&out)[0].3, vec![1, 0, 0, 0]);
}

#[test]
fn unusual_value_types_are_kept_without_data() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(1, 0, RUN, 0)));
    t.ev(6, 100, Some(key(10)), reg_set(1, "odd", 0x2000_0000, 4));
    let out = t.settle();
    let EventKind::RegistryValue(v) = &out[0].kind else { panic!() };
    let RegistryValueAction::Set { value_type, data_unavailable, data_read_after, .. } = &v.action else { panic!() };
    assert_eq!((*value_type, *data_unavailable, *data_read_after), (RegType::Raw(0x2000_0000), true, false));
    assert_eq!(t.p.counters().reg_type_unusual, 1);
    valid(&out);
}

#[test]
fn an_unknown_base_is_named_by_the_seeder_unless_it_was_reused() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    // HKCU\Software was opened before we watched (address 0x50).
    t.ev(5, 100, Some(key(10)), RawEvent::RegCreateKey(reg_open(1, 0x50, "Atlas", 1)));
    t.ev(6, 100, Some(key(10)), RawEvent::RegCreateKey(reg_open(2, 0x60, "Reused", 1)));
    // 0x60 is reused (opened again) before the snapshot is taken.
    t.ev(7, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(0x60, 0, r"\REGISTRY\MACHINE\Elsewhere", 0)));
    t.ev(8, 100, Some(key(10)), RawEvent::RegCreateKey(reg_open(3, 0x70, "Hidden", 1)));
    t.at(1_000);
    t.reply(Reply::Snapshot(Snapshot {
        kind: HandleKind::Key,
        taken: ms(900),
        asked: vec![0x50, 0x60, 0x70],
        named: vec![
            Named { address: 0x50, owner_pid: 100, name: format!(r"\REGISTRY\USER\{SID}\Software") },
            Named { address: 0x60, owner_pid: 100, name: r"\REGISTRY\MACHINE\Elsewhere".into() },
        ],
        unnamable: vec![(0x70, 4)], // a protected process's handle
    }));
    let out = t.settle();
    let keys: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::RegistryKey(k) => Some((k.path.clone(), k.path_unresolved)),
            _ => None,
        })
        .collect();
    assert_eq!(keys, [(format!(r"HKU\{SID}\Software\Atlas"), false), ("Reused".into(), true), ("Hidden".into(), true)]);
    assert_eq!(t.p.counters().registry_unresolved, 2);
    valid(&out);
}

#[test]
fn the_agents_own_closes_do_not_forget_keys() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(1, 0, RUN, 0)));
    let close = RawEvent::RegCloseKey(RegKey { key_object: 1, status: 0, key_name: WStr::default() });
    t.ev(6, 9, Some(AGENT), close);
    t.ev(
        7,
        100,
        Some(key(10)),
        RawEvent::RegDeleteValue(RegDeleteValue {
            key_object: 1,
            status: 0,
            key_name: WStr::default(),
            value_name: wstr("a\0b"),
        }),
    );
    let out = t.settle();
    let v = values(&out);
    assert_eq!(
        (v[0].0.as_str(), v[0].1.as_str(), v[0].2),
        (r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run", "a\0b", false)
    );
}

// ---- network, DNS, self-filter ----

#[test]
fn tcp_and_udp_ends_and_flows() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 4, None, RawEvent::TcpConnect(net(100, "10.0.0.5", 50000, "93.184.216.34", 443)));
    t.ev(6, 4, None, RawEvent::TcpAccept(net(100, "10.0.0.5", 8080, "10.0.0.9", 60000)));
    t.ev(7, 4, None, RawEvent::TcpDisconnect(net(100, "10.0.0.5", 8080, "10.0.0.9", 60000)));
    // UDP: a send and its reply, then nothing for 60 s.
    t.ev(10, 4, None, RawEvent::UdpSend(net(100, "10.0.0.5", 5353, "8.8.8.8", 53)));
    t.ev(11, 4, None, RawEvent::UdpRecv(net(100, "8.8.8.8", 53, "10.0.0.5", 5353))); // saddr is the sender (F5)
    t.ev(12, 4, None, RawEvent::UdpRecv(net(100, "1.1.1.1", 53, "10.0.0.5", 5354)));
    let out = t.settle();
    let nets: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Network(n) => Some((
                n.protocol,
                n.direction,
                matches!(n.action, NetworkAction::Open),
                (n.src_endpoint.ip, n.src_endpoint.port),
                (n.dst_endpoint.ip, n.dst_endpoint.port),
                e.meta.time,
            )),
            _ => None,
        })
        .collect();
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    use {NetworkDirection::*, NetworkProtocol::*};
    assert_eq!(
        (nets[0].0, nets[0].1, nets[0].2, nets[0].3, nets[0].4),
        (Tcp, Outbound, true, (ip("10.0.0.5"), 50000), (ip("93.184.216.34"), 443))
    );
    assert_eq!((nets[1].1, nets[1].3, nets[1].4), (Inbound, (ip("10.0.0.9"), 60000), (ip("10.0.0.5"), 8080)));
    assert_eq!((nets[2].1, nets[2].2), (Inbound, false)); // the Close keeps the accept's direction
    let udp: Vec<_> = nets.iter().filter(|n| n.0 == Udp).collect();
    assert_eq!(udp.len(), 4); // two flows, each one Open and one Close
    assert_eq!((udp[0].1, udp[0].2, udp[0].4), (Outbound, true, (ip("8.8.8.8"), 53)));
    assert_eq!((udp[1].1, udp[1].2, udp[1].3), (Inbound, true, (ip("1.1.1.1"), 53)));
    // The first flow's Close is timestamped at its last datagram (the reply at 11 ms).
    let close = udp.iter().find(|n| !n.2 && n.4 == (ip("8.8.8.8"), 53)).unwrap();
    assert_eq!(close.5, 1_790_000_000_000_000_000 + 11_000_000);
    valid(&out);
}

#[test]
fn dns_responses_map_status_and_answers() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let q = |name: &str, status, results: &str| {
        RawEvent::DnsQuery(DnsQuery {
            query_name: wstr(name),
            query_type: 1,
            query_options: 0,
            query_status: status,
            query_results: wstr(results),
        })
    };
    t.ev(5, 100, Some(key(10)), q("example.com", 0, "type: 5 edge.example.net;93.184.216.34;2606:2800::1;odd;"));
    t.ev(6, 100, Some(key(10)), q("nx.invalid", 9003, ""));
    let out = t.settle();
    let got: Vec<_> = out
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Dns(d) => {
                let DnsAction::Response { rcode, answers, .. } = &d.action;
                Some((
                    d.hostname.clone(),
                    *rcode,
                    answers.iter().map(|a| (a.rr_type, a.data.clone())).collect::<Vec<_>>(),
                ))
            }
            _ => None,
        })
        .collect();
    assert_eq!(got[0].1, Some(0));
    assert_eq!(
        got[0].2,
        [(5, "edge.example.net".into()), (1, "93.184.216.34".into()), (28, "2606:2800::1".into()), (0, "odd".into())]
    );
    assert_eq!((got[1].0.as_str(), got[1].1, got[1].2.len()), ("nx.invalid", Some(3), 0));
    valid(&out);
}

#[test]
fn the_agents_own_activity_is_not_emitted() {
    let mut t = T::new();
    t.p.lookups.live.insert(
        9,
        LiveProcess {
            start_key: AGENT,
            image_path: r"\Device\HarddiskVolume3\Program Files\Atlas\atlas-agent.exe".into(),
            command_line: None,
        },
    );
    t.ev(
        5,
        9,
        Some(AGENT),
        RawEvent::FileCreateNew(FileCreate {
            irp: 1,
            file_object: 1,
            issuing_tid: 1,
            create_options: 0,
            create_attributes: 0,
            share_access: 0,
            file_name: wstr(r"\Device\HarddiskVolume3\ProgramData\Atlas\buffer\1.seg"),
        }),
    );
    t.ev(6, 9, Some(AGENT), RawEvent::RegOpenKey(reg_open(1, 0, RUN, 0)));
    t.ev(7, 9, Some(AGENT), reg_set(1, "canary", 4, 4));
    assert!(t.settle().is_empty());
    assert_eq!(t.p.counters().self_filtered, 2);
}

#[test]
fn a_clean_stop_emits_everything_pending() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::FileDeletePath(path_event(1, 0xA, r"\Device\HarddiskVolume3\x")));
    t.ev(6, 4, None, RawEvent::UdpSend(net(100, "10.0.0.5", 5353, "8.8.8.8", 53)));
    let out = t.p.stop();
    // The delete (its window not yet passed) and the flow's Open and Close.
    assert_eq!(out.len(), 3);
    valid(&out);
}

#[test]
fn without_op_end_operations_are_not_held() {
    let cfg = Config { file_op_end: false, ..Config::default() };
    let mut t = T::with(cfg, FakeLookups::default());
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::FileDeletePath(path_event(1, 0xA, r"\Device\HarddiskVolume3\x")));
    t.at(800); // released; nothing to wait for
    assert_eq!(files(&t.out), [("delete", r"C:\x".into(), 100)]);
}

#[test]
fn pathological_lengths_still_make_valid_events() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    // 40,000 two-byte characters: 80 KB of UTF-8, over the schema's 32 KiB path limit.
    let long = format!(r"\Device\HarddiskVolume3\{}", "é".repeat(40_000));
    t.ev(
        5,
        100,
        Some(key(10)),
        RawEvent::FileCreateNew(FileCreate {
            irp: 1,
            file_object: 1,
            issuing_tid: 1,
            create_options: 0,
            create_attributes: 0,
            share_access: 0,
            file_name: wstr(&long),
        }),
    );
    t.ev(
        6,
        100,
        Some(key(10)),
        RawEvent::RegOpenKey(reg_open(9, 0, &format!(r"\REGISTRY\MACHINE\{}", "é".repeat(40_000)), 0)),
    );
    t.ev(7, 100, Some(key(10)), reg_set(9, &"é".repeat(40_000), 4, 4));
    let out = t.settle();
    assert_eq!(out.len(), 2);
    valid(&out);
}

// ---- plan 1b-3a review: leaks, reuse, waiting ----

fn no_bookkeeping_left(t: &T) {
    for (what, n) in t.p.bookkeeping() {
        assert_eq!(n, 0, "{what}");
    }
}

fn stop_ev(pid: u32, seq: u64) -> RawEvent {
    RawEvent::ProcessStop(ProcessStop {
        pid,
        sequence_number: seq,
        create_time: 0,
        exit_time: 0,
        exit_code: 0,
        image_name: Box::from(&b"x.exe"[..]),
    })
}

fn key_close(key: u64) -> RawEvent {
    RawEvent::RegCloseKey(RegKey { key_object: key, status: 0, key_name: WStr::default() })
}

fn seed_asks(t: &mut T, kind: HandleKind) -> Vec<Vec<u64>> {
    t.requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::Seed { kind: k, addresses } if k == kind && !addresses.is_empty() => Some(addresses),
            _ => None,
        })
        .collect()
}

#[test]
fn bookkeeping_is_freed_when_no_reply_comes() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    for i in 0..50u64 {
        // An 8.3 expansion, an enrichment and join, a value read and two seeder
        // questions: none is ever answered.
        t.ev(5, 100, Some(key(10)), create(i, 0x1000 + i, &format!(r"\Device\HarddiskVolume3\PROGRA~{i}\x"), 0));
        t.ev(6, 100, Some(key(10)), kp_start(1000 + i as u32, 500 + i, 100, 10, CMD));
        t.ev(7, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(0x2000 + i, 0, RUN, 0)));
        t.ev(8, 100, Some(key(10)), reg_set(0x2000 + i, "v", 4, 4));
        t.ev(9, 100, Some(key(10)), reg_set(0x3000 + i, "w", 4, 4));
        t.ev(10, 100, Some(key(10)), write(0x4000 + i));
        t.ev(11, 4, None, RawEvent::FileCleanup(handle(0x4000 + i)));
        t.ev(12, 100, Some(key(10)), op_end(9000 + i, 0xC000_0034)); // failures with no operation
    }
    let out = t.settle();
    assert!(!out.is_empty());
    no_bookkeeping_left(&t);
}

#[test]
fn a_reused_base_never_names_old_children() {
    // 0x50 was opened before we watched, and Run is opened below it. Then 0x50
    // closes: Run can only be named by asking about Run itself.
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(0x60, 0x50, "Run", 0)));
    t.ev(6, 100, Some(key(10)), key_close(0x50));
    t.ev(8, 100, Some(key(10)), reg_set(0x60, "evil", 1, 6));
    t.at(1_000);
    assert_eq!(seed_asks(&mut t, HandleKind::Key), [vec![0x60]]);
    t.reply(Reply::Snapshot(Snapshot {
        kind: HandleKind::Key,
        taken: ms(1_000),
        asked: vec![0x60],
        named: vec![Named { address: 0x60, owner_pid: 100, name: RUN.into() }],
        unnamable: vec![],
    }));
    let v = values(&t.settle());
    assert_eq!((v[0].0.as_str(), v[0].2), (r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run", false));

    // 0x50 is opened again with no close seen (an agent close is ignored, §7.4):
    // a new object at that address, so Run is not "Benign\Run". Unanswered, the
    // floor is what ETW logged.
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(0x60, 0x50, "Run", 0)));
    t.ev(7, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(0x50, 0, r"\REGISTRY\MACHINE\SOFTWARE\Benign", 0)));
    t.ev(8, 100, Some(key(10)), reg_set(0x60, "evil", 1, 6));
    let v = values(&t.settle());
    assert_eq!((v[0].0.as_str(), v[0].2), ("Run", true));
}

#[test]
fn an_address_absent_from_the_table_is_answered_at_once() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), reg_set(0x70, "v", 4, 4)); // its key closed before the read
    t.ev(6, 100, Some(key(10)), write(0x80));
    t.ev(7, 4, None, RawEvent::FileCleanup(handle(0x80)));
    t.at(1_000);
    assert_eq!(seed_asks(&mut t, HandleKind::Key), [vec![0x70]]);
    for (kind, asked) in [(HandleKind::Key, 0x70), (HandleKind::File, 0x80)] {
        let s = Snapshot { kind, taken: ms(1_000), asked: vec![asked], named: vec![], unnamable: vec![] };
        t.reply(Reply::Snapshot(s));
    }
    t.at(1_800); // stream passes 1 000 ms: answered, not held to the 5 s deadline
    assert_eq!(values(&t.out), [(String::new(), "v".into(), true, vec![], true)]);
    let EventKind::RegistryValue(v) = &t.out[0].kind else { panic!() };
    let RegistryValueAction::Set { data_read_after, .. } = &v.action else { panic!() };
    assert!(!data_read_after); // no read was attempted (§7.5)
    let c = t.p.counters();
    assert_eq!((c.registry_unresolved, c.value_read_failed, c.unknown_file_object), (1, 1, 1));
    // Later events on either address do not wait either.
    t.ev(1_900, 100, Some(key(10)), reg_set(0x70, "w", 4, 4));
    t.ev(1_901, 100, Some(key(10)), set_info(0x80, 4));
    t.at(2_700);
    assert_eq!(values(&t.out).len(), 2);
    assert_eq!(t.p.counters().unknown_file_object, 2);
    t.settle();
    no_bookkeeping_left(&t);
}

#[test]
fn a_rename_of_an_unknown_handle_does_not_wait_for_the_seeder() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), RawEvent::FileRenamePath(path_event(1, 0xA, r"\Device\HarddiskVolume3\new.txt")));
    t.ev(300, 100, Some(key(10)), set_info(0xA, 4));
    t.at(1_100); // past the hold and the confirm window; a seeder deadline is 5 s away
    let EventKind::File(r) = &t.out[0].kind else { panic!() };
    let FileAction::Rename { file_result } = &r.action else { panic!() };
    assert_eq!((r.file.path.as_str(), file_result.path.as_str()), ("", r"C:\new.txt"));
    // The handle took the new name.
    assert_eq!(files(&t.out)[1], ("setattr", r"C:\new.txt".into(), 100));
    valid(&t.out);
}

#[test]
fn a_failed_watchlist_open_does_not_suppress_the_retry() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    let sam = r"\Device\HarddiskVolume3\Windows\System32\config\SAM";
    t.ev(5, 100, Some(key(10)), create(1, 0xA, sam, 0));
    t.ev(6, 100, Some(key(10)), op_end(1, 0xC000_0043)); // sharing violation
    t.ev(2_000, 100, Some(key(10)), create(2, 0xB, sam, 0)); // the retry succeeds
    let out = t.settle();
    assert_eq!(files(&out), [("open", r"C:\Windows\System32\config\SAM".into(), 100)]);
}

#[test]
fn a_process_found_live_answers_for_its_pid_from_then_on() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    // A (PID 500) runs and exits; B reuses PID 500, and its Launch is lost.
    t.ev(5, 100, Some(key(10)), kp_start(500, 50, 100, 10, CMD));
    t.ev(5, 100, None, classic(ClassicKind::Start, 500, 100, "a"));
    t.ev(10, 500, Some(key(50)), stop_ev(500, 50));
    let notepad = r"\Device\HarddiskVolume3\Windows\notepad.exe";
    t.p.lookups.live.insert(500, LiveProcess { start_key: key(51), image_path: notepad.into(), command_line: None });
    t.ev(20, 500, Some(key(51)), create(1, 0xA, r"\Device\HarddiskVolume3\x.txt", 0)); // found live
    t.ev(25, 4, None, RawEvent::TcpConnect(net(500, "10.0.0.5", 50_000, "1.1.1.1", 443)));
    let out = t.settle();
    let actor = out.iter().find_map(|e| match &e.kind {
        EventKind::Network(n) => Some(n.actor.clone()),
        _ => None,
    });
    let actor = actor.expect("the connect");
    assert_eq!((actor.uid, actor.file.name.as_str()), (identity().uid(key(51)), "notepad.exe"));
}

#[test]
fn without_on_miss_seeding_only_the_start_up_pass_is_waited_for() {
    let cfg = Config { seed_on_miss: false, ..Config::default() };
    let mut t = T::with(cfg, FakeLookups::default());
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), write(0xA)); // waits for the start-up pass
    t.ev(6, 4, None, RawEvent::FileCleanup(handle(0xA)));
    t.at(1_000);
    assert!(seed_asks(&mut t, HandleKind::File).is_empty()); // no question on a miss
    t.reply(Reply::Snapshot(Snapshot {
        kind: HandleKind::File,
        taken: ms(1_000),
        asked: vec![],
        named: vec![Named { address: 0xA, owner_pid: 100, name: r"\Device\HarddiskVolume3\held.txt".into() }],
        unnamable: vec![],
    }));
    t.at(1_800);
    assert_eq!(files(&t.out), [("update", r"C:\held.txt".into(), 100)]);
    // After the pass, an unknown handle is dropped at once.
    t.ev(1_900, 100, Some(key(10)), write(0xB));
    t.ev(1_901, 4, None, RawEvent::FileCleanup(handle(0xB)));
    t.at(2_700);
    assert_eq!((t.p.counters().unknown_file_object, t.p.completion.pending_len()), (1, 0));
}

#[test]
fn a_clean_stop_drops_what_a_deadline_would() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), write(0xA)); // a handle nobody named
    t.ev(6, 4, None, RawEvent::FileCleanup(handle(0xA)));
    let out = t.p.stop();
    assert!(files(&out).is_empty()); // no placeholder Update with a made-up actor
    assert_eq!(t.p.counters().unknown_file_object, 1);
}

#[test]
fn a_classic_half_past_the_window_does_not_make_a_second_launch() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.p.lookups.live.insert(
        600,
        LiveProcess { start_key: key(60), image_path: CMD.into(), command_line: Some("cmd /live".into()) },
    );
    t.ev(5, 100, Some(key(10)), kp_start(600, 60, 100, 10, CMD));
    t.ev(500, 100, None, classic(ClassicKind::Start, 600, 100, "cmd /late"));
    let out = t.settle();
    assert_eq!(launches(&out).len(), 1);
    assert_eq!(t.p.counters().launch_join_miss, 1);
}

#[test]
fn an_event_older_than_stream_time_is_late() {
    let mut t = T::new();
    running(&mut t, 100, 10, EXPLORER);
    t.ev(5, 100, Some(key(10)), write(0xA));
    t.at(800); // stream time is now 50 ms
    t.ev(30, 100, Some(key(10)), write(0xB)); // newer than anything released, older than stream time
    t.at(900);
    assert_eq!(t.p.counters().late_arrivals, 1);
}

#[test]
fn reg_value_types_map_to_the_schema() {
    assert_eq!(RegType::from_raw(4), RegType::Known(RegValueType::Dword));
}
