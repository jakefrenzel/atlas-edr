# Sub-project 1b-4a-p4 — A Launch Without Its Hash Implementation Plan

> **Status:** Approved by the user (2026-11-05), after an independent review (Review Log; its fixes are in). **For agentic workers:** steps use checkbox (`- [ ] `) syntax for tracking. Nothing in this plan needs the VM, a kernel driver or a run by the user: the host run it depends on is done (P13).

**Goal:** Stop a busy machine from starving the agent's enrichment. Under load a Launch could go out without its hash and signature, which failed `agent-live` now and then (spec §15.3, plan 1b-4a-p3's F11), and an attacker can make that load on purpose. Two fixes, each decided from measured facts (the p4 notes, C1 and C2):
- **the hash workers run at normal priority** (C1): the live test's Launch lost its hash in 13 of 24 runs below normal, and 0 of 56 at normal;
- **the seeder and its namer run at normal priority too** (C2): the seeded check failed in 10 of 16 runs below normal and 0 of 16 at normal, and the start-up pass fell from 12 s to over 30 s to under 1 s.

The deadline clock, the second cause (P1), stays: it goes into the spec's Known Limitations.

**Architecture:** the `SetThreadPriority` calls go, so these threads keep the default. There is no new code in the agent. `tests/binary.rs` gains one check that reads each thread's priority from the running agent.
- **`win/services.rs`:** `hash_worker` no longer lowers its priority.
- **`win/seeder.rs`:** `seeder::run` no longer lowers its priority.
- **`win/handles.rs`:** the `atlas-seeder-namer` thread no longer lowers its priority.
- **`tests/binary.rs`:** `thread_priorities(pid)` lists each named thread's priority; the check `the_workers_run_at_normal_priority` wants `atlas-hash-0`, `atlas-hash-1`, `atlas-seeder` and `atlas-seeder-namer` present and at `THREAD_PRIORITY_NORMAL`.

**Tech stack:** Rust 1.97 (edition 2024). No new dependency: the crate already has `windows`' `Win32_System_Diagnostics_ToolHelp` feature. Measurement tools (git-ignored, already run): `spikes/run-p4-cost.ps1`, `spikes/p4spin`, `spikes/p4-plan-src/cost_summary.py`.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2 to 1b-4a-p3. Section numbers (§) refer to it. The p4 notes (`docs/specs/2026-11-04-etw-sensor-1b-4a-p4-brainstorm-notes.md`) have the facts behind each decision (P1–P14).

**Verification note (2026-11-04 to 2026-11-05, host, Windows 11 build 26200, and the CI runner, build 26100):** every code block below was compiled and run in a scratch worktree of `main` (`de679c7`), `scratch/p4-review`, one commit per task (Task 1 `99e951f`, Task 2 `cc13919`, Task 3's spec text `90999d4`). `scratch/p4` is the draft: its Task 2 (`8e81bfc`) differs from `cc13919` in two comments, and every run below but the last CI run was on the draft's code.
- **Lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean at the end of Tasks 1 and 2, measured in the round trip.
- **Unelevated:** `cargo test --workspace` passes 455 tests and ignores 19 after Task 1 and 455 tests and ignores 19 after Task 2 (plan 1b-4a-p3 left 455 and 19). `atlas-agent` has 266 and 266 unit tests. The new check adds none: it is one check inside the binary test, which CI runs elevated.
- **On the CI runner:** run 42158453063 on Task 3's commit, and run 42078586911 on the draft, passed every job. Run 42079443395 watched `agent-live`'s two tests on the draft's code on 8 fresh VMs, 2 passes each: 16 of 16 each (P12).
- **On the host:** `run-p4-cost.ps1` on a release build of the draft against one of `main`, result `20261105-194041` (P13).
- **Reverts:** the one rule, checked on the CI runner by putting the call back on one thread at a time (run 42078592869, `scratch/p4-reverts`, never for `main`; P11). The test needs elevation, so these ran there, not through `mutate.py`. 3 reverts, all caught by `the_workers_run_at_normal_priority` and no other of the 52 checks; the thread showed -1 (below normal), and 0 without a revert:
  - the hash workers lower their priority (caught)
  - the seeder lowers its priority (caught)
  - the namer lowers its priority (caught)
- **Round trip:** the code was rebuilt from this document alone in a fresh LF worktree of `main` (`spikes/p4-plan-src/roundtrip.py`): every diff applied, and every task's tree is identical to its verified commit; the test counts and the lint above were measured there.
- **Not run:** the Linux build of the tests is run by CI only; Task 3's text changes no code, so no run depends on it.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-4a-p4-plan`. The build runs on `feat/1b-4a-p4`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Portable code:** nothing in `pipeline` changes.
- **The ETW callback is untouched** (§3.2).
- **No behaviour change but the decided one:** the same events, fields and counters as before for the same input. What changes is when these four threads get the CPU on a busy machine, so fewer events reach their deadlines.
- **No thread goes above normal.** Nothing here raises a thread past the default; the ETW consumers and the pipeline stay as they are.
- **The 1 s deadline stays** (R4, C1).
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). No step here needs it: the binary test runs in CI's `agent-live`.
- **The host's data stays on the host:** logs and dumps stay in the git-ignored `spikes\results\`; only summary figures enter the docs.
- **No schema change.**
- **Formatting and lint** at the end of every code task, as in the verification note.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash). Edits that contain backslashes are made with an editor, never a heredoc.

## Decisions

The p4 notes have the facts; the decision log has each entry.

### C1, the hash workers (chosen 2026-11-04: A)

- **(A) Chosen: both hash workers run at normal priority.** The live test's Launch lost its hash in 0 of 56 runs, against 13 of 24 below normal (P2), and module hashes keep up (P3).
- (B) Rejected: a dedicated normal-priority Launch worker with modules below normal: 2 of 32, and modules stop getting hashes on a busy machine, which an attacker could cause on purpose.
- (C) Rejected: stay below normal and raise the 1 s deadline: jobs waited 10 s and more, and R4 rejected longer deadlines, which hold back every later event.

### C2, the seeder and the deadline clock (chosen 2026-11-04: A)

- **(A) Chosen: the seeder and its namer thread run at normal priority too; the deadline clock stays and goes into the Known Limitations.** The seeded check failed in 10 of 16 runs below normal and 0 of 16 at normal (P8). Fixing the clock would save about 2 Launches in 3,000 under overload on a release build (P9).
- (B) Rejected: fix the clock now as well: a benefit too small to measure. Its size was estimated, never traced.
- (C) Rejected: the hash workers only: `agent-live` would keep a known starvation failure.

### C3, the cost of normal priority (chosen 2026-11-05: A, again after the review's M1)

