//! The binary-level live test (sensor spec §12.3; plan 1b-4a, D4): the real
//! `atlas-agent run` and `atlas-agent dump`, end to end. Needs administrator
//! rights, so it is `#[ignore]`d; CI's `agent-live` job runs it:
//!
//! `cargo test -p atlas-agent --test binary -- --ignored --nocapture`
//!
//! 1. `atlas-agent run --data-dir C:\ProgramData\Atlas-Test-binary-<pid>`
//!    starts in its own process group, which creates the data directory;
//! 2. a second `run` is refused by the single-instance lock;
//! 3. an actor (this test binary again, `ATLAS_BINARY_ACTOR=1`) creates,
//!    renames and deletes a file, creates a registry key and value and deletes
//!    them, opens a TCP connection, resolves two names and runs `cmd.exe`;
//! 4. after the first periodic Sensor Health report, Ctrl+Break stops the
//!    agent, which must exit 0 after a clean stop;
//! 5. the buffer is read twice, through `atlas-agent dump` and directly, and
//!    checked: the scenario's classes, the Sensor Health reports with zero
//!    loss, nothing of the agent's own; the directories are protected;
//! 6. `run` refuses a data directory that inherits its parent's ACL, and one
//!    below `%TEMP%`, whose user could swap a folder above it.
//!
//! `ATLAS_AGENT_REPORT=<file>` writes the checks and the notes as JSON. It
//! uses the real session names, so nothing else may run the sensor meanwhile.
#![cfg(windows)]

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atlas_agent::win::datadir;
use atlas_buffer::{Cursor, Reader};
use atlas_schema::classes::file::FileAction;
use atlas_schema::classes::network::NetworkAction;
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::classes::registry::{RegistryKeyAction, RegistryValueAction};
use atlas_schema::classes::sensor_health::{HealthReport, SensorHealthAction};
use atlas_schema::{Event, EventKind, decode_event};
use serde_json::{Value, json};
use windows::Win32::System::Console::{AllocConsole, CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetConsoleWindow};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

const AGENT: &str = env!("CARGO_BIN_EXE_atlas-agent");
const ACTOR_ENV: &str = "ATLAS_BINARY_ACTOR";
const RESULT: &str = "ATLAS_BINARY_RESULT ";
const SESSIONS: [&str; 2] = ["Atlas-Sensor", "Atlas-Process"];
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

// ---------------------------------------------------------------- actor

fn actor() {
    let me = std::process::id();
    // The long form: the runner's TEMP is an 8.3 path, the agent reports long ones.
    let dir = std::env::temp_dir().join(format!("atlas-binary-actor-{me}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let long = std::fs::canonicalize(&dir).unwrap().to_string_lossy().to_string();
    let dir = PathBuf::from(long.strip_prefix(r"\\?\").unwrap_or(&long));

    // Files: create and write, rename, delete.
    let a = dir.join("a.txt");
    std::fs::write(&a, b"atlas").unwrap();
    let b = dir.join("b.txt");
    std::fs::rename(&a, &b).unwrap();
    std::fs::remove_file(&b).unwrap();

    // Registry: a volatile key with a value, then deleted.
    let key = format!(r"Software\AtlasAgentBinary-{me}");
    let mut h = HKEY::default();
    let name = wide(&key);
    // SAFETY: a NUL-terminated name and an out-handle; a volatile key.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
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
    .unwrap();
    let data: Vec<u8> = "binary\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
    // SAFETY: a valid key, a NUL-terminated name and a byte buffer.
    unsafe { RegSetValueExW(h, PCWSTR(wide("v").as_ptr()), None, REG_SZ, Some(&data)) }.ok().unwrap();
    // SAFETY: closes the key we created, then deletes it.
    unsafe {
        let _ = RegCloseKey(h);
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key.as_str()));
    }

    // Network: a loopback TCP connection.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
    let (mut s, _) = l.accept().unwrap();
    c.write_all(&[7; 100]).unwrap();
    let mut buf = [0u8; 100];
    s.read_exact(&mut buf).unwrap();
    drop((c, s));

    // DNS, through the DNS Client service: one name that resolves, one that cannot.
    let names = ["example.com".to_owned(), format!("atlas-binary-{me}.invalid")];
    for n in &names {
        let _ = (n.as_str(), 80).to_socket_addrs();
    }

    // A process.
    let cmd = format!(r"{}\System32\cmd.exe", std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()));
    let mut child = Command::new(&cmd).args(["/c", "exit 3"]).spawn().unwrap();
    let child_pid = child.id();
    assert_eq!(child.wait().unwrap().code(), Some(3));

    let _ = std::fs::remove_dir_all(&dir);
    println!(
        "\n{RESULT}{}",
        json!({ "pid": me, "child_pid": child_pid, "dir": dir, "key": key, "names": names, "cmd": cmd })
    );
    let _ = std::io::stdout().flush();
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

/// Stops whatever the test left running and deletes its directories, also
/// when it fails.
struct Cleanup {
    dirs: Vec<PathBuf>,
    agent: Option<Child>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut a) = self.agent.take()
            && a.try_wait().ok().flatten().is_none()
        {
            let _ = a.kill();
            let _ = a.wait();
            for s in SESSIONS {
                let _ = atlas_etw::session::stop_by_name(s);
            }
        }
        for d in &self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// The agent's stderr (its console log), shared with a reader thread.
type Lines = Arc<Mutex<Vec<String>>>;

fn wait_for(lines: &Lines, needle: &str, timeout: Duration) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if lines.lock().unwrap().iter().any(|l| l.contains(needle)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn run_to_end(args: &[&str], timeout: Duration) -> (Option<i32>, String, String) {
    let mut child = Command::new(AGENT)
        .args(args)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Read both pipes while waiting: a child blocked on a full pipe never exits.
    let drain = |mut r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = r.read_to_end(&mut b);
            String::from_utf8_lossy(&b).into_owned()
        })
    };
    let out = drain(Box::new(child.stdout.take().unwrap()));
    let err = drain(Box::new(child.stderr.take().unwrap()));
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if t0.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    (status.and_then(|s| s.code()), out.join().unwrap(), err.join().unwrap())
}

