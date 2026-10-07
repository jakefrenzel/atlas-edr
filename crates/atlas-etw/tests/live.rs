//! Tier 3 for `atlas-etw` (sensor spec §12.3): real sessions, a scripted
//! scenario, and every event of the scenario's process tree parsed and compared
//! with TDH. Needs administrator rights, so it is `#[ignore]`d; CI runs it
//! explicitly on its (elevated) Windows runner:
//!
//! `cargo test -p atlas-etw --test live -- --ignored --test-threads=1 --nocapture`
//!
//! Two processes: this one observes (sessions, TDH, checks) and re-runs itself
//! as the **actor** (`ATLAS_ETW_ACTOR=1`), which performs the scenario. Only the
//! actor's tree is kept, so the observer's own TDH lookups (registry and file
//! reads) never feed back into what it records.
//!
//! - `ATLAS_ETW_RECORD=<file>` writes the scenario's events as replay fixtures.
//! - `ATLAS_ETW_REPORT=<file>` writes the checks and observations as JSON.
#![cfg(windows)]

mod common;

use atlas_etw::Provider;
use atlas_etw::layout;
use atlas_etw::parse::{ClassicKind, EventMeta, ParseError, RawEvent, WStr};
use atlas_etw::providers::{Enable, session_a};
use atlas_etw::session::{self, Config, EtwError, EventRecord, Kind, STALE, Session, VersionGate, tdh};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::os::windows::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::System::Registry::{NtCreateKey, NtDeleteKey, NtDeleteValueKey, NtSetValueKey};
use windows::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, UNICODE_STRING};
use windows::Win32::NetworkManagement::Dns::*;
use windows::Win32::Security::{LookupAccountSidW, PSID, SID_NAME_USE};
use windows::Win32::Storage::FileSystem::{
    CreateHardLinkW, DeleteFileW, FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::core::{PCWSTR, PWSTR};

const SA: &str = "Atlas-Test-Sensor";
const SB: &str = "Atlas-Test-Process";
const SW: &str = "Atlas-Test-Watch";
/// Microsoft-Windows-Kernel-EventTracing: does it report enable changes? (plan 1b-4 input)
const KERNEL_EVENT_TRACING: u128 = 0xb675ec37_bdb6_4648_bc92_f3fdc74d3ca2;
/// `EVENT_ENABLE_PROPERTY_PROCESS_START_KEY`.
const START_KEY_PROPERTY: u32 = 0x80;
const ACTOR_ENV: &str = "ATLAS_ETW_ACTOR";
const ACTOR_LINE: &str = "ATLAS_ETW_ACTOR_RESULT ";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn qpc() -> i64 {
    let mut v = 0;
    // SAFETY: writes one i64.
    unsafe { QueryPerformanceCounter(&mut v) }.expect("QueryPerformanceCounter");
    v
}

/// One kept event, with TDH's view of it taken inside the callback.
struct Captured {
    meta: EventMeta,
    raw_id: u16,
    opcode: u8,
    flags: u16,
    pid: u32,
    tid: u32,
    ts: i64,
    start_key: Option<u64>,
    payload: Vec<u8>,
    event: Result<RawEvent, ParseError>,
    tdh: Result<Vec<(String, String)>, EtwError>,
    /// TDH's layout of this event, when it differs from our table.
    layout_mismatch: Option<String>,
    /// The classic `UserSID`, resolved the way TDH shows it (`\\DOMAIN\name`).
    account: Option<String>,
}

impl Captured {
    fn tdh_map(&self) -> Map<String, Value> {
        self.tdh.as_ref().map(|v| v.iter().map(|(k, s)| (k.clone(), json!(s))).collect()).unwrap_or_default()
    }

    fn fixture_line(&self) -> Value {
        json!({
            "provider": common::short_name(self.meta.provider),
            "id": self.raw_id,
            "version": self.meta.version,
            "opcode": self.opcode,
            "flags": format!("{:#06x}", self.flags),
            "pid": self.pid,
            "tid": self.tid,
            "ts": self.ts,
            "start_key": self.start_key.map(|k| format!("{k:#018x}")),
            "raw": common::encode_hex(&self.payload),
            "fields": self.tdh_map(),
        })
    }
}

fn account_of(sid: &[u8]) -> Option<String> {
    let (mut n, mut d) = (256u32, 256u32);
    let (mut name, mut dom) = (vec![0u16; 256], vec![0u16; 256]);
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: `sid` holds a valid SID (it parsed); the buffers have the sizes passed.
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            PSID(sid.as_ptr() as *mut _),
            Some(PWSTR(name.as_mut_ptr())),
            &mut n,
            Some(PWSTR(dom.as_mut_ptr())),
            &mut d,
            &mut use_,
        )
    }
    .ok()?;
    Some(format!(
        r"\\{}\{}",
        String::from_utf16_lossy(&dom[..d as usize]),
        String::from_utf16_lossy(&name[..n as usize])
    ))
}

type Tree = Arc<Mutex<HashSet<u32>>>;
type Sink = Arc<Mutex<Vec<Captured>>>;
/// Events of our manifest providers with an ID we did not ask for: the event-ID
/// filter should make this empty.
type Unrequested = Arc<Mutex<BTreeMap<(Provider, u16), u64>>>;
/// A Kernel-EventTracing event: QPC time, ID, header PID, start key, TDH fields.
type Watched = (i64, u16, u32, Option<u64>, Vec<(String, String)>);