- **(A) Chosen: keep normal priority, and carry Session A's stalls into the 1b-4b-1 brainstorm as an open problem.** From the host run (P13): no difference seen in another program's throughput, no ETW session or kernel or user queue loss on either build, and enrichment misses from 1,958–10,323 a run to 0. Its worst moments may be a little worse (about 1 chunk in 24,000), not shown. The stalls happen on `main`'s build too, so going back would not remove them; even if normal priority doubled them, that is about 1.2% of Launches against 0.6%, only with every CPU busy. The user first chose this from a summary that left out `actor_dropped`, and chose it again with the facts (the review's M1).
- (B) Rejected: 20 runs per build first (about 80 minutes), to measure the stall rate: it would show only a large difference, and a large one would call for another fix, such as the ETW consumer's priority, not a return to below normal.
- (C) Rejected: find the cause inside this plan: a new brainstorm in a small, verified plan; the watchdog work already looks at delivery.

## Findings

The p4 notes' P1–P14, in short:
- **P1:** under heavy overload, two causes: starvation (a median wait of 10–14 s below normal, under 20 ms at normal) and the deadline clock (`tick` reads the clock once and requests go out after it).
- **P2:** the live test's Launch lost its hash in 13 of 24 runs below normal, 0 of 56 at normal, 1 of 24 with a tick cap, 2 of 32 with a Launch worker.
- **P3, P4:** a Launch worker starves modules; the tick cap showed no benefit; P4's misses at light load are not reconciled with P9.
- **P5:** the spec gave no measured reason for below normal.
- **P6, P8:** the seeder starves the same way: the seeded check failed in 10 of 16 runs below normal, 0 of 16 at normal; the start-up pass 12 s to over 30 s, then 0.26–0.75 s.
- **P7:** `enrichment_misses` also counts what is pending at stop.
- **P9:** the deadline clock on a release build: 2 of 3,109 Launches under overload.
- **P10:** `no_queue_drops` failed in 2 of 24 runs below normal, 0 of 56 at normal.
- **P11:** the new check fails on each revert.
- **P12:** `agent-live`'s tests passed 16 of 16 each at normal, one pass at the real failure's scale; misses remain, and one light pass's 63 are not explained.
- **P13, the host run:** another program's work -0.63% below normal, -0.59% at normal (no difference seen; 4 runs a build could not show one under about 0.8%); ETW session and kernel and user queue loss 0 on both; misses 1,958–10,323 a run, then 0; the agent 8.1 → 11.0 s of CPU a run; the tail possibly a little worse, not shown. Session A's delivery stalled three times on both builds: `actor_dropped` 21 at normal and 11 below normal, in 1,800 launches each, probably as many Launches without their command line and user; not separable, not explained.
- **P14:** on the CI runner, no such stall in 24 runs below normal or 112 at normal, at matching loads, each about 7 s long.

## Deliberate clarifications of the spec (applied to the spec text in Task 3)

1. **§3.2, §6.3:** the hash workers run at normal priority (C1).
2. **§3.2, §7.4:** the seeder and its namer too; what bounds them is that the start-up pass runs once and is short, not the CPU budget (C2).
3. **§13, §15.3:** the host run and P1–P14.
4. **§16:** the enrichment deadline's clock, a Launch without its hash, and a stall in Session A's delivery.
5. **§17:** the clarifications.

## Review Focus

1. **The four threads run at normal priority in the running agent** (`the_workers_run_at_normal_priority`, read after the actor's activity, so well after each thread's first instructions): each name must be present and each thread with it at normal, so a thread that was renamed or never started fails the check.
2. **No call that lowers a priority is left in the agent** (`git grep SetThreadPriority crates` finds nothing).
3. **Nothing depended on these threads being below normal.** The pipeline takes only short locks with them: the hash lane's, a queue operation. The cache's invalidations reach the workers by a channel (§3.2, review finding 4).
4. **The check does not depend on timing:** a stuck namer is replaced by a new thread of the same name, at the default priority, so the check holds with stuck helpers too. Only the host has stuck helpers: CI's runs list one namer.
5. **Nothing else changes:** every earlier test passes unchanged; the replay and the snapshot do not involve these threads' priority.

Rule 1 was checked by reverting it on each thread (verification note). Rules 2, 3 and 5 are read from the diff and the code; rule 4 from `FileNamer`.

## File Structure

```
crates/atlas-agent/src/win/services.rs    the hash workers keep the default priority; the module comment
crates/atlas-agent/src/win/seeder.rs      the seeder keeps the default priority
crates/atlas-agent/src/win/handles.rs     the namer keeps the default priority
crates/atlas-agent/tests/binary.rs        thread_priorities, the check, the thread list
spikes/                                   git-ignored, already run: run-p4-cost.ps1, p4spin, p4-plan-src/cost_summary.py
docs/specs/2026-10-01-etw-sensor-design.md   Task 3
```

## Interfaces for later plans

- **Plan 1b-4b-1 (the watchdog and restart):** its blinding tests join `agent-live`, which this plan is meant to steady; a stall in Session A's delivery during the binary test would still fail `every_report_measures_zero_loss` (P13), though none did in 16 passes (P12). A watchdog's view of delivery lag could say why it stalls. Plan 1b-4a-p3's item stands: P8 does not settle a table read that fails, so if the start-up pass never replies, `startup_pending` stays true for the whole run and Sensor Health should see it. A new thread such as the watchdog must not be below normal without a measured reason, and should join `NORMAL_PRIORITY_THREADS` if it serves a deadline.
- **Plan 1b-4c (the service):** the 24-hour run reads `enrichment_misses` knowing that it holds Launches and module loads together, and what is pending at stop (P7). If misses there matter, a counter for Launches alone would say what P13 could not. The service must not lower the process's priority class, which the check, reading each thread's priority within the class, would not see.
- **Open, for whichever plan meets them:** the 63 misses of one light pass (P12), P4's misses at light load, and Session A's stalls (P13), with their dropped Launch halves: none explained, and the stalls not tied to priority or cleared of it.
- **Sub-project 8 (final budgets):** what these threads cost an interactive program was not measured; a tail cost below P13's noise may be there.

---

### Task 1: The hash workers run at normal priority (C1)

**Files:**
- Modify: `crates/atlas-agent/src/win/services.rs`, `crates/atlas-agent/tests/binary.rs`

- [ ] **Step 1: The workers.** `hash_worker` no longer lowers its priority; the module comment says why.