#[test]
#[ignore = "needs administrator rights; CI runs it on its Windows runner"]
fn binary_run_and_dump() {
    if std::env::var_os(ACTOR_ENV).is_some() {
        actor();
        return;
    }
    for s in SESSIONS {
        let existing = atlas_etw::session::query_by_name(s).unwrap();
        assert!(existing.is_none(), "{s} is running: stop the other agent first");
    }
    // Ctrl+Break reaches a process group only through a console both share.
    // SAFETY: no arguments; returns this process's console window, if any.
    if unsafe { GetConsoleWindow() }.0.is_null() {
        // SAFETY: no arguments; this process has no console, so one is made.
        let _ = unsafe { AllocConsole() };
    }
    let pid = std::process::id();
    let base = std::env::temp_dir().join(format!("atlas-agent-binary-{pid}"));
    // The data directory needs folders above it that only administrators
    // control: under C:\ProgramData, not %TEMP%.
    let data = PathBuf::from(format!(r"C:\ProgramData\Atlas-Test-binary-{pid}"));
    let loose = PathBuf::from(format!(r"C:\ProgramData\Atlas-Test-binary-{pid}-loose"));
    let early = PathBuf::from(format!(r"C:\ProgramData\Atlas-Test-binary-{pid}-early"));
    for d in [&base, &data, &loose, &early] {
        let _ = std::fs::remove_dir_all(d);
    }
    std::fs::create_dir_all(&base).unwrap();
    let mut cleanup = Cleanup { dirs: vec![base.clone(), data.clone(), loose.clone(), early.clone()], agent: None };
    let mut report = Report::default();

    // ---- start ----
    let started = Instant::now();
    let spawn_ns = unix_now();
    let mut agent = Command::new(AGENT)
        .args(["run", "--data-dir"])
        .arg(&data)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn atlas-agent run");
    let agent_pid = agent.id();
    let lines: Lines = Arc::default();
    let reader = {
        let lines = lines.clone();
        let stderr = agent.stderr.take().unwrap();
        std::thread::spawn(move || {
            for l in BufReader::new(stderr).lines().map_while(Result::ok) {
                lines.lock().unwrap().push(l);
            }
        })
    };
    cleanup.agent = Some(agent);
    let up = wait_for(&lines, "running; Ctrl+C stops", Duration::from_secs(90));
    let up_ns = unix_now();
    report.note("start_ms", json!(started.elapsed().as_millis() as u64));
    assert!(up, "the agent did not start: {:#?}", lines.lock().unwrap());

    // ---- a second instance ----
    let second = base.join("Second");
    let (code, _, err) = run_to_end(&["run", "--data-dir", &second.to_string_lossy()], Duration::from_secs(30));
    report.check("a_second_instance_is_refused", code == Some(1) && err.contains("another instance"), err.trim());
    report.check("the_refused_instance_touched_nothing", !second.exists(), "");

    // ---- the scenario ----
    std::thread::sleep(Duration::from_secs(2));
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "binary_run_and_dump", "--nocapture", "--test-threads=1"])
        .env(ACTOR_ENV, "1")
        .env_remove("ATLAS_AGENT_REPORT")
        .output()
        .expect("run the actor");
    let text = String::from_utf8_lossy(&out.stdout);
    let result: Value = text
        .lines()
        .find_map(|l| l.find(RESULT).map(|i| serde_json::from_str(&l[i + RESULT.len()..]).unwrap()))
        .unwrap_or_else(|| panic!("the actor gave no result: {text}"));
    let actor_pid = result["pid"].as_u64().unwrap() as u32;

    // ---- a periodic report, then the clean stop ----
    // A report that lists the threads: the first one after a minute, a loss
    // report included (review m6).
    let periodic = wait_for(&lines, "atlas-pipeline=", Duration::from_secs(75));
    report.check("a_periodic_report_while_running", periodic, "");
    std::thread::sleep(Duration::from_secs(3));
    let stop_ns = unix_now();
    // SAFETY: the agent leads its own process group on our console.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, agent_pid) };
    report.check("ctrl_break_sent", sent.is_ok(), format!("{sent:?}"));
    let t0 = Instant::now();
    let mut status = None;
    while t0.elapsed() < Duration::from_secs(30) {
        if let Some(s) = cleanup.agent.as_mut().unwrap().try_wait().unwrap() {
            status = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    report.note("stop_ms", json!(t0.elapsed().as_millis() as u64));
    if status.is_none() {
        // Still running: kill it, so its stderr closes and the reader ends.
        let mut a = cleanup.agent.take().unwrap();
        let _ = a.kill();
        let _ = a.wait();
        for s in SESSIONS {
            let _ = atlas_etw::session::stop_by_name(s);
        }
    }
    let _ = reader.join();
    let log = lines.lock().unwrap().clone();
    report.check("clean_exit", status.and_then(|s| s.code()) == Some(0), format!("{status:?}"));
    report.check("the_buffer_was_closed", log.iter().any(|l| l.contains("buffer closed")), "");
    report.check("no_errors_logged", !log.iter().any(|l| l.contains("ERROR")), "");
    for s in SESSIONS {
        let left = atlas_etw::session::query_by_name(s).ok().flatten();
        report.check(&format!("session_stopped_{s}"), left.is_none(), "");
    }

    // ---- the directories ----
    for d in [data.clone(), data.join("buffer"), data.join("logs")] {
        let v = datadir::verify(&d);
        report.check(&format!("protected_{}", d.file_name().unwrap().to_string_lossy()), v.is_ok(), format!("{v:?}"));
    }
    let logged = std::fs::read_dir(data.join("logs"))
        .map(|r| {
            r.filter_map(Result::ok)
                .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("health report")))
        })
        .unwrap_or(false);
    report.check("the_log_file_has_the_reports", logged, "");
    // A periodic report's line names the agent's threads with their CPU (plan 1b-4a-p1).
    let log_text: String = std::fs::read_dir(data.join("logs"))
        .map(|r| r.filter_map(Result::ok).filter_map(|e| std::fs::read_to_string(e.path()).ok()).collect())
        .unwrap_or_default();
    let thread_line = log_text.lines().find(|l| l.contains("health report") && l.contains("atlas-pipeline="));
    report.check("a_report_logs_cpu_per_thread", thread_line.is_some(), thread_line.unwrap_or("").trim());
    // The final report's threads are listed before the sensor stops (review
    // m2). Not a check: a thread shows only if it used a clock tick (~15.6 ms)
    // since the last listing, a few seconds before; its line is kept instead.
    let last = log_text.lines().rfind(|l| l.contains("health report")).unwrap_or("");
    let threads = last.split("threads=").nth(1).unwrap_or("").trim();
    report.note("final_report_threads", json!(threads));
    // A periodic report's line gives each bounded structure's entries and
    // capacity (plan 1b-4a-p2, R6); the pipeline's only while it runs.
    let sizes = log_text.lines().find(|l| l.contains("health report") && l.contains("file_map="));
    report.check("a_report_logs_structure_sizes", sizes.is_some(), sizes.unwrap_or("").trim());
    let sizes = sizes.and_then(|l| l.split("sizes=").nth(1)).unwrap_or("").trim();
    report.note("a_reports_sizes", json!(sizes));

    // ---- the buffer: through dump, and directly ----
    let (code, out, err) = run_to_end(&["dump", "--data-dir", &data.to_string_lossy()], Duration::from_secs(60));
    let json_lines: Vec<Value> = out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    report.check(
        "dump_prints_json_lines",
        code == Some(0) && !json_lines.is_empty() && json_lines.len() == out.lines().count(),
        err.trim(),
    );
    report.check("dump_finds_no_invalid_record", err.contains(" 0 invalid"), err.trim());
    let events = read_buffer(&data.join("buffer"));
    // Against the writer's own count, not only the same disk read twice
    // (review finding 8): a reader that skipped records would agree with itself.
    let written = written_records(&data.join("logs"));
    report.check(
        "dump_prints_every_record",
        json_lines.len() == events.len() && written == Some(events.len()),
        format!("dump {} / read {} / written {written:?}", json_lines.len(), events.len()),
    );
    report.check(
        "the_last_record_is_the_final_report",
        events.last().is_some_and(|e| matches!(e.kind, EventKind::SensorHealth(_)) && e.meta.time >= stop_ns),
        format!("{:?}", events.last().map(|e| e.meta.time - stop_ns)),
    );
    let (_, health_out, _) = run_to_end(
        &["dump", "--data-dir", &data.to_string_lossy(), "--class", "sensor_health"],
        Duration::from_secs(60),
    );
    let reports: Vec<(&Event, &HealthReport, i64)> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::SensorHealth(a) => {
                let SensorHealthAction::Report(r) = &a.action;
                Some((e, r.as_ref(), a.interval_start))
            }
            _ => None,
        })
        .collect();
    report.check("dump_class_filter", health_out.lines().count() == reports.len(), "");
    // The first interval starts at the sensor's start: after the test started
    // the agent, before it saw it running.
    let first_start = reports.first().map(|r| r.2);
    report.check(
        "the_first_interval_starts_at_the_start",
        first_start.is_some_and(|t| (spawn_ns..=up_ns).contains(&t)),
        format!("{first_start:?} in {spawn_ns}..={up_ns}"),
    );
    report.note("events", json!(events.len()));

    analyse_scenario(&mut report, &events, &result, actor_pid, agent_pid);
    analyse_health(&mut report, &reports);

    // ---- a loose data directory ----
    std::fs::create_dir(&loose).unwrap();
    let (code, _, err) = run_to_end(&["run", "--data-dir", &loose.to_string_lossy()], Duration::from_secs(30));
    // Refused for its owner (the runner's account) or its inherited DACL.
    let refused = err.contains("not protected") || err.contains("is owned by");
    report.check("a_loose_directory_is_refused", code == Some(1) && refused, err.trim());
    report.check("nothing_was_written_into_it", std::fs::read_dir(&loose).unwrap().next().is_none(), "");

    // ---- a data directory below a folder its user controls ----
    let below = base.join("Atlas");
    let (code, _, err) = run_to_end(&["run", "--data-dir", &below.to_string_lossy()], Duration::from_secs(30));
    report.check("a_directory_below_temp_is_refused", code == Some(1) && err.contains("a folder above it"), err.trim());
    report.check("it_was_not_created", !below.exists(), "");

    // ---- Ctrl+Break while the sensor starts ----
    stop_during_start(&mut report, &early);

    drop(cleanup);
    finish(report);
}

