//! Tier 3 for the agent (sensor spec §12.3; plan 1b-3c, decision D2): the real
//! sessions, services and pipeline thread (`win::agent`), and a scripted actor.
//! Needs administrator rights, so it is `#[ignore]`d; CI's `agent-live` job
//! runs it:
//!
//! `cargo test -p atlas-agent --test live -- --ignored --nocapture`
//!
//! The agent filters its own events (its start key), so the scenario runs in
//! a second process, the **actor**: this test binary again, with
//! `ATLAS_AGENT_ACTOR=1`. It talks to the observer over its stdin and stdout:
//! 1. it opens a file and a registry key, prints `READY` and its paths;
//! 2. the observer starts the agent and waits for the start-up seeding pass;
//! 3. the observer writes `GO`; the actor writes to the file and creates a key
//!    under the one it holds (both named only by seeding), sets values, opens
//!    a watchlisted file by its long and its 8.3 name, undeletes two files,
//!    deletes one, and runs `cmd.exe /c exit 7`;
//! 4. it prints its result and exits; the observer waits out the deadlines,
//!    stops the agent cleanly and checks the events of the actor's tree.
//!
//! `ATLAS_AGENT_REPORT=<file>` writes the checks and the counters as JSON.
#![cfg(windows)]

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atlas_agent::config::{Config, ServiceConfig};
use atlas_agent::driver::DriverConfig;
use atlas_agent::win::agent::{self, AgentOptions};
use atlas_schema::classes::file::FileAction;
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{RegistryKeyAction, RegistryValueAction};
use atlas_schema::{Event, EventKind, SignatureStatus};
use serde_json::{Value, json};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_DISPOSITION_FLAG_ON_CLOSE, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_DISPOSITION_INFO_EX_FLAGS,
    FileDispositionInfo, FileDispositionInfoEx, GetShortPathNameW, SetFileInformationByHandle,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_DWORD, REG_OPTION_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

const ACTOR_ENV: &str = "ATLAS_AGENT_ACTOR";
const READY: &str = "ATLAS_AGENT_READY ";
const RESULT: &str = "ATLAS_AGENT_RESULT ";
const SA: &str = "Atlas-Test-Agent-Sensor";
const SB: &str = "Atlas-Test-Agent-Process";
/// `DELETE` access.
const DELETE: u32 = 0x0001_0000;
/// A long name, so the file has an 8.3 name distinct from it.
const WATCHED: &str = "watched-secret-file.txt";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

// ---------------------------------------------------------------- actor

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: closes a key we opened, once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn create_key(parent: HKEY, sub: &str) -> Key {
    let mut h = HKEY::default();
    // SAFETY: valid parent, NUL-terminated name and out-handle; a volatile key.
    unsafe {
        RegCreateKeyExW(
            parent,
            &HSTRING::from(sub),
            None,
            None,
            REG_OPTION_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut h,
            None,
        )
    }
    .ok()
    .unwrap_or_else(|e| panic!("create {sub}: {e}"));
    Key(h)
}

fn set_value(k: &Key, name: &str, ty: windows::Win32::System::Registry::REG_VALUE_TYPE, data: &[u8]) {
    let n = wide(name);
    // SAFETY: a valid key, NUL-terminated name and the data buffer.
    unsafe { RegSetValueExW(k.0, PCWSTR(n.as_ptr()), None, ty, Some(data)) }.ok().expect("RegSetValueExW");
}

fn set_disposition(f: &std::fs::File, delete: bool) {
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
    .expect("FileDispositionInfo");
}

/// `FILE_DISPOSITION_FLAG_ON_CLOSE` without `DELETE`: clears delete-on-close.
fn clear_delete_on_close(f: &std::fs::File) {
    let info = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_INFO_EX_FLAGS(FILE_DISPOSITION_FLAG_ON_CLOSE.0) };
    // SAFETY: a valid handle and a buffer of the class's size.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(f.as_raw_handle()),
            FileDispositionInfoEx,
            (&raw const info).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    }
    .expect("FileDispositionInfoEx");
}