/// Keeps the events of the actor's tree. Every process event is kept whatever
/// its PID: real-time buffers are per CPU, so a child's ImageLoad can arrive
/// before its ProcessStart (§3.2); [`final_tree`] rebuilds the tree in time
/// order afterwards. Network events are matched by their payload PID only:
/// their header PID is not the owner (§5.3).
fn tree_callback(tree: Tree, sink: Sink, unrequested: Unrequested) -> impl FnMut(&EventRecord) + Send + 'static {
    let mut gate = VersionGate::new();
    move |rec: &EventRecord| {
        let Some(meta) = rec.meta() else { return };
        let event = gate.parse(rec);
        if matches!(event, Err(ParseError::UnknownEvent)) && meta.provider != Provider::ClassicProcess {
            *unrequested.lock().unwrap().entry((meta.provider, meta.id)).or_default() += 1;
        }
        let keep = {
            let mut t = tree.lock().unwrap();
            let header = t.contains(&rec.pid());
            match &event {
                Ok(RawEvent::ProcessStart(s)) => {
                    if header || t.contains(&s.parent_pid) {
                        t.insert(s.pid);
                    }
                    true
                }
                Ok(RawEvent::ClassicProcess(c)) => {
                    if t.contains(&c.parent_pid) {
                        t.insert(c.pid);
                    }
                    true
                }
                Ok(RawEvent::ImageLoad(_) | RawEvent::ProcessStop(_)) => true,
                Ok(
                    RawEvent::TcpConnect(n)
                    | RawEvent::TcpAccept(n)
                    | RawEvent::TcpDisconnect(n)
                    | RawEvent::UdpSend(n)
                    | RawEvent::UdpRecv(n),
                ) => t.contains(&n.pid),
                // Events of ours we do not parse (classic opcode 11, for one).
                Err(ParseError::UnknownEvent) => false,
                _ => header,
            }
        };
        if !keep {
            return;
        }
        let layout_mismatch = layout::find(meta.provider, meta.id, meta.version).and_then(|l| {
            let ours: Vec<(String, u16)> = l.fields.iter().map(|(n, t)| (n.to_string(), *t as u16)).collect();
            match tdh::event_layout(rec) {
                Ok(theirs) if theirs == ours => None,
                Ok(theirs) => Some(format!("TDH {theirs:?}")),
                Err(e) => Some(e.to_string()),
            }
        });
        let account = match &event {
            Ok(RawEvent::ClassicProcess(c)) => c.user_sid.as_ref().and_then(|s| account_of(s.as_bytes())),
            _ => None,
        };
        sink.lock().unwrap().push(Captured {
            meta,
            raw_id: rec.id(),
            opcode: rec.opcode(),
            flags: rec.flags(),
            pid: rec.pid(),
            tid: rec.tid(),
            ts: rec.timestamp(),
            start_key: rec.start_key(),
            payload: rec.payload().to_vec(),
            event,
            tdh: tdh::decode(rec),
            layout_mismatch,
            account,
        });
    }
}

/// The actor's process tree, rebuilt in timestamp order, applied to what the
/// callback kept (see [`tree_callback`]). The observer's own classic events
/// stay too: its rundown (DCStart, DCEnd) is checked.
fn final_tree(mut events: Vec<Captured>, actor: u32, observer: u32) -> Vec<Captured> {
    events.sort_by_key(|c| c.ts);
    let mut tree = HashSet::from([actor]);
    for c in &events {
        match ok(c) {
            Some(RawEvent::ProcessStart(s)) if tree.contains(&s.parent_pid) => {
                tree.insert(s.pid);
            }
            Some(RawEvent::ClassicProcess(p)) if p.kind == ClassicKind::Start && tree.contains(&p.parent_pid) => {
                tree.insert(p.pid);
            }
            _ => {}
        }
    }
    events.retain(|c| match ok(c) {
        Some(RawEvent::ProcessStart(s)) => tree.contains(&s.pid),
        Some(RawEvent::ProcessStop(s)) => tree.contains(&s.pid),
        Some(RawEvent::ImageLoad(i)) => tree.contains(&i.pid),
        Some(RawEvent::ClassicProcess(p)) => tree.contains(&p.pid) || p.pid == observer,
        Some(
            RawEvent::TcpConnect(n)
            | RawEvent::TcpAccept(n)
            | RawEvent::TcpDisconnect(n)
            | RawEvent::UdpSend(n)
            | RawEvent::UdpRecv(n),
        ) => tree.contains(&n.pid),
        _ => tree.contains(&c.pid),
    });
    events
}

/// Everything Kernel-EventTracing logs, TDH-decoded (exploration for plan 1b-4).
fn watch_callback(sink: Arc<Mutex<Vec<Watched>>>) -> impl FnMut(&EventRecord) + Send {
    move |rec: &EventRecord| {
        if rec.provider_guid() == KERNEL_EVENT_TRACING {
            let fields = tdh::decode(rec).unwrap_or_else(|e| vec![("decode_error".into(), e.to_string())]);
            sink.lock().unwrap().push((rec.timestamp(), rec.id(), rec.pid(), rec.start_key(), fields));
        }
    }
}

/// Named QPC windows around each scenario step.
#[derive(Default)]
struct Steps(Vec<(String, i64, i64)>);

impl Steps {
    fn run<T>(&mut self, name: &str, f: impl FnOnce() -> T) -> T {
        let a = qpc();
        let r = f();
        self.0.push((name.into(), a, qpc()));
        r
    }

    fn window(&self, name: &str) -> (i64, i64) {
        self.0.iter().find(|(n, ..)| n == name).map(|(_, a, b)| (*a, *b)).unwrap_or_else(|| panic!("no step {name}"))
    }

    fn to_json(&self) -> Value {
        json!(self.0.iter().map(|(n, a, b)| json!([n, a, b])).collect::<Vec<_>>())
    }

    fn extend_from_json(&mut self, v: &Value) {
        for s in v.as_array().into_iter().flatten() {
            self.0.push((s[0].as_str().unwrap().into(), s[1].as_i64().unwrap(), s[2].as_i64().unwrap()));
        }
    }
}

/// The results: hard checks (the test fails on any) and observations.
#[derive(Default)]
struct Report {
    checks: Vec<(String, bool, String)>,
    notes: Map<String, Value>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        self.checks.push((name.into(), ok, detail.into()));
    }

    fn note(&mut self, name: &str, v: Value) {
        self.notes.insert(name.into(), v);
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by us and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(path: &str) -> Key {
    let w = wide(path);
    let mut k = HKEY::default();
    // SAFETY: valid strings and out-pointers.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut k,
            None,
        )
    };
    assert!(r.is_ok(), "RegCreateKeyExW: {r:?}");
    Key(k)
}

fn open_key(path: &str) -> Key {
    let w = wide(path);
    let mut k = HKEY::default();
    // SAFETY: valid strings and out-pointer.
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), None, KEY_ALL_ACCESS, &mut k) };
    assert!(r.is_ok(), "RegOpenKeyExW: {r:?}");
    Key(k)
}

/// `DELETE` access, for the disposition steps.
const DELETE: u32 = 0x0001_0000;

fn set_disposition(f: &std::fs::File, delete: bool) {
    use std::os::windows::io::AsRawHandle;
    let info = FILE_DISPOSITION_INFO { DeleteFile: delete };
    // SAFETY: a valid handle and a buffer of the class's size.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(f.as_raw_handle()),
            FileDispositionInfo,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    }
    .expect("SetFileInformationByHandle(FileDispositionInfo)");
}