/// Ctrl+Break as soon as `run` says it is starting the sensor, while the
/// sessions start: it must still stop cleanly (review finding 6), not die with
/// them running.
fn stop_during_start(report: &mut Report, data: &Path) {
    let mut agent = Command::new(AGENT)
        .args(["run", "--data-dir"])
        .arg(data)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn atlas-agent run");
    let lines: Lines = Arc::default();
    let reader = {
        let lines = lines.clone();
        let stderr = agent.stderr.take().unwrap();
        std::thread::spawn(move || {
            for l in BufReader::new(stderr).lines().map_while(Result::ok) {
                lines.lock().unwrap().push(l);
            }
        })
    };
    let starting = wait_for(&lines, "starting the sensor", Duration::from_secs(60));
    // SAFETY: the agent leads its own process group on our console.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, agent.id()) };
    report.check("early_ctrl_break_sent", starting && sent.is_ok(), format!("{sent:?}"));
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(30) && agent.try_wait().unwrap().is_none() {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Still running after 30 s: killed, and reported as no status.
    let finished = agent.try_wait().unwrap().is_some();
    if !finished {
        let _ = agent.kill();
    }
    let status = agent.wait().ok().filter(|_| finished);
    let _ = reader.join();
    let log = lines.lock().unwrap().join("\n");
    report.check("early_stop_exits_0", status.is_some_and(|s| s.code() == Some(0)), format!("{status:?}\n{log}"));
    let lingering: Vec<&str> =
        SESSIONS.into_iter().filter(|s| atlas_etw::session::query_by_name(s).ok().flatten().is_some()).collect();
    report.check("early_stop_left_no_session", lingering.is_empty(), format!("{lingering:?}"));
    for s in lingering {
        let _ = atlas_etw::session::stop_by_name(s);
    }
    let buffer = data.join("buffer");
    let last = if buffer.exists() { read_buffer(&buffer).pop() } else { None };
    report.check(
        "early_stop_wrote_the_final_report",
        last.is_some_and(|e| matches!(e.kind, EventKind::SensorHealth(_))),
        "",
    );
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64
}

/// The records the writer says it wrote: `records=` of the log file's
/// "buffer closed" line.
fn written_records(logs: &Path) -> Option<usize> {
    let text: String = std::fs::read_dir(logs)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect();
    let line = text.lines().rfind(|l| l.contains("buffer closed"))?;
    line.split("records=").nth(1)?.split_whitespace().next()?.parse().ok()
}

fn read_buffer(dir: &Path) -> Vec<Event> {
    let mut r = Reader::open(dir, Cursor::default());
    let mut out = Vec::new();
    while let Some(rec) = r.next_record().unwrap() {
        out.push(decode_event(&rec.payload).expect("a valid record"));
    }
    out
}

fn actor_of(e: &Event) -> Option<u32> {
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
}

fn analyse_scenario(r: &mut Report, events: &[Event], result: &Value, actor_pid: u32, agent_pid: u32) {
    let child_pid = result["child_pid"].as_u64().unwrap() as u32;
    let dir = result["dir"].as_str().unwrap().to_lowercase();
    let key = result["key"].as_str().unwrap().to_lowercase();
    r.check("nothing_from_the_agent_itself", !events.iter().any(|e| actor_of(e) == Some(agent_pid)), "");
    let tree: HashSet<u32> = [actor_pid, child_pid].into();
    let ours: Vec<&Event> = events.iter().filter(|e| actor_of(e).is_some_and(|p| tree.contains(&p))).collect();
    r.note("actor_events", json!(ours.len()));

    let file = |want: fn(&FileAction) -> bool, name: &str| {
        ours.iter().any(|e| {
            matches!(&e.kind, EventKind::File(f) if want(&f.action) && f.file.path.to_lowercase() == format!(r"{dir}\{name}"))
        })
    };
    r.check("file_create", file(|a| matches!(a, FileAction::Create), "a.txt"), "");
    r.check(
        "file_rename",
        file(|a| matches!(a, FileAction::Rename { .. }), "a.txt")
            || file(|a| matches!(a, FileAction::Rename { .. }), "b.txt"),
        "",
    );
    r.check("file_delete", file(|a| matches!(a, FileAction::Delete), "b.txt"), "");

    let key_event = |want: RegistryKeyAction| {
        ours.iter().any(|e| matches!(&e.kind, EventKind::RegistryKey(k) if k.action == want && k.path.to_lowercase().ends_with(&format!(r"\{key}"))))
    };
    r.check("registry_key_create", key_event(RegistryKeyAction::Create), "");
    r.check("registry_key_delete", key_event(RegistryKeyAction::Delete), "");
    r.check(
        "registry_value_set",
        ours.iter().any(|e| {
            matches!(&e.kind, EventKind::RegistryValue(v) if v.name == "v" && matches!(v.action, RegistryValueAction::Set { .. }))
        }),
        "",
    );
    r.check(
        "network_tcp_open",
        ours.iter().any(
            |e| matches!(&e.kind, EventKind::Network(n) if n.actor.pid == actor_pid && n.action == NetworkAction::Open),
        ),
        "",
    );
    let names: Vec<String> =
        result["names"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_lowercase()).collect();
    let dns: Vec<String> = ours
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Dns(d) => Some(d.hostname.to_lowercase()),
            _ => None,
        })
        .collect();
    r.check("dns_query", dns.iter().any(|h| names.contains(h)), format!("{dns:?}"));
    r.check(
        "process_launch",
        ours.iter().any(|e| {
            matches!(&e.kind, EventKind::Process(ProcessActivity::Launch { process, actor }) if process.pid == child_pid && actor.pid == actor_pid && process.cmd_line.contains("exit 3"))
        }),
        "",
    );
    r.check(
        "process_terminate",
        ours.iter().any(|e| {
            matches!(&e.kind, EventKind::Process(ProcessActivity::Terminate { process, exit_code: Some(3) }) if process.pid == child_pid)
        }),
        "",
    );
    r.check(
        "module_load",
        ours.iter().any(|e| matches!(&e.kind, EventKind::Module(m) if m.actor.pid == child_pid)),
        "",
    );
}