```diff
--- a/crates/atlas-agent/src/win/services.rs
+++ b/crates/atlas-agent/src/win/services.rs
@@ -1,9 +1,10 @@
 //! The services' threads and the routing of [`Request`]s to them (sensor spec
 //! §3.2 [4] and [8]; plan 1b-3b, D5).
 //!
-//! - **Hash workers** (2, below normal): `Enrich`, Launch images first (plan
-//!   1b-4a-p2, R4): while the cache is cold a Launch would otherwise wait
-//!   behind dozens of module loads and miss its 1 s deadline (F9).
+//! - **Hash workers** (2): `Enrich`, Launch images first (plan 1b-4a-p2, R4):
+//!   while the cache is cold a Launch would otherwise wait behind dozens of
+//!   module loads and miss its 1 s deadline (F9). They run at normal priority
+//!   (plan 1b-4a-p4, C1): below normal, a busy machine starved them past it.
 //! - **Reader lane** (1): value reads, on the ordered path and the fast path.
 //!   It does no file I/O, so a slow directory never delays a value read (R-M2).
 //! - **Expander lane** (1): 8.3 expansion (D4).
@@ -17,14 +18,14 @@
 //! parked worker costs about 22 µs of CPU, and every file Update, Rename and
 //! Delete invalidates both caches. The hash cache's invalidations go on a
 //! channel the workers drain before their next lookup: the caller (the
-//! pipeline thread) sends without taking a lock a below-normal worker could
-//! hold (review 4). Its key holds the USN, so they only free memory, and past
-//! the lane's capacity the newest is dropped. The expansion cache's wait in a
-//! list the expander applies before its next expansion, so none is applied
-//! late; if the list fills first, the expander clears its whole cache instead
-//! (R-M3), and the list keeps nothing more until then (review 3). Neither is a
-//! counted drop. Replies come back on one channel, which the driver (plan
-//! 1b-3c) feeds to `Pipeline::reply`.
+//! pipeline thread) sends without taking a lock a worker could hold (review 4).
+//! Its key holds the USN, so they only free memory, and past the lane's
+//! capacity the newest is dropped. The expansion cache's wait in a list the
+//! expander applies before its next expansion, so none is applied late; if the
+//! list fills first, the expander clears its whole cache instead (R-M3), and
+//! the list keeps nothing more until then (review 3). Neither is a counted
+//! drop. Replies come back on one channel, which the driver (plan 1b-3c) feeds
+//! to `Pipeline::reply`.
 
 use std::collections::VecDeque;
 use std::sync::atomic::{AtomicUsize, Ordering};
@@ -32,8 +33,6 @@ use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_
 use std::sync::{Arc, Condvar, Mutex, PoisonError};
 use std::time::Instant;
 
-use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
-
 use super::expand::{Dir, Expander, NtDir};
 use super::hash::{Enricher, HashCache};
 use super::seeder::{self, Seeder};
@@ -328,10 +327,6 @@ fn hash_worker(
     counters: &ServiceCounters,
     size_cap: u64,
 ) {
-    // SAFETY: lowers this thread's priority.
-    unsafe {
-        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
-    }
     let enricher = Enricher::new(size_cap);
     while let Some((id, nt_path)) = jobs.pop() {
         forgets.apply(cache);
```

- [ ] **Step 2: The check.** The binary test reads each thread's priority from the running agent, after the actor's activity. At this task the list holds the two hash workers, so the seeder's threads stay as they are until Task 2.

```diff
--- a/crates/atlas-agent/tests/binary.rs
+++ b/crates/atlas-agent/tests/binary.rs
@@ -11,6 +11,7 @@
 //! 3. an actor (this test binary again, `ATLAS_BINARY_ACTOR=1`) creates,
 //!    renames and deletes a file, creates a registry key and value and deletes
 //!    them, opens a TCP connection, resolves two names and runs `cmd.exe`;
+//!    the agent's workers are then found at normal priority;
 //! 4. after the first periodic Sensor Health report, Ctrl+Break stops the
 //!    agent, which must exit 0 after a clean stop;
 //! 5. the buffer is read twice, through `atlas-agent dump` and directly, and
@@ -42,12 +43,18 @@ use atlas_schema::classes::registry::{RegistryKeyAction, RegistryValueAction};
 use atlas_schema::classes::sensor_health::{HealthReport, SensorHealthAction};
 use atlas_schema::{Event, EventKind, decode_event};
 use serde_json::{Value, json};
-use windows::Win32::Foundation::HANDLE;
+use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
 use windows::Win32::System::Console::{AllocConsole, CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetConsoleWindow};
+use windows::Win32::System::Diagnostics::ToolHelp::{
+    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
+};
 use windows::Win32::System::Registry::{
     HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
     RegSetValueExW,
 };
+use windows::Win32::System::Threading::{
+    GetThreadDescription, GetThreadPriority, OpenThread, THREAD_PRIORITY_NORMAL, THREAD_QUERY_LIMITED_INFORMATION,
+};
 use windows::core::{HSTRING, PCWSTR};
 
 const AGENT: &str = env!("CARGO_BIN_EXE_atlas-agent");
@@ -55,6 +62,9 @@ const ACTOR_ENV: &str = "ATLAS_BINARY_ACTOR";
 const RESULT: &str = "ATLAS_BINARY_RESULT ";
 const SESSIONS: [&str; 2] = ["Atlas-Sensor", "Atlas-Process"];
 const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
+/// Below normal, a busy machine starves these past the deadlines they serve
+/// (plan 1b-4a-p4, C1).
+const NORMAL_PRIORITY_THREADS: [&str; 2] = ["atlas-hash-0", "atlas-hash-1"];
 
 fn wide(s: &str) -> Vec<u16> {
     s.encode_utf16().chain([0]).collect()
@@ -308,6 +318,14 @@ fn binary_run_and_dump() {
         .unwrap_or_else(|| panic!("the actor gave no result: {text}"));
     let actor_pid = result["pid"].as_u64().unwrap() as u32;
 
+    // Plan 1b-4a-p4: read here, well after each thread's first instructions.
+    let threads = thread_priorities(agent_pid);
+    let at_normal = NORMAL_PRIORITY_THREADS.iter().all(|name| {
+        let mut named = threads.iter().filter(|(n, _)| n == name).peekable();
+        named.peek().is_some() && named.all(|(_, p)| *p == THREAD_PRIORITY_NORMAL.0)
+    });
+    report.check("the_workers_run_at_normal_priority", at_normal, format!("{threads:?}"));
+
     // ---- a periodic report, then the clean stop ----
     // A report that lists the threads: the first one after a minute, a loss
     // report included (review m6).
@@ -508,6 +526,32 @@ fn stop_during_start(report: &mut Report, data: &Path) {
     );
 }
 
+/// The name and base priority of each of `pid`'s threads.
+fn thread_priorities(pid: u32) -> Vec<(String, i32)> {
+    let mut threads = Vec::new();
+    // SAFETY: the snapshot and each thread handle are closed here; `entry` is a
+    // writable THREADENTRY32 with its size set; the description is freed once read.
+    unsafe {
+        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).expect("a thread snapshot");
+        let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
+        let mut more = Thread32First(snapshot, &mut entry).is_ok();
+        while more {
+            if entry.th32OwnerProcessID == pid
+                && let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID)
+            {
+                if let Ok(name) = GetThreadDescription(thread) {
+                    threads.push((name.to_string().unwrap_or_default(), GetThreadPriority(thread)));
+                    let _ = LocalFree(Some(HLOCAL(name.as_ptr().cast())));
+                }
+                let _ = CloseHandle(thread);
+            }
+            more = Thread32Next(snapshot, &mut entry).is_ok();
+        }
+        let _ = CloseHandle(snapshot);
+    }
+    threads
+}
+
 fn unix_now() -> i64 {
     std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64
 }
```