/// A counted `UNICODE_STRING` over `name` (which may contain NULs).
fn counted(name: &[u16]) -> UNICODE_STRING {
    let len = (name.len() * 2) as u16;
    UNICODE_STRING { Length: len, MaximumLength: len, Buffer: PWSTR(name.as_ptr() as *mut _) }
}

fn dns_query(name: &str) -> u32 {
    let w = wide(name);
    let mut rec: *mut DNS_RECORDA = std::ptr::null_mut();
    // SAFETY: valid name and out-pointer; the result list is freed below.
    let st = unsafe { DnsQuery_W(PCWSTR(w.as_ptr()), DNS_TYPE_A, DNS_QUERY_STANDARD, None, &mut rec, None) };
    if !rec.is_null() {
        // SAFETY: frees the list DnsQuery_W allocated.
        unsafe { DnsFree(Some(rec as *const _), DnsFreeRecordList) };
    }
    st.0
}

/// `KUSER_SHARED_DATA.BootId` (spec §6.1; offset from public symbols, 26200/26300).
fn boot_id() -> u64 {
    // SAFETY: KUSER_SHARED_DATA is mapped read-only at this address in every process.
    u64::from(unsafe { std::ptr::read_volatile(0x7FFE_02C4 as *const u32) })
}

fn ends_with(w: &WStr, suffix: &str) -> bool {
    w.to_string_lossy().to_ascii_lowercase().ends_with(&suffix.to_ascii_lowercase())
}

fn in_window(c: &Captured, (a, b): (i64, i64)) -> bool {
    c.ts >= a && c.ts <= b
}

fn ok(c: &Captured) -> Option<&RawEvent> {
    c.event.as_ref().ok()
}

// ---------------------------------------------------------------- actor

/// The scenario, run in the actor process. Returns what the observer needs to
/// find the events again: step windows, the child's PID, ports, DNS statuses.
fn actor() -> Value {
    let me = std::process::id();
    let mut steps = Steps::default();
    // Let the observer add us to its tree before anything happens.
    std::thread::sleep(Duration::from_millis(500));

    // ---- process ----
    let cmd = format!(r"{}\System32\cmd.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
    let child_pid = steps.run("spawn_child", || {
        let mut c = std::process::Command::new(&cmd).args(["/c", "exit 7"]).spawn().expect("spawn cmd");
        let st = c.wait().expect("wait");
        assert_eq!(st.code(), Some(7));
        c.id()
    });

    // ---- files ----
    let dir = std::env::temp_dir().join(format!("atlas-etw-live-{me}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b, c, d) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("c.txt"), dir.join("d.txt"));
    steps.run("file_create_new_write", || {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&a).unwrap();
        f.write_all(b"hello").unwrap();
    });
    steps.run("file_overwrite", || std::fs::write(&a, b"x").unwrap());
    steps.run("file_set_times", || {
        let f = std::fs::OpenOptions::new().write(true).open(&a).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000)).unwrap();
    });
    steps.run("file_rename", || std::fs::rename(&a, &b).unwrap());
    steps.run("file_delete", || std::fs::remove_file(&b).unwrap());
    std::fs::write(&c, b"c").unwrap();
    steps.run("file_create_existing_fails", || {
        assert!(std::fs::OpenOptions::new().write(true).create_new(true).open(&c).is_err());
    });
    let mut perms = std::fs::metadata(&c).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&c, perms.clone()).unwrap();
    steps.run("file_delete_readonly_fails", || {
        let w = wide(&c.to_string_lossy());
        // SAFETY: a valid path.
        assert!(unsafe { DeleteFileW(PCWSTR(w.as_ptr())) }.is_err());
    });
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&c, perms).unwrap();
    std::fs::remove_file(&c).unwrap();
    steps.run("file_delete_on_close", || {
        // FILE_FLAG_DELETE_ON_CLOSE
        let f = std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(0x0400_0000).open(&d).unwrap();
        drop(f);
        assert!(!d.exists());
    });
    // Deletes the file system reports at Cleanup (plan 1b-3c): an undelete, a
    // hard link and a stream. Each handle closes inside its step, so the
    // Cleanup's OperationEnd falls in the step's window.
    let u = dir.join("u.txt");
    std::fs::write(&u, b"u").unwrap();
    steps.run("file_undelete", || {
        let f = std::fs::OpenOptions::new().access_mode(DELETE).open(&u).unwrap();
        set_disposition(&f, true);
        set_disposition(&f, false);
        drop(f);
        assert!(u.exists());
    });
    let (l, l2) = (dir.join("l.txt"), dir.join("l2.txt"));
    std::fs::write(&l, b"l").unwrap();
    // SAFETY: valid paths.
    unsafe {
        CreateHardLinkW(PCWSTR(wide(&l2.to_string_lossy()).as_ptr()), PCWSTR(wide(&l.to_string_lossy()).as_ptr()), None)
    }
    .expect("CreateHardLinkW");
    steps.run("file_delete_hard_link", || {
        std::fs::remove_file(&l2).unwrap();
        assert!(l.exists());
    });
    let s = dir.join("s.txt");
    std::fs::write(&s, b"s").unwrap();
    std::fs::write(dir.join("s.txt:x"), b"x").unwrap();
    steps.run("file_delete_stream", || {
        std::fs::remove_file(dir.join("s.txt:x")).unwrap();
        assert!(s.exists());
    });
    let _ = std::fs::remove_dir_all(&dir);

    // ---- registry ----
    let key_path = format!(r"Software\AtlasEtwLive-{me}");
    let k = steps.run("reg_create", || create_key(&key_path));
    steps.run("reg_set_value", || {
        // SAFETY: valid key, name and data.
        let r = unsafe { RegSetValueExW(k.0, PCWSTR(wide("v").as_ptr()), None, REG_DWORD, Some(&7u32.to_le_bytes())) };
        assert!(r.is_ok());
    });
    // Names with embedded NULs: values (a known way to hide Run entries) and keys.
    let nul_value = units("a\0b");
    steps.run("reg_set_value_embedded_nul", || {
        let us = counted(&nul_value);
        let data: Vec<u8> = "x\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        // SAFETY: valid key handle, counted name and data buffer.
        let st =
            unsafe { NtSetValueKey(HANDLE(k.0.0), &us, None, REG_SZ.0, Some(data.as_ptr().cast()), data.len() as u32) };
        assert!(st.is_ok(), "NtSetValueKey: {st:?}");
    });
    steps.run("reg_delete_value_embedded_nul", || {
        let us = counted(&nul_value);
        // SAFETY: valid key handle and counted name.
        let st = unsafe { NtDeleteValueKey(HANDLE(k.0.0), &us) };
        assert!(st.is_ok(), "NtDeleteValueKey: {st:?}");
    });
    steps.run("reg_create_key_embedded_nul", || {
        let name = units("k\0x");
        let us = counted(&name);
        let oa = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: HANDLE(k.0.0),
            ObjectName: &us,
            ..Default::default()
        };
        let mut h = HANDLE::default();
        // SAFETY: valid attributes, counted name and out-handle; KEY_ALL_ACCESS.
        let st = unsafe { NtCreateKey(&mut h, KEY_ALL_ACCESS.0, &oa, None, None, 0, None) };
        assert!(st.is_ok(), "NtCreateKey: {st:?}");
        // SAFETY: deletes and closes the key we just created.
        unsafe {
            let _ = NtDeleteKey(h);
            let _ = CloseHandle(h);
        }
    });
    steps.run("reg_delete_value", || {
        // SAFETY: valid key and name.
        assert!(unsafe { RegDeleteValueW(k.0, PCWSTR(wide("v").as_ptr())) }.is_ok());
    });
    let opened = steps.run("reg_open", || open_key(&key_path));
    let dup = steps.run("reg_duplicate", || {
        let mut h = HANDLE::default();
        // SAFETY: duplicates our own key handle within our process.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                HANDLE(opened.0.0),
                GetCurrentProcess(),
                &mut h,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        }
        .expect("DuplicateHandle");
        h
    });
    steps.run("reg_close_duplicate", || {
        // SAFETY: closes the duplicate once.
        unsafe { CloseHandle(dup) }.expect("CloseHandle");
    });
    steps.run("reg_close_original", || drop(opened));
    steps.run("reg_delete_key", || {
        // SAFETY: deletes our test key and its values.
        assert!(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide(&key_path).as_ptr())) }.is_ok());
    });
    steps.run("reg_close_created", || drop(k));

    // ---- network ----
    let tcp = |addr: &str| {
        let l = TcpListener::bind(addr).unwrap();
        let server_port = l.local_addr().unwrap().port();
        let mut c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let client_port = c.local_addr().unwrap().port();
        let (mut s, _) = l.accept().unwrap();
        c.write_all(&[7; 1000]).unwrap();
        let mut buf = [0u8; 1000];
        s.read_exact(&mut buf).unwrap();
        drop(c);
        drop(s);
        [client_port, server_port]
    };
    let tcp4 = steps.run("tcp_v4", || tcp("127.0.0.1:0"));
    let tcp6 = steps.run("tcp_v6", || tcp("[::1]:0"));
    let udp = |addr: &str| {
        let x = UdpSocket::bind(addr).unwrap();
        let y = UdpSocket::bind(addr).unwrap();
        x.send_to(&[1; 100], y.local_addr().unwrap()).unwrap();
        let mut buf = [0u8; 100];
        y.recv_from(&mut buf).unwrap();
        [x.local_addr().unwrap().port(), y.local_addr().unwrap().port()]
    };
    let udp4 = steps.run("udp_v4", || udp("127.0.0.1:0"));
    let udp6 = steps.run("udp_v6", || udp("[::1]:0"));

    // ---- dns ----
    let dns_ok = steps.run("dns_example", || dns_query("example.com"));
    let dns_nx = steps.run("dns_nxdomain", || dns_query("atlas-etw-live.invalid"));
    steps.run("dns_localhost", || dns_query("localhost"));

    json!({
        "pid": me,
        "child_pid": child_pid,
        "steps": steps.to_json(),
        "tcp4": tcp4, "tcp6": tcp6, "udp4": udp4, "udp6": udp6,
        "dns_ok": dns_ok, "dns_nx": dns_nx,
    })
}

