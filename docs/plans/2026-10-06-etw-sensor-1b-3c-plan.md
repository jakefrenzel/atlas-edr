# Sub-project 1b-3c — Agent Driver, Deletes and Live Test Implementation Plan

> **Status:** Approved (2026-10-06). **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Task 7 needs one elevated run on the host, by the user, from a script; nothing in this plan needs the VM or a kernel driver.

**Goal:** Run the sensor end to end (sensor spec §3.2, §11.4, §12.3): the threads that 1b-3a and 1b-3b built, wired together, and tested live at agent level.
- **the driver:** the pipeline thread's loop, and `win::agent`, which starts and stops identity, both sessions, their consumers and intakes, the services and the pipeline thread;
- **the agent-level live test** promised by §12.3: seeding of handles opened before the agent, value reads, the watchlist through an 8.3 name, undelete, the real services' enrichment and the self-filter;
- **file deletes from the Cleanup outcome.** The live test's undelete case found that every undelete produced a false Delete (F1). Deletes now come from what the file system reports at Cleanup (D3).

The sink is a closure. Plan 1b-4 plugs in the buffer writer, and adds the watchdog, Sensor Health reports, the service and the CLI.

**Architecture:**
- **`driver`** (portable): `Driver` runs the pipeline loop over a two-method `Lanes` trait. `win::Services` and the new `fakes::FakeLanes` implement it. Every 10 ms, one pass:
  1. drain both queues into the pipeline;
  2. feed in the replies;
  3. `tick`;
  4. hand the requests to the lanes;
  5. pass the emitted events to the sink.

  Each pass drains at most a queue's capacity, and runs any `Control` closure another thread sent (plan 1b-4's way in). It re-takes the anchor every 60 s, and ends when both queues are closed (D1, D4).
- **`win::agent`:** `start(AgentOptions, sink) -> Running`, and `Running::stop() -> Stopped`. The stop order is §11.4's:
  1. flush the sessions;
  2. wait out the hold;
  3. stop the sessions, so the consumers finish;
  4. drop the intakes;
  5. the pipeline drains and stops, then the services go.
- **Deletes** (D3; `cleanup`, `intake`, `pipeline::file`):
  - **Intake:** the callback remembers each Cleanup's `Irp` until its OperationEnd. It passes on a successful OperationEnd only when its `ExtraInformation`, the file system's `FILE_CLEANUP_*`, reports a removed name. Where the outcome is unknown (SMB), it passes the outcome on only for a file with a delete request outstanding. It also passes on a delete request's own OperationEnd.
  - **Pairing:** each thread reuses one `Irp` for everything, so a Cleanup pairs with the next OperationEnd of its `Irp`, within 1 s, in either delivery order. Any other operation on the `Irp` ends the wait, and a Cleanup left unpaired is counted (`quality.file_cleanup_unpaired`).
  - **Pipeline:** emits the Delete at that Cleanup, with the requester (`DeletePath`, the new `SetDelete` 18, a delete-on-close open) as actor, and `DeletePath`'s own path. An undelete reports `FILE_REMAINS` and gives none.
- **`atlas-etw`:** a parser for Kernel-File 18 `SetDelete`, enabled in Session A. Its live test checks the Cleanup outcomes and `SetDelete`, and the CI recording now includes them.
- **Tests:** the replay of the CI recording also runs through the driver loop, and gives the same events byte for byte. `atlas-agent/tests/live.rs` is the agent-level live test (D2); CI's `agent-live` job runs it.
- **Schema:** two additive Sensor Health counters, `quality.file_delete_outcome_unknown` and `quality.file_cleanup_unpaired`.

**Tech stack:** Rust 1.97 (edition 2024). No new dependencies. Tools: `buf` 1.73.0, `actionlint` 1.7.12.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2, 1b-3a and 1b-3b. Section numbers (§) refer to it.