- [ ] **Step 3: Check and commit**

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 455 tests and ignores 19 (`atlas-agent` unit tests: 266). The check itself runs in CI's `agent-live`, elevated.
```powershell
git add crates/atlas-agent
git commit -m "fix(agent): the hash workers run at normal priority (C1)"
```

### Task 2: The seeder and its namer run at normal priority (C2)

**Files:**
- Modify: `crates/atlas-agent/src/win/seeder.rs`, `crates/atlas-agent/src/win/handles.rs`, `crates/atlas-agent/src/win/services.rs`, `crates/atlas-agent/tests/binary.rs`

- [ ] **Step 1: The seeder.**

```diff
--- a/crates/atlas-agent/src/win/seeder.rs
+++ b/crates/atlas-agent/src/win/seeder.rs
@@ -34,9 +34,7 @@ use std::time::{Duration, Instant};
 
 use windows::Win32::Foundation::FILETIME;
 use windows::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW};
-use windows::Win32::System::Threading::{
-    GetCurrentThread, GetThreadTimes, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
-};
+use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
 use windows::core::w;
 
 use super::handles::{Entry, FileName, FileNamer, duplicate, object_name, open_process, table};
@@ -256,17 +254,14 @@ impl Seeder {
     }
 }
 
-/// The seeder thread: answers `Request::Seed`, below normal priority.
+/// The seeder thread: answers `Request::Seed`. At normal priority (plan
+/// 1b-4a-p4, C2): below normal, a busy machine held the start-up pass for 12 s or more.
 pub(crate) fn run(
     mut seeder: Seeder,
     jobs: Receiver<(HandleKind, Vec<u64>)>,
     replies: Sender<Reply>,
     cfg: &ServiceConfig,
 ) {
-    // SAFETY: lowers this thread's priority.
-    unsafe {
-        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
-    }
     let counters = seeder.counters.clone();
     let mut budget = Budget::new(cfg.seeder_cpu, cfg.seeder_cpu_window);
     let mut waiting = Waiting::default();
```

- [ ] **Step 2: The namer.**

```diff
--- a/crates/atlas-agent/src/win/handles.rs
+++ b/crates/atlas-agent/src/win/handles.rs
@@ -19,10 +19,7 @@ use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_I
 use windows::Win32::Foundation::{DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE, UNICODE_STRING};
 use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_NT};
 use windows::Win32::System::IO::{CancelSynchronousIo, IO_STATUS_BLOCK};
-use windows::Win32::System::Threading::{
-    GetCurrentProcess, GetCurrentThread, OpenProcess, PROCESS_DUP_HANDLE, SetThreadPriority,
-    THREAD_PRIORITY_BELOW_NORMAL,
-};
+use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};
 
 use super::util::{Owned, aligned, as_bytes};
 
@@ -224,10 +221,6 @@ impl Helper {
         let thread = std::thread::Builder::new()
             .name("atlas-seeder-namer".into())
             .spawn(move || {
-                // SAFETY: lowers this thread's priority, as the seeder's.
-                unsafe {
-                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
-                }
                 for h in jobs {
                     let name = disk_file_name(&h);
                     // A failed send means the seeder gave up on this query; `h` closes here.
```

- [ ] **Step 3: The module comment and the check's list.**

```diff
--- a/crates/atlas-agent/src/win/services.rs
+++ b/crates/atlas-agent/src/win/services.rs
@@ -8,7 +8,7 @@
 //! - **Reader lane** (1): value reads, on the ordered path and the fast path.
 //!   It does no file I/O, so a slow directory never delays a value read (R-M2).
 //! - **Expander lane** (1): 8.3 expansion (D4).
-//! - **Seeder** (1, below normal): `Seed`, when `SeDebugPrivilege` is available.
+//! - **Seeder** (1): `Seed`, when `SeDebugPrivilege` is available.
 //!
 //! Lanes are bounded and never block the caller. A full lane drops the request
 //! and counts it: a dropped `Enrich`, `ReadValue`, `Expand` or `Seed` costs its
```

```diff
--- a/crates/atlas-agent/tests/binary.rs
+++ b/crates/atlas-agent/tests/binary.rs
@@ -63,8 +63,8 @@ const RESULT: &str = "ATLAS_BINARY_RESULT ";
 const SESSIONS: [&str; 2] = ["Atlas-Sensor", "Atlas-Process"];
 const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
 /// Below normal, a busy machine starves these past the deadlines they serve
-/// (plan 1b-4a-p4, C1).
-const NORMAL_PRIORITY_THREADS: [&str; 2] = ["atlas-hash-0", "atlas-hash-1"];
+/// (plan 1b-4a-p4, C1 and C2).
+const NORMAL_PRIORITY_THREADS: [&str; 4] = ["atlas-hash-0", "atlas-hash-1", "atlas-seeder", "atlas-seeder-namer"];
 
 fn wide(s: &str) -> Vec<u16> {
     s.encode_utf16().chain([0]).collect()
```

- [ ] **Step 4: Check and commit**

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git grep SetThreadPriority crates
```
Expected: all pass, and the last command finds nothing. Workspace: 455 tests and ignores 19 (`atlas-agent` unit tests: 266).
```powershell
git add crates/atlas-agent
git commit -m "fix(agent): the seeder and its namer run at normal priority (C2)"
```

### Task 3: Documentation

- [ ] **Step 1: The spec.** In `docs/specs/2026-10-01-etw-sensor-design.md`: the status line (its date is the build's merge day: change `2026-11-05` if it differs); §3.2 (the diagram, the invalidations); §6.3 (C1); §7.4 (C2); §13 (the host run); §15.3 (a "Plan 1b-4a-p4" addendum, P1–P14); §16 (the deadline's clock, a Launch without its hash, a stall in Session A's delivery); §17 ("Plan 1b-4a-p4 clarifications").

```diff
--- a/docs/specs/2026-10-01-etw-sensor-design.md
+++ b/docs/specs/2026-10-01-etw-sensor-design.md
@@ -1,6 +1,6 @@
 # Sub-project 1 — ETW Sensor Design (Agent Core)
 