/// Runs the actor (this test binary again) and returns its result.
fn run_actor(tree: &Tree) -> Value {
    let exe = std::env::current_exe().unwrap();
    let child = std::process::Command::new(exe)
        .args(["--ignored", "--exact", "live_scenario", "--nocapture", "--test-threads=1"])
        .env(ACTOR_ENV, "1")
        .env_remove("ATLAS_ETW_RECORD")
        .env_remove("ATLAS_ETW_REPORT")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn actor");
    tree.lock().unwrap().insert(child.id());
    let out = child.wait_with_output().expect("actor");
    let text = String::from_utf8_lossy(&out.stdout);
    // libtest prints "test live_scenario ... " without a newline, so the marker can be mid-line.
    let line = text
        .lines()
        .find_map(|l| l.find(ACTOR_LINE).map(|i| &l[i + ACTOR_LINE.len()..]))
        .unwrap_or_else(|| panic!("actor failed:\n{text}"));
    serde_json::from_str(line).expect("actor result")
}

// ---------------------------------------------------------------- observer

#[test]
#[ignore = "needs administrator rights; CI runs it on its Windows runner"]
fn live_scenario() {
    if std::env::var_os(ACTOR_ENV).is_some() {
        println!(
            "
{ACTOR_LINE}{}",
            actor()
        );
        return;
    }
    let me = std::process::id();
    let tree: Tree = Arc::new(Mutex::new(HashSet::new()));
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let unrequested: Unrequested = Arc::new(Mutex::new(BTreeMap::new()));
    let watch = Arc::new(Mutex::new(Vec::new()));
    let mut report = Report::default();
    let mut steps = Steps::default();

    // The watch session first, so it sees our sessions' enables.
    let sw =
        Session::start(&Config { kind: Kind::Manifest, ..Config::session_b(SW) }).expect("start watch (elevated?)");
    let cw = session::consume(SW, watch_callback(watch.clone())).expect("consume watch");
    sw.enable_guid(KERNEL_EVENT_TRACING, u64::MAX).expect("enable Kernel-EventTracing (elevated?)");

    let sa = Session::start(&Config::session_a(SA)).expect("start A");
    let sb = Session::start(&Config::session_b(SB)).expect("start B");
    let ca = session::consume(SA, tree_callback(tree.clone(), sink.clone(), unrequested.clone())).expect("consume A");
    let cb = session::consume(SB, tree_callback(tree.clone(), sink.clone(), unrequested.clone())).expect("consume B");
    let mut freq = 0;
    // SAFETY: writes one i64.
    unsafe { QueryPerformanceFrequency(&mut freq) }.expect("QueryPerformanceFrequency");
    report.check(
        "consumer_qpc_frequency_is_the_systems",
        ca.qpc_frequency() == freq && cb.qpc_frequency() == freq,
        format!("A {} B {} system {freq}", ca.qpc_frequency(), cb.qpc_frequency()),
    );
    let enables = session_a(true);
    steps.run("enable_a", || {
        for e in &enables {
            sa.enable(e).unwrap_or_else(|err| panic!("enable {:?}: {err}", e.provider));
        }
    });
    std::thread::sleep(Duration::from_millis(1500));

    let actor = steps.run("actor", || run_actor(&tree));
    steps.extend_from_json(&actor["steps"]);

    // ---- session control (§9.1 primitives) ----
    let q = session::query_by_name(SA).expect("query A");
    report.check(
        "query_by_name_logger_id",
        q.is_some_and(|i| i.logger_id == sa.logger_id()),
        format!("{q:?} vs {}", sa.logger_id()),
    );
    let file_state = session::provider_state(Provider::KernelFile).expect("provider_state");
    let ours: Vec<_> = file_state.iter().filter(|p| p.logger_id == sa.logger_id()).collect();
    report.check(
        "provider_state_shows_our_enable",
        ours.iter()
            .any(|p| p.match_any_keyword == 0x1EE0 && p.level == 5 && p.enable_property & START_KEY_PROPERTY != 0),
        format!("{ours:?}"),
    );
    let dns_state = session::provider_state(Provider::DnsClient).expect("provider_state dns");
    report.note(
        "dns_client_instances",
        json!({
            "instances": dns_state.iter().map(|p| p.pid).collect::<HashSet<_>>().len(),
            "with_our_session": dns_state.iter().filter(|p| p.logger_id == sa.logger_id()).count(),
        }),
    );
    let net = enables.iter().find(|e| e.provider == Provider::KernelNetwork).unwrap().clone();
    steps.run("ctl_disable_network", || sa.disable(Provider::KernelNetwork).expect("disable"));
    let after_disable = session::provider_state(Provider::KernelNetwork).expect("state");
    report.check(
        "disable_removes_our_enable",
        !after_disable.iter().any(|p| p.logger_id == sa.logger_id()),
        format!("{after_disable:?}"),
    );
    steps.run("ctl_reenable_network", || sa.enable(&net).expect("re-enable"));
    let after_enable = session::provider_state(Provider::KernelNetwork).expect("state");
    report.check(
        "reenable_restores_it",
        after_enable.iter().any(|p| p.logger_id == sa.logger_id()),
        format!("{after_enable:?}"),
    );
    let file = enables.iter().find(|e| e.provider == Provider::KernelFile).unwrap().clone();
    steps.run("ctl_reapply_same_file_enable", || sa.enable(&file).expect("re-apply"));
    let narrowed = Enable { event_ids: file.event_ids.iter().copied().filter(|&i| i != 16).collect(), ..file.clone() };
    steps.run("ctl_narrow_file_filter", || sa.enable(&narrowed).expect("narrow"));
    steps.run("ctl_restore_file_filter", || sa.enable(&file).expect("restore"));

    std::thread::sleep(Duration::from_millis(1500));
    let _ = sa.flush();

    // ---- a session stopped from outside ends its consumer (§9.1) ----
    let stopped = steps.run("ctl_stop_b_by_name", || session::stop_by_name(SB).expect("stop B"));
    let t0 = Instant::now();
    while !cb.is_finished() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    report.check("stop_by_name_ends_consumer", stopped.is_some() && cb.is_finished(), format!("{stopped:?}"));
    // Our handle to B is stale now: it must not act on whatever holds its LoggerId.
    let stale = (sb.is_current(), sb.query().map_err(|e| e.code));
    report.check(
        "an_externally_stopped_session_is_stale",
        stale == (false, Err(STALE)) && sb.stop().is_err_and(|e| e.code == STALE),
        format!("{stale:?}"),
    );
    report.note("process_trace_status_after_external_stop", json!(cb.close()));

    let info_a = sa.stop().expect("stop A");
    report.check("no_events_lost", info_a.events_lost == 0 && info_a.realtime_buffers_lost == 0, format!("{info_a:?}"));
    let panics = ca.panics();
    report.note("process_trace_status_after_stop", json!(ca.close()));
    let _ = sw.stop();
    let _ = cw.close();
    report.check("no_callback_panics", panics == 0, format!("{panics} panics"));

    let unrequested = unrequested.lock().unwrap().clone();
    report.check(
        "event_id_filters_deliver_only_requested_ids",
        unrequested.is_empty(),
        format!("{:?}", unrequested.iter().map(|((p, id), n)| format!("{p:?} {id}: {n}")).collect::<Vec<_>>()),
    );
    let actor_pid = actor["pid"].as_u64().unwrap() as u32;
    let events = final_tree(std::mem::take(&mut *sink.lock().unwrap()), actor_pid, me);
    analyse(&mut report, &steps, &events, &actor, me);
    explore_watch(&mut report, &steps, &watch.lock().unwrap(), me);
    finish(report, &events);
}