fn short_path(p: &Path) -> String {
    let w = wide(&p.to_string_lossy());
    let mut buf = [0u16; 1024];
    // SAFETY: a NUL-terminated path and a writable buffer.
    let n = unsafe { GetShortPathNameW(PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
    assert!(n > 0 && n < buf.len(), "GetShortPathNameW");
    String::from_utf16_lossy(&buf[..n])
}

/// The long form of an existing directory: the runner's `TEMP` is an 8.3 path
/// (`C:\Users\RUNNER~1\…`), the agent reports long ones.
fn long_dir(p: &Path) -> PathBuf {
    let c = std::fs::canonicalize(p).unwrap().to_string_lossy().to_string();
    PathBuf::from(c.strip_prefix(r"\\?\").unwrap_or(&c))
}

fn read_line(expect: &str) {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line.trim(), expect);
}

/// Deletes the actor's directory and test key when dropped, also when the
/// actor fails (review R-m11).
struct Leftovers {
    dir: PathBuf,
    key: String,
}

impl Drop for Leftovers {
    fn drop(&mut self) {
        // SAFETY: deletes our volatile test key.
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.key.as_str())) };
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn actor() {
    let me = std::process::id();
    let dir = long_dir(&std::env::temp_dir()).join(format!("atlas-agent-live-{me}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key_path = format!(r"Software\AtlasAgentLive-{me}");
    let leftovers = Leftovers { dir: dir.clone(), key: key_path.clone() };

    // ---- before the agent: a held file handle, a held key, the watched file ----
    let held = dir.join("held.txt");
    let mut held_file = std::fs::File::create(&held).unwrap();
    let held_key = create_key(HKEY_CURRENT_USER, &key_path);
    let watched = dir.join(WATCHED);
    std::fs::write(&watched, b"secret").unwrap();
    let short = short_path(&watched);
    assert_ne!(short.to_lowercase(), watched.to_string_lossy().to_lowercase(), "8.3 names are made here");
    println!("\n{READY}{}", json!({ "dir": dir, "key": key_path }));
    let _ = std::io::stdout().flush();
    read_line("GO");

    // ---- seeding: handles the agent never saw opened ----
    held_file.write_all(b"written after the agent started").unwrap();
    drop(held_file);
    let child = create_key(held_key.0, "child");
    drop(child);

    // ---- value reads ----
    let vals = create_key(HKEY_CURRENT_USER, &format!(r"{key_path}\values"));
    let s: Vec<u8> = "hello\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
    set_value(&vals, "s", REG_SZ, &s);
    set_value(&vals, "d", REG_DWORD, &7u32.to_le_bytes());
    drop(vals);

    // ---- watchlist: long name, then 8.3 name ----
    drop(std::fs::File::open(&watched).unwrap());
    drop(std::fs::File::open(&short).unwrap());

    // ---- undeletes, and a delete ----
    let u1 = dir.join("undelete-1.txt");
    std::fs::write(&u1, b"u").unwrap();
    {
        let f = std::fs::OpenOptions::new().access_mode(DELETE).open(&u1).unwrap();
        set_disposition(&f, true);
        set_disposition(&f, false);
    }
    let u2 = dir.join("undelete-2.txt");
    {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(0x4000_0000 | DELETE) // GENERIC_WRITE | DELETE
            .custom_flags(0x0400_0000) // FILE_FLAG_DELETE_ON_CLOSE
            .open(&u2)
            .unwrap();
        clear_delete_on_close(&f);
    }
    let gone = dir.join("deleted.txt");
    std::fs::write(&gone, b"d").unwrap();
    std::fs::remove_file(&gone).unwrap();
    assert!(u1.exists() && u2.exists() && !gone.exists());

    // ---- a process ----
    let cmd = format!(r"{}\System32\cmd.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
    let mut c = Command::new(&cmd).args(["/c", "exit 7"]).spawn().unwrap();
    let child_pid = c.id();
    assert_eq!(c.wait().unwrap().code(), Some(7));

    println!(
        "\n{RESULT}{}",
        json!({
            "pid": me, "child_pid": child_pid, "dir": dir, "key": key_path,
            "watched": watched, "short": short, "cmd": cmd,
        })
    );
    let _ = std::io::stdout().flush();
    // Clean up only once the agent has stopped: the value reads come about a
    // second after the sets, and deleting the watched file opens it again.
    read_line("DONE");
    drop(held_key);
    drop(leftovers);
}

// ---------------------------------------------------------------- observer

#[derive(Default)]
struct Report {
    checks: Vec<(String, bool, String)>,
    notes: serde_json::Map<String, Value>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        self.checks.push((name.into(), ok, detail.into()));
    }

    fn note(&mut self, name: &str, v: Value) {
        self.notes.insert(name.into(), v);
    }
}

/// The actor's line with `marker` from its stdout.
fn wait_line(lines: &mut impl Iterator<Item = std::io::Result<String>>, marker: &str) -> Value {
    for l in lines {
        let l = l.expect("actor stdout");
        if let Some(i) = l.find(marker) {
            return serde_json::from_str(&l[i + marker.len()..]).expect("actor JSON");
        }
    }
    panic!("the actor ended without {marker}");
}

fn dos(p: &str) -> String {
    p.to_lowercase()
}

#[test]
#[ignore = "needs administrator rights; CI runs it on its Windows runner"]
fn live_agent() {
    if std::env::var_os(ACTOR_ENV).is_some() {
        actor();
        return;
    }
    let me = std::process::id();
    let mut actor = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "live_agent", "--nocapture", "--test-threads=1"])
        .env(ACTOR_ENV, "1")
        .env_remove("ATLAS_AGENT_REPORT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the actor");
    let actor_pid = actor.id();
    let mut lines = BufReader::new(actor.stdout.take().unwrap()).lines();
    let ready = wait_line(&mut lines, READY);
    let dir = PathBuf::from(ready["dir"].as_str().unwrap());

    // ---- the agent ----
    let state = tempdir();
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let sink = events.clone();
    let dir_name = dir.file_name().unwrap().to_string_lossy().to_string();
    let config = Config { watchlist_extend: vec![format!(r"**\{dir_name}\{WATCHED}")], ..Config::default() };
    let opts = AgentOptions {
        sensor_session: SA.into(),
        process_session: SB.into(),
        config,
        services: ServiceConfig::default(),
        driver: DriverConfig::default(),
        device_file: state.join("device.json"),
    };
    let started = Instant::now();
    let running = agent::start(opts, move |e| sink.lock().unwrap().push(e)).expect("start the agent (elevated?)");
    let mut report = Report::default();
    report.check("seeding_enabled", running.seeding(), "SeDebugPrivilege");
    // The start-up pass: two table reads per snapshot, one snapshot per kind.
    let svc = running.service_counters().clone();
    let t0 = Instant::now();
    while svc.seeder_table_reads.load(Ordering::Relaxed) < 4 && t0.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(50));
    }
    report.note("startup_seeding_ms", json!(t0.elapsed().as_millis() as u64));
    report.note("start_ms", json!(started.elapsed().as_millis() as u64));

    // ---- the scenario ----
    let mut stdin = actor.stdin.take().unwrap();
    writeln!(stdin, "GO").unwrap();
    let result = wait_line(&mut lines, RESULT);
    // Deadlines: 1 s for most, 5 s for the seeder during the first 30 s (§3.2).
    std::thread::sleep(Duration::from_secs(6));
    // Work on the pipeline thread from outside (plan 1b-4's Sensor Health reads).
    let (tx, rx) = std::sync::mpsc::channel();
    let sent = running.control().send(Box::new(move |p| {
        let _ = tx.send(p.counters().self_filtered);
    }));
    let read = sent.is_ok() && rx.recv_timeout(Duration::from_secs(5)).is_ok();
    report.check("control_reaches_the_pipeline", read && running.pipeline_running(), "");
    let stopped = running.stop();
    let events = std::mem::take(&mut *events.lock().unwrap());
    writeln!(stdin, "DONE").unwrap();
    drop(stdin);
    for _ in lines.by_ref() {}
    assert!(actor.wait().unwrap().success(), "the actor failed");

    analyse(&mut report, &events, &result, actor_pid, me);
    let c = &stopped.pipeline;
    let i = &stopped.intake;
    let s = &stopped.services;
    let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
    report.check("no_queue_drops", get(&i.kernel_queue_drops) == 0 && get(&i.user_queue_drops) == 0, "");
    report.check("no_service_queue_drops", get(&s.service_queue_drops) == 0, "");
    report.check("no_callback_panics", stopped.callback_panics == 0, "");
    // The agent's own hashing, value reads and seeding make events; the
    // self-filter must have dropped some, or the check above proves nothing
    // (review R-m6).
    report.check("the_self_filter_dropped_the_agents_events", c.self_filtered > 0, format!("{}", c.self_filtered));
    report.note(
        "counters",
        json!({
            "events": events.len(),
            "late_arrivals": c.late_arrivals,
            "self_filtered": c.self_filtered,
            "unknown_file_object": c.unknown_file_object,
            "registry_unresolved": c.registry_unresolved,
            "pending_overflow": c.pending_overflow,
            "file_delete_outcome_unknown": c.file_delete_outcome_unknown,
            "fast_reads": get(&i.fast_reads),
            "cleanup_unpaired": get(&i.cleanup_unpaired),
            "cleanup_outcome_late": get(&i.cleanup_outcome_late),
            "early_read_redone": c.early_read_redone,
            "op_end_discarded": get(&i.op_end_discarded),
            "seeder_handles_named": get(&s.seeder_handles_named),
            "seeder_table_reads": get(&s.seeder_table_reads),
        }),
    );
    finish(report);
    let _ = std::fs::remove_dir_all(&state);
}