**Verification note (2026-10-06, host, Windows 11 build 26200, and the CI runner, build 26100):** every code block below was compiled and run in a scratch worktree of `main` (953fcb9).
- **Lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean at the end of every task. `atlas-agent` and `atlas-etw` are also clippy-clean for `x86_64-unknown-linux-gnu`, checked from Windows.
- **Unelevated:** `cargo test --workspace` passes 363 tests and ignores 7 (the two live tests and 1b-3b's five elevated tests). `atlas-agent` has 179 unit tests (1b-3b had 149). Test counts per task were measured by building each intermediate state.
- **Probes:** five elevated runs by the user (`spikes/run-1b3c-*.ps1`; one was discarded for event loss) found F1–F6 before D3 was decided.
- **Elevated on the host:** Task 7's script, run by the user. Four runs, the last two on the reviewed code:
  - **first run:** the agent live test passed all 16 checks it then had, and 1b-3b's elevated tests 167 of 167. `atlas-etw`'s live test failed one new check: the test paired Cleanups with recycled `Irp`s (Task 2, Step 2), a mistake in the test, not the agent.
  - **second run:** everything passed (agent 16/16, elevated 167/167, `atlas-etw` live).
  - **third run, on the reviewed code:** everything passed (agent 19/19, elevated 184/184, `atlas-etw` live). It counted 13 Cleanups unpaired, with a 10 ms pairing window, while the host was busy (90,567 OperationEnds discarded in about 10 s). In every recording, an outcome follows its Cleanup within 0.25 ms, but those hold only the test's own process.
  - **fourth run:** the window became 1 s (the next-operation rule and the value check are what stop false pairings), and the counter was split into "another operation came first" and "past the window". Everything passed again, with 0 of each, but on a quieter host (15,882 OperationEnds). The runs do not settle which case the 13 were; plan 1b-4's 24-hour run measures both counters (Interfaces).
- **On the CI runner** (build 26100; D5):
  - run 37544462603 recorded the fixture, and found F8 (a case-sensitive test comparison);
  - runs 37545024172, 37550612709 and 37553797311, the last two on the reviewed code, passed every job, `etw-live` and `agent-live` included;
  - in the last, the agent live test passed all 19 checks, with 32 events self-filtered and no unpaired Cleanup.
- **Reverts:** each rule and fix in the Review Focus and the Review Log was checked by reverting it, and all 25 reverts make their test fail. Two tests passed with their fix reverted at first (the POSIX match, and the pipeline's next-operation rule) and were changed until the revert failed them.
- **Round trip:** the code was rebuilt from this document alone in a fresh LF worktree of `main`: every diff applied, every file written, the recording copied from its task commit, and the snapshot regenerated as Steps 6 and 7 say. Every task's tree is identical to the verified one.
- **Other checks:** `buf lint` and `buf breaking` (against `main`) pass. Regenerating the golden fixtures changed none (line endings only). `actionlint` passes on `ci.yml`.
- **Not run yet:**
  - the Linux build of `atlas-schema`'s tests (its `criterion` benchmark needs a Linux C compiler; CI has one);
  - `cargo audit` (CI runs it).

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-3c-plan`. The build runs on `feat/1b-3c-driver`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Windows code only in `win`** (and `#[cfg(windows)]` tests). The driver loop is portable and Linux CI builds and tests it.
- **Every `unsafe` block has a `// SAFETY:` comment.** The new ones are in the tests (the actor's Windows calls) and in the test key's drop.
- **The ETW callback stays fast:** the new state is three small hash maps keyed by `Irp` and two bounded sets, one or two operations per Kernel-File event, never a blocking call.
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). The live tests are `#[ignore]`d; CI's `etw-live` and `agent-live` jobs run them, and so does Task 7's script, which the user runs.
- **Recordings are made on CI, never on the host** (§12.2): a host recording would leak usernames, paths and DNS history into the public repo.
- **Additive schema only:** `buf breaking` must pass, and the existing golden fixtures must stay byte-identical.
- **Formatting and lint:** the repo's `rustfmt.toml`; `cargo clippy --workspace --all-targets -- -D warnings` at the end of every task.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash). Edits that contain backslashes are made with an editor, never a heredoc.
- Every commit message ends with the attribution lines the session's system reminder specifies.

## Decisions (chosen 2026-10-06: D1 A, D2 A, D3 A, D4 A, D5 A)

### D1, what the driver is

- **(A) Chosen: the real driver, with its loop portable.** A portable `driver` module runs the pipeline-thread loop over a small `Lanes` trait (`submit`, `replies`), so the loop's threading (queue draining, tick cadence, shutdown) is tested on Linux with fakes, the CI recording included. `win::agent` starts everything around it. 1b-4 plugs in the buffer as the sink and adds the watchdog and the service.
- (B) The real driver, all Windows-only: no fakes, but the loop is tested only by the elevated live test. (C) A test harness inside the live test: smaller, but the tested wiring is not what ships, and 1b-4 would redo it.

### D2, what the agent-level live test covers

The agent filters its own events by start key, so the scenario runs in a second process (the actor), as in `atlas-etw`'s test. The new part: the actor opens handles *before* the agent starts.
- **(A) Chosen: the four agent-only parts, plus the checks only real services can fail.**
  - seeding: a file handle and a key handle opened before the agent, then written to and created under;
  - value reads: a `REG_SZ` and a `REG_DWORD`, checked against `data_read_after`;
  - the watchlist: an Open by long name and one by 8.3 name, both reported under the long name;
  - undelete: a disposition set and cleared, and a delete-on-close cleared through `FileDispositionInformationEx`, against a real delete;
  - real enrichment of the `cmd.exe` Launch: SHA-256, catalog signature, account name, DOS path;
  - the self-filter under the services' own I/O, and no queue drops.
- (B) The whole §12.3 scenario at agent level: the rest uses no Windows service and is covered by replay. (C) Only the four agent-only parts.

### D3, how deletes are detected (re-decided after probes)

The first pick, "a Delete is confirmed by `NameDelete`", was dropped on evidence (F2). The probes:
- **F1:** under 1b-3a's rule, every undelete gave a false Delete.
- **F3:** the Cleanup's OperationEnd carries the file system's `FILE_CLEANUP_*` outcome on NTFS, FAT32 and exFAT, and `0` (unknown) on SMB.

- **(A) Chosen: the Cleanup outcome, with the delete request as fallback where the outcome is unknown.**
  - Session A enables 18 `SetDelete` (its keyword was already on).
  - **The callback** keeps two small maps:
    - each Cleanup's `Irp` until its OperationEnd;
    - the files with a delete request outstanding: set by 26 or an 18-set, cleared by an 18-clear or by the request's own failure.
  - **What the callback passes on:** a successful Cleanup OperationEnd only when it reports a removed name, or reports unknown for a requested file.
  - **The pipeline** emits the Delete at that Cleanup:
    - its path is `DeletePath`'s, a link's or a stream's own; else, for delete-on-close, the handle's name;
    - its actor is the requester, else the process whose Cleanup removed the name.
  - **Gains:**
    - undeletes are right on NTFS, FAT and exFAT, and on SMB when the clear comes before the close;
    - no confirm window for deletes;
    - deletes through handles opened before the agent are caught;
    - links and streams are reported as such.
- (B) `SetDelete` only: a Delete at the requester's Cleanup if still set. It misses a clear on another handle after that Cleanup and deletes through pre-agent handles, and can't tell links and streams apart. (C) Document the false Delete (§16).

### D4, defaults for everything else

Accepted together:
1. the loop: one pass every 10 ms (`tick` well under 50 ms; idle cost 100 wake-ups/s);
2. the sink: a closure on the pipeline thread, called in order;
3. `win::agent::start(options, sink)`, with session names as a parameter (`Atlas-Test-Agent-*` in tests); stale sessions are recreated;
4. the clean stop's order (above);
5. the anchor re-taken by the loop every 60 s;
6. `quality.file_delete_outcome_unknown` for Deletes from the fallback;
7. `atlas-etw`'s actor gains undelete, hard-link and stream steps, and CI records the fixture with 18;
8. known limitations (§16): SMB's late clear, ReFS untested, a target replaced by a rename;
9. the live test's protocol: the actor's stdin and stdout (`READY`, `GO`, `RESULT`, `DONE`);
10. the watchlist via `watchlist_extend`, the 8.3 name via `GetShortPathNameW`;
11. `REG_SZ` and `REG_DWORD` value reads;
12. CI's `agent-live` job runs the live test, plus one elevated host run from a script;
13. one plan, one build PR, the spec revisions applied by a task.

### D5, verifying on the CI runner before the plan

The replay fixture can only be recorded on CI (§12.2). 1b-2's D1 kept planning on the host.
- **(A) Chosen: the scratch branch was pushed (no PR).** CI recorded the fixture (run 37544462603) and ran everything on the runner's build. A second run (37545024172) was green on every job. The plan's code is verified on both builds.
- (B) Record during the build, with the fixture task's assertions unverified until then. (C) Push only for the recording.

## Findings from the build

- **F1, undeletes gave false Deletes:**
  - under 1b-3a's rule, every one of five undelete cases produced a Delete, and setting the disposition twice produced two;
  - `DeletePath` (26) fires only on a set; `SetDelete` (18) fires on set and clear, with `ExtraInformation` 1 or 0, on any handle for the file.
- **F2, `NameDelete` (11) cannot mark a delete:**
  - for ordinary deletes it comes from the System process 0.3–1.2 s later, at the file's final close;
  - a hard-link delete gives none;
  - renames give it at once.
- **F3, the Cleanup outcome:** the Cleanup's OperationEnd carries the documented `FILE_CLEANUP_*` value:
  - on NTFS, FAT32 and exFAT, at once, from the deleting process;
  - an SMB share reports `0`;
  - `FileDispositionInformationEx` is refused on FAT, exFAT and SMB.
- **F4, a POSIX delete while another handle is open** reports `0x24` at the deleter's Cleanup and `0x4` at the last one: one Delete, matched on (`FileKey`, path).
- **F5, `FileKey` is reused** for a new file within seconds. The state keyed by it must go when the file goes, or when the request fails (Review Log, R-1), and the POSIX match also compares the path.
- **F6, host rates over 60 s:** `NameDelete` 2.4/s, `DeletePath` and `SetDelete` 0.7/s each.
- **F7, 1b-3b's symbolic-link test leaked its volatile test key** on every run: `RegDeleteTreeW` follows a link instead of deleting it. A reused PID then failed the test. 44 leaked keys were removed from the host. The link is now deleted through its own handle, and the test checks that the key is gone.
- **F8, key names keep their hive's case** on the runner too (`HKU\<SID>\SOFTWARE`). Only a case-sensitive test comparison noticed; it now ignores case.

## Deliberate clarifications of the spec (applied to the spec text in Task 8)

1. **The driver** (D1, D4; §3.2):
   - a portable loop, one pass every 10 ms;
   - the anchor re-taken every 60 s;
   - it ends when both queues are closed, applying the replies already in and stopping the pipeline;
   - it owns the services.
2. **File Delete** (D3, F1, F3; §5.1, §7.1):
   - **Source:** the Cleanup that removed the name. A request does not emit.
   - **Path:** the handle's name.
   - **Actor:** the requester (26 or 18 set, or a delete-on-close opener, recorded on its Cleanup), else the process whose Cleanup removed the name.
   - **Failures:** a failed request is taken back.
   - **`file.op_end` off:** the request is the Delete.
3. **Session A enables Kernel-File 18 `SetDelete`** (§4.2).
4. **The callback passes on the Cleanup outcomes that report a delete** (§3.2), and for a requested file, those that report nothing.
5. **One Delete for a POSIX delete reported twice** (F4).
6. **Unknown outcomes** (SMB): a Cleanup of a requested file is the Delete, counted as `quality.file_delete_outcome_unknown` (§10.3).
7. **The confirm window** covers only Create and RenamePath (§5.5).
8. **The clean stop's order** (§11.4).
9. **Tests** (§12.1, §12.3): the agent-level live test, and `atlas-etw`'s checks of the outcomes.
10. **Known limitations** (§16):
    - SMB and ReFS;
    - a file replaced by a rename;
    - a Cleanup and its outcome delivered far apart;
    - the delete-on-close limitation is gone;
    - slow failures now concern renames only.
11. **From the review** (Review Log; §3.2, §5.1, §10.3):
    - the path from the request's `DeletePath`;
    - a delete request's successful OperationEnd passed on;
    - pairing by the next operation on the `Irp`, within 1 s, in either delivery order;
    - `quality.file_cleanup_unpaired`;
    - a delete-on-close clear keeps another handle's disposition;
    - the driver's control channel.

## Review Focus

These would slip past a plain unit test, so each has a pinned check:
1. **Undeletes give no Delete** (`an_undelete_gives_no_delete`, and in the replay of the CI recording: one Delete for `u.txt`, the directory's removal; live: `undelete_gives_no_delete`).
2. **The Delete's actor is the requester,** even when another process's Cleanup deletes, and a failed request is not inherited by a reused `FileKey` (`a_delete_comes_from_the_cleanup_that_removed_the_name_with_the_requester_as_actor`, `failed_operations_are_dropped_by_irp_within_the_window`).
3. **The callback passes only what it must** (`cleanup_outcomes_pass_when_they_report_a_delete`, `an_unknown_outcome_passes_only_for_a_requested_delete`):
   - outcomes that report a removed name;
   - unknown outcomes for requested files.

   Never `FILE_REMAINS`, an OperationEnd of another operation, a cleared request, or a failed one.
4. **POSIX double reports** are one Delete, and a reused `FileKey` with another path is not suppressed (`a_posix_delete_reported_twice_is_one_delete`).
5. **Links and streams** are Deletes of their own names (`links_and_streams_are_deletes_of_their_own_name`; `atlas-etw` live: `cleanup_outcome_link_deleted`, `cleanup_outcome_stream_deleted`).
6. **SMB's fallback** is counted (`an_unknown_outcome_reports_a_requested_delete_and_counts_it`).
7. **The driver loop** gives exactly the synchronous replay's events (`the_driver_loop_gives_the_same_events`). It also:
   - releases while idle;
   - drains on close and runs on while one queue is open;
   - re-takes the anchor;
   - hands requests to the lanes.

   (The `driver::tests`.)
8. **The live test's checks** (Task 6), on the host and the runner.
9. **The stop order:** the intakes and `FastRead` go before the services (R-m11 of 1b-3b). `Running::finish` closes the consumers before the driver drops the services; the live test stops cleanly with no callback panics.
10. **The review's cases** (Review Log), each a test:
    - a request on an `Irp` that later fails is kept (`a_later_failure_on_the_requests_irp_does_not_take_the_request_back`, `the_same_with_the_agent_closing_last`, `a_request_that_succeeded_stands_against_a_later_failure_on_its_irp`, `a_requests_operation_end_is_passed_on`);
    - the path after a rename and for an unknown handle (`rename_then_delete_on_one_handle_reports_the_new_name`, `delete_through_a_handle_opened_before_a_rename`, `a_delete_through_an_unknown_handle_without_seeding`);
    - POSIX reports (`a_stale_posix_entry_does_not_suppress_a_later_delete_of_the_same_path`, `a_posix_double_report_through_a_short_name_is_one_delete`);
    - delivery order and `Irp` reuse (`an_outcome_delivered_before_its_cleanup_is_paired`, `an_unpaired_cleanup_is_counted_never_paired_later`, `a_cleanup_never_pairs_with_another_operations_end`, `an_outcome_processed_before_its_late_cleanup_is_paired`, `what_counts_as_removed`);
    - a delete-on-close clear (`clearing_a_handles_delete_on_close_keeps_another_handles_request`, in the intake and the pipeline);
    - the control channel (`control_runs_on_the_pipeline_thread_between_passes`; live: `control_reaches_the_pipeline`) and the bounded drain (`a_capped_drain_loses_nothing`).

Every one of these rules and fixes was checked by reverting it: its test fails (verification note). The bounded drain is the exception: reverting it loses nothing, and its effect is fairness, by reading.

## File Structure

```
crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto   + file_delete_outcome_unknown
crates/atlas-schema/src/classes/sensor_health.rs               + the field
crates/atlas-schema/tests/common/mod.rs                        strategies cover it
crates/atlas-etw/
  src/parse/mod.rs        + RawEvent::FileSetDelete (18)
  src/layout.rs           + 18 v1
  src/providers.rs        Session A: + 18
  tests/common/mod.rs     18 compared with TDH
  tests/live.rs           + undelete, hard link, stream steps; Cleanup outcome checks
  tests/fixtures/scenario.jsonl   re-recorded on CI
crates/atlas-agent/
  src/lib.rs              + cleanup, driver
  src/cleanup.rs          FILE_CLEANUP_* (new)
  src/recent.rs           + RecentMap
  src/intake.rs           + Cleanups (the callback's state)
  src/counters.rs         + file_delete_outcome_unknown
  src/config.rs           comments: what file.op_end now covers
  src/pipeline/mod.rs     + FileSetDelete
  src/pipeline/file.rs    deletes from the Cleanup outcome
  src/pipeline/tests.rs   the delete scenarios
  src/driver.rs           Lanes, DriverConfig, Driver (new)
  src/fakes.rs            + FakeLanes
  src/win/mod.rs          + agent
  src/win/agent.rs        AgentOptions, start, Running, Stopped (new)
  src/win/services.rs     Lanes for Services
  src/win/value.rs        the test key deletes its links (F7)
  tests/replay.rs         the replay through the driver; delete assertions
  tests/snapshots/scenario.jsonl  regenerated
  tests/live.rs           the agent-level live test (new)
.github/workflows/ci.yml  agent-live: + the live test, its report
spikes/run-1b3c-elevated.ps1    git-ignored; Task 7
docs/…                    Task 8
```

## Interfaces for later plans

- **Plan 1b-4 (the buffer, Sensor Health, the watchdog, the service):**
  - **The sink** is called on the pipeline thread, in order, with each emitted event. It must not block: hand the event to the buffer writer thread [6] (§3.2) through a bounded channel. The detection hook (sub-project 3) goes in front of it.
  - **Reaching the pipeline while it runs:** `Running::control()` gives a sender of `driver::Control` closures, run on the pipeline thread between two passes. Through it go:
    - the launch canary's self key (`Pipeline::add_self_key`, §9.2);
    - the Sensor Health reads of `Pipeline::counters()` every 60 s.
  - **Counters:**
    - `Running::intake_counters()` and `service_counters()` are shared atomics, readable at any time;
    - the pipeline's `Counters` are returned by `Driver::run` at the end, and read through `control()` during the run;
    - `callback_panics` comes from `Consumer::panics()`, summed in `Stopped`. 1b-4 exposes it on `Running` for the periodic report;
    - `quality.file_delete_outcome_unknown` comes from `Counters::file_delete_outcome_unknown`, and `quality.file_cleanup_unpaired` is `IntakeCounters::cleanup_unpaired` plus `cleanup_outcome_late`. The 24-hour run reports both parts. If either is not zero on a normally busy host, it is investigated before 1b-4 closes (verification note: a busy host counted 13 with the 10 ms window).
  - **The watchdog's checks:** `Running::pipeline_running()` is false once the pipeline thread has ended. The callbacks' sends then fail with `Disconnected`, which is not counted as a queue drop.
  - **Restarting a session** (the watchdog, §9.1): `Running` keeps spare senders (`spare: Queues`), so the queues stay open while a session's consumer is replaced, and `Running::config()` gives the settings a new `Intake` needs. A new Session A intake also needs a `FastRead`, and `Services::fast_read()` needs the services, which the driver owns once it runs. 1b-4 makes one spare `FastRead` before the driver takes the services, or moves the reader lane's sender into `Running`.
  - **`file.op_end` off** (the §13 fallback) must also leave out Kernel-File 24 and its keyword in Session A's enable. Today only the pipeline's rule changes; `providers::session_a` always enables OP_END.
  - **The clean stop** takes the hold (750 ms) plus the time the consumers need to drain. The service's stop handler must allow for it.

---

### Task 1: The schema counters

**Files:**
- Modify: `crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto`, `crates/atlas-schema/src/classes/sensor_health.rs`, `crates/atlas-schema/tests/common/mod.rs`

- [ ] **Step 1: `quality.file_delete_outcome_unknown` and `quality.file_cleanup_unpaired`**, and the `file_op_failed` comment: a failed delete no longer has anything to drop (D3).

```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
@@ -76,6 +76,12 @@ message SensorQuality {
   optional uint64 enrichment_errors = 16;
   // Registry value names that could end in more than one place (embedded NULs); the last fit was used.
   optional uint64 reg_name_ambiguous = 17;
+  // File deletes reported from the delete request alone, because the file system did not report
+  // the cleanup's outcome (SMB shares): a delete cleared later on another handle is still reported.
+  optional uint64 file_delete_outcome_unknown = 18;
+  // File Cleanups whose outcome could not be paired with them (its OperationEnd came too late,
+  // or never): a delete there may be missed.
+  optional uint64 file_cleanup_unpaired = 19;
 }
 
 // Evictions from bounded structures, and expected drops.
@@ -88,7 +94,7 @@ message SensorHousekeeping {
   optional uint64 hash_cache_evictions = 6;
   // Whole buffer segments deleted by rolling retention (expected while there is no transport).
   optional uint64 retention_evictions = 7;
-  // Failed file creates, deletes and renames that were dropped.
+  // Failed file creates and renames that were dropped (a failed delete never reports one, §5.1).
   optional uint64 file_op_failed = 8;
   optional uint64 pending_overflow = 9;
   // Handle-table seeding.
```

```diff
--- a/crates/atlas-schema/src/classes/sensor_health.rs
+++ b/crates/atlas-schema/src/classes/sensor_health.rs
@@ -168,6 +168,10 @@ counter_group!(
         enrichment_errors: Option<u64>,
         /// Registry value names with more than one possible end (embedded NULs).
         reg_name_ambiguous: Option<u64>,
+        /// Deletes reported from the request alone (the cleanup's outcome was unknown).
+        file_delete_outcome_unknown: Option<u64>,
+        /// Cleanups whose outcome could not be paired with them.
+        file_cleanup_unpaired: Option<u64>,
     }
 );
 
@@ -182,7 +186,7 @@ counter_group!(
         hash_cache_evictions: Option<u64>,
         /// Whole buffer segments deleted by rolling retention (expected without a transport).
         retention_evictions: Option<u64>,
-        /// Failed file creates, deletes and renames that were dropped.
+        /// Failed file creates and renames that were dropped.
         file_op_failed: Option<u64>,
         pending_overflow: Option<u64>,
         seeding_enabled: Option<bool>,
```

The property-test strategy covers the new field:

```diff
--- a/crates/atlas-schema/tests/common/mod.rs
+++ b/crates/atlas-schema/tests/common/mod.rs
@@ -389,8 +389,8 @@ fn arb_health_report() -> BoxedStrategy<HealthReport> {
             callback_panics: f.9,
         });
     let quality = (
-        (counter(), counter(), counter(), counter(), arb_class_counts(), counter(), counter(), counter()),
-        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
+        (counter(), counter(), counter(), counter(), arb_class_counts(), counter(), counter(), counter(), counter()),
+        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
     )
         .prop_map(|(a, b)| SensorQuality {
             late_arrivals: a.0,
@@ -410,6 +410,8 @@ fn arb_health_report() -> BoxedStrategy<HealthReport> {
             enrichment_misses: b.6,
             enrichment_errors: b.7,
             reg_name_ambiguous: b.8,
+            file_delete_outcome_unknown: a.8,
+            file_cleanup_unpaired: b.9,
         });
     let housekeeping = (
         (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-schema -p atlas-proto
buf lint crates/atlas-proto/proto
buf breaking crates/atlas-proto/proto --against '.git#branch=main,subdir=crates/atlas-proto/proto'
$env:ATLAS_UPDATE_FIXTURES = '1'; cargo test -p atlas-schema --test golden; Remove-Item Env:ATLAS_UPDATE_FIXTURES
git diff --ignore-cr-at-eol --stat crates/atlas-schema/tests/fixtures
```
Expected: all pass, `buf` reports nothing, and the fixture diff is empty (`git checkout` the line-ending noise). Workspace: 332 passed, 6 ignored (`atlas-agent` unit tests: 149).
```powershell
git add crates/atlas-proto crates/atlas-schema
git commit -m "feat(schema): Sensor Health file_delete_outcome_unknown and file_cleanup_unpaired"
```

### Task 2: `SetDelete`, the Cleanup outcomes in `atlas-etw`'s live test, and the CI recording

**Files:**
- Modify: `crates/atlas-etw/src/parse/mod.rs`, `src/layout.rs`, `src/providers.rs`, `tests/common/mod.rs`, `tests/live.rs`, `tests/fixtures/scenario.jsonl` (re-recorded)
- Modify: `crates/atlas-agent/src/pipeline/mod.rs` (the new variant, ignored until Task 3), `tests/replay.rs`, `tests/snapshots/scenario.jsonl` (regenerated)

- [ ] **Step 1: The parser.** 18 `SetDelete` has 17's layout (version 1), so `FileSetInfo` serves both. `every_manifest_layout_matches_the_installed_manifest` checks it against the installed manifest.

```diff
--- a/crates/atlas-etw/src/parse/mod.rs
+++ b/crates/atlas-etw/src/parse/mod.rs
@@ -206,15 +206,18 @@ pub struct FileWrite {
     pub extra_flags: u32,
 }
 
-/// Kernel-File 17 `SetInformation`.
+/// Kernel-File 17 `SetInformation` and 18 `SetDelete`.
 #[derive(Debug, Clone, PartialEq, Eq)]
 pub struct FileSetInfo {
     pub irp: u64,
     pub file_object: u64,
     pub file_key: u64,
+    /// For 18, 1 when the delete disposition is set and 0 when it is cleared
+    /// (plan 1b-3c, verified on the host for both disposition classes).
     pub extra_information: u64,
     pub issuing_tid: u32,
-    /// `FILE_INFORMATION_CLASS`: 4 basic (timestamps, attributes), 19 end of file.
+    /// `FILE_INFORMATION_CLASS`: 4 basic (timestamps, attributes), 19 end of file;
+    /// for 18, 13 `FileDispositionInformation` or 64 `FileDispositionInformationEx`.
     pub info_class: u32,
 }
 
@@ -373,6 +376,8 @@ pub enum RawEvent {
     FileClose(FileHandle),
     FileWrite(FileWrite),
     FileSetInfo(FileSetInfo),
+    /// Kernel-File 18 `SetDelete`: a delete disposition set or cleared.
+    FileSetDelete(FileSetInfo),
     FileOpEnd(FileOpEnd),
     FileDeletePath(FilePath),
     FileRenamePath(FilePath),
@@ -434,6 +439,7 @@ fn parse_known(meta: &EventMeta, payload: &[u8], exact: bool) -> Result<RawEvent
         (Provider::KernelFile, 14) => RawEvent::FileClose(file_handle(r, p)?),
         (Provider::KernelFile, 16) => RawEvent::FileWrite(file_write(r, p)?),
         (Provider::KernelFile, 17) => RawEvent::FileSetInfo(file_set_info(r, p)?),
+        (Provider::KernelFile, 18) => RawEvent::FileSetDelete(file_set_info(r, p)?),
         (Provider::KernelFile, 24) => RawEvent::FileOpEnd(file_op_end(r, p)?),
         (Provider::KernelFile, 26) => RawEvent::FileDeletePath(file_path(r, p)?),
         (Provider::KernelFile, 27) => RawEvent::FileRenamePath(file_path(r, p)?),
```

```diff
--- a/crates/atlas-etw/src/layout.rs
+++ b/crates/atlas-etw/src/layout.rs
@@ -298,6 +298,7 @@ pub const LAYOUTS: &[Layout] = &[
     l(Provider::KernelFile, 14, 1, FILE_HANDLE_V1),
     l(Provider::KernelFile, 16, 1, FILE_WRITE_V1),
     l(Provider::KernelFile, 17, 1, FILE_SET_INFO_V1),
+    l(Provider::KernelFile, 18, 1, FILE_SET_INFO_V1),
     l(Provider::KernelFile, 24, 0, FILE_OP_END_V0),
     l(Provider::KernelFile, 26, 1, FILE_PATH_V1),
     l(Provider::KernelFile, 27, 1, FILE_PATH_V1),
```

```diff
--- a/crates/atlas-etw/src/providers.rs
+++ b/crates/atlas-etw/src/providers.rs
@@ -85,7 +85,7 @@ pub fn session_a(udp: bool) -> Vec<Enable> {
         Enable {
             provider: Provider::KernelFile,
             keywords: 0x1EE0,
-            event_ids: vec![12, 13, 14, 16, 17, 24, 26, 27, 30],
+            event_ids: vec![12, 13, 14, 16, 17, 18, 24, 26, 27, 30],
         },
         // CloseKey 0x1, SetValueKey 0x100, DeleteValueKey 0x200, CreateKey 0x1000,
         // OpenKey 0x2000, DeleteKey 0x4000.
```

```diff
--- a/crates/atlas-etw/tests/common/mod.rs
+++ b/crates/atlas-etw/tests/common/mod.rs
@@ -194,7 +194,7 @@ pub fn expected_fields(e: &RawEvent) -> Vec<(&'static str, Expect)> {
             ("IOFlags", Num(w.io_flags.into())),
             ("ExtraFlags", Num(w.extra_flags.into())),
         ],
-        RawEvent::FileSetInfo(i) => vec![
+        RawEvent::FileSetInfo(i) | RawEvent::FileSetDelete(i) => vec![
             ("Irp", Num(i.irp)),
             ("FileObject", Num(i.file_object)),
             ("FileKey", Num(i.file_key)),
```

- [ ] **Step 2: The live test's new steps and checks.** The actor gains three steps:
  - an undelete (disposition set, then cleared);
  - a hard-link delete;
  - a stream delete.

  The observer checks:
  - each step's Cleanup outcome, pairing each Cleanup with the *next* OperationEnd of its `Irp` (Irps are recycled; the first draft paired with any, and the host run caught it);
  - `SetDelete` on set and clear.

```diff
--- a/crates/atlas-etw/tests/live.rs
+++ b/crates/atlas-etw/tests/live.rs
@@ -33,7 +33,9 @@ use windows::Wdk::System::Registry::{NtCreateKey, NtDeleteKey, NtDeleteValueKey,
 use windows::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, UNICODE_STRING};
 use windows::Win32::NetworkManagement::Dns::*;
 use windows::Win32::Security::{LookupAccountSidW, PSID, SID_NAME_USE};
-use windows::Win32::Storage::FileSystem::DeleteFileW;
+use windows::Win32::Storage::FileSystem::{
+    CreateHardLinkW, DeleteFileW, FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
+};
 use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
 use windows::Win32::System::Registry::*;
 use windows::Win32::System::Threading::GetCurrentProcess;
@@ -341,6 +343,24 @@ fn open_key(path: &str) -> Key {
     Key(k)
 }
 
+/// `DELETE` access, for the disposition steps.
+const DELETE: u32 = 0x0001_0000;
+
+fn set_disposition(f: &std::fs::File, delete: bool) {
+    use std::os::windows::io::AsRawHandle;
+    let info = FILE_DISPOSITION_INFO { DeleteFile: delete };
+    // SAFETY: a valid handle and a buffer of the class's size.
+    unsafe {
+        SetFileInformationByHandle(
+            HANDLE(f.as_raw_handle()),
+            FileDispositionInfo,
+            (&raw const info).cast(),
+            size_of::<FILE_DISPOSITION_INFO>() as u32,
+        )
+    }
+    .expect("SetFileInformationByHandle(FileDispositionInfo)");
+}
+
 /// A counted `UNICODE_STRING` over `name` (which may contain NULs).
 fn counted(name: &[u16]) -> UNICODE_STRING {
     let len = (name.len() * 2) as u16;
@@ -434,6 +454,36 @@ fn actor() -> Value {
         drop(f);
         assert!(!d.exists());
     });
+    // Deletes the file system reports at Cleanup (plan 1b-3c): an undelete, a
+    // hard link and a stream. Each handle closes inside its step, so the
+    // Cleanup's OperationEnd falls in the step's window.
+    let u = dir.join("u.txt");
+    std::fs::write(&u, b"u").unwrap();
+    steps.run("file_undelete", || {
+        let f = std::fs::OpenOptions::new().access_mode(DELETE).open(&u).unwrap();
+        set_disposition(&f, true);
+        set_disposition(&f, false);
+        drop(f);
+        assert!(u.exists());
+    });
+    let (l, l2) = (dir.join("l.txt"), dir.join("l2.txt"));
+    std::fs::write(&l, b"l").unwrap();
+    // SAFETY: valid paths.
+    unsafe {
+        CreateHardLinkW(PCWSTR(wide(&l2.to_string_lossy()).as_ptr()), PCWSTR(wide(&l.to_string_lossy()).as_ptr()), None)
+    }
+    .expect("CreateHardLinkW");
+    steps.run("file_delete_hard_link", || {
+        std::fs::remove_file(&l2).unwrap();
+        assert!(l.exists());
+    });
+    let s = dir.join("s.txt");
+    std::fs::write(&s, b"s").unwrap();
+    std::fs::write(dir.join("s.txt:x"), b"x").unwrap();
+    steps.run("file_delete_stream", || {
+        std::fs::remove_file(dir.join("s.txt:x")).unwrap();
+        assert!(s.exists());
+    });
     let _ = std::fs::remove_dir_all(&dir);
 
     // ---- registry ----
@@ -895,6 +945,40 @@ fn analyse(r: &mut Report, steps: &Steps, events: &[Captured], actor: &Value, ob
             .any(|e| matches!(e, RawEvent::FileCreate(c) if ends_with(&c.file_name, r"\d.txt") && c.delete_on_close())),
         "",
     );
+    // The Cleanup outcome (plan 1b-3c): the OperationEnd of the Cleanups in a
+    // step report FILE_CLEANUP_* in ExtraInformation.
+    // Irps are recycled: each Cleanup pairs with the next OperationEnd of its
+    // Irp (the events are in time order).
+    let outcomes = |step: &str| -> Vec<u64> {
+        let ev = fev(step);
+        ev.iter()
+            .enumerate()
+            .filter_map(|(i, e)| {
+                let RawEvent::FileCleanup(h) = e else { return None };
+                ev[i + 1..].iter().find_map(|x| match x {
+                    RawEvent::FileOpEnd(o) if o.irp == h.irp => Some(o.extra_information),
+                    _ => None,
+                })
+            })
+            .collect()
+    };
+    let has = |v: &[u64], bit: u64| v.iter().any(|x| x & bit != 0);
+    let (deleted, link, stream) = (4, 8, 0x10);
+    let o = outcomes("file_delete");
+    r.check("cleanup_outcome_file_deleted", has(&o, deleted), format!("{o:x?}"));
+    let o = outcomes("file_delete_on_close");
+    r.check("cleanup_outcome_delete_on_close", has(&o, deleted), format!("{o:x?}"));
+    let o = outcomes("file_undelete");
+    r.check("cleanup_outcome_undelete_remains", !o.is_empty() && o.iter().all(|x| *x == 2), format!("{o:x?}"));
+    let sd: Vec<u64> = fev("file_undelete")
+        .iter()
+        .filter_map(|e| if let RawEvent::FileSetDelete(i) = e { Some(i.extra_information) } else { None })
+        .collect();
+    r.check("set_delete_on_set_and_clear", sd == [1, 0], format!("{sd:?}"));
+    let o = outcomes("file_delete_hard_link");
+    r.check("cleanup_outcome_link_deleted", has(&o, link) && !has(&o, deleted), format!("{o:x?}"));
+    let o = outcomes("file_delete_stream");
+    r.check("cleanup_outcome_stream_deleted", has(&o, stream) && !has(&o, deleted), format!("{o:x?}"));
 
     // Registry.
     let rev = fev;
```

- [ ] **Step 3: The agent ignores 18 for now**, and the replay's registry check stops pinning the recording's PID:

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -248,6 +248,7 @@ impl<L: Lookups> Pipeline<L> {
             R::FileClose(x) => self.on_file_close(&h, x),
             R::FileWrite(w) => self.on_file_write(&h, w.file_object),
             R::FileSetInfo(i) => self.on_file_set_info(&h, i),
+            R::FileSetDelete(_) => {} // the delete logic comes in Task 3
             R::FileOpEnd(o) => self.on_file_op_end(&h, o),
             R::FileDeletePath(p) => self.on_file_delete_path(&h, p),
             R::FileRenamePath(p) => self.on_file_rename_path(&h, p),
```

```diff
--- a/crates/atlas-agent/tests/replay.rs
+++ b/crates/atlas-agent/tests/replay.rs
@@ -270,7 +270,9 @@ fn the_ci_recording_produces_the_scenario() {
             _ => None,
         })
         .expect("the test key's Create");
-    assert!(key.starts_with(r"HKU\S-1-5-21-") && key.ends_with(r"\Software\AtlasEtwLive-3268"), "{key}");
+    let (head, pid) = key.rsplit_once('-').unwrap();
+    assert!(key.starts_with(r"HKU\S-1-5-21-") && head.ends_with(r"\Software\AtlasEtwLive"), "{key}");
+    assert!(pid.parse::<u32>().is_ok(), "the actor's PID: {key}");
     let set = out.iter().find_map(|e| match &e.kind {
         EventKind::RegistryValue(v) if v.name == "v" => match &v.action {
             RegistryValueAction::Set { data, data_read_after, data_unavailable, .. } => {
```

- [ ] **Step 4: The CI recording.** `committed_fixtures_agree_with_tdh` requires every parsed kind in the fixture, so it fails until the fixture has 18.
  - **Use the verified recording:** the file recorded by run 37544462603's `etw-live` job from this task's code (artifact `etw-live`, 1,658 lines; SHA-256 of the committed, LF form `9307a39c1d03a92cc205f19c78ab486cf5fd3ec7562c35bc0aca1e2f5a0f1e4b`). It holds only the runner's paths, like the old one.
  - **If the artifact has expired or Step 2 changed:** push the branch, wait for `etw-live`, and download its artifact. Review it as Step 5 says.
```powershell
gh run download 37544462603 -R jakefrenzel/atlas-edr -n etw-live -D $env:TEMP\etw-live
Copy-Item $env:TEMP\etw-live\scenario.jsonl crates\atlas-etw\tests\fixtures\scenario.jsonl
```

- [ ] **Step 5: Review the recording.** Against the old fixture it adds:
  - 11 `SetDelete`s;
  - the undelete's, hard link's and stream's Creates, Cleanups and OperationEnds;
  - 6 more `DeletePath`s.

  Check that it holds no path, name or address outside the runner image's own: the runner user `runneradmin`, `D:\a\atlas-edr`, the `atlas-etw-live-<pid>` directory, and the system's paths and keys.

- [ ] **Step 6: Regenerate the agent's replay snapshot and review it.** Under 1b-3a's delete rule (still in force until Task 3), the undelete gives a false Delete of `u.txt` next to the directory removal's real one. Task 3 removes it.
```powershell
$env:ATLAS_UPDATE_SNAPSHOT = '1'; cargo test -p atlas-agent --test replay; Remove-Item Env:ATLAS_UPDATE_SNAPSHOT
git diff --stat crates/atlas-agent/tests/snapshots
```

- [ ] **Step 7: Check and commit**

```powershell
cargo test -p atlas-etw -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 332 passed, 6 ignored (`atlas-agent` unit tests: 149).
```powershell
git add crates/atlas-etw crates/atlas-agent
git commit -m "feat(etw): Kernel-File 18 SetDelete; live test: Cleanup outcomes; CI recording with it"
```

### Task 3: File deletes from the Cleanup outcome

**Files:**
- Create: `crates/atlas-agent/src/cleanup.rs`
- Modify: `src/lib.rs`, `src/recent.rs`, `src/intake.rs`, `src/counters.rs`, `src/config.rs`, `src/pipeline/mod.rs`, `src/pipeline/file.rs`, `src/pipeline/tests.rs`, `tests/replay.rs`, `tests/snapshots/scenario.jsonl` (regenerated)

- [ ] **Step 1: The outcome values**, the check that a value is one (a query's returned length can share a bit), and the Irp of an event that starts an operation.

`crates/atlas-agent/src/cleanup.rs`:
```rust
//! The outcome of a file Cleanup (sensor spec §5.1; plan 1b-3c, decision D3).
//!
//! A file system reports what a Cleanup removed in the IRP's `Information`,
//! which Kernel-File logs as the `ExtraInformation` of the Cleanup's
//! OperationEnd (24): the `FILE_CLEANUP_*` values of `ntifs.h`. Verified on the
//! host (build 26200) for NTFS, FAT32 and exFAT; an SMB share (`\Device\Mup`)
//! reports `UNKNOWN`. A delete is reported when it happens, from the process
//! doing it, and an undelete (a disposition set, then cleared) reports
//! `FILE_REMAINS`.

use atlas_etw::parse::RawEvent;

/// The file system did not say (SMB shares).
pub const UNKNOWN: u64 = 0;
pub const FILE_REMAINS: u64 = 0x2;
pub const FILE_DELETED: u64 = 0x4;
/// One hard link went; the file remains.
pub const LINK_DELETED: u64 = 0x8;
pub const STREAM_DELETED: u64 = 0x10;
/// Set with one of the above for a POSIX-style delete.
pub const POSIX_STYLE_DELETE: u64 = 0x20;

/// The Cleanup removed a name: the file, a hard link or a stream. Only the
/// values a file system reports count (one of the three, with or without the
/// POSIX flag), so another operation's `Information` (a byte count, a
/// query's length) that happens to share a bit is not taken for one.
pub fn removed(info: u64) -> bool {
    matches!(info & !POSIX_STYLE_DELETE, FILE_DELETED | LINK_DELETED | STREAM_DELETED)
}

/// The Irp of a Kernel-File event that starts an operation (every event with
/// an Irp but Cleanup and OperationEnd). Irps are per thread and reused, so
/// such an event ends whatever the Irp did before.
pub fn op_irp(e: &RawEvent) -> Option<u64> {
    match e {
        RawEvent::FileCreate(c) | RawEvent::FileCreateNew(c) => Some(c.irp),
        RawEvent::FileClose(h) => Some(h.irp),
        RawEvent::FileWrite(w) => Some(w.irp),
        RawEvent::FileSetInfo(i) | RawEvent::FileSetDelete(i) => Some(i.irp),
        RawEvent::FileDeletePath(p) | RawEvent::FileRenamePath(p) => Some(p.irp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_counts_as_removed() {
        for info in [FILE_DELETED, LINK_DELETED, STREAM_DELETED, FILE_DELETED | POSIX_STYLE_DELETE] {
            assert!(removed(info), "{info:#x}");
        }
        // 0x18, 0x38: a query's returned length, seen on these Irps (review R-M4).
        for info in [UNKNOWN, FILE_REMAINS, POSIX_STYLE_DELETE, 0x18, 0x38, 0x14, 0x44, 100] {
            assert!(!removed(info), "{info:#x}");
        }
    }
}
```

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -1,5 +1,6 @@
 //! The Atlas agent (sensor spec §3).
 
+pub mod cleanup;
 pub mod completion;
 pub mod config;
 pub mod counters;
```

- [ ] **Step 2: `RecentMap`**: `Recent` with a value per key, forgotten oldest first, by time or past its cap, with no scans.

```diff
--- a/crates/atlas-agent/src/recent.rs
+++ b/crates/atlas-agent/src/recent.rs
@@ -75,10 +75,84 @@ impl<K: Hash + Eq + Clone> Recent<K> {
     }
 }
 
+/// Like [`Recent`], with a value per key: forgotten oldest first, by time or
+/// past the cap, with no scans.
+pub struct RecentMap<K, V> {
+    map: HashMap<K, (i64, V)>,
+    order: VecDeque<(i64, K)>,
+    cap: usize,
+}
+
+impl<K: Hash + Eq + Clone, V> RecentMap<K, V> {
+    pub fn new(cap: usize) -> Self {
+        RecentMap { map: HashMap::new(), order: VecDeque::new(), cap: cap.max(1) }
+    }
+
+    /// Records `key` at `ts`; a second insert replaces the value and the time.
+    pub fn insert(&mut self, key: K, ts: i64, value: V) {
+        self.map.insert(key.clone(), (ts, value));
+        self.order.push_back((ts, key));
+        while self.order.len() > self.cap {
+            self.pop_front();
+        }
+    }
+
+    pub fn get(&self, key: &K) -> Option<&V> {
+        self.map.get(key).map(|(_, v)| v)
+    }
+
+    /// The value and the time it was recorded.
+    pub fn remove(&mut self, key: &K) -> Option<(i64, V)> {
+        self.map.remove(key)
+    }
+
+    /// Forgets keys recorded before `before`.
+    pub fn expire(&mut self, before: i64) {
+        while self.order.front().is_some_and(|(ts, _)| *ts < before) {
+            self.pop_front();
+        }
+    }
+
+    fn pop_front(&mut self) {
+        let Some((ts, key)) = self.order.pop_front() else { return };
+        if self.map.get(&key).is_some_and(|(t, _)| *t == ts) {
+            self.map.remove(&key);
+        }
+    }
+
+    pub fn len(&self) -> usize {
+        self.map.len()
+    }
+
+    pub fn is_empty(&self) -> bool {
+        self.map.is_empty()
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
 
+    #[test]
+    fn a_recent_map_keeps_the_latest_value_and_forgets_the_oldest() {
+        let mut r = RecentMap::new(3);
+        r.insert(1, 10, "a");
+        r.insert(2, 20, "b");
+        r.insert(1, 30, "c"); // replaced: its first queue entry is stale
+        r.expire(25);
+        assert_eq!((r.get(&1), r.get(&2)), (Some(&"c"), None));
+        for (k, ts) in [(3, 31), (4, 32), (5, 33)] {
+            r.insert(k, ts, "x");
+        }
+        assert_eq!((r.len(), r.get(&1)), (3, None), "the cap drops the oldest");
+        assert_eq!(r.remove(&5), Some((33, "x")));
+        // The queue stays bounded however often one key is replaced.
+        for ts in 40..100 {
+            r.insert(9, ts, "y");
+        }
+        assert!(r.order.len() <= 3);
+    }
+
     #[test]
     fn the_latest_time_wins_and_old_keys_expire() {
         let mut r = Recent::new(100);
```

- [ ] **Step 3: The callback's state** (§3.2): `Cleanups`.
  - It keeps each Cleanup's `Irp` until its OperationEnd, which must come within 1 s. An outcome delivered before its Cleanup (another CPU's buffer) waits for it.
  - Any other operation on the `Irp` ends the wait; a Cleanup left unpaired is counted.
  - It keeps the delete requests outstanding, and their `Irp`s until their OperationEnd: a failed one takes the request back, a successful one is passed on.
  - It passes on a successful OperationEnd for a Cleanup that reports a removed name, or nothing for a requested file. A delete passed on is no longer outstanding.
  - A clear on a delete-on-close handle only clears that handle's flag.

```diff
--- a/crates/atlas-agent/src/intake.rs
+++ b/crates/atlas-agent/src/intake.rs
@@ -3,7 +3,8 @@
 //! a hash-map operation or two per event, never a blocking call.
 //!
 //! - Parse failures are counted (`parse_errors`, `unknown_version`).
-//! - Successful OperationEnds are discarded: only failures matter (§5.5).
+//! - Successful OperationEnds are discarded: only failures matter (§5.5), and
+//!   the Cleanup outcomes that report a delete (`cleanup`; plan 1b-3c).
 //! - Session A keeps the early registry key map and sends fast-path value reads.
 //! - DNS-Client events pass a per-PID token bucket, then go to the user-mode
 //!   queue; everything else goes to the kernel queue. Full queues drop and count.
@@ -12,11 +13,13 @@ use std::collections::{HashMap, HashSet, VecDeque};
 use std::sync::Arc;
 use std::sync::mpsc::{SyncSender, TrySendError};
 
-use atlas_etw::parse::{ParseError, RawEvent};
+use atlas_etw::parse::{FileOpEnd, ParseError, RawEvent};
 
+use crate::cleanup;
 use crate::config::{Config, Ticks};
 use crate::counters::IntakeCounters;
 use crate::input::{Header, Incoming, Session};
+use crate::recent::Recent;
 use crate::services::{EarlyKey, ValueRead};
 
 /// Sends a fast-path read to the reader lane (plan 1b-3b). Must not block.
@@ -39,6 +42,7 @@ pub struct Intake {
     early: Option<EarlyKeys>,
     fast_read: Option<FastRead>,
     self_keys: HashSet<u64>,
+    cleanups: Cleanups,
 }
 
 impl Intake {
@@ -58,6 +62,7 @@ impl Intake {
         Intake {
             kernel: queues.kernel,
             user: queues.user,
+            cleanups: Cleanups::new(ticks, counters.clone()),
             counters,
             dns: Buckets::new(cfg.dns_rate_per_pid, ticks.frequency),
             early,
@@ -79,11 +84,31 @@ impl Intake {
                 return;
             }
         };
+        if let Some(irp) = cleanup::op_irp(&event) {
+            self.cleanups.next_op(irp);
+        }
+        let mut early_outcome = None;
         match &event {
             RawEvent::FileOpEnd(o) if !o.failed() => {
-                IntakeCounters::bump(&self.counters.op_end_discarded);
-                return;
+                if let End::Discard = self.cleanups.end(header, o.irp, o.extra_information) {
+                    IntakeCounters::bump(&self.counters.op_end_discarded);
+                    return;
+                }
             }
+            RawEvent::FileOpEnd(o) => self.cleanups.failed(o.irp),
+            RawEvent::FileCleanup(x) => {
+                early_outcome = self
+                    .cleanups
+                    .cleanup(x.irp, x.file_object, x.file_key, header.ts)
+                    .map(|(h, info)| (h, FileOpEnd { irp: x.irp, extra_information: info, status: 0 }));
+            }
+            RawEvent::FileClose(x) => self.cleanups.close(x.file_object),
+            RawEvent::FileCreate(c) if c.delete_on_close() => self.cleanups.delete_on_close(c.file_object, header.ts),
+            RawEvent::FileDeletePath(p) => self.cleanups.request(p.file_key, p.irp, header.ts),
+            RawEvent::FileSetDelete(i) if i.extra_information != 0 => {
+                self.cleanups.request(i.file_key, i.irp, header.ts)
+            }
+            RawEvent::FileSetDelete(i) => self.cleanups.clear(i.file_key, i.file_object),
             RawEvent::RegCreateKey(o) | RawEvent::RegOpenKey(o) if o.status == 0 => {
                 if let Some(m) = &mut self.early {
                     let evicted = m.open(o.key_object, o.base_object, &o.relative_name.to_string_lossy());
@@ -113,9 +138,16 @@ impl Intake {
             }
             _ => {}
         }
-        let inc = Incoming { header, event };
+        self.send(Incoming { header, event });
+        if let Some((h, o)) = early_outcome {
+            // After its Cleanup in the queue; the ordering stage sorts by time anyway.
+            self.send(Incoming { header: h, event: RawEvent::FileOpEnd(o) });
+        }
+    }
+
+    fn send(&mut self, inc: Incoming) {
         if matches!(inc.event, RawEvent::DnsQuery(_)) {
-            if !self.dns.take(header.pid, header.ts) {
+            if !self.dns.take(inc.header.pid, inc.header.ts) {
                 IntakeCounters::bump(&self.counters.dns_rate_limit_drops);
                 return;
             }
@@ -128,6 +160,167 @@ impl Intake {
     }
 }
 
+/// What the callback keeps to pass on Cleanup outcomes (plan 1b-3c, D3):
+/// - each Cleanup's Irp until its OperationEnd;
+/// - an outcome that arrived before its Cleanup (logged on another CPU, R-M4);
+/// - the files with a delete request outstanding, for the file systems that
+///   report no outcome (SMB), and each request's Irp until its OperationEnd.
+///
+/// Irps are per thread and reused for every operation, so an event that starts
+/// another operation on an Irp ends whatever the Irp did before: a Cleanup still
+/// waiting is counted as unpaired (`file_cleanup_unpaired`), never paired with a
+/// later operation's OperationEnd. Bounded: the Irp maps clear past their cap
+/// (their entries go at once unless events are lost), the requests forget the
+/// oldest.
+pub(crate) struct Cleanups {
+    /// Cleanup Irp → (FileObject, FileKey, QPC).
+    irps: HashMap<u64, (u64, u64, i64)>,
+    /// A successful OperationEnd that reports a removed name and found no
+    /// Cleanup: Irp → its header and outcome.
+    early: HashMap<u64, (Header, u64)>,
+    /// FileKeys with a delete requested (`DeletePath`, `SetDelete` set, or a
+    /// delete-on-close handle's Cleanup) and not cleared or reported since.
+    requested: Recent<u64>,
+    /// A request's Irp → its FileKey, until the request's OperationEnd: a
+    /// failed one takes the request back, a successful one is passed on so the
+    /// pipeline knows the request stood (R-M1).
+    request_irps: HashMap<u64, u64>,
+    /// Handles opened delete-on-close and not closed yet.
+    on_close: Recent<u64>,
+    /// An outcome comes within this many ticks of its Cleanup (1 s). The rules
+    /// that keep a Cleanup from pairing with another operation are the next
+    /// operation and the value check; the window bounds what neither sees
+    /// (an operation whose start is not logged, such as a query).
+    window: i64,
+    counters: Arc<IntakeCounters>,
+}
+
+/// What to do with a successful OperationEnd.
+enum End {
+    Pass,
+    Discard,
+}
+
+const CLEANUP_IRPS_CAP: usize = 4096;
+const DELETE_REQUESTS_CAP: usize = 4096;
+
+impl Cleanups {
+    fn new(ticks: Ticks, counters: Arc<IntakeCounters>) -> Self {
+        Cleanups {
+            irps: HashMap::new(),
+            early: HashMap::new(),
+            requested: Recent::new(DELETE_REQUESTS_CAP),
+            request_irps: HashMap::new(),
+            on_close: Recent::new(DELETE_REQUESTS_CAP),
+            window: ticks.frequency.max(1),
+            counters,
+        }
+    }
+
+    /// Another operation starts on `irp` (any Kernel-File event with an Irp
+    /// but a Cleanup or an OperationEnd).
+    fn next_op(&mut self, irp: u64) {
+        self.early.remove(&irp);
+        self.request_irps.remove(&irp);
+        if self.irps.remove(&irp).is_some() {
+            IntakeCounters::bump(&self.counters.cleanup_unpaired);
+        }
+    }
+
+    /// A Cleanup. Returns the outcome to pass on now, if it arrived first.
+    fn cleanup(&mut self, irp: u64, fo: u64, key: u64, ts: i64) -> Option<(Header, u64)> {
+        let early = self.early.remove(&irp);
+        self.next_op(irp);
+        if self.on_close.get(&fo).is_some() {
+            // The FileKey is first known here: a later Cleanup on another
+            // handle may be the one that deletes.
+            self.requested.insert(key, ts);
+        }
+        if let Some((h, info)) = early
+            && h.ts >= ts
+            && h.ts - ts <= self.window
+        {
+            return self.decide(fo, key, info).then_some((h, info));
+        }
+        if self.irps.len() >= CLEANUP_IRPS_CAP {
+            self.irps.clear();
+        }
+        self.irps.insert(irp, (fo, key, ts));
+        None
+    }
+
+    /// A successful OperationEnd: passed on if it is a Cleanup's and reports a
+    /// delete, or reports nothing for a file whose delete was requested; or if
+    /// it is a delete request's.
+    fn end(&mut self, header: Header, irp: u64, info: u64) -> End {
+        let request = self.request_irps.remove(&irp).is_some();
+        let pass = match self.irps.remove(&irp) {
+            Some((fo, key, ts)) if header.ts >= ts && header.ts - ts <= self.window => self.decide(fo, key, info),
+            Some(_) => {
+                IntakeCounters::bump(&self.counters.cleanup_outcome_late);
+                false
+            }
+            None => {
+                if cleanup::removed(info) {
+                    if self.early.len() >= CLEANUP_IRPS_CAP {
+                        self.early.clear();
+                    }
+                    self.early.insert(irp, (header, info));
+                }
+                false
+            }
+        };
+        if pass || request { End::Pass } else { End::Discard }
+    }
+
+    /// Whether a Cleanup's outcome is passed on. A delete passed on is no
+    /// longer outstanding.
+    fn decide(&mut self, fo: u64, key: u64, info: u64) -> bool {
+        let requested = self.requested.get(&key).is_some() || self.on_close.get(&fo).is_some();
+        let pass = cleanup::removed(info) || (info == cleanup::UNKNOWN && requested);
+        if pass {
+            self.requested.remove(&key);
+        }
+        pass
+    }
+
+    /// A failed OperationEnd: a failed request is taken back.
+    fn failed(&mut self, irp: u64) {
+        self.irps.remove(&irp);
+        self.early.remove(&irp);
+        if let Some(key) = self.request_irps.remove(&irp) {
+            self.requested.remove(&key);
+        }
+    }
+
+    fn close(&mut self, fo: u64) {
+        if !self.on_close.is_empty() {
+            self.on_close.remove(&fo);
+        }
+    }
+
+    fn delete_on_close(&mut self, fo: u64, ts: i64) {
+        self.on_close.insert(fo, ts);
+    }
+
+    fn request(&mut self, key: u64, irp: u64, ts: i64) {
+        if self.request_irps.len() >= CLEANUP_IRPS_CAP {
+            self.request_irps.clear();
+        }
+        self.request_irps.insert(irp, key);
+        self.requested.insert(key, ts);
+    }
+
+    /// A `SetDelete` clear. On a delete-on-close handle it is taken as
+    /// `FileDispositionInformationEx` clearing that handle's flag, and leaves a
+    /// disposition another handle set (R-m3); otherwise it clears the file's.
+    fn clear(&mut self, key: u64, fo: u64) {
+        if self.on_close.remove(&fo).is_none() {
+            self.requested.remove(&key);
+        }
+    }
+}
+
 /// The early key map (§7.5): full names only, in arrival order, no seeding.
 /// A relative open whose base is unknown is simply not cached; a miss costs
 /// only a fall back to the ordered path. Bounded, evicting the oldest.
@@ -238,7 +431,9 @@ impl Buckets {
 #[cfg(test)]
 mod tests {
     use super::*;
-    use atlas_etw::parse::{DnsQuery, FileOpEnd, RegKey, RegOpen, RegSetValue, WStr};
+    use atlas_etw::parse::{
+        DnsQuery, FileCreate, FileHandle, FileOpEnd, FileSetInfo, RegKey, RegOpen, RegSetValue, WStr,
+    };
     use std::sync::Mutex;
     use std::sync::mpsc::{Receiver, sync_channel};
 
@@ -291,6 +486,190 @@ mod tests {
         assert!(krx.try_recv().is_err());
     }
 
+    fn ev(i: &mut Intake, e: RawEvent) {
+        i.on_event(header(1, 0), Ok(e));
+    }
+
+    fn cleanup(irp: u64, fo: u64, file_key: u64) -> RawEvent {
+        RawEvent::FileCleanup(FileHandle { irp, file_object: fo, file_key, issuing_tid: 1 })
+    }
+
+    fn outcome(irp: u64, info: u64) -> RawEvent {
+        RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: info, status: 0 })
+    }
+
+    fn passed_outcomes(krx: &Receiver<Incoming>) -> Vec<(u64, u64)> {
+        krx.try_iter()
+            .filter_map(|inc| match inc.event {
+                RawEvent::FileOpEnd(o) if !o.failed() => Some((o.irp, o.extra_information)),
+                _ => None,
+            })
+            .collect()
+    }
+
+    #[test]
+    fn cleanup_outcomes_pass_when_they_report_a_delete() {
+        let (mut i, krx, _, c, _) = intake(64);
+        // Removed names pass; a file that remains does not, nor an OperationEnd
+        // of another operation that happens to carry the same number.
+        for (irp, info) in [(1, cleanup::FILE_DELETED), (2, cleanup::LINK_DELETED), (3, cleanup::STREAM_DELETED)] {
+            ev(&mut i, cleanup(irp, 0x10 + irp, 0x20 + irp));
+            ev(&mut i, outcome(irp, info));
+        }
+        ev(&mut i, cleanup(4, 0x14, 0x24));
+        ev(&mut i, outcome(4, cleanup::FILE_REMAINS));
+        ev(&mut i, outcome(5, cleanup::FILE_DELETED)); // a write of 4 bytes, say
+        assert_eq!(passed_outcomes(&krx), [(1, 4), (2, 8), (3, 0x10)]);
+        assert_eq!(IntakeCounters::get(&c.op_end_discarded), 2);
+    }
+
+    #[test]
+    fn an_unknown_outcome_passes_only_for_a_requested_delete() {
+        let (mut i, krx, _, _, _) = intake(64);
+        let set = |key, on| {
+            RawEvent::FileSetDelete(FileSetInfo {
+                irp: 0,
+                file_object: 0x10,
+                file_key: key,
+                extra_information: on,
+                issuing_tid: 1,
+                info_class: 13,
+            })
+        };
+        // Asked for: passes.
+        ev(&mut i, set(0x20, 1));
+        ev(&mut i, cleanup(1, 0x10, 0x20));
+        ev(&mut i, outcome(1, cleanup::UNKNOWN));
+        // Asked for, then cleared: does not.
+        ev(&mut i, set(0x21, 1));
+        ev(&mut i, set(0x21, 0));
+        ev(&mut i, cleanup(2, 0x10, 0x21));
+        ev(&mut i, outcome(2, cleanup::UNKNOWN));
+        // Never asked for: does not.
+        ev(&mut i, cleanup(3, 0x11, 0x22));
+        ev(&mut i, outcome(3, cleanup::UNKNOWN));
+        // Asked for, but the request failed (a read-only file): does not, even
+        // once the FileKey is reused.
+        ev(
+            &mut i,
+            RawEvent::FileSetDelete(FileSetInfo {
+                irp: 8,
+                file_object: 0x10,
+                file_key: 0x24,
+                extra_information: 1,
+                issuing_tid: 1,
+                info_class: 13,
+            }),
+        );
+        ev(&mut i, RawEvent::FileOpEnd(FileOpEnd { irp: 8, extra_information: 0, status: 0xC000_0121 }));
+        ev(&mut i, cleanup(6, 0x14, 0x24));
+        ev(&mut i, outcome(6, cleanup::UNKNOWN));
+        // A delete-on-close handle: its own Cleanup, and another handle's after it.
+        ev(
+            &mut i,
+            RawEvent::FileCreate(FileCreate {
+                irp: 9,
+                file_object: 0x12,
+                issuing_tid: 1,
+                create_options: FileCreate::DELETE_ON_CLOSE,
+                create_attributes: 0,
+                share_access: 7,
+                file_name: WStr::default(),
+            }),
+        );
+        ev(&mut i, cleanup(4, 0x12, 0x23));
+        ev(&mut i, outcome(4, cleanup::UNKNOWN));
+        // That outcome was the delete: the file's later Cleanups are not (R-m2).
+        ev(&mut i, cleanup(5, 0x13, 0x23));
+        ev(&mut i, outcome(5, cleanup::UNKNOWN));
+        assert_eq!(passed_outcomes(&krx), [(1, 0), (4, 0)]);
+    }
+
+    fn ev_at(i: &mut Intake, ts: i64, e: RawEvent) {
+        i.on_event(header(1, ts), Ok(e));
+    }
+
+    /// The thread moved to another CPU between the Cleanup and its
+    /// OperationEnd, and the OperationEnd's buffer came first (review R-M4).
+    #[test]
+    fn an_outcome_delivered_before_its_cleanup_is_paired() {
+        let (mut i, krx, _, _, _) = intake(64);
+        ev(&mut i, outcome(1, cleanup::FILE_DELETED));
+        ev(&mut i, cleanup(1, 0x10, 0x20));
+        // The thread's next operation on the same Irp: a 100-byte write.
+        ev(&mut i, outcome(1, 100));
+        assert_eq!(passed_outcomes(&krx), [(1, 4)], "the delete passes, the write does not");
+    }
+
+    /// A Cleanup whose outcome never came is not paired with a later operation
+    /// on its Irp, nor with an OperationEnd past the window; both are counted.
+    #[test]
+    fn an_unpaired_cleanup_is_counted_never_paired_later() {
+        let (mut i, krx, _, c, _) = intake(64);
+        ev_at(&mut i, 0, cleanup(1, 0x10, 0x20));
+        ev_at(&mut i, 1, RawEvent::FileClose(FileHandle { irp: 1, file_object: 0x10, file_key: 0x20, issuing_tid: 1 }));
+        ev_at(&mut i, 2, outcome(1, cleanup::FILE_DELETED)); // the Close's own: not a Cleanup's
+        ev_at(&mut i, 3, cleanup(2, 0x11, 0x21));
+        ev_at(&mut i, 1_500, outcome(2, cleanup::FILE_DELETED)); // 1.5 s later; the window is 1 s
+        assert!(passed_outcomes(&krx).is_empty());
+        assert_eq!(IntakeCounters::get(&c.cleanup_unpaired), 1);
+        assert_eq!(IntakeCounters::get(&c.cleanup_outcome_late), 1);
+    }
+
+    /// A delete request's own OperationEnd is passed on, so the pipeline knows
+    /// the request stood (review R-M1).
+    #[test]
+    fn a_requests_operation_end_is_passed_on() {
+        let (mut i, krx, _, _, _) = intake(64);
+        ev(
+            &mut i,
+            RawEvent::FileSetDelete(FileSetInfo {
+                irp: 7,
+                file_object: 0x10,
+                file_key: 0x20,
+                extra_information: 1,
+                issuing_tid: 1,
+                info_class: 13,
+            }),
+        );
+        ev(&mut i, outcome(7, 0));
+        assert_eq!(passed_outcomes(&krx), [(7, 0)]);
+    }
+
+    /// Clearing a delete-on-close handle's flag leaves another handle's
+    /// request (review R-m3): an SMB Cleanup still passes.
+    #[test]
+    fn clearing_a_handles_delete_on_close_keeps_another_handles_request() {
+        let (mut i, krx, _, _, _) = intake(64);
+        let set = |fo, on| {
+            RawEvent::FileSetDelete(FileSetInfo {
+                irp: 9,
+                file_object: fo,
+                file_key: 0x20,
+                extra_information: on,
+                issuing_tid: 1,
+                info_class: 64,
+            })
+        };
+        ev(&mut i, set(0x10, 1));
+        ev(
+            &mut i,
+            RawEvent::FileCreate(FileCreate {
+                irp: 8,
+                file_object: 0x12,
+                issuing_tid: 1,
+                create_options: FileCreate::DELETE_ON_CLOSE,
+                create_attributes: 0,
+                share_access: 7,
+                file_name: WStr::default(),
+            }),
+        );
+        ev(&mut i, set(0x12, 0));
+        ev(&mut i, cleanup(3, 0x14, 0x20));
+        ev(&mut i, outcome(3, cleanup::UNKNOWN));
+        assert!(passed_outcomes(&krx).contains(&(3, 0)));
+    }
+
     #[test]
     fn parse_failures_are_counted_by_kind() {
         let (mut i, _, _, c, _) = intake(8);
```

- [ ] **Step 4: The counter and the settings' comments.**

```diff
--- a/crates/atlas-agent/src/counters.rs
+++ b/crates/atlas-agent/src/counters.rs
@@ -54,6 +54,8 @@ pub struct Counters {
     pub file_object_replaced: u64,
     pub enrichment_misses: u64,
     pub enrichment_errors: u64,
+    /// Deletes reported from the request alone: the Cleanup's outcome was unknown (plan 1b-3c).
+    pub file_delete_outcome_unknown: u64,
     // Housekeeping.
     pub process_cache_evictions: u64,
     pub file_map_evictions: u64,
@@ -84,6 +86,12 @@ pub struct IntakeCounters {
     pub op_end_discarded: AtomicU64,
     /// Value reads sent on the fast path (§7.5).
     pub fast_reads: AtomicU64,
+    /// Cleanups whose outcome could not be paired with them, because another
+    /// operation on their Irp came first. With `cleanup_outcome_late` (an
+    /// outcome past the window), `quality.file_cleanup_unpaired` (plan 1b-3c,
+    /// review R-M4).
+    pub cleanup_unpaired: AtomicU64,
+    pub cleanup_outcome_late: AtomicU64,
 }
 
 impl IntakeCounters {
```

```diff
--- a/crates/atlas-agent/src/config.rs
+++ b/crates/atlas-agent/src/config.rs
@@ -9,7 +9,7 @@ use std::time::Duration;
 pub struct Config {
     /// Ordering stage: how long an event is held before release (§3.2).
     pub hold: Duration,
-    /// Failure-confirm window for Create, DeletePath and RenamePath (§5.5).
+    /// Failure-confirm window for Create and RenamePath (§5.5).
     pub confirm_window: Duration,
     /// Completion deadlines (§3.2).
     pub enrich_deadline: Duration,
@@ -46,9 +46,10 @@ pub struct Config {
     pub pending_cap: usize,
     /// Registry value reads after the event (§7.5).
     pub registry_value_reads: bool,
-    /// Failure confirmation for Create, DeletePath and RenamePath (§5.5). Off
-    /// (the second §13 fallback, with the OP_END keyword disabled), they are
-    /// emitted at once.
+    /// OperationEnds: failure confirmation for Create and RenamePath, and the
+    /// Cleanup outcomes that report deletes (§5.1, §5.5). Off (the second §13
+    /// fallback, with the OP_END keyword disabled), Create and RenamePath are
+    /// emitted at once and a DeletePath is taken as the Delete.
     pub file_op_end: bool,
     /// Seed the key and file maps from the handle table at start (§7.4).
     pub seed_on_start: bool,
```

- [ ] **Step 5: The pipeline.**
  - **26 and 18-set** record the requester and 26's path, taken back if the request fails.
  - **18-clear** forgets it; on a delete-on-close handle it only clears that handle's flag.
  - **A delete-on-close handle's Cleanup** records its opener as the requester.
  - **A Cleanup's outcome** that reports a removed name emits the Delete, with the request's path when there is one. A POSIX double report is one (F4): the second matched by `FileKey` and a handle opened before the first.
  - **Pairing:** any other operation on the `Irp` ends a Cleanup's wait; an outcome processed before its late Cleanup waits for it.
  - **Unknown outcomes:** a requested file's Cleanup is the Delete, counted.
  - **With `file.op_end` off,** the request is the Delete, as before.

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -237,6 +237,9 @@ impl<L: Lookups> Pipeline<L> {
     fn process(&mut self, inc: Incoming) {
         use atlas_etw::parse::RawEvent as R;
         let Incoming { header: h, event } = inc;
+        if let Some(irp) = crate::cleanup::op_irp(&event) {
+            self.file_next_op(irp);
+        }
         match event {
             R::ProcessStart(s) => self.on_process_start(&h, s),
             R::ProcessStop(s) => self.on_process_stop(&h, s),
@@ -248,7 +251,7 @@ impl<L: Lookups> Pipeline<L> {
             R::FileClose(x) => self.on_file_close(&h, x),
             R::FileWrite(w) => self.on_file_write(&h, w.file_object),
             R::FileSetInfo(i) => self.on_file_set_info(&h, i),
-            R::FileSetDelete(_) => {} // the delete logic comes in Task 3
+            R::FileSetDelete(i) => self.on_file_set_delete(&h, i),
             R::FileOpEnd(o) => self.on_file_op_end(&h, o),
             R::FileDeletePath(p) => self.on_file_delete_path(&h, p),
             R::FileRenamePath(p) => self.on_file_rename_path(&h, p),
```

```diff
--- a/crates/atlas-agent/src/pipeline/file.rs
+++ b/crates/atlas-agent/src/pipeline/file.rs
@@ -1,6 +1,13 @@
 //! Files (sensor spec §5.5, §7.1, §7.2): the FileObject map, the failure-confirm
-//! window, Update coalescing, delete-on-close, renames and watchlist Opens.
+//! window, Update coalescing, deletes, renames and watchlist Opens.
 //! Every emitted path with 8.3 components is expanded first (`expand`).
+//!
+//! A Delete comes from the Cleanup that removed the name, as the file system
+//! reports it (`cleanup`; plan 1b-3c, D3), so an undelete gives none. Its actor
+//! is whoever asked for the delete (`DeletePath`, `SetDelete`, a delete-on-close
+//! open), else the process whose Cleanup removed it. Where the file system
+//! reports no outcome (SMB), a Cleanup of a file with a delete outstanding is
+//! taken as the delete (`file_delete_outcome_unknown`).
 
 use std::collections::{HashMap, VecDeque};
 
@@ -10,18 +17,21 @@ use atlas_schema::{Event, EventKind, File, ProcessRef, ProcessUid};
 
 use super::Pipeline;
 use super::expand::{Expansion, Slot};
+use crate::cleanup;
 use crate::completion::{PendingId, Reason, Wait};
 use crate::counters::Class;
 use crate::input::Header;
 use crate::paths::has_short_name;
 use crate::process::{empty_file, file_name};
-use crate::recent::Recent;
+use crate::recent::{Recent, RecentMap};
 use crate::services::{HandleKind, Lookups, Request};
 
 /// Caps of the Irp and coalescing sets (each also expires by time).
 const FAILED_CAP: usize = 1 << 16;
 const CONFIRMED_CAP: usize = 1 << 18;
 const OPENED_CAP: usize = 1 << 16;
+const CLEANUPS_CAP: usize = 1 << 16;
+const REQUESTS_CAP: usize = 1 << 14;
 
 /// A watchlist Open's coalescing key: (actor, logged path lowercased).
 type OpenKey = (ProcessUid, String);
@@ -37,7 +47,8 @@ pub(crate) struct FileEntry {
     pub(crate) nt: Option<String>,
     /// Its long form, once an expansion of `nt` came back.
     pub(crate) expanded: Option<String>,
-    /// Who opened the handle: the actor of its Update and delete-on-close Delete.
+    /// Who opened the handle: the actor of its Update, and of a Delete it asked
+    /// for by opening delete-on-close.
     pub(crate) opener: Option<ProcessRef>,
     written: bool,
     delete_on_close: bool,
@@ -66,10 +77,6 @@ enum Held {
         open: Option<PendingId>,
         coalesce: Option<(OpenKey, i64)>,
     },
-    /// The event's id, unless it went out at once (`file.op_end` off).
-    Delete {
-        id: Option<PendingId>,
-    },
     Rename {
         id: Option<PendingId>,
         fo: u64,
@@ -98,6 +105,30 @@ pub struct Files {
     opened: Recent<OpenKey>,
     /// Pending id → the handle it waits on, to forget it when it leaves.
     waiter_fo: HashMap<PendingId, u64>,
+    /// Cleanups whose outcome may follow: Irp → (FileObject, FileKey). The next
+    /// operation on the Irp ends the wait (Irps are per thread and reused).
+    cleanups: RecentMap<u64, (u64, u64)>,
+    /// An outcome processed before its Cleanup (a late Cleanup): Irp → (its
+    /// header, outcome).
+    early_outcomes: RecentMap<u64, (Header, u64)>,
+    /// Deletes asked for and not cleared or reported: FileKey → who asked.
+    requests: RecentMap<u64, Requester>,
+    /// A request's Irp → its FileKey, until the request's OperationEnd: the
+    /// callback passes on both a failed one (the request is taken back) and a
+    /// successful one (it stands, review R-M1).
+    request_irps: RecentMap<u64, u64>,
+    /// POSIX-style deletes already reported: (FileKey, kind) → when.
+    posix_deleted: RecentMap<(u64, u64), i64>,
+}
+
+/// Who asked for a delete (§5.3: the actor of the Delete).
+#[derive(Debug, Clone)]
+enum Requester {
+    /// A `DeletePath` (with the path it names) or a `SetDelete`: resolved when
+    /// the delete happens.
+    Event(Header, Option<String>),
+    /// The opener of a delete-on-close handle.
+    Opener(ProcessRef),
 }
 
 impl Files {
@@ -115,11 +146,16 @@ impl Files {
             confirmed: Recent::new(CONFIRMED_CAP),
             opened: Recent::new(OPENED_CAP),
             waiter_fo: HashMap::new(),
+            cleanups: RecentMap::new(CLEANUPS_CAP),
+            requests: RecentMap::new(REQUESTS_CAP),
+            early_outcomes: RecentMap::new(REQUESTS_CAP),
+            request_irps: RecentMap::new(REQUESTS_CAP),
+            posix_deleted: RecentMap::new(REQUESTS_CAP),
         }
     }
 
     #[cfg(test)]
-    pub(super) fn sizes(&self) -> [(&'static str, usize); 6] {
+    pub(super) fn sizes(&self) -> [(&'static str, usize); 7] {
         [
             ("file waiters", self.map.values().map(|e| e.waiting.len()).sum()),
             ("file waiter index", self.waiter_fo.len()),
@@ -127,6 +163,7 @@ impl Files {
             ("failed irps", self.failed.len()),
             ("confirmed irps", self.confirmed.len()),
             ("watchlist opens", self.opened.len()),
+            ("cleanups awaiting an outcome", self.cleanups.len() + self.early_outcomes.len() + self.request_irps.len()),
         ]
     }
 
@@ -235,7 +272,7 @@ impl<L: Lookups> Pipeline<L> {
                     self.files.opened.remove(&key);
                 }
             }
-            Held::Delete { id } | Held::Rename { id, .. } => {
+            Held::Rename { id, .. } => {
                 if let Some(id) = id {
                     self.completion.cancel(id);
                 }
@@ -372,15 +409,95 @@ impl<L: Lookups> Pipeline<L> {
         }
     }
 
+    /// A delete asked for. It happens, if at all, at a Cleanup (`on_cleanup_outcome`).
+    /// Without OperationEnds (`file.op_end` off) there is no outcome, and the
+    /// request is reported as the delete (an undelete then gives one too).
     pub(super) fn on_file_delete_path(&mut self, h: &Header, p: FilePath) {
+        if self.cfg.file_op_end {
+            self.ask_delete(h, p.file_key, p.irp, Some(p.file_path.to_string_lossy()));
+            return;
+        }
         let Some(actor) = self.actor_sync(h, Class::File) else { return };
         let nt = p.file_path.to_string_lossy();
         self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
         let (file, expand) = self.emit_file(&nt, None);
         let ev = self.file_event(h.ts, actor, file, FileAction::Delete);
-        let waits = self.confirm_waits();
-        let id = self.push_with(ev, waits, expansion(expand, Expansion::file(None)));
-        self.hold(h.ts, p.irp, Held::Delete { id });
+        self.push_with(ev, vec![], expansion(expand, Expansion::file(None)));
+    }
+
+    /// Records who asked for a delete. A failed request is taken back when its
+    /// OperationEnd comes, so a FileKey later reused by another file does not
+    /// inherit it.
+    fn ask_delete(&mut self, h: &Header, key: u64, irp: u64, path: Option<String>) {
+        // A `SetDelete` comes before its `DeletePath`, which names the path.
+        let path = path.or_else(|| match self.files.requests.get(&key) {
+            Some(Requester::Event(_, p)) => p.clone(),
+            _ => None,
+        });
+        self.files.requests.insert(key, h.ts, Requester::Event(*h, path));
+        self.files.request_irps.insert(irp, h.ts, key);
+    }
+
+    /// A delete disposition set (`ExtraInformation` 1) or cleared (0), on any
+    /// handle for the file. On a delete-on-close handle a clear is taken as
+    /// `FileDispositionInformationEx` clearing that handle's flag, which leaves
+    /// a disposition another handle set (review R-m3).
+    pub(super) fn on_file_set_delete(&mut self, h: &Header, i: FileSetInfo) {
+        if i.extra_information != 0 {
+            self.ask_delete(h, i.file_key, i.irp, None);
+            return;
+        }
+        match self.files.map.get_mut(&i.file_object) {
+            Some(e) if e.delete_on_close => e.delete_on_close = false,
+            _ => {
+                self.files.requests.remove(&i.file_key);
+            }
+        }
+    }
+
+    /// A Cleanup's outcome (`cleanup`), which the callback passes on only when
+    /// it reports a removed name, or nothing for a file with a delete asked for.
+    fn on_cleanup_outcome(&mut self, h: &Header, fo: u64, key: u64, info: u64) {
+        let opener = self.files.map.get(&fo).filter(|e| e.delete_on_close).and_then(|e| e.opener.clone());
+        let requester = self.files.requests.get(&key).cloned().or(opener.map(Requester::Opener));
+        if !cleanup::removed(info) {
+            if info != cleanup::UNKNOWN || requester.is_none() {
+                return;
+            }
+            self.counters.file_delete_outcome_unknown += 1;
+        }
+        self.files.requests.remove(&key);
+        // A POSIX-style delete while another handle is open is reported twice:
+        // the name goes at the deleter's Cleanup (`POSIX_STYLE_DELETE`), the
+        // file at the last handle's (without it). Report it once. Only a handle
+        // opened before the POSIX delete can still reach the deleted file, so a
+        // FileKey reused by a new file is never mistaken for it (review R-M3).
+        let kind = info & !cleanup::POSIX_STYLE_DELETE;
+        if info & cleanup::POSIX_STYLE_DELETE != 0 {
+            self.files.posix_deleted.insert((key, kind), h.ts, h.ts);
+        } else if let Some(&at) = self.files.posix_deleted.get(&(key, kind))
+            && self.files.map.get(&fo).is_none_or(|e| e.since < at)
+        {
+            self.files.posix_deleted.remove(&(key, kind));
+            return;
+        }
+        let (actor, path) = match requester {
+            Some(Requester::Opener(a)) => (Some(a), None),
+            Some(Requester::Event(rh, path)) => (self.actor_sync(&rh, Class::File), path),
+            None => (self.actor_sync(h, Class::File), None),
+        };
+        let Some(actor) = actor else { return };
+        match path {
+            // The request's own path: current even after a rename, and known
+            // for a handle the agent never saw opened (review R-M2).
+            Some(nt) => {
+                self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
+                let (file, expand) = self.emit_file(&nt, None);
+                let ev = self.file_event(h.ts, actor, file, FileAction::Delete);
+                self.push_with(ev, vec![], expansion(expand, Expansion::file(None)));
+            }
+            None => self.file_action_on(h.ts, fo, actor, FileAction::Delete, Fill::File, true),
+        }
     }
 
     pub(super) fn on_file_rename_path(&mut self, h: &Header, p: FilePath) {
@@ -414,7 +531,23 @@ impl<L: Lookups> Pipeline<L> {
 
     pub(super) fn on_file_op_end(&mut self, h: &Header, o: FileOpEnd) {
         if !o.failed() {
-            return; // the callback discards these (§3.2); replay may not
+            // The callback passes on Cleanup outcomes and delete requests'
+            // OperationEnds (§3.2); replay may pass more.
+            self.files.request_irps.remove(&o.irp); // the request stands
+            match self.files.cleanups.remove(&o.irp) {
+                Some((_, (fo, key))) => self.on_cleanup_outcome(h, fo, key, o.extra_information),
+                // Its Cleanup may still come, late (review R-m1).
+                None if cleanup::removed(o.extra_information) || o.extra_information == cleanup::UNKNOWN => {
+                    self.files.early_outcomes.insert(o.irp, h.ts, (*h, o.extra_information));
+                }
+                None => {}
+            }
+            return;
+        }
+        self.files.cleanups.remove(&o.irp);
+        self.files.early_outcomes.remove(&o.irp);
+        if let Some((_, key)) = self.files.request_irps.remove(&o.irp) {
+            self.files.requests.remove(&key);
         }
         // The most recent held operation with this Irp (Irps are recycled).
         let base = self.files.held_base;
@@ -433,12 +566,41 @@ impl<L: Lookups> Pipeline<L> {
         }
     }
 
+    /// Another operation starts on `irp`: whatever the Irp did before is over
+    /// (Irps are per thread and reused). A Cleanup still waiting gets no outcome.
+    pub(super) fn file_next_op(&mut self, irp: u64) {
+        self.files.cleanups.remove(&irp);
+        self.files.early_outcomes.remove(&irp);
+        self.files.request_irps.remove(&irp);
+    }
+
     pub(super) fn on_file_cleanup(&mut self, h: &Header, x: FileHandle) {
+        let op_end = self.cfg.file_op_end;
+        // A late Cleanup whose outcome was processed first.
+        let early = self.files.early_outcomes.remove(&x.irp).map(|(_, v)| v).filter(|(oh, _)| oh.ts >= h.ts);
+        self.file_next_op(x.irp);
+        if op_end && early.is_none() {
+            self.files.cleanups.insert(x.irp, h.ts, (x.file_object, x.file_key));
+        }
+        self.cleanup_handle(h, &x);
+        if let Some((oh, info)) = early {
+            self.on_cleanup_outcome(&oh, x.file_object, x.file_key, info);
+        }
+    }
+
+    fn cleanup_handle(&mut self, h: &Header, x: &FileHandle) {
+        let op_end = self.cfg.file_op_end;
         let Some(e) = self.files.touch(x.file_object) else { return };
         let update = e.written && !e.cleaned;
-        let delete = e.delete_on_close;
+        // With OperationEnds the outcome reports the delete, possibly on another
+        // handle's Cleanup: the FileKey remembers who asked.
+        let delete = e.delete_on_close && !op_end;
+        let asked = if op_end && e.delete_on_close { e.opener.clone() } else { None };
         e.cleaned = true;
         let opener = e.opener.clone();
+        if let Some(a) = asked {
+            self.files.requests.insert(x.file_key, h.ts, Requester::Opener(a));
+        }
         for action in [update.then_some(FileAction::Update), delete.then_some(FileAction::Delete)].into_iter().flatten()
         {
             // The actor is the handle's opener, never the Cleanup's header (§7.1).
@@ -468,7 +630,7 @@ impl<L: Lookups> Pipeline<L> {
     /// An operation stands (§5.5).
     fn confirm(&mut self, held: Held) {
         match held {
-            Held::Create { open, .. } | Held::Delete { id: open } => {
+            Held::Create { open, .. } => {
                 if let Some(id) = open {
                     self.completion.resolve(id, Reason::Confirm);
                 }
@@ -504,6 +666,12 @@ impl<L: Lookups> Pipeline<L> {
             }
         }
         self.files.failed.expire(stream.saturating_sub(window));
+        // A Cleanup's OperationEnd follows it at once: a window is plenty.
+        self.files.cleanups.expire(stream.saturating_sub(window));
+        // Kept longer: a slow request's OperationEnd, and a Cleanup that comes
+        // late, are recognised for 40 windows (10 s), like late failures.
+        self.files.request_irps.expire(stream.saturating_sub(window.saturating_mul(40)));
+        self.files.early_outcomes.expire(stream.saturating_sub(window.saturating_mul(40)));
         self.files.confirmed.expire(stream.saturating_sub(window.saturating_mul(40)));
         self.files.opened.expire(stream.saturating_sub(self.ticks.of(self.cfg.watchlist_coalesce)));
     }
```

- [ ] **Step 6: The scenarios.** The old delete tests now go through the outcome. New:
  - an undelete, on a local volume and on SMB;
  - the requester as actor across processes;
  - a pre-agent handle's delete;
  - links and streams;
  - POSIX double reports with a reused `FileKey`;
  - the unknown-outcome fallback;
  - a failed request not inherited by a reused `FileKey`;
  - the review's cases (Review Log): shared `Irp`s, renames, unknown handles, stale POSIX entries, 8.3 handles, delivery order, a delete-on-close clear.

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -215,6 +215,31 @@ fn op_end(irp: u64, status: u32) -> RawEvent {
     RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: 0, status })
 }
 
+/// A Cleanup with its Irp and FileKey, for the outcome that follows.
+fn cleanup(irp: u64, fo: u64, file_key: u64) -> RawEvent {
+    RawEvent::FileCleanup(FileHandle { irp, file_object: fo, file_key, issuing_tid: 1 })
+}
+
+/// A Cleanup's successful OperationEnd reporting `FILE_CLEANUP_*` (`info`).
+fn outcome(irp: u64, info: u64) -> RawEvent {
+    RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: info, status: 0 })
+}
+
+fn set_delete(fo: u64, file_key: u64, delete: bool) -> RawEvent {
+    RawEvent::FileSetDelete(FileSetInfo {
+        irp: 0,
+        file_object: fo,
+        file_key,
+        extra_information: u64::from(delete),
+        issuing_tid: 1,
+        info_class: 13,
+    })
+}
+
+fn delete_path(irp: u64, fo: u64, file_key: u64, path: &str) -> RawEvent {
+    RawEvent::FileDeletePath(FilePath { file_key, ..path_event(irp, fo, path) })
+}
+
 fn path_event(irp: u64, fo: u64, path: &str) -> FilePath {
     FilePath {
         irp,
@@ -562,41 +587,200 @@ fn one_update_per_written_handle_with_the_opener_as_actor() {
     valid(&out);
 }
 
+/// The renames' results, the only part a rename of an unknown handle carries.
+fn rename_results(events: &[Event]) -> Vec<String> {
+    events
+        .iter()
+        .filter_map(|e| match &e.kind {
+            EventKind::File(f) => match &f.action {
+                FileAction::Rename { file_result } => Some(file_result.path.clone()),
+                _ => None,
+            },
+            _ => None,
+        })
+        .collect()
+}
+
 #[test]
-fn delete_on_close_emits_a_delete_at_cleanup() {
+fn delete_on_close_is_a_delete_when_the_cleanup_reports_it() {
     let mut t = T::new();
     running(&mut t, 100, 10, EXPLORER);
     let f = r"\Device\HarddiskVolume3\tmp\d.txt";
     t.ev(5, 100, Some(key(10)), create(1, 0xA, f, FileCreate::DELETE_ON_CLOSE));
-    t.ev(6, 100, Some(key(10)), RawEvent::FileCleanup(handle(0xA)));
+    t.ev(6, 100, Some(key(10)), cleanup(2, 0xA, 0x50));
+    t.ev(6, 100, Some(key(10)), outcome(2, crate::cleanup::FILE_DELETED));
+    // Cleared through FileDispositionInformationEx before the Cleanup: no outcome, no Delete.
+    t.ev(7, 100, Some(key(10)), create(3, 0xB, r"\Device\HarddiskVolume3\tmp\e.txt", FileCreate::DELETE_ON_CLOSE));
+    t.ev(8, 100, Some(key(10)), set_delete(0xB, 0x51, false));
+    t.ev(9, 100, Some(key(10)), cleanup(4, 0xB, 0x51));
+    t.ev(9, 100, Some(key(10)), outcome(4, crate::cleanup::FILE_REMAINS));
     assert_eq!(files(&t.settle()), [("delete", r"C:\tmp\d.txt".into(), 100)]);
 }
 
+#[test]
+fn an_undelete_gives_no_delete() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, r"\Device\HarddiskVolume3\u.txt", 0));
+    t.ev(6, 100, Some(key(10)), set_delete(0xA, 0x50, true));
+    t.ev(6, 100, Some(key(10)), delete_path(2, 0xA, 0x50, r"\Device\HarddiskVolume3\u.txt"));
+    t.ev(7, 100, Some(key(10)), set_delete(0xA, 0x50, false));
+    t.ev(8, 100, Some(key(10)), cleanup(3, 0xA, 0x50));
+    t.ev(8, 100, Some(key(10)), outcome(3, crate::cleanup::FILE_REMAINS));
+    // On SMB the outcome is unknown: a cleared request is not reported either.
+    t.ev(9, 100, Some(key(10)), create(4, 0xB, r"\Device\Mup\srv\share\u.txt", 0));
+    t.ev(10, 100, Some(key(10)), delete_path(5, 0xB, 0x60, r"\Device\Mup\srv\share\u.txt"));
+    t.ev(11, 100, Some(key(10)), set_delete(0xB, 0x60, false));
+    t.ev(12, 100, Some(key(10)), cleanup(6, 0xB, 0x60));
+    t.ev(12, 100, Some(key(10)), outcome(6, crate::cleanup::UNKNOWN));
+    assert!(files(&t.settle()).is_empty());
+    assert_eq!(t.p.counters().file_delete_outcome_unknown, 0);
+}
+
+#[test]
+fn a_delete_comes_from_the_cleanup_that_removed_the_name_with_the_requester_as_actor() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    running(&mut t, 200, 20, CMD);
+    let f = r"\Device\HarddiskVolume3\shared.txt";
+    // 200 holds the file open; 100 asks for the delete and closes first.
+    t.ev(5, 200, Some(key(20)), create(1, 0xB, f, 0));
+    t.ev(6, 100, Some(key(10)), create(2, 0xA, f, 0));
+    t.ev(7, 100, Some(key(10)), set_delete(0xA, 0x50, true));
+    t.ev(7, 100, Some(key(10)), delete_path(3, 0xA, 0x50, f));
+    t.ev(8, 100, Some(key(10)), cleanup(4, 0xA, 0x50));
+    t.ev(8, 100, Some(key(10)), outcome(4, crate::cleanup::FILE_REMAINS));
+    t.at(2_000);
+    assert!(files(&t.out).is_empty(), "not deleted yet");
+    // 200's Cleanup is the last: the name goes then.
+    t.ev(3_000, 200, Some(key(20)), cleanup(5, 0xB, 0x50));
+    t.ev(3_000, 200, Some(key(20)), outcome(5, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    assert_eq!(files(&out), [("delete", r"C:\shared.txt".into(), 100)]);
+    assert!(
+        t.requests().iter().any(|r| matches!(r, Request::InvalidateHash { nt_path } if nt_path == f)),
+        "the hash cache forgets the file"
+    );
+    // A delete nobody was seen asking for (a handle from before the agent):
+    // the process whose Cleanup removed the name.
+    t.ev(4_000, 200, Some(key(20)), create(6, 0xC, r"\Device\HarddiskVolume3\other.txt", 0));
+    t.ev(4_001, 200, Some(key(20)), cleanup(7, 0xC, 0x70));
+    t.ev(4_001, 200, Some(key(20)), outcome(7, crate::cleanup::FILE_DELETED | crate::cleanup::POSIX_STYLE_DELETE));
+    assert_eq!(files(&t.settle()), [("delete", r"C:\other.txt".into(), 200)]);
+    valid(&out);
+}
+
+#[test]
+fn a_posix_delete_reported_twice_is_one_delete() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let d = r"\Device\HarddiskVolume3\dir";
+    let posix = crate::cleanup::FILE_DELETED | crate::cleanup::POSIX_STYLE_DELETE;
+    let deleted = crate::cleanup::FILE_DELETED;
+    // remove_dir_all: an enumeration handle stays open while another deletes.
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, d, 0));
+    t.ev(6, 100, Some(key(10)), create(2, 0xB, d, 0));
+    t.ev(7, 100, Some(key(10)), delete_path(3, 0xB, 0x50, d));
+    t.ev(8, 100, Some(key(10)), cleanup(4, 0xB, 0x50));
+    t.ev(8, 100, Some(key(10)), outcome(4, posix));
+    t.ev(9, 100, Some(key(10)), cleanup(5, 0xA, 0x50));
+    t.ev(9, 100, Some(key(10)), outcome(5, deleted));
+    // The FileKey reused by another file, deleted the classic way: its own Delete.
+    t.ev(10, 100, Some(key(10)), create(6, 0xC, r"\Device\HarddiskVolume3\new.txt", 0));
+    t.ev(11, 100, Some(key(10)), cleanup(7, 0xC, 0x50));
+    t.ev(11, 100, Some(key(10)), outcome(7, deleted));
+    // A POSIX delete with no second report (a mapped file: no other handle),
+    // then its FileKey reused by another file: that file's delete stands.
+    t.ev(12, 100, Some(key(10)), create(8, 0xE, r"\Device\HarddiskVolume3\mapped.txt", 0));
+    t.ev(13, 100, Some(key(10)), cleanup(9, 0xE, 0x60));
+    t.ev(13, 100, Some(key(10)), outcome(9, posix));
+    t.ev(14, 100, Some(key(10)), create(10, 0xF, r"\Device\HarddiskVolume3\later.txt", 0));
+    t.ev(15, 100, Some(key(10)), cleanup(11, 0xF, 0x60));
+    t.ev(15, 100, Some(key(10)), outcome(11, deleted));
+    let out = t.settle();
+    let got: Vec<_> = files(&out).into_iter().map(|(a, p, _)| (a, p)).collect();
+    assert_eq!(
+        got,
+        [
+            ("delete", r"C:\dir".into()),
+            ("delete", r"C:\new.txt".into()),
+            ("delete", r"C:\mapped.txt".into()),
+            ("delete", r"C:\later.txt".into())
+        ]
+    );
+}
+
+#[test]
+fn links_and_streams_are_deletes_of_their_own_name() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let link = r"\Device\HarddiskVolume3\link.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, link, 0));
+    t.ev(6, 100, Some(key(10)), delete_path(2, 0xA, 0x50, link));
+    t.ev(7, 100, Some(key(10)), cleanup(3, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), outcome(3, crate::cleanup::LINK_DELETED));
+    let stream = r"\Device\HarddiskVolume3\s.txt:x";
+    t.ev(8, 100, Some(key(10)), create(4, 0xB, stream, 0));
+    t.ev(9, 100, Some(key(10)), delete_path(5, 0xB, 0x60, stream));
+    t.ev(10, 100, Some(key(10)), cleanup(6, 0xB, 0x60));
+    t.ev(10, 100, Some(key(10)), outcome(6, crate::cleanup::STREAM_DELETED));
+    let out = t.settle();
+    let got: Vec<_> = files(&out).into_iter().map(|(a, p, _)| (a, p)).collect();
+    assert_eq!(got, [("delete", r"C:\link.txt".into()), ("delete", r"C:\s.txt:x".into())]);
+}
+
+#[test]
+fn an_unknown_outcome_reports_a_requested_delete_and_counts_it() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\Mup\srv\share\x.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, 0));
+    t.ev(6, 100, Some(key(10)), delete_path(2, 0xA, 0x50, f));
+    t.ev(7, 100, Some(key(10)), cleanup(3, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), outcome(3, crate::cleanup::UNKNOWN));
+    // Nothing asked for on this one.
+    t.ev(8, 100, Some(key(10)), create(4, 0xB, r"\Device\Mup\srv\share\y.txt", 0));
+    t.ev(9, 100, Some(key(10)), cleanup(5, 0xB, 0x60));
+    t.ev(9, 100, Some(key(10)), outcome(5, crate::cleanup::UNKNOWN));
+    let out = t.settle();
+    assert_eq!(files(&out).len(), 1);
+    assert_eq!(files(&out)[0].0, "delete");
+    assert_eq!(t.p.counters().file_delete_outcome_unknown, 1);
+}
+
 #[test]
 fn failed_operations_are_dropped_by_irp_within_the_window() {
     let mut t = T::new();
     running(&mut t, 100, 10, EXPLORER);
-    let p = |irp, path: &str| path_event(irp, 0xF, path);
-    // A failed delete, a failed rename, a delete that stands.
-    t.ev(5, 100, Some(key(10)), RawEvent::FileDeletePath(p(1, r"\Device\HarddiskVolume3\ro.txt")));
-    t.ev(6, 100, Some(key(10)), op_end(1, 0xC000_0121));
-    t.ev(7, 100, Some(key(10)), RawEvent::FileRenamePath(p(2, r"\Device\HarddiskVolume3\new.txt")));
+    running(&mut t, 200, 20, CMD);
+    let p = |irp, path: &str| RawEvent::FileRenamePath(path_event(irp, 0xF, path));
+    // A failed rename, a rename that stands.
+    t.ev(7, 100, Some(key(10)), p(2, r"\Device\HarddiskVolume3\new.txt"));
     t.ev(8, 100, Some(key(10)), op_end(2, 0xC000_0035));
-    t.ev(9, 100, Some(key(10)), RawEvent::FileDeletePath(p(3, r"\Device\HarddiskVolume3\ok.txt")));
+    t.ev(9, 100, Some(key(10)), p(3, r"\Device\HarddiskVolume3\ok.txt"));
     t.ev(10, 100, Some(key(10)), op_end(3, 0x104)); // informational, not a failure
-    // A failure long after the window: the delete stands, counted as late.
-    t.ev(30, 100, Some(key(10)), RawEvent::FileDeletePath(p(5, r"\Device\HarddiskVolume3\slow.txt")));
+    // A failure long after the window: the rename stands, counted as late.
+    t.ev(30, 100, Some(key(10)), p(5, r"\Device\HarddiskVolume3\slow.txt"));
     t.ev(900, 100, Some(key(10)), op_end(5, 0xC000_0001));
     // The failure is processed first when its operation arrives late (after
     // the ordering stage released newer events): the ring of failures catches it.
     t.ev(1_000, 100, Some(key(10)), op_end(4, 0xC000_0043));
     t.at(2_000);
-    t.ev(999, 100, Some(key(10)), RawEvent::FileDeletePath(p(4, r"\Device\HarddiskVolume3\late.txt")));
+    t.ev(999, 100, Some(key(10)), p(4, r"\Device\HarddiskVolume3\late.txt"));
+    // A failed delete: no Cleanup ever reports it removed, and the request is
+    // taken back: the FileKey, reused by another file that 200 deletes through
+    // a handle we never saw opened, does not make 100 its actor.
+    t.ev(2_100, 100, Some(key(10)), delete_path(6, 0xE, 0x50, r"\Device\HarddiskVolume3\ro.txt"));
+    t.ev(2_101, 100, Some(key(10)), op_end(6, 0xC000_0121));
+    t.ev(2_200, 200, Some(key(20)), create(7, 0xD, r"\Device\HarddiskVolume3\other.txt", 0));
+    t.ev(2_201, 200, Some(key(20)), cleanup(8, 0xD, 0x50));
+    t.ev(2_201, 200, Some(key(20)), outcome(8, crate::cleanup::FILE_DELETED));
     let out = t.settle();
-    let got: Vec<String> = files(&out).into_iter().map(|(_, p, _)| p).collect();
-    assert_eq!(got, [r"C:\ok.txt", r"C:\slow.txt"]);
+    assert_eq!(rename_results(&out), [r"C:\ok.txt", r"C:\slow.txt"]);
+    assert_eq!(files(&out).len(), 3);
+    assert_eq!(files(&out)[2], ("delete", r"C:\other.txt".into(), 200));
     let c = t.p.counters();
-    assert_eq!((c.file_op_failed, c.file_op_late_failure, c.late_arrivals), (3, 1, 1));
+    assert_eq!((c.file_op_failed, c.file_op_late_failure, c.late_arrivals), (2, 1, 1));
 }
 
 #[test]
@@ -723,7 +907,9 @@ fn short_names_in_emitted_paths_are_expanded() {
     // A failed expansion leaves the path as logged.
     let mut t = T::new();
     running(&mut t, 100, 10, EXPLORER);
-    t.ev(5, 100, Some(key(10)), RawEvent::FileDeletePath(path_event(1, 0xB, short)));
+    t.ev(5, 100, Some(key(10)), create(1, 0xB, short, 0));
+    t.ev(6, 100, Some(key(10)), cleanup(2, 0xB, 0x50));
+    t.ev(6, 100, Some(key(10)), outcome(2, crate::cleanup::FILE_DELETED));
     t.at(1_000);
     for r in t.requests() {
         if let Request::Expand { id, slot, .. } = r {
@@ -1054,10 +1240,10 @@ fn the_agents_own_activity_is_not_emitted() {
 fn a_clean_stop_emits_everything_pending() {
     let mut t = T::new();
     running(&mut t, 100, 10, EXPLORER);
-    t.ev(5, 100, Some(key(10)), RawEvent::FileDeletePath(path_event(1, 0xA, r"\Device\HarddiskVolume3\x")));
+    t.ev(5, 100, Some(key(10)), RawEvent::FileRenamePath(path_event(1, 0xA, r"\Device\HarddiskVolume3\x")));
     t.ev(6, 4, None, RawEvent::UdpSend(net(100, "10.0.0.5", 5353, "8.8.8.8", 53)));
     let out = t.p.stop();
-    // The delete (its window not yet passed) and the flow's Open and Close.
+    // The rename (its window not yet passed) and the flow's Open and Close.
     assert_eq!(out.len(), 3);
     valid(&out);
 }
@@ -1335,3 +1521,223 @@ fn an_event_older_than_stream_time_is_late() {
 fn reg_value_types_map_to_the_schema() {
     assert_eq!(RegType::from_raw(4), RegType::Known(RegValueType::Dword));
 }
+
+// ---- the independent review's cases (plan 1b-3c, Review Log) ----
+
+fn set_delete_irp(irp: u64, fo: u64, file_key: u64, delete: bool) -> RawEvent {
+    RawEvent::FileSetDelete(FileSetInfo {
+        irp,
+        file_object: fo,
+        file_key,
+        extra_information: u64::from(delete),
+        issuing_tid: 1,
+        info_class: 13,
+    })
+}
+
+/// Real Irps are per-thread and reused: the request, its Cleanup and the next
+/// failed open all carry the same Irp. The request's own successful
+/// OperationEnd never reaches the pipeline (the callback drops it).
+#[test]
+fn a_later_failure_on_the_requests_irp_does_not_take_the_request_back() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    running(&mut t, 200, 20, CMD);
+    let f = r"\Device\HarddiskVolume3\shared.txt";
+    t.ev(5, 200, Some(key(20)), create(9, 0xB, f, 0));
+    t.ev(6, 100, Some(key(10)), create(3, 0xA, f, 0));
+    t.ev(7, 100, Some(key(10)), set_delete_irp(3, 0xA, 0x50, true));
+    t.ev(7, 100, Some(key(10)), delete_path(3, 0xA, 0x50, f));
+    t.ev(8, 100, Some(key(10)), cleanup(3, 0xA, 0x50));
+    // (FILE_REMAINS: the callback drops it)
+    // 100 checks whether the file is gone: delete pending, the open fails.
+    t.ev(9, 100, Some(key(10)), create(3, 0xC, f, 0));
+    t.ev(9, 100, Some(key(10)), op_end(3, 0xC000_0056));
+    // 200 closes last.
+    t.ev(50, 200, Some(key(20)), cleanup(10, 0xB, 0x50));
+    t.ev(50, 200, Some(key(20)), outcome(10, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    assert_eq!(files(&out), [("delete", r"C:\shared.txt".into(), 100)], "the requester");
+}
+
+/// The same, with the last handle the agent's own (a hash worker): the actor
+/// is still the requester, so the Delete is not self-filtered away.
+#[test]
+fn the_same_with_the_agent_closing_last() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    running(&mut t, 999, 0x999, CMD);
+    let f = r"\Device\HarddiskVolume3\shared.txt";
+    t.ev(5, 999, Some(key(0x999)), create(9, 0xB, f, 0));
+    t.ev(6, 100, Some(key(10)), create(3, 0xA, f, 0));
+    t.ev(7, 100, Some(key(10)), set_delete_irp(3, 0xA, 0x50, true));
+    t.ev(7, 100, Some(key(10)), delete_path(3, 0xA, 0x50, f));
+    t.ev(8, 100, Some(key(10)), cleanup(3, 0xA, 0x50));
+    t.ev(9, 100, Some(key(10)), create(3, 0xC, f, 0));
+    t.ev(9, 100, Some(key(10)), op_end(3, 0xC000_0056));
+    t.ev(50, 999, Some(key(0x999)), cleanup(10, 0xB, 0x50));
+    t.ev(50, 999, Some(key(0x999)), outcome(10, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    assert_eq!(files(&out), [("delete", r"C:\shared.txt".into(), 100)], "the requester");
+}
+
+/// Rename, then delete through the same handle, inside the confirm window.
+#[test]
+fn rename_then_delete_on_one_handle_reports_the_new_name() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let old = r"\Device\HarddiskVolume3\a.txt";
+    let new = r"\Device\HarddiskVolume3\b.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, old, 0));
+    t.ev(6, 100, Some(key(10)), RawEvent::FileRenamePath(path_event(1, 0xA, new)));
+    t.ev(7, 100, Some(key(10)), delete_path(1, 0xA, 0x50, new));
+    t.ev(8, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(8, 100, Some(key(10)), outcome(1, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    let d: Vec<_> = files(&out).into_iter().filter(|f| f.0 == "delete").collect();
+    assert_eq!(d, [("delete", r"C:\b.txt".into(), 100)]);
+}
+
+/// A handle opened before another handle renamed the file, then used to delete it.
+#[test]
+fn delete_through_a_handle_opened_before_a_rename() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let old = r"\Device\HarddiskVolume3\a.txt";
+    let new = r"\Device\HarddiskVolume3\b.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xB, old, 0));
+    t.ev(6, 100, Some(key(10)), create(2, 0xA, old, 0));
+    t.ev(7, 100, Some(key(10)), RawEvent::FileRenamePath(path_event(2, 0xA, new)));
+    t.ev(800, 100, Some(key(10)), delete_path(3, 0xB, 0x50, new));
+    t.ev(801, 100, Some(key(10)), cleanup(3, 0xB, 0x50));
+    t.ev(801, 100, Some(key(10)), outcome(3, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    let d: Vec<_> = files(&out).into_iter().filter(|f| f.0 == "delete").collect();
+    assert_eq!(d, [("delete", r"C:\b.txt".into(), 100)]);
+}
+
+/// A POSIX delete that is never reported a second time (a mapped file), then
+/// the same path recreated, its FileKey reused, and deleted the ordinary way.
+#[test]
+fn a_stale_posix_entry_does_not_suppress_a_later_delete_of_the_same_path() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\app.dll";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, 0));
+    t.ev(6, 100, Some(key(10)), delete_path(1, 0xA, 0x50, f));
+    t.ev(7, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), outcome(1, crate::cleanup::FILE_DELETED | crate::cleanup::POSIX_STYLE_DELETE));
+    t.ev(60_000, 100, Some(key(10)), create(2, 0xB, f, 0));
+    t.ev(60_001, 100, Some(key(10)), delete_path(2, 0xB, 0x50, f));
+    t.ev(60_002, 100, Some(key(10)), cleanup(2, 0xB, 0x50));
+    t.ev(60_002, 100, Some(key(10)), outcome(2, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    let d: Vec<_> = files(&out).into_iter().filter(|f| f.0 == "delete").collect();
+    assert_eq!(d.len(), 2, "{d:?}");
+}
+
+/// A POSIX delete whose other handle was opened by its 8.3 name.
+#[test]
+fn a_posix_double_report_through_a_short_name_is_one_delete() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let long = r"\Device\HarddiskVolume3\longdirectory";
+    let short = r"\Device\HarddiskVolume3\LONGDI~1";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, short, 0));
+    t.ev(6, 100, Some(key(10)), create(2, 0xB, long, 0));
+    t.ev(7, 100, Some(key(10)), delete_path(2, 0xB, 0x50, long));
+    t.ev(8, 100, Some(key(10)), cleanup(2, 0xB, 0x50));
+    t.ev(8, 100, Some(key(10)), outcome(2, crate::cleanup::FILE_DELETED | crate::cleanup::POSIX_STYLE_DELETE));
+    t.ev(9, 100, Some(key(10)), cleanup(3, 0xA, 0x50));
+    t.ev(9, 100, Some(key(10)), outcome(3, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    let d: Vec<_> = files(&out).into_iter().filter(|f| f.0 == "delete").collect();
+    assert_eq!(d.len(), 1, "{d:?}");
+}
+
+/// Without seeding (no SeDebugPrivilege), a delete through a handle the agent
+/// never saw opened. The DeletePath carries the path; 1b-3a reported it.
+#[test]
+fn a_delete_through_an_unknown_handle_without_seeding() {
+    let cfg = Config { seed_on_miss: false, seed_on_start: false, ..Config::default() };
+    let mut t = T::with(cfg, FakeLookups::default());
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\pre.txt";
+    t.ev(6, 100, Some(key(10)), delete_path(1, 0xA, 0x50, f));
+    t.ev(7, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    t.ev(7, 100, Some(key(10)), outcome(1, crate::cleanup::FILE_DELETED));
+    let out = t.settle();
+    assert_eq!(
+        files(&out),
+        [("delete", r"C:\pre.txt".into(), 100)],
+        "unknown_file_object {}",
+        t.p.counters().unknown_file_object
+    );
+}
+
+/// The Cleanup's outcome processed before the Cleanup (a late Cleanup).
+#[test]
+fn an_outcome_processed_before_its_late_cleanup_is_paired() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\x.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, 0));
+    t.ev(6, 100, Some(key(10)), delete_path(1, 0xA, 0x50, f));
+    t.ev(8, 100, Some(key(10)), outcome(1, crate::cleanup::FILE_DELETED));
+    t.at(2_000);
+    t.ev(7, 100, Some(key(10)), cleanup(1, 0xA, 0x50)); // late
+    let out = t.settle();
+    let d: Vec<_> = files(&out).into_iter().filter(|f| f.0 == "delete").collect();
+    assert_eq!(d.len(), 1, "{d:?}");
+}
+
+/// A late Cleanup's outcome must not be another operation's OperationEnd.
+#[test]
+fn a_cleanup_never_pairs_with_another_operations_end() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\x.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, 0));
+    t.ev(6, 100, Some(key(10)), cleanup(1, 0xA, 0x50));
+    // (its outcome, FILE_REMAINS, the callback drops)
+    t.ev(8, 100, Some(key(10)), create(1, 0xB, f, 0));
+    t.ev(9, 100, Some(key(10)), outcome(1, crate::cleanup::FILE_DELETED)); // no Cleanup on 0xB: not ours
+    let out = t.settle();
+    assert!(files(&out).iter().all(|f| f.0 != "delete"), "{:?}", files(&out));
+}
+
+/// Clearing a delete-on-close handle's flag (`FileDispositionInformationEx`)
+/// leaves the disposition another handle set (review R-m3).
+#[test]
+fn clearing_a_handles_delete_on_close_keeps_another_handles_request() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    running(&mut t, 200, 20, CMD);
+    let f = r"\Device\HarddiskVolume3\both.txt";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, 0));
+    t.ev(6, 100, Some(key(10)), set_delete(0xA, 0x50, true));
+    t.ev(6, 100, Some(key(10)), delete_path(2, 0xA, 0x50, f));
+    t.ev(7, 200, Some(key(20)), create(3, 0xB, f, FileCreate::DELETE_ON_CLOSE));
+    t.ev(8, 200, Some(key(20)), set_delete(0xB, 0x50, false));
+    t.ev(9, 200, Some(key(20)), cleanup(4, 0xB, 0x50));
+    t.ev(9, 200, Some(key(20)), outcome(4, crate::cleanup::FILE_DELETED));
+    assert_eq!(files(&t.settle()), [("delete", r"C:\both.txt".into(), 100)]);
+}
+
+/// The request's own OperationEnd (passed on by the callback) closes its Irp:
+/// a failure of a later operation on the Irp whose start the agent does not
+/// log (a query) does not take it back (review R-M1).
+#[test]
+fn a_request_that_succeeded_stands_against_a_later_failure_on_its_irp() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    running(&mut t, 200, 20, CMD);
+    let f = r"\Device\HarddiskVolume3\kept.txt";
+    t.ev(5, 200, Some(key(20)), create(9, 0xB, f, 0));
+    t.ev(6, 100, Some(key(10)), delete_path(3, 0xA, 0x50, f));
+    t.ev(6, 100, Some(key(10)), outcome(3, 0)); // the request's success
+    t.ev(7, 100, Some(key(10)), op_end(3, 0xC000_0022)); // a query on the same Irp fails
+    t.ev(50, 200, Some(key(20)), cleanup(10, 0xB, 0x50));
+    t.ev(50, 200, Some(key(20)), outcome(10, crate::cleanup::FILE_DELETED));
+    assert_eq!(files(&t.settle()), [("delete", r"C:\kept.txt".into(), 100)]);
+}
```

- [ ] **Step 7: The replay's delete assertions, and the snapshot.**
  - `u.txt` has one Delete: the directory's removal; the undelete gives none.
  - `l2.txt` and `s.txt:x` have one each.

  Each Delete now carries its Cleanup's time, a few microseconds after the request.

```diff
--- a/crates/atlas-agent/tests/replay.rs
+++ b/crates/atlas-agent/tests/replay.rs
@@ -252,6 +252,14 @@ fn the_ci_recording_produces_the_scenario() {
     // final delete (after clearing read-only) does.
     assert_eq!(files.iter().filter(|(a, n)| a == "delete" && n == "c.txt").count(), 1, "{files:?}");
     assert!(counters.file_op_failed >= 2, "{counters:?}");
+    // Deletes come from the Cleanup outcome (plan 1b-3c): the undelete gives
+    // none (u.txt's one Delete is the directory's removal at the end), the
+    // hard link and the stream each give their own.
+    let deletes = |n: &str| files.iter().filter(|(a, x)| a == "delete" && x == n).count();
+    assert_eq!(deletes("u.txt"), 1, "{files:?}");
+    assert_eq!(deletes("l2.txt"), 1, "{files:?}");
+    assert_eq!(deletes("s.txt:x"), 1, "{files:?}");
+    assert_eq!(counters.file_delete_outcome_unknown, 0, "{counters:?}");
     // One spelling: no emitted path keeps the runner's 8.3 user name (plan 1b-3a Q4).
     for e in &out {
         if let EventKind::File(f) = &e.kind {
```

```powershell
$env:ATLAS_UPDATE_SNAPSHOT = '1'; cargo test -p atlas-agent --test replay; Remove-Item Env:ATLAS_UPDATE_SNAPSHOT
git diff --stat crates/atlas-agent/tests/snapshots
```
Expected diff:
- the false Delete of `u.txt` goes;
- the other Deletes move to their Cleanup's time;
- event ids after them shift by one.

The deletes: one each for `b.txt`, `c.txt`, `d.txt`, `l.txt`, `l2.txt`, `s.txt`, `s.txt:x`, `u.txt` and the directory.

- [ ] **Step 8: Check and commit**

```powershell
cargo test -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 356 passed, 6 ignored (`atlas-agent` unit tests: 173).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): file deletes from the Cleanup outcome; undeletes give none"
```

### Task 4: The driver loop

**Files:**
- Create: `crates/atlas-agent/src/driver.rs`
- Modify: `src/lib.rs`, `src/fakes.rs`, `tests/replay.rs`

- [ ] **Step 1: `Driver`, `Lanes`, `DriverConfig`, `Control`.** The module's tests run it on a real thread with a real clock:
  - an event comes out after the hold with no other traffic;
  - closing one queue keeps the loop going, closing both drains what is held;
  - the anchor is re-taken on its period;
  - the start-up seeding requests reach the lanes;
  - a `Control` closure runs on the pipeline thread;
  - a drain capped at one event per pass loses nothing.

`crates/atlas-agent/src/driver.rs`:
```rust
//! The pipeline thread's loop (sensor spec §3.2; plan 1b-3c, decision D1).
//!
//! Every `cadence` (10 ms) one pass:
//! 1. drains the kernel and user-mode queues into [`Pipeline::push`], at most
//!    a queue's capacity each, so a flood cannot starve `tick` (review R-m8);
//! 2. feeds the services' replies into [`Pipeline::reply`], and runs any
//!    [`Control`] another thread sent (review R-M5);
//! 3. calls [`Pipeline::tick`] with the current QPC;
//! 4. hands the pipeline's requests to the services;
//! 5. passes the emitted events, in order, to the sink.
//!
//! The anchor is re-taken every `anchor_every` (60 s, §3.3). The loop ends when
//! both queues are closed (every sender gone): it applies the replies already
//! in, then [`Pipeline::stop`] sends what is pending as at its deadlines
//! (§11.4). The driver owns the services, so they are dropped after the
//! callbacks' senders (and their `FastRead`), the order 1b-3b's R-m11 needs.
//!
//! Portable: Windows supplies the clock, the anchor and the services
//! (`win::agent`); the tests use fakes.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use atlas_schema::Event;

use crate::counters::Counters;
use crate::input::Incoming;
use crate::pipeline::Pipeline;
use crate::services::{Lookups, Reply, Request};
use crate::time::Anchor;

/// What the loop needs from the services: hand a request to its lane, and
/// collect the replies so far. Neither may block (§3.2).
pub trait Lanes {
    fn submit(&self, r: Request);
    fn replies(&self) -> Vec<Reply>;
}

/// The loop's settings (sensor spec §3.2, §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverConfig {
    /// One pass every this long; `tick` must come at least every 50 ms.
    pub cadence: Duration,
    /// The anchor pair is re-taken this often (§3.3).
    pub anchor_every: Duration,
    /// Queue capacities (§3.2): kernel providers and Session B, and the
    /// user-mode providers.
    pub kernel_queue_cap: usize,
    pub user_queue_cap: usize,
}

impl Default for DriverConfig {
    fn default() -> Self {
        DriverConfig {
            cadence: Duration::from_millis(10),
            anchor_every: Duration::from_secs(60),
            kernel_queue_cap: 65_536,
            user_queue_cap: 8_192,
        }
    }
}

/// The current QPC.
pub type Clock = Box<dyn FnMut() -> i64 + Send>;
/// A fresh anchor pair (QPC, Unix time).
pub type AnchorSource = Box<dyn FnMut() -> Anchor + Send>;
/// Work another thread wants done on the pipeline thread, between two passes:
/// plan 1b-4's canary self key (`Pipeline::add_self_key`), its Sensor Health
/// reads of `Pipeline::counters`.
pub type Control<L> = Box<dyn FnOnce(&mut Pipeline<L>) + Send>;

pub struct Driver<L, S> {
    pipeline: Pipeline<L>,
    lanes: S,
    kernel: Receiver<Incoming>,
    user: Receiver<Incoming>,
    control: Receiver<Control<L>>,
    clock: Clock,
    anchor: AnchorSource,
    cfg: DriverConfig,
}

impl<L: Lookups, S: Lanes> Driver<L, S> {
    pub fn new(
        pipeline: Pipeline<L>,
        lanes: S,
        kernel: Receiver<Incoming>,
        user: Receiver<Incoming>,
        clock: Clock,
        anchor: AnchorSource,
        cfg: DriverConfig,
    ) -> Self {
        // No control until `with_control`: a receiver whose sender is gone.
        let control = channel().1;
        Driver { pipeline, lanes, kernel, user, control, clock, anchor, cfg }
    }

    /// Runs what arrives on `control` on the pipeline thread, once per pass.
    pub fn with_control(mut self, control: Receiver<Control<L>>) -> Self {
        self.control = control;
        self
    }

    /// Runs until both queues are closed, then stops the pipeline. Returns its
    /// counters.
    pub fn run(mut self, mut sink: impl FnMut(Event)) -> Counters {
        let mut next_anchor = Instant::now() + self.cfg.anchor_every;
        loop {
            let started = Instant::now();
            let open = self.pass(&mut sink);
            if !open {
                break;
            }
            if started >= next_anchor {
                self.pipeline.set_anchor((self.anchor)());
                next_anchor = started + self.cfg.anchor_every;
            }
            std::thread::sleep(self.cfg.cadence.saturating_sub(started.elapsed()));
        }
        for r in self.lanes.replies() {
            self.pipeline.reply(r);
        }
        for e in self.pipeline.stop() {
            sink(e);
        }
        self.pipeline.counters()
    }

    /// One pass. Returns whether a queue is still open.
    fn pass(&mut self, sink: &mut impl FnMut(Event)) -> bool {
        let kernel = drain(&self.kernel, &mut self.pipeline, self.cfg.kernel_queue_cap);
        let user = drain(&self.user, &mut self.pipeline, self.cfg.user_queue_cap);
        for r in self.lanes.replies() {
            self.pipeline.reply(r);
        }
        while let Ok(c) = self.control.try_recv() {
            c(&mut self.pipeline);
        }
        let out = self.pipeline.tick((self.clock)());
        for r in self.pipeline.take_requests() {
            self.lanes.submit(r);
        }
        for e in out {
            sink(e);
        }
        kernel || user
    }
}

/// Pushes what is queued, at most `max` events. Returns whether the queue is
/// still open.
fn drain<L: Lookups>(rx: &Receiver<Incoming>, p: &mut Pipeline<L>, max: usize) -> bool {
    for _ in 0..max.max(1) {
        match rx.try_recv() {
            Ok(inc) => p.push(inc),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{SyncSender, channel, sync_channel};
    use std::sync::{Arc, Mutex};

    use atlas_etw::parse::{FileCreate, RawEvent};
    use atlas_schema::{BootId, DeviceUid};

    use super::*;
    use crate::config::{Config, Ticks};
    use crate::fakes::{FakeLanes, FakeLookups, sequential_ids};
    use crate::input::{Header, Session};
    use crate::pipeline::Setup;
    use crate::process::Identity;

    const FREQ: i64 = 10_000_000;

    /// A driver on its own thread with a real clock; the sink sends events back.
    struct Running {
        kernel: Option<SyncSender<Incoming>>,
        user: Option<SyncSender<Incoming>>,
        events: std::sync::mpsc::Receiver<Event>,
        thread: std::thread::JoinHandle<Counters>,
        t0: Instant,
    }

    fn qpc(t0: Instant) -> i64 {
        (t0.elapsed().as_nanos() / 100) as i64
    }

    fn start(cfg: DriverConfig, anchor: AnchorSource) -> Running {
        let t0 = Instant::now();
        let setup = Setup {
            config: Config::default(),
            ticks: Ticks::new(FREQ),
            anchor: Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 },
            identity: Identity {
                device: DeviceUid::from_bytes([0xd0; 16]),
                boot: BootId::from_bytes([0xb0; 16]),
                kernel_boot_id: 7,
            },
            current_control_set: 1,
            self_keys: vec![],
            started: 0,
        };
        let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
        let (ktx, krx) = sync_channel(cfg.kernel_queue_cap);
        let (utx, urx) = sync_channel(cfg.user_queue_cap);
        let (etx, events) = channel();
        let driver = Driver::new(p, FakeLanes::new(|_| None), krx, urx, Box::new(move || qpc(t0)), anchor, cfg);
        let thread = std::thread::spawn(move || driver.run(|e| etx.send(e).unwrap()));
        Running { kernel: Some(ktx), user: Some(utx), events, thread, t0 }
    }

    /// A file created by the Idle process (a built-in actor), at QPC `ts`.
    fn created(ts: i64, name: &str) -> Incoming {
        Incoming {
            header: Header { session: Session::Sensor, pid: 0, tid: 1, ts, start_key: None },
            event: RawEvent::FileCreateNew(FileCreate {
                irp: 1,
                file_object: 0xA,
                issuing_tid: 1,
                create_options: 0,
                create_attributes: 0,
                share_access: 7,
                file_name: name.into(),
            }),
        }
    }

    fn no_anchor() -> AnchorSource {
        Box::new(|| Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 })
    }

    #[test]
    fn events_come_out_after_the_hold_while_idle_and_the_loop_ends_when_the_queues_close() {
        let r = start(DriverConfig::default(), no_anchor());
        let send = Instant::now();
        r.kernel.as_ref().unwrap().send(created(qpc(r.t0), r"\Device\HarddiskVolume3\a.txt")).unwrap();
        // No other event comes: idle ticks alone release it, after the 750 ms hold.
        let e = r.events.recv_timeout(Duration::from_secs(5)).expect("released while idle");
        let waited = send.elapsed();
        // At least the hold; the upper bound is loose for a loaded CI runner (review R-m10).
        assert!(waited >= Duration::from_millis(700) && waited < Duration::from_secs(5), "{waited:?}");
        assert!(matches!(&e.kind, atlas_schema::EventKind::File(_)));
        drop(r.kernel);
        drop(r.user);
        r.thread.join().unwrap();
    }

    #[test]
    fn closing_the_queues_drains_what_is_held() {
        let r = start(DriverConfig::default(), no_anchor());
        let k = r.kernel.unwrap();
        // The user-mode queue first: the loop runs on while one queue is open.
        drop(r.user);
        for i in 0..3 {
            k.send(created(qpc(r.t0), &format!(r"\Device\HarddiskVolume3\{i}.txt"))).unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));
        drop(k); // well inside the hold: the stop sends them
        r.thread.join().unwrap();
        assert_eq!(r.events.try_iter().count(), 3);
    }

    #[test]
    fn the_anchor_is_retaken_on_its_period() {
        let taken = Arc::new(AtomicUsize::new(0));
        let n = taken.clone();
        let cfg = DriverConfig { anchor_every: Duration::from_millis(20), ..DriverConfig::default() };
        let r = start(
            cfg,
            Box::new(move || {
                n.fetch_add(1, Ordering::Relaxed);
                Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 }
            }),
        );
        std::thread::sleep(Duration::from_millis(250));
        drop(r.kernel);
        drop(r.user);
        r.thread.join().unwrap();
        let n = taken.load(Ordering::Relaxed);
        assert!((1..=13).contains(&n), "{n} refreshes in 250 ms");
    }

    #[test]
    fn the_loop_hands_requests_to_the_lanes() {
        // The replay test (`tests/replay.rs`) checks the whole loop against the
        // synchronous replay; here, only that the lanes are used from the loop.
        let submitted = Arc::new(Mutex::new(0usize));
        let s = submitted.clone();
        struct Counting(Arc<Mutex<usize>>);
        impl Lanes for Counting {
            fn submit(&self, _: Request) {
                *self.0.lock().unwrap() += 1;
            }
            fn replies(&self) -> Vec<Reply> {
                Vec::new()
            }
        }
        let setup = Setup {
            config: Config::default(),
            ticks: Ticks::new(FREQ),
            anchor: Anchor { qpc: 0, unix_ns: 0 },
            identity: Identity {
                device: DeviceUid::from_bytes([1; 16]),
                boot: BootId::from_bytes([2; 16]),
                kernel_boot_id: 7,
            },
            current_control_set: 1,
            self_keys: vec![],
            started: 0,
        };
        let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
        let (ktx, krx) = sync_channel(4);
        let (utx, urx) = sync_channel(4);
        drop((ktx, utx));
        let cfg = DriverConfig { cadence: Duration::ZERO, ..DriverConfig::default() };
        Driver::new(p, Counting(s), krx, urx, Box::new(|| 0), no_anchor(), cfg).run(|_| {});
        // The start-up seeding pass (`seed_on_start`) is asked for on the first pass.
        assert_eq!(*submitted.lock().unwrap(), 2, "a Seed per handle kind");
    }

    #[test]
    fn control_runs_on_the_pipeline_thread_between_passes() {
        let setup = Setup {
            config: Config::default(),
            ticks: Ticks::new(FREQ),
            anchor: Anchor { qpc: 0, unix_ns: 0 },
            identity: Identity {
                device: DeviceUid::from_bytes([1; 16]),
                boot: BootId::from_bytes([2; 16]),
                kernel_boot_id: 7,
            },
            current_control_set: 1,
            self_keys: vec![],
            started: 0,
        };
        let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
        let (ktx, krx) = sync_channel(4);
        let (_utx, urx) = sync_channel(4);
        let (ctx, crx) = channel::<Control<FakeLookups>>();
        let t0 = Instant::now();
        let driver = Driver::new(
            p,
            FakeLanes::new(|_| None),
            krx,
            urx,
            Box::new(move || qpc(t0)),
            no_anchor(),
            DriverConfig::default(),
        )
        .with_control(crx);
        let thread = std::thread::spawn(move || driver.run(|_| {}));
        // A Sensor Health read while the loop runs (plan 1b-4).
        let (tx, rx) = channel();
        ctx.send(Box::new(move |p: &mut Pipeline<FakeLookups>| tx.send(p.counters()).unwrap())).unwrap();
        let counters = rx.recv_timeout(Duration::from_secs(5)).expect("the control ran");
        assert_eq!(counters.late_arrivals, 0);
        drop((ktx, _utx));
        thread.join().unwrap();
    }

    #[test]
    fn a_capped_drain_loses_nothing() {
        // One event per queue per pass (review R-m8): the rest waits for the
        // next pass, and closing the queue still drains it all.
        let cfg = DriverConfig { kernel_queue_cap: 1, user_queue_cap: 1, ..DriverConfig::default() };
        let r = start(cfg, no_anchor());
        let k = r.kernel.unwrap();
        drop(r.user);
        let now = qpc(r.t0);
        // The queue holds one: each send waits for a pass to make room.
        for i in 0..5 {
            k.send(created(now, &format!(r"\Device\HarddiskVolume3\{i}.txt"))).unwrap();
        }
        drop(k);
        r.thread.join().unwrap();
        assert_eq!(r.events.try_iter().count(), 5);
    }
}
```

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -4,6 +4,7 @@ pub mod cleanup;
 pub mod completion;
 pub mod config;
 pub mod counters;
+pub mod driver;
 pub mod evict;
 pub mod fakes;
 pub mod input;
```

- [ ] **Step 2: `FakeLanes`**, answering each request at once through a closure, the reply collected on the next pass.

```diff
--- a/crates/atlas-agent/src/fakes.rs
+++ b/crates/atlas-agent/src/fakes.rs
@@ -2,11 +2,13 @@
 //! tables, and event ids that depend only on the event time and order.
 
 use std::collections::HashMap;
+use std::sync::Mutex;
 
 use atlas_schema::EventId;
 
+use crate::driver::Lanes;
 use crate::pipeline::IdGen;
-use crate::services::{LiveProcess, Lookups};
+use crate::services::{LiveProcess, Lookups, Reply, Request};
 
 /// [`Lookups`] answered from tables.
 #[derive(Debug, Clone, Default)]
@@ -45,6 +47,37 @@ impl Lookups for FakeLookups {
     }
 }
 
+/// How [`FakeLanes`] answers a request, if at all.
+type Answer = Box<dyn Fn(&Request) -> Option<Reply> + Send>;
+
+/// Services that answer every request at once, through `answer`: the reply
+/// is collected on the next pass, as a real lane's would be at the earliest.
+pub struct FakeLanes {
+    answer: Answer,
+    replies: Mutex<Vec<Reply>>,
+    /// Every request submitted, in order.
+    pub submitted: Mutex<Vec<Request>>,
+}
+
+impl FakeLanes {
+    pub fn new(answer: impl Fn(&Request) -> Option<Reply> + Send + 'static) -> Self {
+        FakeLanes { answer: Box::new(answer), replies: Mutex::default(), submitted: Mutex::default() }
+    }
+}
+
+impl Lanes for FakeLanes {
+    fn submit(&self, r: Request) {
+        if let Some(reply) = (self.answer)(&r) {
+            self.replies.lock().expect("replies").push(reply);
+        }
+        self.submitted.lock().expect("submitted").push(r);
+    }
+
+    fn replies(&self) -> Vec<Reply> {
+        std::mem::take(&mut *self.replies.lock().expect("replies"))
+    }
+}
+
 /// UUIDv7 ids from the event's time (milliseconds) and a counter, so a replay
 /// gives the same ids every run.
 pub fn sequential_ids() -> IdGen {
```

- [ ] **Step 3: The replay through the loop.** The replay's setup is shared, the fake workers become a function from request to reply, and `run_driven` runs the same recording through `Driver`:
  - its fake clock moves 100 ms per pass;
  - it drops the intake (closing the queues) once the replay's horizon passes.

  `the_driver_loop_gives_the_same_events` compares the encoded events with the synchronous replay's, one by one.

```diff
--- a/crates/atlas-agent/tests/replay.rs
+++ b/crates/atlas-agent/tests/replay.rs
@@ -11,12 +11,14 @@
 use std::collections::HashMap;
 use std::path::PathBuf;
 use std::sync::Arc;
-use std::sync::mpsc::sync_channel;
+use std::sync::mpsc::{Receiver, sync_channel};
+use std::time::Duration;
 
 use atlas_agent::config::{Config, Ticks};
 use atlas_agent::counters::IntakeCounters;
-use atlas_agent::fakes::{FakeLookups, sequential_ids};
-use atlas_agent::input::{Header, Session};
+use atlas_agent::driver::{Driver, DriverConfig};
+use atlas_agent::fakes::{FakeLanes, FakeLookups, sequential_ids};
+use atlas_agent::input::{Header, Incoming, Session};
 use atlas_agent::intake::{Intake, Queues};
 use atlas_agent::pipeline::{Pipeline, Setup};
 use atlas_agent::process::Identity;
@@ -109,7 +111,67 @@ fn live_processes(lines: &[Line]) -> HashMap<u32, LiveProcess> {
     out
 }
 
+/// The pipeline and an intake for the recording, the intake's queues filled.
+struct Replay {
+    p: Pipeline<FakeLookups>,
+    intake: Intake,
+    krx: Receiver<Incoming>,
+    urx: Receiver<Incoming>,
+    counters: Arc<IntakeCounters>,
+    first: i64,
+    last: i64,
+}
+
 fn run() -> (Vec<Event>, atlas_agent::counters::Counters) {
+    let Replay { mut p, intake, krx, urx, counters, first, last } = replay();
+    let mut out = Vec::new();
+    let step = FREQ / 10; // 100 ms ticks
+    let mut now = first;
+    loop {
+        for inc in krx.try_iter().chain(urx.try_iter()) {
+            p.push(inc);
+        }
+        out.extend(p.tick(now));
+        for r in p.take_requests() {
+            if let Some(reply) = answer(&r) {
+                p.reply(reply);
+            }
+        }
+        if now > last + 70 * FREQ {
+            break;
+        }
+        now += step;
+    }
+    out.extend(p.stop());
+    drop(intake);
+    assert_eq!(IntakeCounters::get(&counters.parse_errors), 0);
+    (out, p.counters())
+}
+
+/// The same recording through the driver's loop (plan 1b-3c, D1): its fake
+/// clock moves 100 ms per pass, and closes the queues once the replay's
+/// horizon has passed, which ends the loop.
+fn run_driven() -> Vec<Event> {
+    let Replay { p, intake, krx, urx, first, last, .. } = replay();
+    let step = FREQ / 10;
+    let mut now = first - step;
+    let mut intake = Some(intake);
+    let clock = Box::new(move || {
+        now += step;
+        if now > last + 70 * FREQ {
+            intake.take();
+        }
+        now
+    });
+    let anchor = Box::new(|| panic!("no anchor refresh within a replay"));
+    let cfg = DriverConfig { cadence: Duration::ZERO, ..DriverConfig::default() };
+    let driver = Driver::new(p, FakeLanes::new(answer), krx, urx, clock, anchor, cfg);
+    let mut out = Vec::new();
+    driver.run(|e| out.push(e));
+    out
+}
+
+fn replay() -> Replay {
     let lines = fixture();
     let first = lines.first().unwrap().header.ts;
     let last = lines.iter().map(|l| l.header.ts).max().unwrap();
@@ -132,7 +194,7 @@ fn run() -> (Vec<Event>, atlas_agent::counters::Counters) {
         self_keys: vec![],
         started: first,
     };
-    let mut p = Pipeline::new(setup, lookups, sequential_ids()).unwrap();
+    let p = Pipeline::new(setup, lookups, sequential_ids()).unwrap();
     let (ktx, krx) = sync_channel(65_536);
     let (utx, urx) = sync_channel(8_192);
     let counters = Arc::new(IntakeCounters::default());
@@ -141,33 +203,15 @@ fn run() -> (Vec<Event>, atlas_agent::counters::Counters) {
     for l in &lines {
         intake.on_event(l.header, parse(&l.meta, &l.payload));
     }
-    let mut out = Vec::new();
-    let step = FREQ / 10; // 100 ms ticks
-    let mut now = first;
-    loop {
-        for inc in krx.try_iter().chain(urx.try_iter()) {
-            p.push(inc);
-        }
-        out.extend(p.tick(now));
-        for r in p.take_requests() {
-            answer(&mut p, r);
-        }
-        if now > last + 70 * FREQ {
-            break;
-        }
-        now += step;
-    }
-    out.extend(p.stop());
-    assert_eq!(IntakeCounters::get(&counters.parse_errors), 0);
-    (out, p.counters())
+    Replay { p, intake, krx, urx, counters, first, last }
 }
 
 /// The fake workers: a fixed hash for every file, the scenario's DWORD for the
 /// value it sets, the runner's long user name for 8.3 names, nothing from the seeder.
-fn answer(p: &mut Pipeline<FakeLookups>, r: Request) {
+fn answer(r: &Request) -> Option<Reply> {
     match r {
-        Request::Enrich { id, .. } => p.reply(Reply::Enriched {
-            id,
+        Request::Enrich { id, .. } => Some(Reply::Enriched {
+            id: *id,
             hashes: Some(Hashes { sha256: Some([0xaa; 32]) }),
             signature: None,
             error: false,
@@ -178,14 +222,14 @@ fn answer(p: &mut Pipeline<FakeLookups>, r: Request) {
                 size: 4,
                 data: 7u32.to_le_bytes().to_vec(),
             });
-            p.reply(Reply::ValueRead { id, result });
+            Some(Reply::ValueRead { id: *id, result })
         }
         // The runner's user is `runneradmin`, which 8.3 shortens to `RUNNER~1`.
         Request::Expand { id, slot, nt_path } => {
             let long_path = nt_path.contains("RUNNER~1").then(|| nt_path.replace("RUNNER~1", "runneradmin"));
-            p.reply(Reply::Expanded { id, slot, long_path });
+            Some(Reply::Expanded { id: *id, slot: *slot, long_path })
         }
-        Request::Seed { .. } | Request::InvalidateHash { .. } => {}
+        Request::Seed { .. } | Request::InvalidateHash { .. } => None,
     }
 }
 
@@ -345,6 +389,16 @@ fn the_output_matches_the_snapshot() {
     assert_eq!(got.len(), want.len(), "event count differs");
 }
 
+#[test]
+fn the_driver_loop_gives_the_same_events() {
+    let a: Vec<Vec<u8>> = run().0.into_iter().map(encode_event).collect();
+    let b: Vec<Vec<u8>> = run_driven().into_iter().map(encode_event).collect();
+    assert_eq!(a.len(), b.len());
+    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
+        assert_eq!(x, y, "event {i} differs");
+    }
+}
+
 #[test]
 fn the_replay_is_deterministic() {
     let a: Vec<Vec<u8>> = run().0.into_iter().map(encode_event).collect();
```

- [ ] **Step 4: Check and commit**

```powershell
cargo test -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass, the snapshot unchanged. Workspace: 363 passed, 6 ignored (`atlas-agent` unit tests: 179).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): the driver loop, tested with the CI recording"
```

### Task 5: `win::agent`

**Files:**
- Create: `crates/atlas-agent/src/win/agent.rs`
- Modify: `src/win/mod.rs`, `src/win/services.rs`

- [ ] **Step 1: Start and stop.** `start` does §11.4's order:
  1. checks `ServiceConfig`;
  2. runs `identity::start`;
  3. starts the services, turning seeding off without `SeDebugPrivilege`;
  4. builds the pipeline with `WinLookups` and UUIDv7 ids;
  5. creates both intakes, Session A's with a `FastRead`;
  6. starts the sessions, recreating stale ones, and enables Session A's providers;
  7. starts the consumers;
  8. spawns the pipeline thread running `Driver`, with a control channel.

  `Running::stop` flushes, waits out the hold, and stops the sessions. It waits for the consumers to finish, reads their panic counts, then closes them, which drops the intakes and `FastRead`. It drops the spare senders and joins the pipeline thread; the driver drops the services last. Dropping `Running` does the same without waiting for the hold. `Running` also gives `control()`, `config()` and `pipeline_running()` for plan 1b-4.

`crates/atlas-agent/src/win/agent.rs`:
```rust
//! Starting and stopping the sensor (sensor spec §3.2, §11.4; plan 1b-3c,
//! decisions D1 and D4): identity, the two sessions and their consumers, the
//! callbacks' intakes, the Windows services, and the pipeline thread running
//! [`crate::driver::Driver`] into a sink.
//!
//! Plan 1b-4 adds the buffer writer as the sink, the watchdog, Sensor Health,
//! the service and the CLI around this.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::Duration;

use atlas_etw::providers;
use atlas_etw::session::{self, Consumer, EtwError, EventRecord, Session, VersionGate};
use atlas_schema::{Event, EventId};

use super::identity::{self, IdentityError, Started};
use super::lookups::WinLookups;
use super::services::Services;
use super::util::qpc_now;
use crate::config::{Config, ServiceConfig};
use crate::counters::{Counters, IntakeCounters, ServiceCounters};
use crate::driver::{Control, Driver, DriverConfig};
use crate::input::{Header, Session as Which};
use crate::intake::{Intake, Queues};
use crate::pipeline::{Pipeline, Setup};
use crate::watchlist::BadPattern;

/// What to start.
#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// Session A (the manifest providers) and Session B (the system logger's
    /// process events). Tests use `Atlas-Test-*` names (§12.3).
    pub sensor_session: String,
    pub process_session: String,
    pub config: Config,
    pub services: ServiceConfig,
    pub driver: DriverConfig,
    /// `device.json` (§6.1).
    pub device_file: PathBuf,
}

#[derive(Debug)]
pub enum AgentError {
    Identity(IdentityError),
    Etw(EtwError),
    Watchlist(BadPattern),
    /// A configuration `check()` refused.
    Config(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Identity(e) => write!(f, "{e}"),
            AgentError::Etw(e) => write!(f, "{e}"),
            AgentError::Watchlist(e) => write!(f, "watchlist: {e:?}"),
            AgentError::Config(e) => write!(f, "configuration: {e}"),
        }
    }
}

impl std::error::Error for AgentError {}

/// The running sensor. [`Running::stop`] stops it cleanly; dropping it stops
/// it without waiting for the hold.
pub struct Running {
    sensor: Option<Session>,
    process: Option<Session>,
    consumers: Vec<Consumer>,
    /// Senders kept so the queues stay open while a session is restarted
    /// (plan 1b-4's watchdog); dropped at stop, which ends the pipeline loop.
    spare: Option<Queues>,
    pipeline: Option<JoinHandle<Counters>>,
    control: Sender<Control<WinLookups>>,
    intake: Arc<IntakeCounters>,
    services: Arc<ServiceCounters>,
    seeding: bool,
    started: Started,
    /// The pipeline's settings as started (seeding turned off without
    /// `SeDebugPrivilege`): what a replacement `Intake` needs (plan 1b-4).
    config: Config,
}

/// What a stopped sensor counted.
#[derive(Debug)]
pub struct Stopped {
    pub pipeline: Counters,
    pub intake: Arc<IntakeCounters>,
    pub services: Arc<ServiceCounters>,
    /// Callbacks that panicked, both sessions (`loss.callback_panics`).
    pub callback_panics: u64,
}

/// Starts the sensor (§11.4): identity, the services, both sessions and their
/// consumers, then the pipeline thread. The pipeline's start-up seeding pass
/// is asked for on its first pass, after the sessions started (§3.2).
pub fn start(opts: AgentOptions, sink: impl FnMut(Event) + Send + 'static) -> Result<Running, AgentError> {
    opts.services.check().map_err(|e| AgentError::Config(e.to_string()))?;
    let started = identity::start(&opts.device_file).map_err(AgentError::Identity)?;
    let services = Services::start(&opts.services);
    let mut config = opts.config.clone();
    if !services.seeding() {
        config.seed_on_start = false;
        config.seed_on_miss = false;
    }
    let setup = Setup {
        config: config.clone(),
        ticks: started.ticks,
        anchor: started.anchor,
        identity: started.identity,
        current_control_set: started.current_control_set,
        self_keys: vec![started.self_key],
        started: started.started,
    };
    let pipeline =
        Pipeline::new(setup, WinLookups::new(), Box::new(|_| EventId::new_v7())).map_err(AgentError::Watchlist)?;

    let (ktx, krx) = sync_channel(opts.driver.kernel_queue_cap);
    let (utx, urx) = sync_channel(opts.driver.user_queue_cap);
    let queues = || Queues { kernel: ktx.clone(), user: utx.clone() };
    let intake_counters = Arc::new(IntakeCounters::default());
    let self_keys = [started.self_key];
    let fast_read = config.registry_value_reads.then(|| services.fast_read());
    let sensor_intake =
        Intake::new(Which::Sensor, &config, started.ticks, queues(), intake_counters.clone(), fast_read, &self_keys);
    let process_intake =
        Intake::new(Which::Process, &config, started.ticks, queues(), intake_counters.clone(), None, &self_keys);

    let sensor = Session::start(&session::Config::session_a(&opts.sensor_session)).map_err(AgentError::Etw)?;
    for e in providers::session_a(config.network_udp) {
        sensor.enable(&e).map_err(AgentError::Etw)?;
    }
    let process = Session::start(&session::Config::session_b(&opts.process_session)).map_err(AgentError::Etw)?;
    let consumers = vec![
        session::consume(&opts.sensor_session, callback(Which::Sensor, sensor_intake)).map_err(AgentError::Etw)?,
        session::consume(&opts.process_session, callback(Which::Process, process_intake)).map_err(AgentError::Etw)?,
    ];

    let service_counters = services.counters();
    let seeding = services.seeding();
    let (control, control_rx) = channel();
    let driver =
        Driver::new(pipeline, services, krx, urx, Box::new(qpc_now), Box::new(identity::anchor), opts.driver.clone())
            .with_control(control_rx);
    let pipeline = std::thread::Builder::new()
        .name("atlas-pipeline".into())
        .spawn(move || driver.run(sink))
        .expect("spawn the pipeline thread");
    Ok(Running {
        sensor: Some(sensor),
        process: Some(process),
        consumers,
        spare: Some(queues()),
        pipeline: Some(pipeline),
        control,
        intake: intake_counters,
        services: service_counters,
        seeding,
        started,
        config,
    })
}

/// The ETW callback (§3.2 [1]): parse with the version check, then the intake.
fn callback(which: Which, mut intake: Intake) -> impl FnMut(&EventRecord) + Send + 'static {
    let mut gate = VersionGate::new();
    move |rec| {
        let header =
            Header { session: which, pid: rec.pid(), tid: rec.tid(), ts: rec.timestamp(), start_key: rec.start_key() };
        intake.on_event(header, gate.parse(rec));
    }
}

impl Running {
    /// Whether the seeder runs (`housekeeping.seeding_enabled`).
    pub fn seeding(&self) -> bool {
        self.seeding
    }

    pub fn started(&self) -> &Started {
        &self.started
    }

    pub fn intake_counters(&self) -> &Arc<IntakeCounters> {
        &self.intake
    }

    pub fn service_counters(&self) -> &Arc<ServiceCounters> {
        &self.services
    }

    /// The pipeline's settings as started.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Work to run on the pipeline thread between two passes (`driver::Control`):
    /// the canary's self key, Sensor Health reads (plan 1b-4).
    pub fn control(&self) -> Sender<Control<WinLookups>> {
        self.control.clone()
    }

    /// Whether the pipeline thread still runs: if it panicked, the callbacks'
    /// sends fail and nothing is emitted (the watchdog's check, plan 1b-4).
    pub fn pipeline_running(&self) -> bool {
        self.pipeline.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// A clean stop (§11.4): flush both sessions, wait out the hold, stop the
    /// sessions so the consumers deliver the rest and finish, then let the
    /// pipeline drain and stop. No event logged before this call is lost.
    pub fn stop(mut self) -> Stopped {
        for s in [&self.sensor, &self.process].into_iter().flatten() {
            let _ = s.flush();
        }
        std::thread::sleep(self.config.hold);
        self.finish()
    }

    fn finish(&mut self) -> Stopped {
        for s in [self.sensor.take(), self.process.take()].into_iter().flatten() {
            let _ = s.stop();
        }
        // A stopped session's consumer delivers what is left and finishes; its
        // panic count is read after that (review R-m7).
        let t0 = std::time::Instant::now();
        while self.consumers.iter().any(|c| !c.is_finished()) && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let callback_panics = self.consumers.iter().map(Consumer::panics).sum();
        // Closing a consumer drops its callback, and with it the intake's senders
        // and Session A's `FastRead`: before the services go (R-m11).
        for c in self.consumers.drain(..) {
            c.close();
        }
        self.spare = None;
        let pipeline = self.pipeline.take().map(|t| t.join().expect("the pipeline thread")).unwrap_or_default();
        Stopped { pipeline, intake: self.intake.clone(), services: self.services.clone(), callback_panics }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.pipeline.is_some() {
            self.finish();
        }
    }
}
```

```diff
--- a/crates/atlas-agent/src/win/mod.rs
+++ b/crates/atlas-agent/src/win/mod.rs
@@ -5,12 +5,15 @@
 //! - [`lookups::WinLookups`]: the [`crate::services::Lookups`] the pipeline thread calls.
 //! - [`Services`]: the hash workers, the reader and expander lanes and the seeder, answering
 //!   [`crate::services::Request`]s.
+//! - [`agent`]: the sessions, consumers and pipeline thread, started and stopped together
+//!   (plan 1b-3c).
 
 // The structure offsets used here (handle table, telemetry, process list,
 // directory entries) are the x64 layouts.
 #[cfg(not(target_pointer_width = "64"))]
 compile_error!("atlas-agent's Windows services support 64-bit Windows only");
 
+pub mod agent;
 mod expand;
 mod handles;
 mod hash;
```

```diff
--- a/crates/atlas-agent/src/win/services.rs
+++ b/crates/atlas-agent/src/win/services.rs
@@ -60,6 +60,17 @@ pub struct Services {
     counters: Arc<ServiceCounters>,
 }
 
+/// The driver's view (plan 1b-3c): the same two calls.
+impl crate::driver::Lanes for Services {
+    fn submit(&self, r: Request) {
+        Services::submit(self, r);
+    }
+
+    fn replies(&self) -> Vec<Reply> {
+        Services::replies(self).collect()
+    }
+}
+
 impl Services {
     /// Starts the threads. Enables `SeBackupPrivilege` for value reads and
     /// `SeDebugPrivilege` for the seeder; without them reads use a normal open
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 363 passed, 6 ignored (`atlas-agent` unit tests: 179). (`win::agent` is exercised by Task 6's live test.)
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): win::agent: sessions, consumers and the pipeline thread, started and stopped"
```

### Task 6: The agent-level live test, and the CI job

**Files:**
- Create: `crates/atlas-agent/tests/live.rs`
- Modify: `.github/workflows/ci.yml`, `crates/atlas-agent/src/win/value.rs`

- [ ] **Step 1: The test** (D2). The actor is this test binary again, talking over its stdin and stdout:
  1. it opens a file handle and a key handle and creates the watched file, then prints `READY`;
  2. the observer starts the agent and waits for the start-up seeding pass (four table reads);
  3. the observer writes `GO`; the actor runs the scenario and prints `RESULT`;
  4. the observer waits out the deadlines (6 s), stops the agent cleanly, writes `DONE`, and checks the actor tree's events;
  5. only then does the actor clean up: the value reads come about a second after the sets, and deleting the watched file would open it again. A drop guard cleans up if the actor fails.

  Paths are reported in long form (the runner's `TEMP` is 8.3), and key paths are compared ignoring case (F8). Before stopping, the observer reads the pipeline's counters through `control()`, and it checks that the self-filter dropped some of the agent's own events.

`crates/atlas-agent/tests/live.rs`:
```rust
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
```

- [ ] **Step 2: The CI job** runs it after the service tests and uploads its report:

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -83,11 +83,13 @@ jobs:
           path: ${{ runner.temp }}/etw/
           if-no-files-found: ignore
 
-  # Tier 3 for atlas-agent's Windows services (plan 1b-3b): the tests that need
+  # Tier 3 for atlas-agent (plans 1b-3b, 1b-3c): the Windows-service tests that need
   # SeDebugPrivilege or SeBackupPrivilege (seeding from the handle table, value
-  # reads with backup semantics, the System process's creation time). They are
-  # #[ignore]d locally; the hosted runner's account is an administrator. Plan
-  # 1b-3c adds the agent-level live test here.
+  # reads with backup semantics, the System process's creation time), and the
+  # agent-level live test: real sessions, services and pipeline thread against a
+  # scripted actor (seeding, value reads, 8.3 + watchlist, undelete, enrichment,
+  # self-filter). They are #[ignore]d locally; the hosted runner's account is an
+  # administrator.
   agent-live:
     runs-on: windows-latest
     steps:
@@ -100,6 +102,20 @@ jobs:
       - uses: Swatinem/rust-cache@v2
       - name: Elevated service tests
         run: cargo test -p atlas-agent --lib -- --ignored --test-threads=1 --nocapture
+      - name: Agent live test
+        shell: bash
+        env:
+          ATLAS_AGENT_REPORT: ${{ runner.temp }}/agent/report.json
+        run: |
+          mkdir -p "$RUNNER_TEMP/agent"
+          cargo test -p atlas-agent --test live -- --ignored --test-threads=1 --nocapture
+      - name: Upload the report
+        if: always()
+        uses: actions/upload-artifact@v7
+        with:
+          name: agent-live
+          path: ${{ runner.temp }}/agent/
+          if-no-files-found: ignore
 
   proto:
     runs-on: ubuntu-latest
```

- [ ] **Step 3: The test key's leak** (F7):
  - the test key remembers its symbolic-link children and deletes them through their own handles before the tree;
  - the symlink test checks the key is gone after the drop.

  The leaked keys are volatile (gone at logoff). On a machine that ran 1b-3b's tests they can also be removed by opening each `link` child with `REG_OPTION_OPEN_LINK` and `NtDeleteKey`.

```diff
--- a/crates/atlas-agent/src/win/value.rs
+++ b/crates/atlas-agent/src/win/value.rs
@@ -65,11 +65,11 @@ fn open_key(nt_path: &str, backup: bool) -> Option<Owned> {
 pub(crate) mod tests {
     use super::*;
     use windows::Wdk::Foundation::{NtQueryObject, OBJECT_INFORMATION_CLASS};
-    use windows::Wdk::System::Registry::NtSetValueKey;
+    use windows::Wdk::System::Registry::{NtDeleteKey, NtSetValueKey};
     use windows::Win32::Foundation::UNICODE_STRING;
     use windows::Win32::System::Registry::{
         HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_BINARY, REG_DWORD, REG_LINK, REG_OPTION_CREATE_LINK,
-        REG_OPTION_VOLATILE, REG_SZ, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
+        REG_OPTION_VOLATILE, REG_SZ, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
     };
     use windows::core::{HSTRING, PCWSTR};
 
@@ -79,12 +79,15 @@ pub(crate) mod tests {
     pub(crate) struct TestKey {
         pub hkey: HKEY,
         sub: String,
+        /// Symbolic-link children: `RegDeleteTreeW` follows a link instead of
+        /// deleting it, which left the whole test key behind (plan 1b-3c, F7).
+        links: std::cell::RefCell<Vec<HKEY>>,
     }
 
     impl TestKey {
         pub(crate) fn new(tag: &str) -> Self {
             let sub = format!(r"Software\AtlasTest-{tag}-{}", std::process::id());
-            TestKey { hkey: Self::create(&sub, REG_OPTION_VOLATILE.0), sub }
+            TestKey { hkey: Self::create(&sub, REG_OPTION_VOLATILE.0), sub, links: Default::default() }
         }
 
         fn create(sub: &str, options: u32) -> HKEY {
@@ -109,7 +112,11 @@ pub(crate) mod tests {
         }
 
         pub(crate) fn child(&self, name: &str, options: u32) -> HKEY {
-            Self::create(&format!(r"{}\{name}", self.sub), options)
+            let h = Self::create(&format!(r"{}\{name}", self.sub), options);
+            if options & REG_OPTION_CREATE_LINK.0 != 0 {
+                self.links.borrow_mut().push(h);
+            }
+            h
         }
 
         /// The key's NT name (`\REGISTRY\USER\<SID>\Software\…`).
@@ -148,8 +155,12 @@ pub(crate) mod tests {
 
     impl Drop for TestKey {
         fn drop(&mut self) {
-            // SAFETY: deletes the test tree.
+            // SAFETY: deletes the link keys through their own handles (opened
+            // with KEY_ALL_ACCESS), then the test tree.
             unsafe {
+                for h in self.links.borrow().iter() {
+                    let _ = NtDeleteKey(HANDLE(h.0));
+                }
                 let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.sub.as_str()));
             }
         }
@@ -235,6 +246,13 @@ pub(crate) mod tests {
         let through = format!(r"{}\link", k.path());
         assert_eq!(read(&read_of(through, &units("v")), false), None, "the link itself has no value v");
         assert!(read(&read_of(TestKey::nt_name(target), &units("v")), false).is_some());
+        // Dropping the test key deletes it, link included (plan 1b-3c, F7).
+        let sub = HSTRING::from(k.sub.as_str());
+        drop(k);
+        let mut h = HKEY::default();
+        // SAFETY: test-only; opens (read access) a key that must be gone.
+        let open = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &sub, None, KEY_QUERY_VALUE, &mut h) };
+        assert!(open.is_err(), "the test key is still there");
     }
 
     #[test]
```

- [ ] **Step 4: Check and commit**

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
actionlint .github/workflows/ci.yml
```
Expected: all pass (the live test is ignored). Workspace: 363 passed, 7 ignored (`atlas-agent` unit tests: 179).
```powershell
git add .github crates/atlas-agent
git commit -m "test(agent): the agent-level live test in CI's agent-live job; the test-key leak fixed"
```

### Task 7: The elevated run on the host (the user)

The live tests need administrator rights. Claude writes the script; the user runs it in an elevated PowerShell.

- [ ] **Step 1: The script** (git-ignored, in `spikes/`). It runs on the host and changes nothing lasting:
  - short `Atlas-Test-*` ETW sessions, stopped at the end, also on failure;
  - volatile HKCU test keys and `%TEMP%` test directories, which the tests delete;
  - two short-lived children;
  - build output in the checkout.

  It runs from the repo by default; `-wt` names another checkout.

`spikes/run-1b3c-elevated.ps1`:
```powershell
# Plan 1b-3c: the agent-level live test and the other elevated tests, in an
# elevated shell, from the repo (or the checkout given as -wt).
# Runs on the HOST, about 5 minutes the first time (a debug build in the
# checkout), then about 2. Changes nothing lasting:
#   - short ETW sessions named Atlas-Test-* (stopped at the end, also on failure);
#   - volatile HKCU test keys and %TEMP% test directories (deleted by the tests);
#   - two short-lived cmd.exe / ping.exe children;
#   - build output in the worktree's target directory.
# 1. the agent live test (crates/atlas-agent/tests/live.rs);
# 2. atlas-agent's elevated service tests (1b-3b), including the test-key fix;
# 3. atlas-etw's live test with the new actor steps (no recording is written).
# Run from an elevated PowerShell (Windows PowerShell 5.1 or pwsh 7):
#   & C:\Users\jakef\Desktop\atlas-edr\spikes\run-1b3c-elevated.ps1
# (verify-first, from the scratch worktree: add -wt C:\Users\jakef\Desktop\atlas-wt-1b3c)
param([string]$wt = 'C:\Users\jakef\Desktop\atlas-edr')

$ErrorActionPreference = 'Stop'
try { Stop-Transcript | Out-Null } catch { }

$out = 'C:\Users\jakef\Desktop\atlas-edr\spikes\results\1b3c-elevated'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
New-Item -ItemType Directory -Force -Path $out | Out-Null
Start-Transcript -Path (Join-Path $out "transcript-$stamp.txt") | Out-Null

function Invoke-Cargo([string]$name, [string[]]$cargoArgs) {
    $log = Join-Path $out "$name-$stamp.txt"
    Write-Host "== ${name}: cargo $($cargoArgs -join ' ')"
    # cargo reports progress on stderr; don't let PowerShell treat it as an error.
    $ErrorActionPreference = 'Continue'
    & cargo @cargoArgs 2>&1 | ForEach-Object { $line = "$_"; Write-Host $line; Add-Content -Path $log -Value $line -Encoding UTF8 }
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $summary = Select-String -Path $log -Pattern '^test result:' | Select-Object -Last 1
    if ($code -ne 0) { return "FAIL ${name}: exit $code ($($summary.Line))" }
    if (-not $summary) { return "FAIL ${name}: no test summary" }
    if ($summary.Line -notmatch ' 0 ignored') { return "FAIL ${name}: tests still ignored: $($summary.Line)" }
    return "PASS ${name}: $($summary.Line)"
}

$results = @()
try {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Not elevated: open PowerShell with "Run as administrator" and run this line again.'
    }
    if (-not (Test-Path (Join-Path $wt 'Cargo.toml'))) { throw "Worktree not found: $wt" }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo is not on PATH in this window.' }
    Write-Host "Windows build: $([Environment]::OSVersion.Version)"
    Set-Location $wt
    $env:ATLAS_AGENT_REPORT = Join-Path $out "agent-report-$stamp.json"
    $env:ATLAS_ETW_REPORT = Join-Path $out "etw-report-$stamp.json"
    Remove-Item Env:ATLAS_ETW_RECORD -ErrorAction SilentlyContinue
    $results += Invoke-Cargo 'agent-live' @('test', '-p', 'atlas-agent', '--test', 'live', '--', '--ignored', '--test-threads=1', '--nocapture')
    $results += Invoke-Cargo 'agent-elevated' @('test', '-p', 'atlas-agent', '--lib', '--', '--include-ignored', '--test-threads=1')
    $results += Invoke-Cargo 'etw-live' @('test', '-p', 'atlas-etw', '--test', 'live', '--', '--ignored', '--test-threads=1', '--nocapture')
}
catch {
    $results += "FAIL: $_"
}
finally {
    foreach ($s in @(logman query -ets) | Select-String -Pattern 'Atlas-Test-\S+' -AllMatches) {
        foreach ($m in $s.Matches) { logman stop $m.Value -ets | Out-Null; "stopped leftover session $($m.Value)" }
    }
    foreach ($r in $results) { Write-Host $r -ForegroundColor $(if ($r -like 'PASS*') { 'Green' } else { 'Red' }) }
    Stop-Transcript | Out-Null
    Write-Host "Results in $out"
}
```

Check that it parses in both `powershell.exe` and `pwsh` before handing it over.

- [ ] **Step 2: The user runs it** from an elevated PowerShell: `& C:\Users\jakef\Desktop\atlas-edr\spikes\run-1b3c-elevated.ps1`.

Expected: three `PASS` lines (`agent-live`, `agent-elevated`, `etw-live`); in the agent report all 19 checks pass, and `cleanup_unpaired` is in the notes. Claude reads `spikes\results\1b3c-elevated\` and records the numbers in the PR.

### Task 8: Documentation

- [ ] **Step 1: The spec and the schema reference.** In `docs/specs/2026-10-01-etw-sensor-design.md`:
  - the status line (its date is the build's merge day: change `2026-10-07` if it differs);
  - clarification 11, from the review, in §3.2, §5.1 and §10.3;
  - §3.2: clarifications 1 and 4;
  - §4.2: clarification 3;
  - §5.1: clarifications 2, 5 and 6;
  - §5.5: clarification 7;
  - §7.1: clarification 2;
  - §10.3: clarification 6;
  - §11.4: clarification 8;
  - §12.1, §12.3: clarification 9;
  - §15.3: a "Plan 1b-3c" addendum with F1–F8;
  - §16: clarification 10;
  - §17: a "Plan 1b-3c clarifications" paragraph.

  Also in the docs:
  - `docs/schema-reference.md`: `quality` gains file deletes without an outcome, and `housekeeping`'s dropped failed operations become creates and renames.
  - `docs/architecture-overview.md`: roadmap row 1 ("plans 1b-1 to 1b-3c done; plan 1b-4 next", with a link to this plan), and a decision-log row for the build.

```diff
--- a/docs/specs/2026-10-01-etw-sensor-design.md
+++ b/docs/specs/2026-10-01-etw-sensor-design.md
@@ -1,6 +1,6 @@
 # Sub-project 1 — ETW Sensor Design (Agent Core)
 
-**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (a minimal driver and the agent-level live test) is next. Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
+**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-07); plan 1b-4 is next. Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
 **Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
 **Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).
 
@@ -116,11 +116,13 @@ ETW (Session A: manifest providers; Session B: system logger, process events)
 **Service lanes** (plan 1b-3b). The hash workers, the reader lane, the expander lane and the seeder each take at most 8,192 waiting requests. A full lane drops the request and counts `service_queue_drops`: a dropped work request costs its event's deadline, and a dropped invalidation clears the expansion cache. The reader lane does no file I/O, so a stalled directory never delays a value read. The pipeline thread takes no lock to hand work out.
 
 **One thread for [2], [3] and [5]** (plan 1b-3a, D2). The three stages are one loop on the pipeline thread, with the clock passed in: the driver pushes queued events and worker replies, and calls `tick(now)` at least every 50 ms, which returns the events to emit in order. Measured at 0.57 µs per event in steady state, and 0.91 µs at 52,000 events/s of never-repeated addresses.
+
+**The driver** (plan 1b-3c, D1, D4). The pipeline thread runs one pass every 10 ms: drain both queues into the pipeline, feed in the services' replies, `tick` with the current QPC, hand the new requests to the services, and pass the emitted events, in order, to the sink (the buffer writer, plan 1b-4). It re-takes the anchor every 60 s (§3.3). The loop is portable and tested with fakes, including the CI recording, which it turns into exactly the events of the synchronous replay. The loop ends when both queues are closed: it applies the replies already in and stops the pipeline (§11.4). It owns the services, so they go after the callbacks' senders and Session A's `FastRead`.
 - Stream time advances to each released event before that event is processed. Seeder snapshots, confirm windows and Launch halves that fall due by then are applied first.
 - Sweeps over whole tables (UDP idle, cache retention) run once per second of stream time.
 - A pending event's request bookkeeping is freed when the event leaves [5], whether or not its reply came; a later reply is ignored. Each request gets at most one reply.
 
-**Backpressure.** The ETW callback must stay fast: a slow consumer makes ETW drop events in the kernel. The callback parses, enqueues and keeps the small early registry map (a hash-map operation per CreateKey, OpenKey or CloseKey). Successful OperationEnds, about half of Kernel-File's volume, are discarded right there: they cost parse time but never reach [2]. Queues never block (kernel queue default 65,536 entries; user-mode queue 8,192); drops are counted per queue. User-mode providers get their own queue so a process flooding forged DNS-Client events (§4.4) cannot push kernel events out.
+**Backpressure.** The ETW callback must stay fast: a slow consumer makes ETW drop events in the kernel. The callback parses, enqueues and keeps the small early registry map (a hash-map operation per CreateKey, OpenKey or CloseKey). Successful OperationEnds, about half of Kernel-File's volume, are discarded right there: they cost parse time but never reach [2]. The exceptions are a Cleanup's OperationEnd that reports a delete, and a delete request's (§5.1, plan 1b-3c): the callback remembers each Cleanup's and each request's `Irp` until its OperationEnd, one or two hash-map operations per Kernel-File event. Queues never block (kernel queue default 65,536 entries; user-mode queue 8,192); drops are counted per queue. User-mode providers get their own queue so a process flooding forged DNS-Client events (§4.4) cannot push kernel events out.
 
 ### 3.3 Time base
 
@@ -151,7 +153,7 @@ Each provider is enabled with the listed keywords and an `EVENT_FILTER_TYPE_EVEN
 | Provider | Keywords | Event IDs allowed |
 |---|---|---|
 | Microsoft-Windows-Kernel-Process | `WINEVENT_KEYWORD_PROCESS`, `WINEVENT_KEYWORD_IMAGE` | 1 ProcessStart, 2 ProcessStop, 5 ImageLoad |
-| Microsoft-Windows-Kernel-File | `FILEIO` 0x20, `OP_END` 0x40, `CREATE` 0x80, `WRITE` 0x200, `DELETE_PATH` 0x400, `RENAME_SETLINK_PATH` 0x800, `CREATE_NEW_FILE` 0x1000 (mask `0x1EE0`) | 12 Create, 13 Cleanup, 14 Close, 16 Write, 17 SetInformation, 24 OperationEnd, 26 DeletePath, 27 RenamePath, 30 CreateNewFile |
+| Microsoft-Windows-Kernel-File | `FILEIO` 0x20, `OP_END` 0x40, `CREATE` 0x80, `WRITE` 0x200, `DELETE_PATH` 0x400, `RENAME_SETLINK_PATH` 0x800, `CREATE_NEW_FILE` 0x1000 (mask `0x1EE0`) | 12 Create, 13 Cleanup, 14 Close, 16 Write, 17 SetInformation, 18 SetDelete, 24 OperationEnd, 26 DeletePath, 27 RenamePath, 30 CreateNewFile |
 | Microsoft-Windows-Kernel-Registry | `CloseKey` 0x1, `SetValueKey` 0x100, `DeleteValueKey` 0x200, `CreateKey` 0x1000, `OpenKey` 0x2000, `DeleteKey` 0x4000 (mask `0x7301`) | 1 CreateKey, 2 OpenKey, 3 DeleteKey, 5 SetValueKey, 6 DeleteValueKey, 13 CloseKey |
 | Microsoft-Windows-Kernel-Network | `IPV4` 0x10, `IPV6` 0x20 (mask `0x30`) | TCP 12/28 connect, 13/29 disconnect, 15/31 accept; UDP 42/43/58/59 (`network.udp`, on by default, §7.3) |
 | Microsoft-Windows-DNS-Client | the Operational channel keyword `0x8000000000000000`, the only one that delivers 3008 (S3) | 3008 |
@@ -184,7 +186,7 @@ DNS-Client logs from user mode. Any process can register its provider GUID and w
 | Module Load | Kernel-Process 5 | `actor` = payload `ProcessID` (§5.3); `module.file` from `ImageName`; `base_address` = `ImageBase`; hashes/signature per §6.3. |
 | File Create | Kernel-File 30 `CreateNewFile` | New files only; 30 fires only when the create succeeded (S6). Overwriting an existing file has no 30: it is a truncation and maps to File Update (§7.1). |
 | File Update | Kernel-File 16 `Write`; 17 `SetInformation` with `InfoClass` 19 (end of file) | Coalesced per handle (§7.1). A truncation on a mapped handle (the overwrite case) counts as a write. |
-| File Delete | Kernel-File 26 `DeletePath`; 12 `Create` with `FILE_DELETE_ON_CLOSE` | `file.path` from `FilePath`, emitted unless a failed OperationEnd arrives within the confirm window (§5.5). A handle created with `FILE_DELETE_ON_CLOSE` (`CreateOptions` bit `0x1000`) produces no 26; Delete is emitted at its Cleanup (§7.1). |
+| File Delete | Kernel-File 13 `Cleanup` and its 24 `OperationEnd`; requests from 26 `DeletePath`, 18 `SetDelete` and 12 `Create` with `FILE_DELETE_ON_CLOSE` | Emitted at the Cleanup that removed the name, as the file system reports it (plan 1b-3c, D3): the Cleanup's OperationEnd carries `FILE_CLEANUP_*` (`ntifs.h`) in `ExtraInformation`: `0x4` file deleted, `0x8` a hard link, `0x10` a stream, `0x20` set with them for a POSIX-style delete, `0x2` nothing removed, `0` unknown; any other value is not an outcome. `file.path` is the 26's `FilePath` (current after a rename, and known for a handle the agent never saw opened; a link's or a stream's own path for those), else the handle's name. The actor is whoever asked: the process of the 26 or 18 that set the disposition, or the opener of a delete-on-close handle; with no request seen, the process whose Cleanup removed the name. A reported delete is no longer outstanding. A request does not emit: an undelete (the disposition set, then cleared; 18 carries 1 for a set and 0 for a clear, on any handle for the file) reports `0x2` and gives no Delete, and a failed delete never reports one, so no confirm window is needed. A request whose own operation fails (a read-only file) is taken back when its OperationEnd comes, so a `FileKey` reused by another file does not inherit it; the callback passes on the request's successful OperationEnd too, because each thread reuses one `Irp` for every operation and a later one may fail. A clear on a delete-on-close handle is taken as clearing that handle's flag (`FileDispositionInformationEx`), and leaves a disposition another handle set. A POSIX delete while another handle is open is reported twice (the name at the deleter's Cleanup with `0x20`, the file at the last handle's): it is one Delete, the second report matched by `FileKey` and a handle opened before the first. **Pairing a Cleanup with its outcome:** the next OperationEnd of the Cleanup's `Irp`, within 1 s; an outcome delivered before its Cleanup (logged on another CPU) is paired when the Cleanup comes; any other operation on the `Irp` ends the wait, and a Cleanup left unpaired is counted as `quality.file_cleanup_unpaired`. Where the outcome is unknown (an SMB share), a Cleanup of a file with a request outstanding is the Delete, counted as `quality.file_delete_outcome_unknown`. With `file.op_end` off there is no outcome, and the request is the Delete, as before 1b-3c. |
 | File Rename | Kernel-File 27 `RenamePath` | `file` (the original) from the `FileObject` map; `file_result` from `FilePath`, which is the new name (S6). Emitted unless a failed OperationEnd arrives within the confirm window (§5.5); a confirmed rename also updates the map entry (§7.1). |
 | File SetAttributes | Kernel-File 17 `SetInformation` | Only `InfoClass` 4 = `FileBasicInformation` (timestamps and attributes; timestomping). |
 | File Open (new, §10.1) | Kernel-File 12 `Create` | Only paths matching the watchlist (§7.2). |
@@ -247,13 +249,13 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 
 - **Failed operations are dropped.**
   - Registry: keep only `Status` = 0. This also drops the `STATUS_REPARSE` (`0x104`) first attempt that precedes a `CurrentControlSet` open (S7).
-  - Kernel-File: 30 fires only on success. Create (12), DeletePath (26) and RenamePath (27) fire before the outcome (S6). Session A's callback forwards only failed OperationEnds: 24s whose `Status` has **error** severity (its top two bits set). Success, informational and warning codes such as `STATUS_REPARSE` (0x104), `STATUS_OPLOCK_BREAK_IN_PROGRESS` (0x108) and `STATUS_BUFFER_OVERFLOW` (0x80000005) are not failures. This is narrower than `!NT_SUCCESS`, which also counts warnings (plan 1b-2). A failure is the exception, so success is the default.
-  - [3] holds each 12, 26 and 27 for a **failure-confirm window** of stream time: default 250 ms after its timestamp, configurable. By then its OperationEnd, logged within microseconds of the operation, has almost surely passed the ordering stage too.
+  - Kernel-File: 30 fires only on success. Create (12), DeletePath (26) and RenamePath (27) fire before the outcome (S6). A Delete comes from the Cleanup that removed the name (§5.1), so a failed delete has nothing to cancel; only 12 and 27 need the window (plan 1b-3c). Session A's callback forwards only failed OperationEnds: 24s whose `Status` has **error** severity (its top two bits set). Success, informational and warning codes such as `STATUS_REPARSE` (0x104), `STATUS_OPLOCK_BREAK_IN_PROGRESS` (0x108) and `STATUS_BUFFER_OVERFLOW` (0x80000005) are not failures. This is narrower than `!NT_SUCCESS`, which also counts warnings (plan 1b-2). A failure is the exception, so success is the default.
+  - [3] holds each 12 and 27 for a **failure-confirm window** of stream time: default 250 ms after its timestamp, configurable. By then its OperationEnd, logged within microseconds of the operation, has almost surely passed the ordering stage too.
   - A failed 24 within the window cancels the event: dropped, `file_op_failed` counted, and for a 12 its map entry removed. It is matched to the most recent pending event with the same `Irp` and an earlier timestamp. Irps are recycled, so the window keeps the match local.
   - No failure by the end of the window means the event stands; it is emitted, or for a 12 its entry stays. A short ring of recent failed 24s (also covering the window) catches a failure that arrives before its event.
   - The window bounds the table; pending entries expire by stream time.
   - A slow operation that completes after the window is treated as successful. A failure it later reports is counted as `file_op_late_failure`, not reversed (§16). Such a failure is recognised for 40 confirm windows (10 s) after its operation stood (plan 1b-3a).
-  - With `file.op_end` off (§11.2, §13), Create, DeletePath and RenamePath are emitted at once, without the window.
+  - With `file.op_end` off (§11.2, §13), Create, DeletePath and RenamePath are emitted at once, without the window; a DeletePath is then the Delete (§5.1).
 - **File paths:** NT device paths (`\Device\HarddiskVolume3\…`) → drive paths (`C:\…`) via a device map from `QueryDosDeviceW`, built at start and refreshed every 60 s and on a lookup miss (at most once per 5 s). Prefixes match only on a path-component boundary (`HarddiskVolume1` never matches `HarddiskVolume10\…`). Unmappable paths (shadow copies, network redirectors, unmounted volumes) stay as NT paths. Only `\Device\…` targets are kept: a `subst` drive's target is itself a drive path (plan 1b-3b).
 - **Registry paths:** come from the key map (§7.4); `KeyName` and `BaseName` are always empty (S7). Then: `\REGISTRY\MACHINE\…` → `HKLM\…`; `\REGISTRY\USER\<SID>\…` → `HKU\<SID>\…`; and `HKLM\SYSTEM\ControlSet00N\…` → `HKLM\SYSTEM\CurrentControlSet\…` when N is the current control set (from `HKLM\SYSTEM\Select\Current`, read at start). These are the forms Sigma uses. WOW64 views stay as `…\WOW6432Node\…` (the kernel logs the real path).
 - **Self-filtering** happens at emission, after [3] has updated its maps, joins and canary matching. Otherwise the agent's own CreateKey/OpenKey would be missing from the key map, and the registry canary could not be named.
@@ -315,8 +317,8 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 
 **Cleanup (13), the last handle closed.**
 - A written entry emits **one** File Update.
-- A `delete_on_close` entry emits a File Delete.
-- The actor of both is the entry's opener, never the Cleanup's header. The cache manager, or the agent's own duplicate during seeding (§7.4), can be the closing context.
+- A Cleanup whose OperationEnd reports a removed name emits a File Delete (§5.1). A `delete_on_close` entry records its opener as the file's requester at its Cleanup, so the Delete keeps that actor when another handle's Cleanup is the one that deletes (plan 1b-3c).
+- The actor of an Update is the entry's opener, never the Cleanup's header. The cache manager, or the agent's own duplicate during seeding (§7.4), can be the closing context.
 - A File Update or Delete from a provisional entry waits in [5] with a "drop at deadline" seeder reason (§3.2): if the entry is still unresolved at the deadline, the event is dropped and `unknown_file_object` counts it. This is a deliberate carve-out from E11: a file event without a path, and usually without a trustworthy actor, has no detection value.
 
 **Close and after.**
@@ -329,7 +331,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 - A RenamePath on a handle whose name is unknown is emitted at once, with an empty source. It does not wait for the seeder, because a snapshot read after the rename could only give the new name. The handle takes the new name once the rename stands.
 - With on-miss seeding off (§11.2), such events wait only for the start-up pass, while it is outstanding.
 
-**Bounds.** The map is bounded (cap + counter; past the cap, the least recently used eighth goes at once, plan 1b-3a). Seeded entries do not know `delete_on_close`, so a handle opened with that flag before the agent started produces no Delete (§16).
+**Bounds.** The map is bounded (cap + counter; past the cap, the least recently used eighth goes at once, plan 1b-3a). Seeded entries do not know `delete_on_close`, but the Cleanup outcome still reports the delete of a handle opened with that flag before the agent started (plan 1b-3c); its actor is then the process whose Cleanup removed the name, the handle's owner.
 
 ### 7.2 Watchlist
 
@@ -519,6 +521,8 @@ One activity, `Report`, carrying the §9.3 counters (`u64`, all optional), the i
 
 **Plan 1b-3b addition:** `housekeeping.service_queue_drops`, for requests dropped because a service lane was full (§3.2).
 
+**Plan 1b-3c additions:** `quality.file_delete_outcome_unknown`, for Deletes reported from the request alone because the file system reported no Cleanup outcome, and `quality.file_cleanup_unpaired`, for Cleanups whose outcome could not be paired with them (§5.1).
+
 ### 10.4 Registry fields (additive, from S4 and S7)
 
 | Message | Field | Meaning |
@@ -563,7 +567,7 @@ Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9
 ### 11.4 Lifecycle
 
 - **Start:** take the mutex → verify the data directory (§11.1) → load config → load `device.json` → compute `boot_id` → start sessions (recreating stale ones) → seed the process cache from the rundown → start threads → seed the key and file maps from the handle table (§7.4).
-- **Clean stop:** flush both sessions → drain the ordering and completion stages → flush the buffer → stop the sessions. No in-flight event is lost on a clean stop. Pending events go out as at their deadlines, so one marked "drop at deadline" is dropped (plan 1b-3a).
+- **Clean stop:** flush both sessions → wait out the hold → stop the sessions, so the consumers deliver the rest and finish → drain the ordering and completion stages → flush the buffer (plan 1b-3c: the queues close when the consumers do, which ends the pipeline loop). No in-flight event is lost on a clean stop. Pending events go out as at their deadlines, so one marked "drop at deadline" is dropped (plan 1b-3a).
 - **Diagnostic log:** `tracing` with a rolling file in `logs\`; its writes are self-filtered (§5.5).
 
 ## 12. Testing
@@ -571,7 +575,7 @@ Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9
 ### 12.1 Tier 1 — pure, Linux CI
 
 - Parser unit tests per event and version, built from fixture byte strings, for both pointer sizes where relevant.
-- **Property tests:** the ordering stage releases in timestamp order and passes late events through; the completion stage preserves order; actor lookup-at-timestamp resolves correctly across PID reuse; the coalescer emits exactly one Update per written handle (truncations included) and a Delete at Cleanup for delete-on-close handles; a Create, DeletePath or RenamePath is dropped iff a failed OperationEnd for its `Irp` falls in its confirm window, with recycled `Irp`s, a failure arriving before its event, and late failures; the key map resolves names through any chain of relative opens, including bases resolved later by the seeder, removes entries on CloseKey and keeps tombstones while children are pending; seeder snapshots apply only by the stream-time rule (§7.4), never name an address that changed owner after the event, and never overwrite newer ETW entries; the negative cache stops re-reads; an early (fast-path) read that disagrees with [3]'s path is redone; every UDP Open gets exactly one Close; retention and head + tail overflow never delete the pinned head; buffer round-trip; recovery after truncation at **every** byte offset of a segment (simulated torn writes).
+- **Property tests:** the ordering stage releases in timestamp order and passes late events through; the completion stage preserves order; actor lookup-at-timestamp resolves correctly across PID reuse; the coalescer emits exactly one Update per written handle (truncations included); a Create or RenamePath is dropped iff a failed OperationEnd for its `Irp` falls in its confirm window, with recycled `Irp`s, a failure arriving before its event, and late failures; the key map resolves names through any chain of relative opens, including bases resolved later by the seeder, removes entries on CloseKey and keeps tombstones while children are pending; seeder snapshots apply only by the stream-time rule (§7.4), never name an address that changed owner after the event, and never overwrite newer ETW entries; the negative cache stops re-reads; an early (fast-path) read that disagrees with [3]'s path is redone; every UDP Open gets exactly one Close; retention and head + tail overflow never delete the pinned head; buffer round-trip; recovery after truncation at **every** byte offset of a segment (simulated torn writes).
 - **Fuzz targets** (each its own fuzz workspace under its crate, run 10 min nightly): `parse_any` (dispatches over every parser), `dns_query_results`, `buffer_recover` (arbitrary bytes as a segment file).
 - **CI wiring** (part of the plan): `fuzz.yml` becomes a matrix over (fuzz workspace, target) with per-target corpus cache keys, crash artifacts, and PR path filters per crate; `ci.yml` runs `cargo check` on each fuzz workspace and `cargo audit` on each fuzz lockfile; `atlas-agent` builds on the Linux job.
 
@@ -586,7 +590,7 @@ Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9
 
 - Tests start real sessions on `windows-latest` (the runner is admin), run the scripted scenario, and assert the expected normalized events, filtered to the test's own process tree, with timeouts. The scenario: spawn a process with a known command line; create, write, overwrite, rename and delete a file; a failed delete (no event); a delete-on-close file; an undelete (disposition set, then cleared) and a delete-on-close cleared through `FileDispositionInformationEx`, which must produce no Delete (result documented either way); open a watchlisted path, also through its 8.3 name; create, set and delete a registry key and value (with the value data read after); a handle opened **before** the agent starts that is then written to (file) or created under (registry), named by seeding; open a TCP connection; send UDP; resolve a name.
 - Marked `#[ignore]` locally; CI runs them explicitly.
-- **Split by crate** (plan 1b-2): `atlas-etw`'s live test covers the ETW level of this scenario (process, files, failed operations, delete-on-close, registry including embedded NULs and the CloseKey handle test, TCP and UDP over IPv4 and IPv6, DNS), plus the session primitives and the event-ID filters. It runs the scenario in a second process (the actor), so the observer's own TDH lookups are not recorded. The agent-level parts (watchlist and 8.3, undelete, seeding, value reads) come with plan 1b-3c. CI's `agent-live` job runs the service tests that need `SeDebugPrivilege` or `SeBackupPrivilege` (plan 1b-3b) and will run 1b-3c's live test. Test sessions are named `Atlas-Test-*`.
+- **Split by crate** (plan 1b-2): `atlas-etw`'s live test covers the ETW level of this scenario (process, files, failed operations, delete-on-close, registry including embedded NULs and the CloseKey handle test, TCP and UDP over IPv4 and IPv6, DNS), plus the session primitives and the event-ID filters. It runs the scenario in a second process (the actor), so the observer's own TDH lookups are not recorded. The agent-level parts (watchlist and 8.3, undelete, seeding, value reads) are `atlas-agent`'s live test (plan 1b-3c, D2): the real sessions, services and pipeline thread against an actor that opens a file and a key handle before the agent starts and is then told, over its stdin, to run the scenario. It also checks the real enrichment of the `cmd.exe` Launch (SHA-256, catalog signature, account, DOS path), that nothing of the agent's own (hashing, value reads, seeding) is emitted, and that no queue dropped. `atlas-etw`'s live test also checks the Cleanup outcomes of a delete, a delete-on-close, an undelete, a hard-link delete and a stream delete, and `SetDelete` on set and clear. CI's `agent-live` job runs the service tests that need `SeDebugPrivilege` or `SeBackupPrivilege` (plan 1b-3b) and the agent live test. Test sessions are named `Atlas-Test-*`.
 
 ### 12.4 Known gaps
 
@@ -708,6 +712,16 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **B4, a failure "before its event"** happens only when the operation itself arrives late: an OperationEnd is always logged after its operation.
 - **B5, the CI recording resolves end to end with no seeding:** every registry path through the actor's own absolute opens, every file actor, and the observer (running before the sessions started) through the rundown.
 
+**Plan 1b-3c (2026-10-06): deletes, the driver and the live test** (verified on the host, build 26200, unelevated and in five elevated probe runs and two test runs; and on the CI runner, 26100):
+- **F1, undeletes gave false Deletes:** every one of five undelete cases (the disposition set, then cleared, through `FileDispositionInformation` and `FileDispositionInformationEx`, on one handle or two) produced a Delete under 1b-3a's rule, and setting it twice produced two. `DeletePath` (26) fires only on a set; `SetDelete` (18) fires on both, with `ExtraInformation` 1 or 0.
+- **F2, `NameDelete` (11) cannot mark a delete:** for ordinary deletes it comes from the System process 0.3–1.2 s later, at the file's final close; a hard-link delete gives none; renames give it at once. It needs the FILENAME keyword, which stays off.
+- **F3, the Cleanup outcome:** the Cleanup's OperationEnd carries the file system's `FILE_CLEANUP_*` outcome on NTFS, FAT32 and exFAT, at once and from the deleting process; an SMB share reports 0. `FileDispositionInformationEx` is refused on FAT, exFAT and SMB.
+- **F4, a POSIX delete while another handle is open** reports `0x24` at the deleter's Cleanup and `0x4` at the last one.
+- **F5, `FileKey` is reused** for a new file within seconds of a delete, so state keyed by it is dropped when the file goes, and the POSIX match also compares the path.
+- **F6, host rates over 60 s:** `NameDelete` 2.4/s, `DeletePath` and `SetDelete` 0.7/s each.
+- **F7, 1b-3b's symbolic-link test leaked its volatile test key** on every run (`RegDeleteTreeW` follows a link instead of deleting it), and a reused PID then failed the test. The link is now deleted through its own handle.
+- **F8, key names keep their hive's case on the runner too:** the CI runner's `HKU\<SID>\SOFTWARE` is upper case, which only a case-sensitive test noticed; paths are compared ignoring case (§5.5).
+
 **Plan 1b-3b (2026-10-06): building the Windows services** (verified on the host, unelevated and in four elevated runs):
 - **F1, `GetLongPathNameW` cannot expand NT paths:** it rejects `\\?\GLOBALROOT\Device\…` (`ERROR_INVALID_NAME`) and costs 1.55 ms for a path with 7 short components. Per-component `NtQueryDirectoryFile` takes about 54 µs per short component (§7.2).
 - **F2, a handle can change between the table read and the duplicate;** names are verified by a second read (§7.4).
@@ -726,8 +740,10 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **Registry key rename** is invisible in v1: no event carries the new name (S7).
 - **Registry value data is read after the event** (§7.5): within about 0.1–0.3 s on the fast path, about 1 s otherwise. A value changed in that window is reported in its new state (type and length checks catch many cases) or as unavailable. An attacker can set a value and replace it at once. Values with types above `REG_QWORD` are emitted without data (`raw_type`, §7.5).
 - **A lost CreateKey/OpenKey (or file Create)** leaves a stale map entry. When the address is reused, the stale name can be attached to the new handle: a wrong path, not an unresolved one. The Sensor Health loss counters flag the affected interval.
-- **Slow file operations:** a delete or rename that fails after the failure-confirm window is reported as having happened (counted as `file_op_late_failure`).
-- **Handles opened with `FILE_DELETE_ON_CLOSE` before the agent started** produce no Delete, because seeding cannot see the flag.
+- **Slow file operations:** a rename that fails after the failure-confirm window is reported as having happened (counted as `file_op_late_failure`).
+- **Deletes on SMB shares** report no Cleanup outcome: the first Cleanup of any handle to a file with a delete request outstanding stands for the Delete, so a disposition cleared after that Cleanup still gives one, and the time is that Cleanup's (`file_delete_outcome_unknown`). ReFS is untested and, if it reports no outcome, takes the same path.
+- **A Cleanup and its outcome delivered far apart** (more than 1 s, or with another operation on the thread's `Irp` in between) are not paired: the delete is missed and counted (`file_cleanup_unpaired`).
+- **A file replaced by a rename** (`MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`) gives no Delete for the replaced target: no Cleanup of a handle to it removes the name.
 - **Unresolvable handles:** handles in processes the agent cannot open (protected processes), and name queries that block, stay unnamed after seeding (§7.4). Their registry events carry `path_unresolved`; their file writes are counted, not emitted.
 - **Hive export** (`reg save HKLM\SAM`) has an ETW event but no schema class yet: a later additive class.
 - **An open is not a read:** watchlist `Open` events show access intent. 8.3 names are expanded before matching (§7.2); a name that cannot be expanded (e.g. a file deleted at once) is matched as logged.
@@ -828,3 +844,15 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 13. the `agent-live` CI job;
 14. known limitations of expansion and hashing;
 15. 64-bit Windows only.
+
+**Plan 1b-3c clarifications (2026-10-06).** Applied to §3.2, §4.2, §5.1, §5.5, §7.1, §10.3, §11.4, §12.1, §12.3, §15.3 and §16:
+1. the driver: a portable loop every 10 ms, the anchor every 60 s, the end when the queues close;
+2. a File Delete comes from the Cleanup outcome, its actor from the request; undeletes give none;
+3. Session A enables Kernel-File 18 `SetDelete`;
+4. the callback passes on the Cleanup outcomes that report a delete;
+5. one Delete for a POSIX delete reported twice;
+6. the request stands for the Delete where the outcome is unknown, counted as `quality.file_delete_outcome_unknown`;
+7. the clean stop's order;
+8. the agent-level live test, and `atlas-etw`'s checks of the outcomes; the delete rules are covered by scenario tests and the replay of the CI recording, not by a property test;
+9. known limitations: SMB and ReFS, a file replaced by a rename, a Cleanup and its outcome delivered far apart; the delete-on-close limitation is gone;
+10. the path from the request's `DeletePath`; a delete request's successful OperationEnd passed on; pairing by the next operation on the `Irp`, within 1 s, in either delivery order; `quality.file_cleanup_unpaired`; a delete-on-close clear keeps another handle's disposition; the driver's control channel (review, Review Log of plan 1b-3c).
```

```diff
--- a/docs/schema-reference.md
+++ b/docs/schema-reference.md
@@ -158,8 +158,8 @@ The agent's own loss, quality, housekeeping, resource and buffer figures, every
 | Group | Holds |
 |---|---|
 | `loss` | ETW events and buffers lost per session, queue drops, DNS rate-limit drops, `actor_dropped` per class, buffer backlog drops, events skipped because the ETW callback panicked |
-| `quality` | late arrivals, parse errors, unknown versions, join misses, `actor_unresolved` per class, unresolved file objects and registry paths, value-read failures, invalid buffer records, enrichment misses, ambiguous registry value names |
-| `housekeeping` | evictions from every bounded map and cache, retention evictions, dropped failed file operations, seeding results, requests dropped because a service queue was full |
+| `quality` | late arrivals, parse errors, unknown versions, join misses, `actor_unresolved` per class, unresolved file objects and registry paths, value-read failures, invalid buffer records, enrichment misses, ambiguous registry value names, file deletes reported without a Cleanup outcome (SMB), Cleanups whose outcome could not be paired |
+| `housekeeping` | evictions from every bounded map and cache, retention evictions, dropped failed file creates and renames, seeding results, requests dropped because a service queue was full |
 | `resources` | agent CPU time in the interval (ns), working set (bytes) |
 | `buffer` | write errors, recoveries, `failing`, rejected records, segments that could not be deleted, startup recovery (truncated bytes, cursor reset, foreign segments), corrupt segments, disk bytes |
 | `gap` | events deleted from the buffer before delivery: `events`, and `first_time` / `last_time` (both or neither, ordered) |
```

```powershell
git add docs
git commit -m "docs(1): plan 1b-3c clarifications, findings and schema reference"
```

### Task 9: PR and merge

- [ ] **Step 1:** Push `feat/1b-3c-driver` and open the PR. The body summarises:
  - the driver and `win::agent`;
  - the delete change (D3, F1–F4);
  - the live test;
  - the test counts and the elevated run's results.

  It ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto` (with `buf breaking`), `powershell` and `audit`. If a test fails only on the runner, fix it with a test, never by skipping.
- [ ] **Step 3:** When every check on the PR's current head is green, merge. Then:
  - update the decision log's build row if anything changed on the way;
  - delete the remote `scratch/1b-3c` branch (D5).

## Review Log

**Self-review, 2026-10-06, before the independent review.**
- **R-1:** a delete request whose operation failed (a read-only file) stayed recorded for its `FileKey`. A file that later reused the key would have inherited it: the wrong actor on its Delete, and on SMB a Cleanup reported as a Delete. A failed request is now taken back at its OperationEnd, in the callback and in the pipeline (`failed_operations_are_dropped_by_irp_within_the_window`, `an_unknown_outcome_passes_only_for_a_requested_delete`).
- Two tests that passed with their fix reverted were strengthened until the revert failed them: the POSIX match, and the test-key leak (the symlink test now checks that the key is gone).

**Independent review, 2026-10-06.** A separate agent read:
- the plan, the spec, the earlier plans' interfaces;
- all of the changed code;
- the probe recordings.

It confirmed its findings with 10 probe tests in a copy of the worktree, all of which failed on the draft's code; they are now pinned tests. It found no blocking defect, 5 major and 11 minor findings. Every finding below was fixed in the code embedded above unless noted, and each fix with a pinned test was checked by reverting it: its test fails.

**Major**
- **R-M1, a later failure on the request's `Irp` withdrew the request.**
  - **The problem:** a thread reuses one `Irp` for every operation, and the request's own successful OperationEnd never reached the pipeline. So a failed open on the same `Irp` 250 ms later (a check that finds the file delete-pending) took the request back. The Delete then got the last closer as actor, or was self-filtered away when that was the agent.
  - **The fix:**
    - the callback passes on a request's successful OperationEnd;
    - any other operation on the `Irp` ends the request's wait in both places.
- **R-M2, the Delete's path came from the handle map.**
  - **The problem:**
    - after a rename it was the old name, both on one handle within the confirm window and on a handle opened before another handle renamed the file;
    - for a handle the agent never saw opened, the Delete waited on the seeder, or with seeding off was dropped where 1b-3a reported it.
  - **The fix:** the request keeps `DeletePath`'s own path, which is used when present. The handle's name is the fallback, for delete-on-close.
- **R-M3, the POSIX match could suppress a real Delete or duplicate one.**
  - **The problem:**
    - a POSIX delete never reported twice (a mapped file) left its entry behind, and a later file at the same path, with the reused `FileKey`, lost its Delete;
    - a second handle opened by its 8.3 name did not match, so the delete was reported twice.
  - **The fix:** the second report is matched by `FileKey` and a handle opened before the first. A new file's handles never are.
- **R-M4, a Cleanup and its OperationEnd were paired in arrival order.**
  - **Why that is unsafe:** the callbacks see cross-CPU delivery order; that is why the ordering stage exists. An OperationEnd delivered first lost its Delete. The late Cleanup then waited, and could pair with a later operation's OperationEnd: a query's returned length shares the deleted bits, which would give a false Delete.
  - **What the recordings cannot show:** no recording settles how often this happens, because every one is sorted by time when written.
  - **The fix:**
    - only real outcome values count;
    - pairing must come within a window (10 ms first, then 1 s after a busy host run counted 13 unpaired Cleanups; verification note);
    - any other operation on the `Irp` ends the wait;
    - an outcome delivered first waits for its Cleanup, in the callback and in the pipeline (R-m1);
    - a Cleanup left unpaired is counted as `quality.file_cleanup_unpaired`, split into "another operation came first" and "past the window", so the live runs and plan 1b-4's 24-hour run measure it. The CI runner counted 0.
- **R-M5, nothing could reach the pipeline once the driver owns it** (the canary's `add_self_key`, Sensor Health reads).
  - **The fix:** a `driver::Control` channel run between passes, plus `Running::control()`, `config()` and `pipeline_running()`.
  - **Interfaces:** updated, including that the callbacks' `Disconnected` sends are not counted.

**Minor**
- **Fixed:**
  - R-m1: an outcome processed before its late Cleanup was lost (with R-M4).
  - R-m2: requests outlived the delete they reported (links, streams, unknown outcomes); a reported delete is no longer outstanding. A request's `Irp` is kept 10 s, so a slow failure still takes it back.
  - R-m3: a delete-on-close clear through `FileDispositionInformationEx` wiped another handle's disposition request.
  - R-m5: the SMB limitation text understated the behaviour (§16).
  - R-m6: the live test's self-filter check could pass with nothing filtered; it now also requires `self_filtered > 0` (the host run filtered 7, the runner 37).
  - R-m7: `callback_panics` was read before the consumers finished.
  - R-m8: the drain was unbounded; it now takes at most a queue's capacity per pass.
  - R-m9: documentation slips (a property-test claim in §12.1, a misplaced doc comment, the spec status date).
  - R-m10: two timing asserts were tight for a loaded runner.
  - R-m11: the live test's actor leaked its directory and key when it failed.
- **Documented, not changed:** R-m4. POSIX link or stream deletes reported twice are untested. The new `FileKey` rule covers them the same way, by kind.

**Sound, per the review:**
- the outcome values against `ntifs.h`;
- directories, SMB's zero, and `SetDelete` before `DeletePath`;
- bounded state with no scans;
- the `file.op_end` off path;
- the stop order (1b-3b's R-m11) and partial-start failures;
- the replay's equivalence through the driver;
- the live undelete checks under 1b-3a's rule;
- the test-key fix;
- the explanation of the first host run's `atlas-etw` failure.