-**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15), and plan 1b-4a-p2 (on-miss seeding off; the pipeline's measured fixes; cache invalidations that wake no thread; Launch images hashed first; each bounded structure's size in the log; memory that follows contents; no power throttling), done (2026-10-27); the hour run (2026-10-31) met the CPU budget but not the working set, and produced plan 1b-4a-p3 (file map entries leave after their Cleanup; no provisional entry that nothing can name; one 1 s outcome window), done (2026-11-04); the hour run again (2026-11-04) met both budgets and ended the profiling step; plan 1b-4b (blinding detection) is next, then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
+**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15), and plan 1b-4a-p2 (on-miss seeding off; the pipeline's measured fixes; cache invalidations that wake no thread; Launch images hashed first; each bounded structure's size in the log; memory that follows contents; no power throttling), done (2026-10-27); the hour run (2026-10-31) met the CPU budget but not the working set, and produced plan 1b-4a-p3 (file map entries leave after their Cleanup; no provisional entry that nothing can name; one 1 s outcome window), done (2026-11-04); the hour run again (2026-11-04) met both budgets and ended the profiling step; plan 1b-4a-p4 (the hash workers and the seeder at normal priority) is done (2026-11-05); plan 1b-4b (blinding detection) is next, then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
 **Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
 **Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).
 
@@ -80,11 +80,11 @@ ETW (Session A: manifest providers; Session B: system logger, process events)
       actor resolution · canary matching · path normalization               │
       then, at emission: self-filter on the resolved actor                  │
       → domain Events (some pending: see "Pending events")                  │
-   │              ▲ seeder replies         [4] workers (2, below-normal) ◄──┘
+   │              ▲ seeder replies         [4] workers (2) ◄────────────────┘
    │              │                            SHA-256 + Authenticode (§6.3), Launch images first
    │              │                            + 1 reader lane: registry value reads (§7.5)
    │              │                            + 1 expander lane: 8.3 name expansion (§7.2)
-   │         [8] seeder (below-normal): names key and file handles from the
+   │         [8] seeder: names key and file handles from the
    │             system handle table at start, and on a miss if enabled (§7.4)
 [5] completion stage: emits in order; a pending event holds the line until
       completed, cancelled, or past its deadline, then goes as-is
@@ -115,7 +115,7 @@ ETW (Session A: manifest providers; Session B: system logger, process events)
 **Seeding.** Handles opened before the sessions started are never named by ETW (§7.4). The seeder [8] names them from the system handle table. At start the sessions start first and the seeder runs after, so no handle falls between the snapshot and the first event. Each snapshot is stamped with its QPC time, and [3] applies it in stream-time order (§7.4).
 
 **Service lanes** (plan 1b-3b). The hash workers, the reader lane, the expander lane and the seeder each take at most 8,192 waiting requests; the hash workers have two queues of 8,192, Launch images taken before module loads (plan 1b-4a-p2, R4). A full lane drops the request and counts `service_queue_drops`: a dropped work request costs its event's deadline. The reader lane does no file I/O, so a stalled directory never delays a value read.
-- **Invalidations wake no thread** (plan 1b-4a-p2, R3a). Waking a parked worker costs about 22 µs of CPU, and under load invalidations are the most frequent request. The hash cache's go on a channel the hash workers drain before their next lookup, so the pipeline thread takes no lock a below-normal worker could hold (review finding 4); the cache's key holds the USN, so they only free memory, and past 8,192 waiting the newest is dropped. The 8.3 cache's invalidations wait in a shared list (at most 8,192) that the expander applies before its next expansion; a list that overflows clears the expansion cache instead, and keeps nothing more until then.
+- **Invalidations wake no thread** (plan 1b-4a-p2, R3a). Waking a parked worker costs about 22 µs of CPU, and under load invalidations are the most frequent request. The hash cache's go on a channel the hash workers drain before their next lookup, so the pipeline thread takes no lock a worker could hold (review finding 4); the cache's key holds the USN, so they only free memory, and past 8,192 waiting the newest is dropped. The 8.3 cache's invalidations wait in a shared list (at most 8,192) that the expander applies before its next expansion; a list that overflows clears the expansion cache instead, and keeps nothing more until then.
 - So to hand work out the pipeline thread takes only short locks, shared with threads of its own priority or for a queue operation: the hash lane's and the expander's list's.
 
 **One thread for [2], [3] and [5]** (plan 1b-3a, D2). The three stages are one loop on the pipeline thread, with the clock passed in: the driver pushes queued events and worker replies, and calls `tick(now)` at least every 50 ms, which returns the events to emit in order. Measured at 0.57 µs per event in steady state, and 0.91 µs at 52,000 events/s of never-repeated addresses.
@@ -296,7 +296,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 
 - Applies to `process.file` on Launch and `module.file` on Module Load.
 - **Cache key:** volume serial + 128-bit file ID (`FILE_ID_INFO`) + **the file's USN** (`FSCTL_READ_FILE_USN_DATA`). The USN changes on every modification and, unlike last-write time, cannot be set back by a user. On volumes without a USN journal, results are not cached. Entries for a path are also invalidated when the pipeline sees an Update, Rename or Delete on it; a hash worker applies them before its next lookup (§3.2, plan 1b-4a-p2). A SetAttributes does not invalidate (plan 1b-4a-p1, D6): attributes and timestamps change neither a file's content nor its name, and the System process sets the basic information of the DLLs that starting processes load, so invalidating there re-hashed them on every launch.