fn analyse_health(r: &mut Report, reports: &[(&Event, &HealthReport, i64)]) {
    r.note("reports", json!(reports.len()));
    r.check("a_periodic_and_a_final_report", reports.len() >= 2, format!("{}", reports.len()));
    let zero = Some(0);
    let mut losses = Vec::new();
    for (i, (_, h, _)) in reports.iter().enumerate() {
        let l = &h.loss;
        let all = [
            ("sensor_events", l.sensor_session_events_lost),
            ("process_events", l.process_session_events_lost),
            ("sensor_buffers", l.sensor_session_buffers_lost),
            ("process_buffers", l.process_session_buffers_lost),
            ("kernel_queue", l.kernel_queue_drops),
            ("user_queue", l.user_queue_drops),
            ("buffer", l.buffer_backlog_drops),
            ("callback_panics", l.callback_panics),
        ];
        for (name, v) in all {
            if v != zero {
                losses.push(format!("report {i}: {name} {v:?}"));
            }
        }
        if !l.actor_dropped.is_empty() {
            losses.push(format!("report {i}: actor_dropped {:?}", l.actor_dropped));
        }
    }
    r.check("every_report_measures_zero_loss", losses.is_empty(), losses.join("; "));
    r.check(
        "the_buffer_is_healthy",
        reports.iter().all(|(_, h, _)| h.buffer.failing == Some(false) && h.buffer.write_errors == zero),
        "",
    );
    r.check(
        "the_first_report_carries_the_recovery",
        reports
            .first()
            .is_some_and(|(_, h, _)| h.buffer.truncated_bytes == zero && h.buffer.cursor_reset == Some(false)),
        "",
    );
    r.check(
        "the_intervals_chain",
        reports.windows(2).all(|w| w[1].2 == w[0].0.meta.time),
        format!("{:?}", reports.iter().map(|(e, _, s)| (*s, e.meta.time)).collect::<Vec<_>>()),
    );
    r.check(
        "resources_measured",
        reports
            .iter()
            .all(|(_, h, _)| h.resources.working_set.is_some_and(|w| w > 0) && h.resources.cpu_time.is_some()),
        "",
    );
    r.check("seeding_reported", reports.iter().all(|(_, h, _)| h.housekeeping.seeding_enabled == Some(true)), "");
    // Every report measures what each session delivered; the actor's launches
    // reach Session B, its files Session A.
    let received =
        |f: fn(&HealthReport) -> Option<u64>| reports.iter().map(|(_, h, _)| f(h)).collect::<Option<Vec<_>>>();
    let sensor = received(|h| h.throughput.sensor_session_events_received);
    let process = received(|h| h.throughput.process_session_events_received);
    let sums = (sensor.as_ref().map(|v| v.iter().sum::<u64>()), process.as_ref().map(|v| v.iter().sum::<u64>()));
    // Session A (files, registry, network) delivers far more than Session B
    // (processes): a swap of the two shows (review m5). The final report,
    // made after the stop, counts too.
    r.check(
        "events_received_measured",
        matches!(sums, (Some(s), Some(p)) if s > p && p > 0),
        format!("{sensor:?} {process:?}"),
    );
    r.check(
        "the_final_report_counts_events_received",
        sensor.as_ref().and_then(|v| v.last()).is_some_and(|&n| n > 0),
        format!("{sensor:?}"),
    );
    r.note("events_received", json!({ "sensor": sums.0, "process": sums.1 }));
    let unpaired: u64 = reports.iter().filter_map(|(_, h, _)| h.quality.file_cleanup_unpaired).sum();
    r.note("file_cleanup_unpaired", json!(unpaired));
    let cpu: u64 = reports.iter().filter_map(|(_, h, _)| h.resources.cpu_time).sum();
    r.note("cpu_ms", json!(cpu / 1_000_000));
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