fn port(v: &Value, i: usize) -> u16 {
    v[i].as_u64().unwrap() as u16
}

fn analyse(r: &mut Report, steps: &Steps, events: &[Captured], actor: &Value, observer: u32) {
    let actor_pid = actor["pid"].as_u64().unwrap() as u32;
    let child = actor["child_pid"].as_u64().unwrap() as u32;
    // Every kept event parsed, matches TDH field by field, and has our layout.
    // TDH cannot decode only one event of the scenario: the SetValueKey whose
    // value name embeds a NUL (F4), checked separately below.
    let nul_window = steps.window("reg_set_value_embedded_nul");
    let mut bad = Vec::new();
    for c in events {
        match &c.event {
            Ok(e) => {
                let excused = c.tdh.is_err() && matches!(e, RawEvent::RegSetValue(_)) && in_window(c, nul_window);
                if c.tdh.is_err() && !excused {
                    bad.push(format!("{:?}: TDH could not decode it: {:?}", c.meta, c.tdh.as_ref().err()));
                }
                let errs = common::compare(e, &c.tdh_map());
                if !errs.is_empty() && c.tdh.is_ok() {
                    bad.push(format!("{:?}: {}", c.meta, errs.join("; ")));
                }
                if let RawEvent::ClassicProcess(p) = e {
                    let tdh_sid = c.tdh_map().get("UserSID").and_then(Value::as_str).map(str::to_string);
                    if p.user_sid.is_some() && c.account != tdh_sid {
                        bad.push(format!("{:?}: UserSID TDH {tdh_sid:?}, ours {:?}", c.meta, c.account));
                    }
                }
            }
            Err(e) => bad.push(format!("{:?}: parse error {e}", c.meta)),
        }
        if let Some(m) = &c.layout_mismatch {
            bad.push(format!("{:?}: layout differs: {m}", c.meta));
        }
    }
    bad.sort();
    bad.dedup();
    r.check("every_event_agrees_with_tdh", bad.is_empty(), bad.join("\n"));
    let kinds: HashSet<_> = events.iter().map(|c| (c.meta.provider, c.meta.id)).collect();
    let missing: BTreeSet<_> =
        layout::LAYOUTS.iter().map(|l| (l.provider, l.id)).filter(|k| !kinds.contains(k)).collect();
    r.check("every_parsed_kind_was_seen", missing.is_empty(), format!("missing {missing:?}"));
    r.note("events_kept", json!(events.len()));
    r.note("tdh_decode_failures", json!(events.iter().filter(|c| c.tdh.is_err()).count()));

    let evs = || events.iter().filter_map(|c| ok(c).map(|e| (c, e)));

    // Process: Launch, image load, stop, both classic halves, and the start key formula.
    let start = evs().find_map(|(_, e)| match e {
        RawEvent::ProcessStart(s) if s.pid == child => Some(s.clone()),
        _ => None,
    });
    let child_key =
        evs().find_map(|(c, e)| matches!(e, RawEvent::ImageLoad(i) if i.pid == child).then_some(c.start_key).flatten());
    r.check(
        "start_key_is_bootid_shl_48_or_sequence",
        matches!((&start, child_key), (Some(s), Some(k)) if k == (boot_id() << 48) | s.sequence_number),
        format!("seq {:?}, child key {child_key:x?}, boot id {}", start.as_ref().map(|s| s.sequence_number), boot_id()),
    );
    r.check(
        "process_start_parent_is_the_actor",
        start.as_ref().is_some_and(|s| s.parent_pid == actor_pid && ends_with(&s.image_name, r"\cmd.exe")),
        format!("{start:?}"),
    );
    r.check(
        "image_load_for_child",
        evs().any(
            |(_, e)| matches!(e, RawEvent::ImageLoad(i) if i.pid == child && ends_with(&i.image_name, r"\cmd.exe")),
        ),
        "",
    );
    r.check(
        "process_stop_exit_code",
        evs().any(|(_, e)| matches!(e, RawEvent::ProcessStop(s) if s.pid == child && s.exit_code == 7)),
        "",
    );
    let classic_start = evs().find_map(|(_, e)| match e {
        RawEvent::ClassicProcess(c) if c.pid == child && c.kind == ClassicKind::Start => Some(c.clone()),
        _ => None,
    });
    r.check(
        "classic_start_has_command_line_and_user",
        classic_start
            .as_ref()
            .is_some_and(|c| c.command_line.to_string_lossy().contains("exit 7") && c.user_sid.is_some()),
        format!("{classic_start:?}"),
    );
    r.check(
        "classic_end_for_child",
        evs().any(|(_, e)| matches!(e, RawEvent::ClassicProcess(c) if c.pid == child && c.kind == ClassicKind::End)),
        "",
    );
    let rundown: Vec<_> = evs()
        .filter_map(|(_, e)| match e {
            RawEvent::ClassicProcess(c) if c.pid == observer => Some(format!("{:?}", c.kind)),
            _ => None,
        })
        .collect();
    r.check("rundown_has_the_observers_dcstart", rundown.iter().any(|k| k == "DcStart"), format!("{rundown:?}"));
    r.note("observer_classic_events", json!(rundown));
    if let Some(s) = &start {
        // The join (§5.2): both halves of the child's launch, microseconds apart.
        let t_kp =
            events.iter().find(|c| matches!(ok(c), Some(RawEvent::ProcessStart(x)) if x.pid == s.pid)).map(|c| c.ts);
        let t_cl = events
            .iter()
            .find(|c| matches!(ok(c), Some(RawEvent::ClassicProcess(x)) if x.pid == s.pid && x.kind == ClassicKind::Start))
            .map(|c| c.ts);
        r.note("launch_join_qpc_delta", json!(t_kp.zip(t_cl).map(|(a, b)| (a - b).abs())));
    }

    // Files.
    let fev = |step: &str| -> Vec<&RawEvent> {
        let w = steps.window(step);
        events.iter().filter(|c| in_window(c, w)).filter_map(ok).collect()
    };
    let created = fev("file_create_new_write");
    let new_fo = created.iter().find_map(|e| match e {
        RawEvent::FileCreateNew(c) if ends_with(&c.file_name, r"\a.txt") => Some(c.file_object),
        _ => None,
    });
    r.check("file_create_new_30", new_fo.is_some(), "");
    r.check(
        "file_write_and_cleanup_on_that_handle",
        new_fo.is_some_and(|fo| {
            created.iter().any(|e| matches!(e, RawEvent::FileWrite(w) if w.file_object == fo))
                && created.iter().any(|e| matches!(e, RawEvent::FileCleanup(h) if h.file_object == fo))
        }),
        "",
    );
    let ow = fev("file_overwrite");
    r.check(
        "file_overwrite_is_create_plus_eof_truncation",
        ow.iter().any(|e| matches!(e, RawEvent::FileCreate(c) if ends_with(&c.file_name, r"\a.txt")))
            && ow.iter().any(|e| matches!(e, RawEvent::FileSetInfo(i) if i.info_class == 19))
            && !ow.iter().any(|e| matches!(e, RawEvent::FileCreateNew(_))),
        format!("{ow:?}"),
    );
    r.check(
        "file_set_times_is_info_class_4",
        fev("file_set_times").iter().any(|e| matches!(e, RawEvent::FileSetInfo(i) if i.info_class == 4)),
        "",
    );
    r.check(
        "file_rename_carries_new_name",
        fev("file_rename")
            .iter()
            .any(|e| matches!(e, RawEvent::FileRenamePath(p) if ends_with(&p.file_path, r"\b.txt"))),
        "",
    );
    r.check(
        "file_delete_path",
        fev("file_delete")
            .iter()
            .any(|e| matches!(e, RawEvent::FileDeletePath(p) if ends_with(&p.file_path, r"\b.txt"))),
        "",
    );
    let failed_irp = |step: &str, status: u32, pick: &dyn Fn(&RawEvent) -> Option<u64>| {
        let ev = fev(step);
        let irps: Vec<u64> = ev.iter().filter_map(|e| pick(e)).collect();
        ev.iter().any(|e| matches!(e, RawEvent::FileOpEnd(o) if o.status == status && irps.contains(&o.irp)))
    };
    r.check(
        "failed_create_new_has_failed_opend",
        failed_irp("file_create_existing_fails", 0xC000_0035, &|e| match e {
            RawEvent::FileCreate(c) if ends_with(&c.file_name, r"\c.txt") => Some(c.irp),
            _ => None,
        }),
        "",
    );
    r.check(
        "failed_delete_has_failed_opend",
        failed_irp("file_delete_readonly_fails", 0xC000_0121, &|e| match e {
            RawEvent::FileDeletePath(p) => Some(p.irp),
            _ => None,
        }),
        "",
    );
    r.check(
        "delete_on_close_flag_on_create",
        fev("file_delete_on_close")
            .iter()
            .any(|e| matches!(e, RawEvent::FileCreate(c) if ends_with(&c.file_name, r"\d.txt") && c.delete_on_close())),
        "",
    );
    // The Cleanup outcome (plan 1b-3c): the OperationEnd of the Cleanups in a
    // step report FILE_CLEANUP_* in ExtraInformation.
    // Irps are recycled: each Cleanup pairs with the next OperationEnd of its
    // Irp (the events are in time order).
    let outcomes = |step: &str| -> Vec<u64> {
        let ev = fev(step);
        ev.iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let RawEvent::FileCleanup(h) = e else { return None };
                ev[i + 1..].iter().find_map(|x| match x {
                    RawEvent::FileOpEnd(o) if o.irp == h.irp => Some(o.extra_information),
                    _ => None,
                })
            })
            .collect()
    };
    let has = |v: &[u64], bit: u64| v.iter().any(|x| x & bit != 0);
    let (deleted, link, stream) = (4, 8, 0x10);
    let o = outcomes("file_delete");
    r.check("cleanup_outcome_file_deleted", has(&o, deleted), format!("{o:x?}"));
    let o = outcomes("file_delete_on_close");
    r.check("cleanup_outcome_delete_on_close", has(&o, deleted), format!("{o:x?}"));
    let o = outcomes("file_undelete");
    r.check("cleanup_outcome_undelete_remains", !o.is_empty() && o.iter().all(|x| *x == 2), format!("{o:x?}"));
    let sd: Vec<u64> = fev("file_undelete")
        .iter()
        .filter_map(|e| if let RawEvent::FileSetDelete(i) = e { Some(i.extra_information) } else { None })
        .collect();
    r.check("set_delete_on_set_and_clear", sd == [1, 0], format!("{sd:?}"));
    let o = outcomes("file_delete_hard_link");
    r.check("cleanup_outcome_link_deleted", has(&o, link) && !has(&o, deleted), format!("{o:x?}"));
    let o = outcomes("file_delete_stream");
    r.check("cleanup_outcome_stream_deleted", has(&o, stream) && !has(&o, deleted), format!("{o:x?}"));

    // Registry.
    let rev = fev;
    let created_key = rev("reg_create").iter().find_map(|e| match e {
        RawEvent::RegCreateKey(o) if o.status == 0 && o.disposition == 1 => Some(o.clone()),
        _ => None,
    });
    r.check(
        "reg_create_key_created_new",
        created_key.as_ref().is_some_and(|o| ends_with(&o.relative_name, &format!("AtlasEtwLive-{actor_pid}"))),
        format!("{created_key:?}"),
    );
    let ko = created_key.as_ref().map(|o| o.key_object);
    r.check(
        "reg_set_value_on_created_key",
        rev("reg_set_value").iter().any(|e| {
            matches!(e, RawEvent::RegSetValue(v) if Some(v.key_object) == ko && v.value_name.to_string_lossy() == "v" && v.value_type == 4 && v.data_size == 4)
        }),
        "",
    );
    let nul_value = units("a\0b");
    r.check(
        "embedded_nul_value_name_on_set",
        rev("reg_set_value_embedded_nul")
            .iter()
            .any(|e| matches!(e, RawEvent::RegSetValue(v) if v.value_name.as_units() == nul_value && v.value_type == 1 && v.data_size == 4)),
        format!("{:?}", rev("reg_set_value_embedded_nul")),
    );
    r.check(
        "embedded_nul_value_name_on_delete",
        rev("reg_delete_value_embedded_nul")
            .iter()
            .any(|e| matches!(e, RawEvent::RegDeleteValue(v) if v.value_name.as_units() == nul_value && v.status == 0)),
        format!("{:?}", rev("reg_delete_value_embedded_nul")),
    );
    r.check(
        "embedded_nul_key_name_on_create",
        rev("reg_create_key_embedded_nul").iter().any(
            |e| matches!(e, RawEvent::RegCreateKey(o) if o.relative_name.as_units() == units("k\0x") && o.status == 0),
        ),
        format!("{:?}", rev("reg_create_key_embedded_nul")),
    );
    r.check(
        "reg_delete_value",
        rev("reg_delete_value").iter().any(
            |e| matches!(e, RawEvent::RegDeleteValue(v) if v.value_name.to_string_lossy() == "v" && v.status == 0),
        ),
        "",
    );
    let opened = rev("reg_open").iter().find_map(|e| match e {
        RawEvent::RegOpenKey(o) if o.status == 0 => Some(o.key_object),
        _ => None,
    });
    let closes = |step: &str| {
        rev(step).iter().filter(|e| matches!(e, RawEvent::RegCloseKey(k) if Some(k.key_object) == opened)).count()
    };
    r.note(
        "closekey_per_handle",
        json!({
            "closes_when_duplicate_closed": closes("reg_close_duplicate"),
            "closes_when_original_closed": closes("reg_close_original"),
        }),
    );
    r.check(
        "closekey_only_on_the_last_handle",
        closes("reg_close_duplicate") == 0 && closes("reg_close_original") == 1,
        "",
    );
    r.check(
        "reg_delete_key",
        rev("reg_delete_key").iter().any(|e| matches!(e, RawEvent::RegDeleteKey(k) if k.status == 0)),
        "",
    );
    r.check(
        "closekey_for_created_key",
        rev("reg_close_created").iter().any(|e| matches!(e, RawEvent::RegCloseKey(k) if Some(k.key_object) == ko)),
        "",
    );

    // Network: which end is saddr (§7.3)? Local for connect, accept and UDP send;
    // remote (the sender) for UDP receive.
    let (tcp4, tcp6, udp4, udp6) = (&actor["tcp4"], &actor["tcp6"], &actor["udp4"], &actor["udp6"]);
    let net = |step: &str| -> Vec<String> {
        rev(step)
            .iter()
            .filter_map(|e| match e {
                RawEvent::TcpConnect(n) => Some(format!("connect s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                RawEvent::TcpAccept(n) => Some(format!("accept s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                RawEvent::TcpDisconnect(n) => {
                    Some(format!("disconnect s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport))
                }
                RawEvent::UdpSend(n) => Some(format!("udp_send s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                RawEvent::UdpRecv(n) => Some(format!("udp_recv s{}:{} d{}:{}", n.saddr, n.sport, n.daddr, n.dport)),
                _ => None,
            })
            .collect()
    };
    let has = |step: &str, f: &dyn Fn(&RawEvent) -> bool| rev(step).iter().any(|e| f(e));
    for (step, ports, v6) in [("tcp_v4", tcp4, false), ("tcp_v6", tcp6, true)] {
        let (client, server) = (port(ports, 0), port(ports, 1));
        r.check(
            &format!("{step}_connect_and_accept_have_local_as_saddr"),
            has(step, &|e| {
                matches!(e, RawEvent::TcpConnect(n) if n.sport == client && n.dport == server && n.daddr.is_ipv6() == v6)
            }) && has(step, &|e| matches!(e, RawEvent::TcpAccept(n) if n.sport == server && n.dport == client)),
            format!("{:?}", net(step)),
        );
        r.check(&format!("{step}_disconnect"), has(step, &|e| matches!(e, RawEvent::TcpDisconnect(_))), "");
    }
    for (step, ports, v6) in [("udp_v4", udp4, false), ("udp_v6", udp6, true)] {
        let (sender, receiver) = (port(ports, 0), port(ports, 1));
        r.check(
            &format!("{step}_send_local_is_saddr_receive_local_is_daddr"),
            has(step, &|e| {
                matches!(e, RawEvent::UdpSend(n) if n.sport == sender && n.dport == receiver && n.daddr.is_ipv6() == v6)
            }) && has(step, &|e| matches!(e, RawEvent::UdpRecv(n) if n.sport == sender && n.dport == receiver)),
            format!("{:?}", net(step)),
        );
    }

    // DNS.
    let q = |step: &str| -> Vec<String> {
        rev(step)
            .iter()
            .filter_map(|e| match e {
                RawEvent::DnsQuery(q) => Some(format!(
                    "{} type {} status {} results {:?}",
                    q.query_name.to_string_lossy(),
                    q.query_type,
                    q.query_status,
                    q.query_results
                )),
                _ => None,
            })
            .collect()
    };
    r.check(
        "dns_3008_for_example_com",
        has("dns_example", &|e| {
            matches!(e, RawEvent::DnsQuery(q) if q.query_name.to_string_lossy() == "example.com" && q.query_type == 1)
        }),
        format!("{:?} (DnsQuery_W returned {})", q("dns_example"), actor["dns_ok"]),
    );
    r.check(
        "dns_3008_for_nxdomain",
        has("dns_nxdomain", &|e| {
            matches!(e, RawEvent::DnsQuery(q) if q.query_name.to_string_lossy() == "atlas-etw-live.invalid" && q.query_status != 0)
        }),
        format!("{:?} (DnsQuery_W returned {})", q("dns_nxdomain"), actor["dns_nx"]),
    );
    r.note("dns_localhost", json!(q("dns_localhost")));
}

/// What Kernel-EventTracing logged around each enable change, and whom it
/// attributes the change to (plan 1b-4 input).
fn explore_watch(r: &mut Report, steps: &Steps, watch: &[Watched], observer: u32) {
    let mut out = Map::new();
    for step in [
        "enable_a",
        "ctl_disable_network",
        "ctl_reenable_network",
        "ctl_reapply_same_file_enable",
        "ctl_narrow_file_filter",
        "ctl_restore_file_filter",
        "ctl_stop_b_by_name",
    ] {
        let (a, b) = steps.window(step);
        let evs: Vec<Value> = watch
            .iter()
            .filter(|(ts, ..)| *ts >= a && *ts <= b)
            .map(|(_, id, pid, key, f)| {
                json!({
                    "id": id,
                    "header_pid_is_the_caller": *pid == observer,
                    "has_start_key": key.is_some(),
                    "fields": f.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<Map<_, _>>(),
                })
            })
            .collect();
        out.insert(step.into(), Value::Array(evs));
    }
    out.insert("total_events".into(), json!(watch.len()));
    r.note("kernel_event_tracing", Value::Object(out));
}

fn finish(r: Report, events: &[Captured]) {
    let failed: Vec<_> = r.checks.iter().filter(|(_, ok, _)| !ok).collect();
    for (name, ok, detail) in &r.checks {
        let detail = if *ok || detail.is_empty() { String::new() } else { format!(": {detail}") };
        println!("{} {name}{detail}", if *ok { "PASS" } else { "FAIL" });
    }
    let doc = json!({
        "checks": r.checks.iter().map(|(n, ok, d)| json!({ "name": n, "ok": ok, "detail": d })).collect::<Vec<_>>(),
        "notes": r.notes,
    });
    if let Ok(path) = std::env::var("ATLAS_ETW_REPORT") {
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).expect("write report");
    }
    if let Ok(path) = std::env::var("ATLAS_ETW_RECORD") {
        let mut lines: Vec<&Captured> = events.iter().filter(|c| c.event.is_ok() && c.tdh.is_ok()).collect();
        lines.sort_by_key(|c| c.ts);
        let text: String = lines.iter().map(|c| format!("{}\n", c.fixture_line())).collect();
        std::fs::write(&path, text).expect("write fixtures");
    }
    assert!(failed.is_empty(), "{} check(s) failed", failed.len());
}