-- **Work** runs on 2 below-normal-priority worker threads, never on an ETW or pipeline thread. Launch images are taken before module loads (plan 1b-4a-p2, R4): while the cache is cold a Launch would otherwise wait behind dozens of module loads and could miss its deadline (plan 1b-4a-p1, F9). Files are opened with full sharing (`FILE_SHARE_READ | WRITE | DELETE`) and read sequentially.
+- **Work** runs on 2 worker threads, never on an ETW or pipeline thread. They run at normal priority (plan 1b-4a-p4, C1): below normal, a busy machine starved them past the deadline, and a load an attacker can make would strip Launches of their hashes (§15.3, P1, P2). No thread of the agent runs above normal, so they share the CPU with the ETW consumers, the pipeline and other programs as equals. Launch images are taken before module loads (plan 1b-4a-p2, R4): while the cache is cold a Launch would otherwise wait behind dozens of module loads and could miss its deadline (plan 1b-4a-p1, F9). Files are opened with full sharing (`FILE_SHARE_READ | WRITE | DELETE`) and read sequentially.
 - **SHA-256** for files up to the size cap (default 100 MB); larger files skip hashing.
 - **Signature:** `WinVerifyTrust` with `WTD_UI_NONE`, `WTD_REVOKE_NONE`, `WTD_CACHE_ONLY_URL_RETRIEVAL` (no network calls from the sensor), and catalog lookup (`CryptCATAdmin*`) for catalog-signed OS files. `signer` = the leaf certificate's subject CN. Mapping: valid chain → `Valid`; no signature (`TRUST_E_NOSIGNATURE`) → `Unsigned`; a signature that fails verification (bad digest, untrusted root, explicit distrust, revoked per local cache) → `Invalid`; any operational error (file locked, CryptSvc unavailable, timeout) → signature **absent**, counted.
 - **Deadline:** the completion stage's (§3.2), 1 s. A late result is cached for the next event on the same file. S10 measured 38 first launches with no miss (p99 540 ms), so the deadline stays. Plan 1b-4a-p2 (R4) kept it at 1 s for start-up too: the completion stage emits in order, so a longer deadline would hold every later event (about 80,000 in 5 s at 16,000 events/s, near the 100,000 pending cap) exactly when the workers fall behind.
@@ -404,6 +404,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
   - **Verification (F2).** A handle can be closed and its value reused for another object between the table read and the duplicate. The table is read a second time after naming, and a name is kept only if its handle still sits at the address. Key duplicates stay open until then and are checked themselves. File duplicates are closed as soon as they are named, so an owner's close still releases its sharing at once, and the check is on the holder's (PID, handle). T_snap is the QPC of that second read.
   - When file seeding pauses (2 stuck helpers), it stays paused for the rest of that snapshot. A name query blocks when another thread's synchronous operation on the same file object is pending, and `CancelSynchronousIo` does not free it; the device check never blocks (F3).
   - The CPU budget charges the seeder thread's CPU time, not the start-up pass. Re-reads that wait for it are merged per kind and served oldest first; each counts once in `seeder_deferred_rereads`. A table read that fails gets no reply.
+  - The seeder and its namer threads run at normal priority (plan 1b-4a-p4, C2): below normal, a busy machine held the start-up pass for 12 s to over 30 s, against under 1 s at normal (§15.3, P8). The budget does not bound that pass; what bounds it is that it runs once and is short, and with on-miss seeding off no re-read is ever sent.
   - Kernel key names keep their case (`\Registry\Machine\…`); §5.5's root mapping already ignores case (F4).
 - **Refinements** (plan 1b-3a):
   - **Address reuse.** Key objects are per handle, so a successful CreateKey or OpenKey at an address is a new object there. Children waiting on that address belonged to the earlier object: they become orphans, which only the seeder naming the child handle itself can name. The same holds for the children of an unknown base that closes, and of a tombstone whose address the seeder names after the tombstone closed. Otherwise a reused address would give a key the path of an unrelated one.
@@ -665,6 +666,8 @@ Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gate
 
 **The hour run** (2026-10-31, 74.7 minutes of normal use on the merged plan 1b-4a-p2 code, AC power, the Silent plan; §15.3): CPU averaged **1.59% of one core**, within the workday budget (per-minute p10 / p50 / p90 / max 0.67 / 0.98 / 3.35 / 7.87%); nothing lost. The working set reached **281 MB**, over budget: the file map kept FileObjects past their Cleanup whose Close Windows sent late or not at all (a `du` walk running in the background), up to its cap. **Plan 1b-4a-p3** (H1, H2, H3): entries leave the map after their Cleanup's outcome window, and no provisional entry is made that nothing can name. On the host, with Git's `du` walking the same folder: 30 minutes on a probe build of the old code left 231k entries and 312 MB (F3); 10 minutes on plan 1b-4a-p3's code, about 4.5k–4.9k entries, flat, and at most 65 MB (F9). **The hour run again** (2026-11-04, 70.6 minutes on plan 1b-4a-p3's code, AC power, the Silent plan; §15.3, F10): the working set at most **63.6 MB**, within budget, CPU 0.97% of one core, nothing lost. It was a lighter hour than the first, with about two-thirds of the events a minute, so 1.59% stays the heavier hour's figure. Both workday budgets are met, and the profiling step ends.
 
+**Plan 1b-4a-p4** (2026-11-05, the host, AC power, the Silent plan; §15.3, P13): what the hash workers at normal priority cost, with all 22 CPUs busy and a cold hash for each launch. Another program did 0.59% less work with the agent than without it, against 0.63% with them below normal: no difference seen, though 4 runs a build could not show one under about 0.8%. Its worst moments may be a little worse (about 1 chunk in 24,000 more over 50 ms; not shown, the two runs without the agent differ by as much). ETW sessions and the kernel and user queues lost nothing on either build, and enrichment misses went from 1,958–10,323 a run to 0. Not everything arrived in time: on both builds Session A's delivery stalled now and then, and each of the three stalls dropped 10–11 Launches' classic halves (`actor_dropped`: 21 in 1,800 launches at normal, 11 in 1,800 below normal; too few to tell the builds apart, P14). The agent used about 3 s more CPU a run (8.1 → 11.0 s), nearly all in the hash workers; that they now finish their work is the likely reason, not measured. The load began after the start-up pass, so the seeder was not exercised, and the workers were far from flat out (about 7.5 s of CPU between them in about 110 s). An interactive program's feel was not measured.
+
 **Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays; applied by plan 1b-4a-p2); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.
 
 ## 14. Definition of Done