fn tempdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("atlas-agent-live-state-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn analyse(r: &mut Report, events: &[Event], result: &Value, actor_pid: u32, observer: u32) {
    let child_pid = result["child_pid"].as_u64().unwrap() as u32;
    let dir = result["dir"].as_str().unwrap().to_lowercase();
    let key = result["key"].as_str().unwrap().to_string();
    let tree: HashSet<u32> = [actor_pid, child_pid].into();
    let actor_of = |e: &Event| -> Option<u32> {
        Some(match &e.kind {
            EventKind::File(f) => f.actor.pid,
            EventKind::RegistryKey(k) => k.actor.pid,
            EventKind::RegistryValue(v) => v.actor.pid,
            EventKind::Process(ProcessActivity::Launch { actor, .. }) => actor.pid,
            EventKind::Process(ProcessActivity::Terminate { process, .. }) => process.pid,
            EventKind::Module(m) => m.actor.pid,
            EventKind::Network(n) => n.actor.pid,
            EventKind::Dns(d) => d.actor.pid,
            _ => return None,
        })
    };
    r.check(
        "nothing_from_the_agent_itself",
        !events.iter().any(|e| actor_of(e) == Some(observer)),
        "the agent's hashing, value reads and seeding are self-filtered",
    );
    let ours: Vec<&Event> = events.iter().filter(|e| actor_of(e).is_some_and(|p| tree.contains(&p))).collect();
    r.note("actor_events", json!(ours.len()));
    let files: Vec<(&FileAction, String)> = ours
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::File(f) if f.file.path.to_lowercase().starts_with(&dir) => {
                Some((&f.action, f.file.path.to_lowercase()))
            }
            _ => None,
        })
        .collect();
    r.note("file_events", json!(files.iter().map(|(a, p)| format!("{a:?} {p}")).collect::<Vec<_>>()));
    let in_dir = |n: &str| format!(r"{dir}\{n}");

    // Seeding: the write on a handle opened before the agent.
    r.check(
        "seeded_file_handle_update",
        files.iter().any(|(a, p)| matches!(a, FileAction::Update) && *p == in_dir("held.txt")),
        "",
    );
    let child_key = ours.iter().find_map(|e| match &e.kind {
        EventKind::RegistryKey(k) if k.action == RegistryKeyAction::Create && k.path.ends_with(r"\child") => {
            Some((k.path.clone(), k.path_unresolved))
        }
        _ => None,
    });
    // Key names keep the case their hive stores (`SOFTWARE` on the CI runner).
    let under = |path: &str, sub: &str| path.to_lowercase().ends_with(&format!(r"\{key}\{sub}").to_lowercase());
    r.check(
        "seeded_key_handle_create_under",
        child_key.as_ref().is_some_and(|(p, unresolved)| !unresolved && under(p, "child")),
        format!("{child_key:?}"),
    );

    // Value reads.
    let value = |name: &str| {
        ours.iter().find_map(|e| match &e.kind {
            EventKind::RegistryValue(v) if v.name == name && under(&v.key_path, "values") => match &v.action {
                RegistryValueAction::Set { data, data_read_after, .. } => Some((data.clone(), *data_read_after)),
                RegistryValueAction::Delete => None,
            },
            _ => None,
        })
    };
    let s = value("s");
    let hello: Vec<u8> = "hello\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
    r.check("value_read_reg_sz", s.as_ref().is_some_and(|(d, after)| *after && *d == hello), format!("{s:?}"));
    let d = value("d");
    r.check(
        "value_read_reg_dword",
        d.as_ref().is_some_and(|(v, after)| *after && *v == 7u32.to_le_bytes()),
        format!("{d:?}"),
    );

    // Watchlist: two Opens, both under the long name.
    let watched = result["watched"].as_str().unwrap().to_lowercase();
    let opens: Vec<&String> = files.iter().filter(|(a, _)| matches!(a, FileAction::Open)).map(|(_, p)| p).collect();
    r.check(
        "watchlist_open_long_and_8dot3",
        opens.len() == 2 && opens.iter().all(|p| **p == watched),
        format!("{opens:?} (short {})", result["short"]),
    );

    // Undeletes give no Delete; the real delete gives one.
    let deletes: Vec<&String> = files.iter().filter(|(a, _)| matches!(a, FileAction::Delete)).map(|(_, p)| p).collect();
    r.check(
        "undelete_gives_no_delete",
        !deletes.iter().any(|p| p.ends_with("undelete-1.txt") || p.ends_with("undelete-2.txt")),
        format!("{deletes:?}"),
    );
    r.check("delete_is_reported", deletes.contains(&&in_dir("deleted.txt")), format!("{deletes:?}"));

    // The Launch, enriched by the real services.
    let launch = ours.iter().find_map(|e| match &e.kind {
        EventKind::Process(ProcessActivity::Launch { process, actor }) if process.pid == child_pid => {
            Some((process.clone(), actor.clone()))
        }
        _ => None,
    });
    let cmd = dos(result["cmd"].as_str().unwrap());
    r.check(
        "launch_dos_path_and_command_line",
        launch
            .as_ref()
            .is_some_and(|(p, a)| dos(&p.file.path) == cmd && p.cmd_line.contains("exit 7") && a.pid == actor_pid),
        format!("{:?}", launch.as_ref().map(|(p, _)| (&p.file.path, &p.cmd_line))),
    );
    r.check(
        "launch_sha256",
        launch.as_ref().is_some_and(|(p, _)| p.file.hashes.as_ref().is_some_and(|h| h.sha256.is_some())),
        "",
    );
    r.check(
        "launch_signature_valid",
        launch.as_ref().is_some_and(|(p, _)| {
            p.file.signature.as_ref().is_some_and(|s| s.status == SignatureStatus::Valid && s.signer.is_some())
        }),
        format!("{:?}", launch.as_ref().map(|(p, _)| &p.file.signature)),
    );
    r.check(
        "launch_account_name",
        launch.as_ref().is_some_and(|(p, _)| p.user.as_ref().is_some_and(|u| u.name.contains('\\'))),
        format!("{:?}", launch.as_ref().map(|(p, _)| &p.user)),
    );
    r.check(
        "terminate_exit_code",
        ours.iter().any(|e| {
            matches!(&e.kind,
            EventKind::Process(ProcessActivity::Terminate { process, exit_code: Some(7) }) if process.pid == child_pid)
        }),
        "",
    );
}

fn finish(r: Report) {
    let failed: Vec<_> = r.checks.iter().filter(|(_, ok, _)| !ok).collect();
    for (name, ok, detail) in &r.checks {
        println!("{} {name} {detail}", if *ok { "PASS" } else { "FAIL" });
    }
    println!("notes: {}", Value::Object(r.notes.clone()));
    if let Some(path) = std::env::var_os("ATLAS_AGENT_REPORT") {
        let checks: Vec<Value> =
            r.checks.iter().map(|(n, ok, d)| json!({ "name": n, "ok": ok, "detail": d })).collect();
        std::fs::write(path, serde_json::to_string_pretty(&json!({ "checks": checks, "notes": r.notes })).unwrap())
            .unwrap();
    }
    assert!(
        failed.is_empty(),
        "{} check(s) failed: {:?}",
        failed.len(),
        failed.iter().map(|f| &f.0).collect::<Vec<_>>()
    );
}