@@ -824,6 +827,24 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **F10, the hour run again** (Task 4, `feat/1b-4a-p3`'s release build, 2026-11-04, 70.6 minutes of normal use, AC power, the Silent plan): the working set 47 MB at the first report, 63.6 MB at most, with no upward trend; CPU 0.97% of one core (per-minute p10 / p50 / p90 / max 0.44 / 0.70 / 1.53 / 5.84%); nothing lost, both unpaired-Cleanup parts 0, no enrichment miss, no file map eviction; the file map 3,620–5,043 entries, `cleaned_fos` at most 465, `retiring_fos` at most 1,615. It was a lighter hour than F1 (392k sensor events a minute against 584k; 48,503 records against 134,276), so the lower CPU is no evidence of a CPU gain and F1's 1.59% stays the heavier hour's figure. F9 is the file map's figure under a `du` walk.
 - **F11, a launch without its hash, again** (CI on PR #35's head, 2026-11-04): `agent-live` failed once on `launch_sha256` and `launch_signature_valid`, with 79 enrichment misses in a run of 711 events, and passed twice on the same commit with 2 misses each. The signature is plan 1b-4a-p1's F9, which the Launch lane (plan 1b-4a-p2, R4) lowered but did not remove. The live test failed in 1 of 9 runs on this plan's code against 0 of 9 sampled before it, too few to compare; the two highest miss counts, 79 and 32, are both on this plan's code, and the highest before it is 2. This plan's code changes no hash, enrichment or Launch line, so no link is known, but none is ruled out. F10's hour had no miss, so it seems to need a cold start on a busy machine. The user merged the build and took the question to the 1b-4b brainstorm.
 
+**Plan 1b-4a-p4 (2026-11-04 to 2026-11-05): a launch without its hash** (a probe build on the CI runner, 4 vCPUs, under a load generator, on debug and release builds; the host; the p4 notes have the details):
+- **P1, two causes, under heavy overload:** starvation (below normal, a hash took about 4,700 ms of wall time for about 19 ms of CPU, and jobs waited a median of 10–14 s; at normal, under 20 ms) and the deadline clock (§16; at normal, 1,905 of 1,980 late Launches were on the pipeline's side).
+- **P2, the live test's Launch, at 2–8 times the real failure's load:** its hash was missing in 13 of 24 runs with the workers below normal, 0 of 56 at normal, 1 of 24 at normal with a cap of 2,000 events a tick, and 2 of 32 with a dedicated normal-priority Launch worker and modules below normal.
+- **P3:** with the Launch worker, module hashes stop on a busy machine: 288 finished, median 13.7 s, against 5,751, median 9 ms, with all workers at normal.
+- **P4:** the tick cap showed no benefit: on release builds no pass took over 50 ms in the light-load runs. 1–3 Launches in about 400 per cell still missed on the pipeline's side; that is not reconciled with P9's lower rate under a heavier load.
+- **P5:** this spec gave no measured reason for below normal.
+- **P6, the seeder starves the same way:** with all CPUs busy `seeded_file_handle_update` failed in 22 of 32 runs (0 of 32 with light load).
+- **P7, `enrichment_misses` also counts what is pending at stop:** a quiet run reported 61 with no late reply; two launches came at the very end.
+- **P8, the seeder at normal priority:** the seeded check failed in 10 of 16 runs below normal and 0 of 16 at normal, and the start-up pass fell from 12 s to over 30 s (eight runs reached the test's 30 s wait) to 0.26–0.75 s. A table read that fails is not settled by this.
+- **P9, the deadline clock:** under overload (about 1.2M events), 2 of 3,109 Launches missed on the pipeline's side on a release build, against 1,639 of 4,363 on a debug build (about 2.5M events), whose pipeline cannot keep up.
+- **P10:** `no_queue_drops` failed in 2 of 24 runs below normal and 0 of 56 at normal.
+- **P11, the check can fail:** putting the call back on the hash workers, the seeder or the namer failed `the_workers_run_at_normal_priority` and no other of the 52 checks.
+- **P12, `agent-live`'s tests at normal priority, no load added** (8 fresh VMs, 2 passes each): the live and the binary test each passed 16 of 16. One pass reached the real failure's scale (257,826 events against 228k): 3 misses, and its Launch kept its hash. One light pass had 63 misses, not explained. Misses remain at normal priority.
+- **P13, the host run** (§13; no agent, then below normal and normal in the order B N B N N B N B, then no agent; 90 s each, with all 22 CPUs busy and a cold hash for each launch): another program's work 743,507 chunks with no agent, -0.63% below normal, -0.59% at normal (between the builds p = 0.91, and a 95% interval of about ±0.8%). At normal its chunks over 50 ms (54 → 85) and its p99.99 (42.8 → 56.8 ms) were higher in all 4 adjacent pairs, but p = 0.09 and 0.06, and the two no-agent runs had 52 and 80 chunks, 43 and 55 ms. ETW session and kernel and user queue loss 0 on both builds in all 18 reports (10.7M and 11.3M sensor events). Enrichment misses 10,323, 4,061, 2,647 and 1,958 below normal, 8,958 of them in reports that did not end at stop, and 0 in all 9 reports at normal; service queue drops 1,592 in one run below normal. Launches and module loads are one counter, so how many were Launches is not known.
+- **The stalls in P13:** three reports, two in one run at normal and one in one run below normal, had about 20,000 late arrivals each, about the 750 ms hold at this load; every other report had at most 326. Only those three had `actor_dropped` (Process: 10, 11 and 11) and `launch_join_miss` (10, 32 and 22). A classic half whose process is gone when its join window ends is dropped, one of each; a Kernel-Process half that then goes out alone is a second join miss. The counts fit that exactly (10 + 32 = 2 × 21, 22 = 2 × 11), which would be about 32 of 3,600 Launches without their command line and user. That is inferred: each run's buffer was wiped. Why delivery stalled is not known.
+- **P14, the same signal on the CI runner** (the probes' reports, which hold late arrivals but not `actor_dropped`): 1,000 late arrivals or more in 0 of 24 runs below normal and 0 of 112 at normal at the same loads (at most 64), and the live test's Launch kept its command line in all of them. 7 of 8 overload runs on a debug build did stall, on a pipeline that cannot keep up (P9), and one of 32 with the Launch worker. These runs last about 7 s, so nothing here shows that a stall depends on the workers' priority, or that it does not.
+- **Not shown:** the real failure was not reproduced, so starvation is its likely cause, not a proven one, and 0 of 56 is not never. Normal priority's cost was measured on one laptop, on one plan, against one synthetic program; ETW loss with the workers flat out on few CPUs was not measured.
+
 ## 16. Known Limitations
 
 - **No boot-time coverage:** events before the agent starts (early boot, or while the service is stopped) are not captured. An AutoLogger session is sub-project 8.
@@ -845,6 +866,9 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **Argument spoofing:** a process created suspended whose command line is rewritten before it runs shows the original (fake) command line. The kernel driver (sub-project 6) does not fix this either; detection relies on the behaviour that follows.
 - **8.3 expansion** (plan 1b-3b) happens about 1 s after the event. A directory swapped within that window resolves to the new one's long name. Paths on network redirectors are not expanded. A directory whose listing stalls (a Cloud Files placeholder, a symbolic link to a share) holds the expander lane: events waiting for an expansion go out at their deadline with the logged path, and a watchlist pattern then matches only the logged form.
 - **File reads for hashing have no timeout:** a stalled read holds one of the two hash workers.
+- **A Launch can still go out without its hash and signature:** the deadline stays 1 s (§6.3), a late result only helps the next event on that file, and a load an attacker makes can use up the 1 s. Normal priority made this rare, not impossible (§15.3, P2). `enrichment_misses` counts it, with module loads and with what is pending at stop.
+- **The enrichment deadline starts before its request is sent** (plan 1b-4a-p4): a deadline runs from the clock value of the pass (a late event processed in `push`, from the last pass's), and the requests go out after the pass. A Launch processed late in a long pass has used up part of its 1 s, and a pass over 1 s leaves it none. On a release build this took 2 of 3,109 Launches under overload (§15.3, P9; P4 is not reconciled with that).
+- **A stall in Session A's delivery longer than the 750 ms hold** costs Launches their classic half (§5.2): counted in `actor_dropped` and `launch_join_miss`, and seen with every CPU busy on either priority (§15.3, P13). Why it stalls is not known.
 - **64-bit Windows only:** the structure offsets the services read are the x64 layouts.
 - **An admin attacker can blind ETW.** This sub-project detects and reports it (§9); resisting it is sub-project 8, and the driver (sub-project 6) adds an independent sensor.
 
@@ -1009,3 +1033,9 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 2. an entry made for a Cleanup's outcome on a handle the agent never saw opened leaves like any other;
 3. an outcome exactly one window after its Cleanup still finds its entry;
 4. a late 8.3 expansion fills a map entry only if the entry still has the NT path it asked about: after a Close and a Create at the same address, the next file's Update carried the first file's path (on `main` since plan 1b-3a).
+
+**Plan 1b-4a-p4 clarifications (2026-11-05).** A Launch without its hash (decision log, 2026-11-04 and 2026-11-05). Applied to §3.2, §6.3, §7.4, §13, §15.3 and §16:
+1. the hash workers run at normal priority (C1);
+2. the seeder and its namer run at normal priority; what bounds them is that the start-up pass runs once and is short, not the CPU budget (C2);
+3. the enrichment deadline's clock, a Launch without its hash, and a stall in Session A's delivery are Known Limitations (C2, C3);
+4. the binary test reads each of these threads' priority from the running agent (`the_workers_run_at_normal_priority`).
```

- [ ] **Step 2: Commit**

```powershell
git add docs
git commit -m "docs(1): plan 1b-4a-p4 clarifications and findings"
```

### Task 4: PR and merge

- [ ] **Step 1:** Push `feat/1b-4a-p4` and open the PR. The body summarises C1 and C2 with P2, P8 and P13, the deadline clock left as a limitation, the test counts, and ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto`, `powershell` and `audit`. If a test fails only on the runner, fix it with a test, never by skipping. A failure of `launch_sha256` here is news, not the old flake: it goes back to the user with its figures.
- [ ] **Step 3:** On `feat/1b-4a-p4`, before the merge: the roadmap and a decision log row for the build, in their own commit (`docs(1): plan 1b-4a-p4 built: roadmap and decision log`), so nothing goes to `main` outside the PR.
- [ ] **Step 4:** When every check on the PR's current head is green, merge. Then delete the remote `scratch/p4`, `scratch/p4-review`, `scratch/p4-probe` and `scratch/p4-reverts` branches (ask first about the worktrees). The 1b-4b-1 brainstorm follows.

## Review Log

### Independent review (2026-11-05, of the draft on `scratch/p4` at `36de51a`)

An independent agent read the draft against the spec, the p4 notes, the code and the raw host run. It applied the plan's diffs to `main`, linted Task 1's commit, recomputed P13 from the raw results, and read the CI runs' artifacts. It found no defect in the code or in the check, one major issue in the evidence, four minor issues and some nits, all of them in the text.

**Major:**
1. **M1: the late-arrival bursts came with loss, and the draft left it out.** All three bursts carried `actor_dropped` (Process: 10, 11, 11) and `launch_join_miss` (10, 32, 22), and no other report did. The agent itself treats `actor_dropped` as loss (`loss_rose`), §9.3 lists it, and the binary test fails on it, but `cost_summary.py`'s list left it out, so the notes said "not loss" and §13 "nothing was lost". The user had chosen C3 on that summary. **Fixed:** the counters added to the tool; P13, §13, §15.3 and §16 now give them; P14 added from the CI runs' reports; C3 put to the user again, who chose A again. The counts fit about 32 of 3,600 Launches going out without command line and user; that is inferred, since each run's buffer was wiped.

**Minor:**
1. **m1: "with on-miss seeding off re-reads come only while it is pending" was false.** `ask` returns at once when on-miss seeding is off, so no re-read is sent. **Fixed** (§7.4, P8).
2. **m2: "no difference" in throughput was stronger than the data allow.** The difference has a 95% interval of about ±0.8%, wider than the whole effect. **Fixed:** "no difference seen", with the interval.
3. **m3: P13 covers less than §13 said.** The load began after the start-up pass, so the seeder was not exercised, and the workers were far from flat out. **Fixed** (§13, §15.3, P13).
4. **m4: P4 and P9 do not obviously agree.** P4's 1–3 Launches in about 400 at light load sit oddly with P9's 2 of 3,109 under overload. The analysis behind them was not kept, so they could not be reconciled here. **Stated as such** in §15.3, §16 and the notes.

**Nits, fixed:** "12–30 s" was cut off at the test's 30 s wait (now 12 s to over 30 s, and "12 s or more" in `seeder.rs`); `NORMAL_PRIORITY_THREADS`' comment cites C1 and C2; §16 says the deadline runs from the pass's clock value, a late event's from the last pass; C2's "a much larger plan" was never traced, so the rejection now rests on the benefit; P12 names the branch it ran on; Task 4's roadmap update moves into the PR; `run-p4-cost.ps1`'s header gives the order it runs, and P13 states it.

**Nits, not changed:** the check could assert every named thread at normal, and `GetPriorityClass` too. It would cover the reader and the expander, but the rule here is these four threads and it was verified as it stands; "Interfaces" carries the rule for a new thread and for the service's priority class.

**What held up:** every plan diff applies to `main` and gives `scratch/p4`'s tree; Task 1 stands alone as a commit; P13's table, the p-values and the CI run counts all recompute from the sources; nothing depends on these threads being below normal (the pipeline shares only the hash lane's short lock and non-blocking channels with them, and the budget and the stuck-helper logic use CPU and wall time, not priority); start and stop order do not change; `GetThreadPriority` reads the base priority and its error value is not 0, and the check cannot pass on an absent thread; `git grep` finds exactly the three calls the plan removes, and no stale text about priorities.
