# Sub-project 1b-4a — The Runnable Agent Implementation Plan

> **Status:** Draft (2026-10-13), reviewed (Review Log), for approval. **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Task 10 needs two runs on the host by the user, from scripts: the elevated tests, and about an hour of `atlas-agent run` during normal use. Nothing in this plan needs the VM or a kernel driver.

**Goal:** Make the sensor a program (sensor spec §8, §9.3, §11): an `atlas-agent` executable whose `run` command runs the sensor in a console until Ctrl+C and stops it cleanly, storing every event in the on-disk buffer with Sensor Health reports among them, and whose `dump` command prints the buffer.
- **`run`:** elevation, the single-instance lock (an Administrators-only private namespace), the data directory (created protected, or verified, with every folder above it), `agent.toml`, the diagnostic log, the buffer writer thread as the pipeline's sink, the health thread, and a clean stop on Ctrl+C, Ctrl+Break or closing the console;
- **Sensor Health reports** every 60 s, at once when a loss counter rises, and at the clean stop;
- **`dump`:** the buffer as JSON lines, read-only, with `--follow`, `--from` and `--class`;
- **a fix carried in from 1b-3c's interfaces:** with `file.op_end` off (a §13 fallback), Session A no longer enables OperationEnd and the callback no longer waits for Cleanup outcomes (F1);
- **six tests from plans 1b-3b and 1b-3c that failed now and then on a loaded machine** wait for what they check (F8);
- **the fixes from this plan's independent review** (Review Log): two major and six minor findings, each with a test checked by reverting its fix.

This is the first of plan 1b-4's three parts (S1). Plan 1b-4b adds blinding detection to the health thread; plan 1b-4c adds the service and the acceptance runs.

**Architecture:**
- **`settings`** (portable): `agent.toml` parsed with `serde` and `toml` over the built-in defaults: `Settings { config, services, buffer, log_level }`. A missing file is the defaults; an unknown key, a wrong type or a value out of range is refused with a message that names the key.
- **`health`** (portable): a `Snapshot` holds every cumulative counter at one instant; `report(prev, now)` makes a `HealthReport` of the differences (session counters wrap; a source that could not be read is absent, and `advance` carries it to the next report), plus the `Diagnostics` that only the log gets. `loss_rose` says when a report is due at once.
- **`store`** (portable): the buffer writer thread [6]. `Sink::event` moves each event into a bounded channel (65,536) and never blocks; a full channel drops and counts. The thread encodes and appends, ticks about every 100 ms, and fills in a Sensor Health report's buffer figures (the `buffer` group, `loss.buffer_backlog_drops`, `retention_evictions`, the gap, and the first report's recovery) as it appends it. A report the backlog cannot take is held (up to 16) and written first. `Store::is_running` says whether the thread still runs; `Store::close` returns the final counts, or the thread's panic as an error.
- **`dump`** (portable): reads the buffer with `atlas_buffer::Reader`, validates each record, and prints protobuf's standard JSON mapping, the replay snapshot's rendering (shared).
- **`win::datadir`:** checks every folder above the data directory (no one but SYSTEM, Administrators and TrustedInstaller can swap it), then creates the directory, `buffer\` and `logs\` with a protected DACL (SYSTEM and Administrators only), or verifies them through one handle that does not follow reparse points.
- **`win::instance`:** the single-instance lock: a mutex inside a private namespace bounded by the Administrators SID, so no other user can take it first (§11.3).
- **`win::health`:** the health thread [7]: a snapshot every 5 s, a report every 60 s or when a loss counter rose, its figures in the log; the final report at the stop.
- **`win::run`** and `main.rs`: the `clap` command line, the start order (the console handler before the sessions), the wait loop (which also ends if the pipeline or the writer thread dies), and the clean stop: the health thread, the sensor, the final report, the buffer. A dead thread's panic does not take `run` down: the stop still sends the final report and flushes.
- **Tests:** unit tests for every portable part (on Linux too), unelevated and elevated tests of the directory check, and a binary-level test in CI's `agent-live` job that runs the executable end to end (D4).

**Tech stack:** Rust 1.97 (edition 2024). New dependencies: `clap` 4.6 (derive), `serde` 1 (derive), `toml` 1.1, `tracing` 0.1, `tracing-appender` 0.2, `tracing-subscriber` 0.3. `atlas-agent` now also depends on `atlas-buffer`, `atlas-proto` and `prost-reflect` outside its tests. Tools: `buf` 1.73.0, `actionlint` 1.7.12.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2 to 1b-3c. Section numbers (§) refer to it. Brainstorm: `docs/specs/2026-10-12-etw-sensor-1b-4-brainstorm-notes.md`.

**Verification note (2026-10-12 and 2026-10-13, host, Windows 11 build 26200, and the CI runner, build 26100):** every code block below was compiled and run in a scratch worktree of `main` (ffdd7ce), the review's fixes included.
- **Lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean at the end of every task. `atlas-agent` and `atlas-etw` are also clippy-clean for `x86_64-unknown-linux-gnu`, checked from Windows at every task.
- **Unelevated:** `cargo test --workspace` passes 402 tests and ignores 18 (the two live tests, the binary test, 1b-3b's five elevated tests and this plan's ten: three of the directory, five of the lock, two of `run`). `atlas-agent` has 217 unit tests (1b-3c had 179). Test counts per task were measured by building each intermediate state.
- **Elevated on the host:** Task 10, Step 1's script, run by the user:
  - 2026-10-12, the draft: everything passed. `atlas-agent`'s unit tests 215 of 215 with the ignored ones; the agent live test (1b-3c's 19 checks, with this plan's `file.op_end` change); the binary test (493 events; the stop took 1.06 s; the loose directory refused because "its DACL is not protected", since an elevated create makes Administrators the owner). No session, key or directory was left behind;
  - 2026-10-13, after the review's fixes (eight runs, each with its fixes' reverts): the agent live test passed each time, and the elevated unit tests did too, but in the first run two of the lock's tests took a second lock in-process under the same alias (`ERROR_DUP_NAME`), which the next run's tests no longer did; the binary test passed every check of every fix, but its zero-loss check failed in four runs while the host was busy (F10); one run's new check found F9;
  - 2026-10-13, the final code, with every elevated revert: `atlas-agent`'s unit tests 232 of 232 with the ignored ones; the agent live test passed; the binary test passed every check but zero loss, on a busy host (23,964 events in its minute: kernel-queue drops and late launch joins, F10); all 23 reverts of the run caught. No session or directory was left behind.
- **The hour run on the host:** Task 10, Step 2, by the user, 103 minutes of normal use (F4): nothing lost, both unpaired-Cleanup parts 0, but over the §13 budget (D6).
- **The lock's probe on the host** (2026-10-13, before Task 8's lock was written): an unelevated squatter and an elevated holder in session 3, and a probe run as SYSTEM in session 0 by a scheduled task. Details in Task 8, Step 3.
- **On the CI runner** (the scratch branch pushed, no PR; D5.11):
  - run 38337462994 found F3, the first part of F7, a test check that matched `Users` in the test's own path, and that the loose test directory was refused for its owner, not its DACL (the runner's account owns what it creates);
  - run 38338143580 found F2 and that `Get-Acl` returned nothing (F7);
  - run 38338607392 found that the deny case's directory could not be deleted (F7);
  - run 38338927363 passed every job, `agent-live` included: the binary test passed every check with 851 events and no loss (F5);
  - after the review's fixes, every push passed every job; run 38432008944 (the fixes' last code) ran every elevated test, the agent live test and the binary test with its new checks, all passing.
- **Reverts:** every rule and fix in the Review Focus, and each of the review's fixes, was checked by reverting it in the source and running its test, which must then fail (`spikes/1b4a-plan-src/mutate.py`; the source restored after each). 72 reverts, all caught on the final code, 2026-10-13:
  - **the draft's rules (34):** the sink's and the writer's drops, held reports, the recovery, gaps, the buffer figures' differences; the health wrap, carried sources, the unpaired sum, losses reported at once; each directory refusal; unknown keys and sections, a key applied; `dump`'s skipping and filter; the three halves of the `file.op_end` fix; and the six timing tests (a lookup asked twice, new SIDs waited for, the TTL, a stuck helper, the anchor's period, the anchor retaken at all). On the first pass three were not caught: the null-DACL revert crashed the test (now counted as caught: an access violation fails the run), one TTL revert passed once and was caught on a rerun, and "the buffer figures are differences" needed the test `each_report_counts_since_the_last_one`;
  - **the review's fixes (38, 15 of them elevated):** the lock (6: the boundary without the SID, the mutex back in `Global\`, either default DACL, no open of an existing namespace, a foreign DACL not taken as another instance); a dead pipeline (3) and a dead writer (3); the backlog drops (2); the folders above (7); the watchlist (2); the handler's place (1, caught by its own check, since the binary test's zero-loss check can fail for other reasons, F10); the test fixes (9, none hanging); the binary test's new checks (3, each by its own check); the counts `close` returns (2). In this round one revert first missed: a refused open of the namespace was unreachable (the create refuses such a token first); the branch was removed, and a comment says why.
- **Round trip:** the code was rebuilt from this document alone in a fresh LF worktree of `main`: every diff applied, every file written, and `Cargo.lock` copied from Task 3's commit (Task 3 says `cargo` writes it). Every task's tree is identical to the verified one.
- **Other checks:** `buf lint` and `buf breaking` (against `main`) pass. Regenerating the golden fixtures changes none. `actionlint` passes on `ci.yml`. The three scripts parse in Windows PowerShell 5.1 and PowerShell 7.
- **Not run yet:** the Linux build of the tests is run by CI only (Linux clippy here).

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-4a-plan`. The build runs on `feat/1b-4a-agent`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Windows code only in `win`**, `main.rs`'s `run` arm and `#[cfg(windows)]` tests. `settings`, `health`, `store` and `dump` are portable, and Linux CI builds and tests them; on Linux `atlas-agent run` prints that it needs Windows.
- **Every `unsafe` block has a `// SAFETY:` comment.** The new ones are in `win::datadir`, `win::instance`, `win::health` and `win::run`, and in the binary test.
- **The sink never blocks** (plan 1b-3c's interface): it is called on the pipeline thread.
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). The elevated tests are `#[ignore]`d; CI's `agent-live` job runs them, and so does Task 10's script, which the user runs. The hour run is also the user's.
- **Recordings are made on CI, never on the host** (§12.2). This plan records nothing; the host run's buffer stays on the host, and only its summary figures enter the docs.
- **Additive schema only:** this plan adds no field; it widens one comment. `buf breaking` must pass and the golden fixtures stay byte-identical.
- **Formatting and lint:** the repo's `rustfmt.toml`; `cargo clippy --workspace --all-targets -- -D warnings` at the end of every task.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash). Edits that contain backslashes are made with an editor, never a heredoc.
- Every commit message ends with the attribution lines the session's system reminder specifies.

## Decisions

### S1, how plan 1b-4 is split (chosen 2026-10-12: A)

- **(A) Chosen: three plans**, each brainstormed, planned, reviewed and built before the next:
  - **1b-4a, a runnable agent** (this plan);
  - **1b-4b, blinding detection:** the watchdog (sessions, `ProcessTrace` returning, provider checks, the Kernel-EventTracing decision), session restart (spare queues, the spare `FastRead`), the three canaries, Event Log Activity records;
  - **1b-4c, the service and acceptance:** `service install|uninstall|start|stop`, the stop handler's allowance, Windows Event Log start-up errors; the blinding, crash-safety and 24-hour tests, the §13 budget, the unpaired-Cleanup measurement, the runbook.
- (B) Two plans (runnable + service, then watchdog + acceptance), each about 1.5 times plan 1b-3c, with service install landing before blinding detection. (C) One plan, about twice plan 1b-3c.

### D1, where Sensor Health is assembled (chosen 2026-10-12: A)

- **(A) Chosen: a new health thread,** which becomes the watchdog thread [7] in 1b-4b. Every 5 s it takes a counters snapshot (the pipeline's through `control()`, one pass), reads the atomics, the session queries and the process's CPU time and working set. Every 60 s it sends the differences to the writer thread, which fills in the `buffer` group and the gap from its own stats as it appends.
- (B) The pipeline thread, in the driver loop: Windows-only sources injected into the portable loop, and the watchdog still needs its own thread and route to the writer. (C) The writer thread: it would wait on the pipeline while not draining the sink's channel.

### D2, the data directory under `run` (chosen 2026-10-12: A)

`C:\ProgramData` grants `BUILTIN\Users:(CI)(WD,AD,WEA,WA)` (verified), so an `Atlas\` that inherits its ACL is writable by any user.
- **(A) Chosen: 1b-4a builds the create-and-verify code (§11.1) and `run` uses it;** 1b-4c's `service install` calls the same code.
- (B) Defer the ACL to 1b-4c: every host run until then uses a user-writable directory, and 1b-4c needs a migration. (C) No default directory: the same weakness, wherever it is.

### D3, `dump`'s format (chosen 2026-10-12: A)

- **(A) Chosen: protobuf's standard JSON mapping,** the one the replay snapshot already uses (`prost-reflect` with `atlas_proto::FILE_DESCRIPTOR_SET`), one rendering function shared. A `--readable` switch can come later.
- (B) Readable conversions (UUIDs, hex hashes, RFC 3339): an Atlas-only format to maintain. (C) OCSF JSON: the exporter is a later adapter.

### D4, the end-to-end proof (chosen 2026-10-12: A)

- **(A) Chosen: a binary-level test in CI's `agent-live` job, plus about an hour of `run` on the host during normal use.** The test runs `atlas-agent run` against an actor, stops it with Ctrl+Break and checks with `dump`. The hour run is summarised from its Sensor Health records and its log: losses, CPU and working set against §13, and both unpaired-Cleanup parts. Verified: Ctrl+C cannot be sent to a child in its own process group; Ctrl+Break can.
- (B) CI only: the host figures wait for 1b-4c. (C) Host only: no regression guard.

### D5, defaults for everything else (chosen 2026-10-12, accepted together)

1. the binary `atlas-agent` (`src/main.rs`, `clap` derive), with `run` and `dump`;
2. `run`: elevation required, the `Global\Atlas-Agent` mutex (replaced after the review by an Administrators-only private namespace, finding 1), `--data-dir` (default `C:\ProgramData\Atlas`), sessions `Atlas-Sensor` and `Atlas-Process`; Ctrl+C, Ctrl+Break and closing the console all stop it cleanly;
3. the directory: SDDL `O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)`, verified at start (owner Administrators or SYSTEM, a protected DACL, full control for SYSTEM and Administrators only, no reparse point), and `buffer\` and `logs\` likewise;
4. the sink moves each `Event` into a bounded channel (65,536) and never blocks; the writer thread encodes; drops count in `loss.buffer_backlog_drops` (its comment widened, no new field);
5. rolling retention with the 1 GiB cap; a gap's time from decoding its record;
6. Sensor Health every 60 s, at once when a loss counter rose (checked every 5 s), and a final one at the clean stop; the first carries the buffer's recovery; session loss counters differenced with wrapping arithmetic; `GetProcessTimes` and `GetProcessMemoryInfo`; the time from the system clock the anchor reads (the draft said "QPC through the anchor"; the code never did); UUIDv7 ids;
7. the two unpaired-Cleanup parts go to the diagnostic log with each report; the schema keeps their sum;
8. `agent.toml` in the data directory (`serde`, `toml`, unknown keys refused), the §11.2 keys that exist today, `transport = "none"` only; an invalid file stops start-up with a clear message (the Event Log comes with 1b-4c);
9. the log: `tracing` to the console and `logs\`, daily, 7 files, its level from the config (default `info`), self-filtered by the agent's start key;
10. `dump --follow --from <segment:offset> --class <name>`, read-only, elevated (the directory is SYSTEM and Administrators only);
11. one plan, one build PR, the spec revisions by a docs task; verify-first in a scratch worktree and on the CI runner (a pushed scratch branch, no PR); an independent review with reverted-fix checks.

### D6, the host run's budget overrun (chosen 2026-10-12: A)

The hour run (F4) lost nothing but used 10.1% of one core on average and 200 MB of working set, against §13's 2% and 150 MB.
- **(A) Chosen: this plan goes ahead and records the figures; the cause is profiled before plan 1b-4b.** The code this plan adds is not where the time goes: the writer handles about 72 records a second and the health thread wakes every 5 s, while the pipeline built by 1b-2 to 1b-3c parses about 10,000 ETW events a second. A profiling step (CPU per thread, then per stage) comes before any §13 fallback, and each fallback or fix is a decision-log entry. The budget is 1b-4c's gate (DoD item 4).
- (B) Profile first, then write this plan: a delay, for a cause most likely in earlier code. (C) Apply the first fallbacks now (on-miss seeding off, then `file.op_end` off) and re-run: quick, but it guesses the cause and gives up coverage before knowing it is needed.

## Findings from the build

- **F1, `file.op_end` off left OperationEnd enabled:** only the pipeline's rule changed (plan 1b-3a). Session A still enabled event 24 and the `OP_END` keyword, and the callback still waited for every Cleanup's outcome. Since the pipeline then ignores outcomes, the fallback would have cost the same in the kernel and counted nothing useful. Session A now leaves both out (mask `0x1EA0`), and the intake keeps no Cleanup state, so no Cleanup is counted unpaired. 1b-3c's interfaces listed half of this (the enable) for 1b-4; the intake half was found here, so both moved into 1b-4a from 1b-4b.
- **F2, NTFS protects an explicit DACL given at creation:** a directory created with a security descriptor reads back with `SE_DACL_PROTECTED` set even when the SDDL has no `P` flag. The elevated test's "inheriting directory" case passed the check on the first CI run for that reason; it is now built as a plain create whose owner is then set to Administrators, which is what an elevated `mkdir` makes.
- **F3, the CI runner's `%TEMP%` is an 8.3 path** (`C:\Users\RUNNER~1\…`). The agent reports long paths (§7.2), so the binary test's actor works under the long form of its directory.
- **F4, the host run** (2026-10-12, 103 minutes of normal use, release build, `C:\ProgramData\Atlas`; summarised by Task 10's script from the agent's own records):
  - 103 reports, each about 60 s, the intervals chained without a gap;
  - **nothing lost:** no events or buffers lost in either session, no queue drops, no backlog drops, no callback panics, no DNS rate-limit drops, no `actor_dropped`;
  - **both unpaired-Cleanup parts 0** (1b-3c's open measurement, for a normally busy host; 1b-4c measures 24 hours);
  - 447,287 records, 150 MB of buffer; no invalid records or corrupt segments; the clean stop took 1.0 s;
  - **CPU averaged 10.1% of one core:** 3–4% in quiet minutes, 40–70% in the busiest, which were those with the most file activity (`housekeeping.file_op_failed` at 40,000–65,000 a minute against about 8,000 when quiet);
  - **the working set** grew from 122 MB to 200 MB in the first hour, then held; no cache or map evicted anything;
  - over §13's budget (D6). Quality counters, which §13 does not gate: 68,620 late arrivals, 48,411 file events with an unresolved actor, 7,412 unresolved registry paths, 8,270 failed value reads.
- **F5, on the CI runner:** the binary test's run wrote 851 events and two reports with nothing lost, used 6.2 s of CPU in about a minute (under the test's own load), and stopped in 0.9 s.
- **F6, the stop key:** `GenerateConsoleCtrlEvent` cannot deliver Ctrl+C to a process group other than the caller's own, so the binary test starts the agent in a new process group and sends Ctrl+Break, which the agent treats like Ctrl+C. A test process without a console allocates one first.
- **F7, test-only findings on the runner:**
  - a child's stdout and stderr must both be drained while it runs: `dump` stalled at 102 of 477 lines on a pipe the test did not read;
  - Windows PowerShell's `Get-Acl` returned nothing inside the test process, so the tests read a directory's descriptor back through the API;
  - a deny entry for `FILE_GENERIC_WRITE` on a test directory kept `remove_dir_all` from deleting it; the deny case now denies only adding files (`0x2`).
- **F8, six timing-dependent tests** from plans 1b-3b and 1b-3c failed intermittently when the per-task test counts were measured: in repeated runs of `atlas-agent`'s unit tests on the host (22 logical CPUs, every test in parallel), about one run in 20 had a failure. Each assumed a helper or driver thread would run within 50–250 ms: three account-lookup tests, two file-namer tests and the driver's anchor test. CI had not shown it. They now wait for the state they check (Task 1); 100 runs in a row then passed. The review then found two of them could no longer catch their bug or could hang (finding 8); they were fixed again.
- **F9, the records logged at stop fell short** (found by the review's new binary check): `Store::close` copied the writer's counts before `Writer::close` wrote what was still queued, so `records=` missed a stop's burst (5,108 logged for 9,258 on disk). `Writer::close` now returns its final counts (Task 5).
- **F10, the zero-loss check under host bursts:** in four of the host's elevated runs on 2026-10-13, the binary test's Sensor Health showed losses while the host was busy (18,000–36,000 events in the test's minute against about 1,400 when quiet): kernel-queue drops, and process launches whose two halves met too late (`actor_dropped`). The runner, and quiet host runs, lost nothing. It is the pipeline's throughput, the same cause as F4's CPU; the profiling before 1b-4b (D6) covers it, and until then that one check can fail on a busy host.

## Deliberate clarifications of the spec (applied to the spec text in Task 11)

1. **Plan 1b-4 is three plans** (S1): 1b-4a, a runnable agent; 1b-4b, blinding detection; 1b-4c, the service and acceptance (status line, §9.1).
2. **The sink** (D5.4; §3.2): a bounded channel of 65,536 events to the writer thread, never blocking; its drops counted in `loss.buffer_backlog_drops`, whose comment now says so (§10.3). The writer ticks about every 100 ms (at most about 200).
3. **Thread [7] starts as the health thread** (D1; §3.2): a snapshot every 5 s, a report every 60 s, at once on a loss, and at the clean stop. 1b-4b adds the watchdog's checks to it.
4. **How a report is made** (D1, D5.6; §9.3):
   - the differences of two snapshots; the sessions' 32-bit counters wrap;
   - a source that could not be read (the pipeline did not answer within 2 s, a session query failed) is absent, and the next report covers its interval;
   - the writer fills in the `buffer` group, `loss.buffer_backlog_drops`, `retention_evictions`, the gap and, in a run's first report, the recovery; a report the backlog cannot take is held, up to 16, and written first;
   - the report's time is the system clock the anchor reads; if the pipeline thread died, the final report goes out without its counters;
   - the intervals chain from the anchor taken at start; the final report is taken after the sessions stopped.
5. **Three counters not measured yet** (§9.3): `quality.buffer_invalid_records` and `buffer.corrupt_segments` (the agent never reads its own buffer back; `dump` counts both) and `housekeeping.seeder_negative_cache_size` (the seeder exposes no size). The two unpaired-Cleanup parts go to the diagnostic log (D5.7).
6. **`file.op_end` off** (F1; §4.2): Session A leaves out 24 and `OP_END`, and the callback pairs no Cleanup.
7. **The data directory** (D2; §11.1): created or verified by `run`; one handle that does not follow reparse points; deny entries accepted; the files inside are covered by the directory's DACL (only SYSTEM and Administrators can create them) rather than checked one by one, and the buffer already refuses reparse-point segments. Until the service exists, a refusal is printed on the console. Every folder above it is checked too (review finding 4).
8. **`agent.toml`** (D5.8; §11.2): unknown keys and sections refused, errors naming the key; per-class enable deferred (the last §13 fallback; added if the fallbacks reach it); errors on the console and in the log until the service exists.
9. **The CLI** (D3, D5; §11.3): `run`'s `--data-dir`, session names, stop signals and exit codes (0 clean stop, 1 start-up refused, 2 the pipeline or the writer thread ended); the single-instance lock (review finding 1); `dump`'s format, `--from segment:offset`, class names and stderr summary (none when `--follow` is ended by Ctrl+C).
10. **Start and clean stop** (§11.4): the elevation check first; the log once the config is loaded; the buffer before the sessions; the console handler just before the sessions; the health thread last. The clean stop: the health thread, the sensor, the final report, the buffer; the same after a thread died. The log's files.
11. **The binary-level test** (D4; §12.3).
12. **The host run's figures** (F4, D6; §13, §15.3): over budget; profiled before 1b-4b.
13. **The review fixes** (Review Log; §17): a paragraph listing them.

## Review Focus

These would slip past a plain unit test, so each has a pinned check:
1. **The sink never blocks, and every event it cannot pass on is counted** (`a_full_channel_drops_and_counts_never_blocks`, which sends from its own thread so a blocking send fails it instead of hanging it), including the writer's backlog drops (`a_report_the_backlog_cannot_take_is_held_and_written_first` checks `drops`), and each report counts the drops of everything queued ahead of it (`a_report_counts_the_backlog_drops_of_everything_queued_before_it`).
2. **A report is a difference, and nothing is counted twice or lost:**
   - session counters wrap (`session_counters_wrap`);
   - an unread source is absent, then covered by the next report (`what_was_not_read_is_absent_and_counted_later`);
   - the intervals chain from the start, the last one ending after the stop (the binary test's `the_intervals_chain`, `the_first_interval_starts_at_the_start`, `the_last_record_is_the_final_report`).
3. **A loss is reported at once** (`a_rising_loss_counter_is_seen_and_other_counters_are_not`), by the health thread's real loop, which then restarts the period and returns the last interval at the stop (`a_loss_is_reported_at_once_and_restarts_the_period`).
4. **The writer's figures:** in order with the events (`events_and_reports_reach_the_disk_in_order_with_the_buffers_figures`); the recovery only in the first report (`only_the_first_report_carries_the_recovery`); a held report written before later events, the held queue bounded (`a_report_the_backlog_cannot_take_is_held_and_written_first`, `held_reports_are_bounded`); one gap per report (`gaps_are_merged_into_one`); the counts `close` returns include its own last write (`close_counts_what_its_last_write_wrote`, and the buffer's `close_writes_everything`).
5. **The directory check refuses every loose case:** a wrong owner, an unprotected or null DACL, an allow entry for anyone else (`descriptors_are_checked`), an inherited ACL (`a_folder_with_an_inherited_acl_is_refused`), a junction, not followed (`a_junction_is_refused_without_following_it`), a file (`a_file_is_refused`); and elevated, what `prepare` creates is accepted again and a probe file inside has only SYSTEM and Administrators (`prepare_creates_a_protected_directory_and_accepts_it_again`), while loose directories made the way Windows makes them are refused (`loose_protected_directories_are_refused`). The descriptor checks also refuse an inherit-only allow for anyone else on the directory, and conditional and object entries. **The folders above:** checked from the volume root down, by their descriptors (`ancestor_descriptors_are_checked`), on this machine's real `C:\` and `C:\ProgramData` (`the_folders_above_the_default_pass`), below `%TEMP%` (`a_data_directory_below_the_users_temp_is_refused`), for relative and dotted paths (`a_relative_or_dotted_path_is_refused`), and elevated with real folders and a junction (`folders_above_that_others_control_are_refused`).
6. **`agent.toml`:** unknown keys refused by name (`unknown_keys_are_refused_by_name`); every key reaches the setting it names (`every_key_is_applied`); the buffer cap follows the buffer's own rule (`the_buffer_cap_matches_the_buffers_own_rule`); the documented example parses (`the_documented_example_parses`); a bad watchlist pattern is refused by key (`wrong_types_and_values_are_refused`).
7. **`dump`:** invalid records skipped and counted, never printed (`prints_protobuf_json_lines_and_skips_invalid_records`); the same rendering as the replay snapshot (`the_rendering_is_the_replay_snapshots`, and the replay test now renders through it); `--class` and `--from` (`class_and_from_select_records`); `--follow` waits and stops (`follow_waits_for_new_records_until_stopped`).
8. **`file.op_end` off** (F1): the enable (`op_end_off_drops_event_24_and_its_keyword_only`) and the intake (`with_op_end_off_no_cleanup_is_paired_or_counted`).
9. **The binary test's checks** (Task 9), on the runner and the host: a second instance refused; a periodic report during the run; Ctrl+Break gives a clean exit with the buffer flushed; the directories protected; a log file; `dump` gives as many lines as the writer logged as written; the class filter; every class the actor produces; zero loss in every report (F10); a loose directory and one below `%TEMP%` refused; Ctrl+Break while the sensor starts still stops it cleanly, with no session left and a final report.
10. **The clean stop loses nothing** (§11.4): the final report is taken after `Running::stop`, and `Store::close` writes what is held and queued. The binary test checks that the log says the buffer was closed, that the final report (which covers the stop) counts no loss, and that `dump` prints every record the writer says it wrote (`dump_prints_every_record`, against `records=`, F9).
11. **The timing fixes keep the tests' meaning** (Task 1): each still fails when its property breaks (verification, reverts): a lookup waited for twice, a stuck helper not set aside, an anchor retaken on every pass (measured between the refreshes themselves), a timeout ignored (a timer opens the gate, so the test fails instead of hanging).
12. **The single-instance lock cannot be taken by another user** (§11.3): taken once until dropped (`the_lock_is_taken_once_until_dropped`), squats block nothing (`squatted_names_do_not_block_it`), a token without Administrators is refused (`a_token_without_administrators_is_refused_held_or_not`), a lock held under another DACL still counts (`a_lock_held_under_another_dacl_is_another_instance`), and only SYSTEM and Administrators are in the DACLs (`the_namespace_and_the_mutex_grant_system_and_administrators_only`).
13. **A dead thread does not take `run` down:** the pipeline's (`a_pipeline_that_dies_still_gets_its_final_report_and_a_flushed_buffer`) and the writer's (`a_buffer_writer_that_dies_stops_the_agent`, `a_writer_that_dies_is_seen_and_close_says_why`).

Every one of these rules and fixes was checked by reverting it: its test fails (verification note). Items 9 and 10 are checked by the binary test on the runner and the host, not by a revert.

## File Structure

```
Cargo.toml                         + clap, serde, toml, tracing, tracing-appender, tracing-subscriber, atlas-buffer
Cargo.lock                         + 35 packages
crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto   buffer_backlog_drops: comment widened
crates/atlas-schema/src/classes/sensor_health.rs               the same comment
crates/atlas-buffer/
  src/writer.rs                    close returns the final Stats (F9)
  tests/log.rs                     close_writes_everything counts them
crates/atlas-etw/
  src/providers.rs                 session_a(udp, op_end)
  src/session/consumer.rs          + Consumer::panic_counter
  tests/live.rs                    session_a(true, true)
crates/atlas-agent/
  src/driver.rs, src/win/handles.rs, src/win/lookups.rs   six tests wait for what they check (F8)
  Cargo.toml                       the new dependencies; windows features Console, ProcessStatus
  src/main.rs                      the CLI: run, dump (new)
  src/lib.rs                       + dump, health, settings, store; panic_message
  src/settings.rs                  agent.toml (new)
  src/counters.rs                  + IntakeTotals, ServiceTotals
  src/health.rs                    Snapshot, report, loss_rose, event (new)
  src/store.rs                     Sink, Store, the writer thread (new)
  src/dump.rs                      dump, render, ClassName, parse_cursor (new)
  src/intake.rs                    no Cleanup state with file.op_end off
  src/win/mod.rs                   + datadir, health, instance, run
  src/win/util.rs                  + SecurityDescriptor (moved from datadir)
  src/win/datadir.rs               prepare, verify, the folders above (new)
  src/win/instance.rs              the single-instance lock (new)
  src/win/health.rs                the health thread (new)
  src/win/run.rs                   atlas-agent run (new)
  src/win/agent.rs                 health_sources(); session_a's op_end; Stopped's session figures; a panicked pipeline joined without panicking
  tests/live.rs                    Stopped::pipeline is a Result
  tests/replay.rs                  the snapshot rendered through dump::render
  tests/binary.rs                  the binary-level test (new)
.github/workflows/ci.yml           agent-live: + the binary test
spikes/run-1b4a-elevated.ps1       git-ignored; Task 10
spikes/run-1b4a-hour.ps1           git-ignored; Task 10
spikes/summarize-1b4a.ps1          git-ignored; Task 10
docs/…                             Task 11
```

## Interfaces for later plans

- **The profiling step (before 1b-4b, D6):**
  - Sensor Health's `resources.cpu_time` is the whole process's. The next measurement is CPU per thread (`GetThreadTimes`, or a short WPR trace in the VM or by the user), then per stage on the pipeline thread.
  - Candidates, in the order the figures point: Session A's callback parsing every Kernel-File event, about half of them successful OperationEnds it discards (`op_end_discarded`); the pipeline's file handling (`file_op_failed` tracked CPU in F4); on-miss seeding (`seeder_table_reads`).
  - The working set: which maps hold what after an hour (no map evicted anything in F4).
  - Each fix or §13 fallback is a decision-log entry; the hour run (`spikes/run-1b4a-hour.ps1`, `summarize-1b4a.ps1`) measures the result the same way.
- **Plan 1b-4b (blinding detection):**
  - **The watchdog lives in the health thread** (`win::health::HealthThread`): its loop already wakes every 5 s (`HealthConfig::check_every`) with the sessions' names and the counters in `HealthSources`. The session checks, provider checks and canary matching go there.
  - **Event Log Activity records** go to the writer through the same sender as the reports (`Store::health()`), as `Record::Event`.
  - **Restarting a session** still needs the spare queues, `Running::config()` and a spare `FastRead` (plan 1b-3c's interfaces); `Running` now also keeps the session names.
  - **The canary directory** joins `datadir::SUBDIRS`, so `prepare` creates and verifies it; `HKLM\SOFTWARE\Atlas` needs the same SDDL.
  - **The Kernel-EventTracing decision** (§9.1) is still open.
  - **A restarted session's counters start again from 0:** the health thread's next report would take the 32-bit wrap of the difference (about 4.29e9 lost) unless the base is reset at the restart (the review's note).
- **Plan 1b-4c (the service and acceptance):**
  - **`service install`** calls `win::datadir::prepare`.
  - **The service's main** reuses `win::run`'s sequence and takes the same lock (`win::instance::acquire`), so it and `run` exclude each other across sessions (verified on the host). Today `run_with_sink` installs the console handler itself; the service passes its own stop signal instead (a `Receiver<()>` taken by it).
  - **Start-up errors** are printed with `eprintln!` before the log exists, and logged after; the service reports both to the Event Log (§11.1, §11.2).
  - **The stop handler's allowance:** the clean stop took 1.0 s on the host and 0.9 s on the runner, the 750 ms hold included.
  - **The 24-hour run** is summarised the same way as the hour run (`summarize-1b4a.ps1` reads the Sensor Health records and the log), including both unpaired-Cleanup parts; if either is not zero on a normally busy host it is investigated before 1b-4 closes.
  - **`quality.buffer_invalid_records`** and **`housekeeping.seeder_negative_cache_size`** are absent from every report (clarification 5); a crash-safety test that replays the buffer can report the first.
- **Sub-project 2 (the transport):** `dump::render` and `atlas_buffer::Reader` are the read side; `transport = "none"` is the only value `settings` accepts today.

---

### Task 1: Six timing-dependent tests wait for what they check

**Files:**
- Modify: `crates/atlas-agent/src/driver.rs`, `crates/atlas-agent/src/win/handles.rs`, `crates/atlas-agent/src/win/lookups.rs` (tests only)

- [ ] **Step 1: The fixes** (F8). Each test assumed a helper or driver thread would run within 50–250 ms; on a loaded machine it sometimes did not. Each now waits for the state it checks, and bounds what it asserts by the time that actually passed, so it still fails when its property breaks:
  - **the account lookups** (`accounts_are_cached_and_never_wait_long`, `one_stuck_lookup_does_not_make_every_new_sid_wait`, `failures_are_retried_after_their_ttl`): the slow lookup blocks on a gate the test opens, instead of sleeping; answers are waited for with a 1–2 s timeout; "not waited for" means well under that timeout; the failure TTL is 500 ms rather than 50. The first slow lookup is bounded too (under twice its timeout), and a timer opens the gate after four timeouts, so a lookup that ignored its timeout fails the test instead of hanging it (review finding 8);
  - **the namer** (`disk_files_are_named_and_others_are_not`, `blocked_queries_time_out_and_recover`): a 5 s timeout where an answer is expected; the blocked case asks until a helper is actually stuck (a time-out whose helper still answers within the grace period is only late), and waits for the stuck helper to return;
  - **the driver** (`the_anchor_is_retaken_on_its_period`): it waits for four refreshes and records when each came; every gap between two must be at least the period, less one cadence (a refresh follows its pass). Measured between the refreshes themselves, a slow thread start cannot hide refreshes that come too often (review finding 8).

```diff
--- a/crates/atlas-agent/src/win/lookups.rs
+++ b/crates/atlas-agent/src/win/lookups.rs
@@ -330,61 +330,83 @@ mod tests {
         assert!(query_drives().iter().any(|(dev, d)| d.eq_ignore_ascii_case(drive) && dev.starts_with(r"\Device\")));
     }
 
+    /// A lookup that blocks until the test releases it, so no assertion
+    /// depends on how fast the helper thread is scheduled.
+    fn gated(slow: &'static str) -> (impl Fn(&str) -> Option<String> + Send + 'static, Sender<()>, Calls) {
+        let (release, gate) = channel::<()>();
+        let gate = Mutex::new(gate);
+        let calls: Calls = Arc::default();
+        let c = calls.clone();
+        let resolve = move |sid: &str| {
+            c.lock().unwrap().push(sid.to_string());
+            if sid == slow {
+                let _ = gate.lock().unwrap().recv();
+            }
+            (sid != "none").then(|| format!("D\\{sid}"))
+        };
+        (resolve, release, calls)
+    }
+
+    type Calls = Arc<Mutex<Vec<String>>>;
+
+    /// Asks until the helper's answer has arrived.
+    fn answered(a: &mut Accounts, sid: &str) -> Option<String> {
+        let t0 = Instant::now();
+        loop {
+            let n = a.name(sid);
+            if n.is_some() || t0.elapsed() > Duration::from_secs(10) {
+                return n;
+            }
+            std::thread::sleep(Duration::from_millis(20));
+        }
+    }
+
     #[test]
     fn accounts_are_cached_and_never_wait_long() {
-        let calls = Arc::new(Mutex::new(Vec::new()));
-        let c = calls.clone();
-        let mut a = Accounts::new(
-            move |sid| {
-                c.lock().unwrap().push(sid.to_string());
-                if sid == "slow" {
-                    std::thread::sleep(Duration::from_millis(300));
-                }
-                (sid != "none").then(|| format!("D\\{sid}"))
-            },
-            Duration::from_millis(100),
-            Duration::from_secs(600),
-        );
+        let (resolve, release, calls) = gated("slow");
+        let timeout = Duration::from_secs(1);
+        let mut a = Accounts::new(resolve, timeout, Duration::from_secs(600));
         assert_eq!(a.name("S-1").as_deref(), Some(r"D\S-1"));
         assert_eq!(a.name("S-1").as_deref(), Some(r"D\S-1"));
         assert_eq!(a.name("none"), None);
         assert_eq!(a.name("none"), None, "failures are cached too");
+        // If the timeout were ignored, a timer opens the gate: the test fails
+        // on the answer or the time instead of hanging (review finding 8).
+        let timer = release.clone();
+        std::thread::spawn(move || {
+            std::thread::sleep(timeout * 4);
+            let _ = timer.send(());
+        });
         let t = Instant::now();
         assert_eq!(a.name("slow"), None, "a slow lookup times out");
-        assert!(t.elapsed() < Duration::from_millis(250));
+        assert!(t.elapsed() < timeout * 2, "{:?}: waited past the timeout", t.elapsed());
         let t = Instant::now();
         assert_eq!(a.name("slow"), None, "still running: not asked twice");
-        assert!(t.elapsed() < Duration::from_millis(50), "and not waited for twice");
-        std::thread::sleep(Duration::from_millis(300));
-        assert_eq!(a.name("slow").as_deref(), Some(r"D\slow"), "finished in the background");
+        assert!(t.elapsed() < timeout / 2, "and not waited for twice");
+        release.send(()).unwrap();
+        assert_eq!(answered(&mut a, "slow").as_deref(), Some(r"D\slow"), "finished in the background");
         assert_eq!(*calls.lock().unwrap(), ["S-1", "none", "slow"]);
     }
 
     /// R-M4: behind one stuck lookup, new SIDs are not waited for one by one.
     #[test]
     fn one_stuck_lookup_does_not_make_every_new_sid_wait() {
-        let mut a = Accounts::new(
-            |sid| {
-                if sid == "stuck" {
-                    std::thread::sleep(Duration::from_millis(500));
-                }
-                Some(format!("D\\{sid}"))
-            },
-            Duration::from_millis(100),
-            Duration::from_secs(600),
-        );
+        let (resolve, release, _) = gated("stuck");
+        let timeout = Duration::from_secs(1);
+        let mut a = Accounts::new(resolve, timeout, Duration::from_secs(600));
         assert_eq!(a.name("stuck"), None);
         let t = Instant::now();
         for i in 0..10 {
             assert_eq!(a.name(&format!("S-{i}")), None);
         }
-        assert!(t.elapsed() < Duration::from_millis(50), "{:?} for ten new SIDs", t.elapsed());
-        std::thread::sleep(Duration::from_millis(600));
-        assert_eq!(a.name("S-9").as_deref(), Some(r"D\S-9"), "answered in the background");
+        assert!(t.elapsed() < timeout / 2, "{:?} for ten new SIDs", t.elapsed());
+        release.send(()).unwrap();
+        assert_eq!(answered(&mut a, "S-9").as_deref(), Some(r"D\S-9"), "answered in the background");
         assert_eq!(a.name("S-10").as_deref(), Some(r"D\S-10"), "waited for again once the helper answers");
     }
 
-    /// R-m4: a failure is retried after its TTL.
+    /// R-m4: a failure is retried after its TTL. The TTL is far longer than
+    /// the gap between the first two calls, also on a loaded machine.
     #[test]
     fn failures_are_retried_after_their_ttl() {
         let calls = Arc::new(Mutex::new(0));
@@ -394,13 +416,13 @@ mod tests {
                 *c.lock().unwrap() += 1;
                 None
             },
-            Duration::from_millis(100),
-            Duration::from_millis(50),
+            Duration::from_secs(2),
+            Duration::from_millis(500),
         );
         assert_eq!(a.name("S-1"), None);
         assert_eq!(a.name("S-1"), None);
         assert_eq!(*calls.lock().unwrap(), 1, "cached");
-        std::thread::sleep(Duration::from_millis(60));
+        std::thread::sleep(Duration::from_millis(600));
         assert_eq!(a.name("S-1"), None);
         assert_eq!(*calls.lock().unwrap(), 2, "asked again");
     }
```

```diff
--- a/crates/atlas-agent/src/win/handles.rs
+++ b/crates/atlas-agent/src/win/handles.rs
@@ -348,7 +348,8 @@ mod tests {
 
     #[test]
     fn disk_files_are_named_and_others_are_not() {
-        let mut namer = FileNamer::new(Duration::from_millis(200), 2);
+        // A timeout no answer comes near, even with the helper thread starting under load.
+        let mut namer = FileNamer::new(Duration::from_secs(5), 2);
         let exe = std::env::current_exe().unwrap();
         let f = std::fs::File::open(&exe).unwrap();
         let FileName::Named(n, _) = namer.name(dup_own(HANDLE(f.as_raw_handle()))) else { panic!("not named") };
@@ -386,19 +387,32 @@ mod tests {
         lock(&holder);
         let waiter_raw = HANDLE(waiter.as_raw_handle());
         let mut namer = FileNamer::new(Duration::from_millis(100), 1);
-        let d = dup_own(waiter_raw);
         let t = std::thread::spawn(move || {
             lock(&waiter);
             waiter
         });
-        std::thread::sleep(Duration::from_millis(100)); // the lock request is pending
-        assert_eq!(namer.name(d), FileName::TimedOut);
+        // Until the lock request is pending the query is answered (or, on a
+        // loaded machine, merely late: a time-out whose helper still answers
+        // is not stuck). Ask until a helper is stuck, rather than guessing how
+        // long the thread takes to start.
+        let t0 = std::time::Instant::now();
+        let got = loop {
+            let got = namer.name(dup_own(waiter_raw));
+            if (got == FileName::TimedOut && namer.stuck() == 1) || t0.elapsed() > Duration::from_secs(10) {
+                break got;
+            }
+            std::thread::sleep(Duration::from_millis(20));
+        };
+        assert_eq!(got, FileName::TimedOut);
         assert_eq!(namer.stuck(), 1);
         assert!(namer.paused());
         // SAFETY: test-only; releasing the first lock grants the second.
         unsafe { UnlockFile(HANDLE(holder.as_raw_handle()), 0, 0, 1, 0) }.unwrap();
         drop(t.join().unwrap());
-        std::thread::sleep(Duration::from_millis(100));
+        let t0 = std::time::Instant::now();
+        while namer.stuck() != 0 && t0.elapsed() < Duration::from_secs(10) {
+            std::thread::sleep(Duration::from_millis(20));
+        }
         assert_eq!(namer.stuck(), 0, "the helper returned");
         assert!(!namer.paused());
         drop(holder);
```

```diff
--- a/crates/atlas-agent/src/driver.rs
+++ b/crates/atlas-agent/src/driver.rs
@@ -162,7 +162,6 @@ fn drain<L: Lookups>(rx: &Receiver<Incoming>, p: &mut Pipeline<L>, max: usize) -
 
 #[cfg(test)]
 mod tests {
-    use std::sync::atomic::{AtomicUsize, Ordering};
     use std::sync::mpsc::{SyncSender, channel, sync_channel};
     use std::sync::{Arc, Mutex};
 
@@ -268,22 +267,33 @@ mod tests {
 
     #[test]
     fn the_anchor_is_retaken_on_its_period() {
-        let taken = Arc::new(AtomicUsize::new(0));
-        let n = taken.clone();
-        let cfg = DriverConfig { anchor_every: Duration::from_millis(20), ..DriverConfig::default() };
+        let calls = Arc::new(Mutex::new(Vec::<Instant>::new()));
+        let c = calls.clone();
+        let every = Duration::from_millis(50);
+        let cfg = DriverConfig { anchor_every: every, ..DriverConfig::default() };
+        let slack = cfg.cadence;
         let r = start(
             cfg,
             Box::new(move || {
-                n.fetch_add(1, Ordering::Relaxed);
+                c.lock().unwrap().push(Instant::now());
                 Anchor { qpc: 0, unix_ns: 1_790_000_000_000_000_000 }
             }),
         );
-        std::thread::sleep(Duration::from_millis(250));
+        // Retaken: wait for a few refreshes, however slowly the thread starts.
+        let t0 = Instant::now();
+        while calls.lock().unwrap().len() < 4 && t0.elapsed() < Duration::from_secs(10) {
+            std::thread::sleep(Duration::from_millis(5));
+        }
         drop(r.kernel);
         drop(r.user);
         r.thread.join().unwrap();
-        let n = taken.load(Ordering::Relaxed);
-        assert!((1..=13).contains(&n), "{n} refreshes in 250 ms");
+        let calls = calls.lock().unwrap();
+        assert!(calls.len() >= 4, "{} refreshes in 10 s", calls.len());
+        // On its period, measured between the refreshes themselves, so a slow
+        // thread start cannot hide refreshes that come too often (review
+        // finding 8). A refresh follows its pass, hence one cadence of slack.
+        let gaps: Vec<Duration> = calls.windows(2).map(|w| w[1] - w[0]).collect();
+        assert!(gaps.iter().all(|g| *g + slack >= every), "{gaps:?}");
     }
 
     #[test]
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Run the unit tests repeatedly (verification: 100 runs in a row passed). Workspace: 363 passed, 7 ignored (`atlas-agent` unit tests: 179).
```powershell
git add crates/atlas-agent
git commit -m "test(agent): six timing-dependent tests wait for what they check"
```

### Task 2: `file.op_end` off leaves out OperationEnd and Cleanup pairing

**Files:**
- Modify: `crates/atlas-etw/src/providers.rs`, `crates/atlas-etw/tests/live.rs`
- Modify: `crates/atlas-agent/src/intake.rs`, `crates/atlas-agent/src/win/agent.rs`

- [ ] **Step 1: Session A's enable follows `file.op_end`** (F1). `session_a` takes the setting; off drops event 24 from Kernel-File's ID filter and the `OP_END` keyword (0x40) from its mask, and changes nothing else.

```diff
--- a/crates/atlas-etw/src/providers.rs
+++ b/crates/atlas-etw/src/providers.rs
@@ -71,22 +71,26 @@ pub struct Enable {
 pub const LEVEL: u8 = 5;
 
 /// Session A's providers, keywords and event IDs (sensor spec §4.2).
-/// `udp`: the `network.udp` setting (§7.3), on by default.
-pub fn session_a(udp: bool) -> Vec<Enable> {
+/// `udp`: the `network.udp` setting (§7.3), on by default. `op_end`: the
+/// `file.op_end` setting (§5.5, §13); off leaves out Kernel-File 24 and its
+/// `OP_END` keyword (plan 1b-4a).
+pub fn session_a(udp: bool, op_end: bool) -> Vec<Enable> {
     let mut net = vec![12, 13, 15, 28, 29, 31];
     if udp {
         net.extend([42, 43, 58, 59]);
     }
+    let mut file_ids = vec![12, 13, 14, 16, 17, 18, 24, 26, 27, 30];
+    let mut file_keywords = 0x1EE0;
+    if !op_end {
+        file_ids.retain(|&id| id != 24);
+        file_keywords &= !0x40;
+    }
     vec![
         // WINEVENT_KEYWORD_PROCESS 0x10, WINEVENT_KEYWORD_IMAGE 0x40.
         Enable { provider: Provider::KernelProcess, keywords: 0x50, event_ids: vec![1, 2, 5] },
         // FILEIO 0x20, OP_END 0x40, CREATE 0x80, WRITE 0x200, DELETE_PATH 0x400,
         // RENAME_SETLINK_PATH 0x800, CREATE_NEW_FILE 0x1000.
-        Enable {
-            provider: Provider::KernelFile,
-            keywords: 0x1EE0,
-            event_ids: vec![12, 13, 14, 16, 17, 18, 24, 26, 27, 30],
-        },
+        Enable { provider: Provider::KernelFile, keywords: file_keywords, event_ids: file_ids },
         // CloseKey 0x1, SetValueKey 0x100, DeleteValueKey 0x200, CreateKey 0x1000,
         // OpenKey 0x2000, DeleteKey 0x4000.
         Enable { provider: Provider::KernelRegistry, keywords: 0x7301, event_ids: vec![1, 2, 3, 5, 6, 13] },
@@ -112,7 +116,7 @@ mod tests {
 
     #[test]
     fn every_enabled_event_has_a_parser() {
-        for e in session_a(true) {
+        for e in session_a(true, true) {
             for id in e.event_ids {
                 assert!(layout::newest(e.provider, id).is_some(), "{:?} {id}", e.provider);
             }
@@ -121,10 +125,22 @@ mod tests {
 
     #[test]
     fn udp_off_drops_only_the_datagram_events() {
-        let on = session_a(true);
-        let off = session_a(false);
+        let on = session_a(true, true);
+        let off = session_a(false, true);
         let net = |v: &[Enable]| v.iter().find(|e| e.provider == Provider::KernelNetwork).unwrap().event_ids.clone();
         assert_eq!(net(&on), [12, 13, 15, 28, 29, 31, 42, 43, 58, 59]);
         assert_eq!(net(&off), [12, 13, 15, 28, 29, 31]);
     }
+
+    #[test]
+    fn op_end_off_drops_event_24_and_its_keyword_only() {
+        let file = |v: Vec<Enable>| v.into_iter().find(|e| e.provider == Provider::KernelFile).unwrap();
+        let on = file(session_a(true, true));
+        let off = file(session_a(true, false));
+        assert_eq!((on.keywords, off.keywords), (0x1EE0, 0x1EA0));
+        assert_eq!(on.event_ids, [12, 13, 14, 16, 17, 18, 24, 26, 27, 30]);
+        assert_eq!(off.event_ids, [12, 13, 14, 16, 17, 18, 26, 27, 30]);
+        let others = |v: Vec<Enable>| v.into_iter().filter(|e| e.provider != Provider::KernelFile).collect::<Vec<_>>();
+        assert_eq!(others(session_a(true, true)), others(session_a(true, false)));
+    }
 }
```

```diff
--- a/crates/atlas-etw/tests/live.rs
+++ b/crates/atlas-etw/tests/live.rs
@@ -664,7 +664,7 @@ fn live_scenario() {
         ca.qpc_frequency() == freq && cb.qpc_frequency() == freq,
         format!("A {} B {} system {freq}", ca.qpc_frequency(), cb.qpc_frequency()),
     );
-    let enables = session_a(true);
+    let enables = session_a(true, true);
     steps.run("enable_a", || {
         for e in &enables {
             sa.enable(e).unwrap_or_else(|err| panic!("enable {:?}: {err}", e.provider));
```

```diff
--- a/crates/atlas-agent/src/win/agent.rs
+++ b/crates/atlas-agent/src/win/agent.rs
@@ -131,7 +131,7 @@ pub fn start(opts: AgentOptions, sink: impl FnMut(Event) + Send + 'static) -> Re
         Intake::new(Which::Process, &config, started.ticks, queues(), intake_counters.clone(), None, &self_keys);
 
     let sensor = Session::start(&session::Config::session_a(&opts.sensor_session)).map_err(AgentError::Etw)?;
-    for e in providers::session_a(config.network_udp) {
+    for e in providers::session_a(config.network_udp, config.file_op_end) {
         sensor.enable(&e).map_err(AgentError::Etw)?;
     }
     let process = Session::start(&session::Config::session_b(&opts.process_session)).map_err(AgentError::Etw)?;
```

- [ ] **Step 2: The intake keeps no Cleanup state with `file.op_end` off.** `Intake::cleanups` becomes an `Option`, `None` when the setting is off: no Cleanup waits for an outcome, so none is counted unpaired, and every Cleanup still goes to the pipeline. A successful OperationEnd that arrives anyway is discarded and counted as before.

```diff
--- a/crates/atlas-agent/src/intake.rs
+++ b/crates/atlas-agent/src/intake.rs
@@ -42,7 +42,9 @@ pub struct Intake {
     early: Option<EarlyKeys>,
     fast_read: Option<FastRead>,
     self_keys: HashSet<u64>,
-    cleanups: Cleanups,
+    /// `None` while `file.op_end` is off: no OperationEnd comes, so no
+    /// Cleanup is paired or counted as unpaired (plan 1b-4a).
+    cleanups: Option<Cleanups>,
 }
 
 impl Intake {
@@ -62,7 +64,7 @@ impl Intake {
         Intake {
             kernel: queues.kernel,
             user: queues.user,
-            cleanups: Cleanups::new(ticks, counters.clone()),
+            cleanups: cfg.file_op_end.then(|| Cleanups::new(ticks, counters.clone())),
             counters,
             dns: Buckets::new(cfg.dns_rate_per_pid, ticks.frequency),
             early,
@@ -84,31 +86,41 @@ impl Intake {
                 return;
             }
         };
-        if let Some(irp) = cleanup::op_irp(&event) {
-            self.cleanups.next_op(irp);
-        }
         let mut early_outcome = None;
-        match &event {
-            RawEvent::FileOpEnd(o) if !o.failed() => {
-                if let End::Discard = self.cleanups.end(header, o.irp, o.extra_information) {
-                    IntakeCounters::bump(&self.counters.op_end_discarded);
-                    return;
-                }
+        if let Some(cleanups) = &mut self.cleanups {
+            if let Some(irp) = cleanup::op_irp(&event) {
+                cleanups.next_op(irp);
             }
-            RawEvent::FileOpEnd(o) => self.cleanups.failed(o.irp),
-            RawEvent::FileCleanup(x) => {
-                early_outcome = self
-                    .cleanups
-                    .cleanup(x.irp, x.file_object, x.file_key, header.ts)
-                    .map(|(h, info)| (h, FileOpEnd { irp: x.irp, extra_information: info, status: 0 }));
-            }
-            RawEvent::FileClose(x) => self.cleanups.close(x.file_object),
-            RawEvent::FileCreate(c) if c.delete_on_close() => self.cleanups.delete_on_close(c.file_object, header.ts),
-            RawEvent::FileDeletePath(p) => self.cleanups.request(p.file_key, p.irp, header.ts),
-            RawEvent::FileSetDelete(i) if i.extra_information != 0 => {
-                self.cleanups.request(i.file_key, i.irp, header.ts)
+            match &event {
+                RawEvent::FileOpEnd(o) if !o.failed() => {
+                    if let End::Discard = cleanups.end(header, o.irp, o.extra_information) {
+                        IntakeCounters::bump(&self.counters.op_end_discarded);
+                        return;
+                    }
+                }
+                RawEvent::FileOpEnd(o) => cleanups.failed(o.irp),
+                RawEvent::FileCleanup(x) => {
+                    early_outcome = cleanups
+                        .cleanup(x.irp, x.file_object, x.file_key, header.ts)
+                        .map(|(h, info)| (h, FileOpEnd { irp: x.irp, extra_information: info, status: 0 }));
+                }
+                RawEvent::FileClose(x) => cleanups.close(x.file_object),
+                RawEvent::FileCreate(c) if c.delete_on_close() => cleanups.delete_on_close(c.file_object, header.ts),
+                RawEvent::FileDeletePath(p) => cleanups.request(p.file_key, p.irp, header.ts),
+                RawEvent::FileSetDelete(i) if i.extra_information != 0 => {
+                    cleanups.request(i.file_key, i.irp, header.ts)
+                }
+                RawEvent::FileSetDelete(i) => cleanups.clear(i.file_key, i.file_object),
+                _ => {}
             }
-            RawEvent::FileSetDelete(i) => self.cleanups.clear(i.file_key, i.file_object),
+        } else if let RawEvent::FileOpEnd(o) = &event
+            && !o.failed()
+        {
+            // None is enabled; should one come, it is not needed.
+            IntakeCounters::bump(&self.counters.op_end_discarded);
+            return;
+        }
+        match &event {
             RawEvent::RegCreateKey(o) | RawEvent::RegOpenKey(o) if o.status == 0 => {
                 if let Some(m) = &mut self.early {
                     let evicted = m.open(o.key_object, o.base_object, &o.relative_name.to_string_lossy());
@@ -444,12 +456,18 @@ mod tests {
     type Reads = Arc<Mutex<Vec<(EarlyKey, ValueRead)>>>;
 
     fn intake(kernel_cap: usize) -> (Intake, Receiver<Incoming>, Receiver<Incoming>, Arc<IntakeCounters>, Reads) {
+        intake_with(kernel_cap, Config { dns_rate_per_pid: 2, ..Config::default() })
+    }
+
+    fn intake_with(
+        kernel_cap: usize,
+        cfg: Config,
+    ) -> (Intake, Receiver<Incoming>, Receiver<Incoming>, Arc<IntakeCounters>, Reads) {
         let (ktx, krx) = sync_channel(kernel_cap);
         let (utx, urx) = sync_channel(4);
         let counters = Arc::new(IntakeCounters::default());
         let reads: Reads = Arc::new(Mutex::new(Vec::new()));
         let r = reads.clone();
-        let cfg = Config { dns_rate_per_pid: 2, ..Config::default() };
         let i = Intake::new(
             Session::Sensor,
             &cfg,
@@ -616,6 +634,23 @@ mod tests {
         assert_eq!(IntakeCounters::get(&c.cleanup_outcome_late), 1);
     }
 
+    /// With `file.op_end` off no OperationEnd is enabled, so a Cleanup waits
+    /// for nothing: none is counted unpaired, and every Cleanup still goes to
+    /// the pipeline (plan 1b-4a).
+    #[test]
+    fn with_op_end_off_no_cleanup_is_paired_or_counted() {
+        let (mut i, krx, _, c, _) = intake_with(64, Config { file_op_end: false, ..Config::default() });
+        ev_at(&mut i, 0, cleanup(1, 0x10, 0x20));
+        ev_at(&mut i, 1, RawEvent::FileClose(FileHandle { irp: 1, file_object: 0x10, file_key: 0x20, issuing_tid: 1 }));
+        ev_at(&mut i, 2, cleanup(2, 0x11, 0x21));
+        ev_at(&mut i, 2_000, outcome(2, cleanup::FILE_DELETED)); // none is enabled; should one come
+        let got: Vec<_> = krx.try_iter().map(|inc| inc.event).collect();
+        assert_eq!(got.iter().filter(|e| matches!(e, RawEvent::FileCleanup(_))).count(), 2);
+        assert!(!got.iter().any(|e| matches!(e, RawEvent::FileOpEnd(_))));
+        assert_eq!(IntakeCounters::get(&c.cleanup_unpaired), 0);
+        assert_eq!(IntakeCounters::get(&c.cleanup_outcome_late), 0);
+    }
+
     /// A delete request's own OperationEnd is passed on, so the pipeline knows
     /// the request stood (review R-M1).
     #[test]
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-etw -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 365 passed, 7 ignored (`atlas-agent` unit tests: 180).
```powershell
git add crates
git commit -m "fix(etw,agent): file.op_end off leaves out OperationEnd and Cleanup pairing"
```

### Task 3: `agent.toml`

**Files:**
- Modify: `Cargo.toml`, `crates/atlas-agent/Cargo.toml`, `crates/atlas-agent/src/lib.rs`, `Cargo.lock` (by `cargo`)
- Create: `crates/atlas-agent/src/settings.rs`

- [ ] **Step 1: The dependencies of the whole plan,** added here so the lock file changes once. `atlas-proto` and `prost-reflect` move from dev-dependencies to dependencies (`dump` uses them, Task 6).

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,10 +9,12 @@ license = "AGPL-3.0-only"
 publish = false
 
 [workspace.dependencies]
+atlas-buffer = { path = "crates/atlas-buffer" }
 atlas-etw = { path = "crates/atlas-etw" }
 atlas-proto = { path = "crates/atlas-proto" }
 atlas-schema = { path = "crates/atlas-schema" }
 blake3 = "1.8"
+clap = { version = "4.6", features = ["derive"] }
 crc32c = "0.6"
 globset = "0.4"
 criterion = "0.8"
@@ -21,9 +23,14 @@ prost-build = "0.14"
 prost-reflect = { version = "0.16", features = ["serde"] }
 proptest = "1.11"
 protox = "0.9"
+serde = { version = "1", features = ["derive"] }
 serde_json = "1"
 sha2 = "0.11"
 tempfile = "3.27"
 thiserror = "2"
+toml = "1.1"
+tracing = "0.1"
+tracing-appender = "0.2"
+tracing-subscriber = "0.3"
 uuid = { version = "1.26", features = ["v7"] }
 windows = "0.62"
```

```diff
--- a/crates/atlas-agent/Cargo.toml
+++ b/crates/atlas-agent/Cargo.toml
@@ -8,11 +8,20 @@ license.workspace = true
 publish.workspace = true
 
 [dependencies]
+atlas-buffer.workspace = true
 atlas-etw.workspace = true
+atlas-proto.workspace = true
 atlas-schema.workspace = true
 blake3.workspace = true
+clap.workspace = true
 globset.workspace = true
+prost-reflect.workspace = true
+serde.workspace = true
 serde_json.workspace = true
+toml.workspace = true
+tracing.workspace = true
+tracing-appender.workspace = true
+tracing-subscriber.workspace = true
 uuid = { workspace = true, features = ["v4"] }
 
 [target.'cfg(windows)'.dependencies]
@@ -44,7 +53,5 @@ features = [
 ]
 
 [dev-dependencies]
-atlas-proto.workspace = true
 prost.workspace = true
-prost-reflect.workspace = true
 proptest.workspace = true
```

`cargo` adds 35 packages to `Cargo.lock`. The versions verified: `clap` 4.6.7, `serde` 1.0.229, `toml` 1.1.8, `tracing` 0.1.44, `tracing-appender` 0.2.5, `tracing-subscriber` 0.3.23.

- [ ] **Step 2: The settings.** `Settings::load(path)` gives the defaults when the file is missing; `Settings::parse(text)` reads the file into raw structures (`deny_unknown_fields` on each), then checks each value's range and builds the pipeline's `Config`, the services' `ServiceConfig`, the buffer's cap and overflow policy, and the log level. The module comment is the file's documentation, and a test parses it.
  - `transport` accepts only `"none"` until sub-project 2.
  - `buffer.cap_mib` is at least 64 (four 16 MiB segments, the buffer's own minimum) and at most 1 TiB.
  - `pipeline.hold_ms` is 100–10,000; the other durations have ranges of their own, each in the error message.
  - `watchlist.replace` replaces the built-in list, `watchlist.extend` adds to the list in use; both are glob patterns (§7.2). The list in use is compiled here, so a bad pattern is refused naming `watchlist.extend` or `watchlist.replace` (review finding 5), not later at the sensor's start.
  - There is no per-class enable (clarification 8).

`crates/atlas-agent/src/settings.rs`:
````rust
//! `agent.toml` (sensor spec §11.2; plan 1b-4a, D5): the settings an operator
//! may change, over built-in defaults. A missing file means the defaults; an
//! unknown key, a wrong type or a value out of range stops start-up with a
//! message naming the key. No hot reload.
//!
//! ```toml
//! transport = "none"          # the only value until sub-project 2
//! log_level = "info"          # error, warn, info, debug, trace
//!
//! [buffer]
//! cap_mib = 1024              # at least 64
//! overflow = "retention"      # retention, head_tail, drop_oldest, drop_newest
//!
//! [pipeline]
//! hold_ms = 750               # the ordering hold (§3.2)
//! deadline_ms = 1000          # completion deadline: enrichment, join, value read, 8.3 expansion
//! confirm_window_ms = 250     # failure confirmation (§5.5)
//!
//! [enrichment]
//! size_cap_mib = 100          # larger files are not hashed (§6.3)
//!
//! [watchlist]
//! replace = ['\Users\*\.ssh\*']   # instead of the built-in list
//! extend = ['**\secrets.txt']     # added to the list in use
//!
//! [network]
//! udp = true
//! udp_idle_s = 60
//! dns_rate_per_pid = 100      # DNS-Client events per process per second (§4.4)
//!
//! [file]
//! op_end = true               # off: the second §13 fallback
//!
//! [registry]
//! value_reads = true
//!
//! [seeding]
//! on_start = true
//! on_miss = true
//! cpu_ms_per_minute = 600     # the seeder's CPU budget (§7.4)
//! ```

use std::fmt;
use std::path::Path;
use std::time::Duration;

use atlas_buffer::Overflow;
use serde::Deserialize;

use crate::config::{Config, ServiceConfig};
use crate::watchlist::Watchlist;

/// The settings in effect: the defaults with `agent.toml` applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub config: Config,
    pub services: ServiceConfig,
    pub buffer: BufferSettings,
    pub log_level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferSettings {
    pub cap_bytes: u64,
    pub overflow: Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            config: Config::default(),
            services: ServiceConfig::default(),
            buffer: BufferSettings { cap_bytes: 1 << 30, overflow: Overflow::Retention },
            log_level: LogLevel::Info,
        }
    }
}

/// Why `agent.toml` was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SettingsError {}

/// The smallest buffer: four 16 MiB segments (`atlas-buffer`'s rule).
const MIN_CAP_MIB: u64 = 64;

impl Settings {
    /// Reads `path`; the defaults if it does not exist.
    pub fn load(path: &Path) -> Result<Settings, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text).map_err(|e| SettingsError(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(SettingsError(format!("{}: {e}", path.display()))),
        }
    }

    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let file: File = toml::from_str(text).map_err(|e| SettingsError(e.to_string().trim_end().to_owned()))?;
        file.apply()
    }
}

// The file's shape: every key optional, unknown keys refused.

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    transport: Option<String>,
    log_level: Option<LogLevel>,
    #[serde(default)]
    buffer: BufferFile,
    #[serde(default)]
    pipeline: PipelineFile,
    #[serde(default)]
    enrichment: EnrichmentFile,
    #[serde(default)]
    watchlist: WatchlistFile,
    #[serde(default)]
    network: NetworkFile,
    #[serde(default)]
    file: FileFile,
    #[serde(default)]
    registry: RegistryFile,
    #[serde(default)]
    seeding: SeedingFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BufferFile {
    cap_mib: Option<u64>,
    overflow: Option<OverflowFile>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OverflowFile {
    Retention,
    HeadTail,
    DropOldest,
    DropNewest,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PipelineFile {
    hold_ms: Option<u64>,
    deadline_ms: Option<u64>,
    confirm_window_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrichmentFile {
    size_cap_mib: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchlistFile {
    replace: Option<Vec<String>>,
    extend: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkFile {
    udp: Option<bool>,
    udp_idle_s: Option<u64>,
    dns_rate_per_pid: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFile {
    op_end: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    value_reads: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedingFile {
    on_start: Option<bool>,
    on_miss: Option<bool>,
    cpu_ms_per_minute: Option<u64>,
}

/// `key` must lie in `range`; the error names the key.
fn ranged(key: &str, v: u64, range: std::ops::RangeInclusive<u64>) -> Result<u64, SettingsError> {
    if range.contains(&v) {
        Ok(v)
    } else {
        Err(SettingsError(format!("{key} = {v} is outside {}..={}", range.start(), range.end())))
    }
}

impl File {
    fn apply(self) -> Result<Settings, SettingsError> {
        let mut s = Settings::default();
        if let Some(t) = self.transport
            && t != "none"
        {
            return Err(SettingsError(format!("transport = {t:?}: only \"none\" is supported")));
        }
        if let Some(l) = self.log_level {
            s.log_level = l;
        }

        if let Some(mib) = self.buffer.cap_mib {
            s.buffer.cap_bytes = ranged("buffer.cap_mib", mib, MIN_CAP_MIB..=1 << 20)? << 20;
        }
        if let Some(o) = self.buffer.overflow {
            s.buffer.overflow = match o {
                OverflowFile::Retention => Overflow::Retention,
                OverflowFile::HeadTail => Overflow::HeadTail,
                OverflowFile::DropOldest => Overflow::DropOldest,
                OverflowFile::DropNewest => Overflow::DropNewest,
            };
        }

        let c = &mut s.config;
        if let Some(ms) = self.pipeline.hold_ms {
            c.hold = Duration::from_millis(ranged("pipeline.hold_ms", ms, 100..=10_000)?);
        }
        if let Some(ms) = self.pipeline.deadline_ms {
            let d = Duration::from_millis(ranged("pipeline.deadline_ms", ms, 100..=60_000)?);
            c.enrich_deadline = d;
            c.join_deadline = d;
            c.value_read_deadline = d;
            c.expand_deadline = d;
        }
        if let Some(ms) = self.pipeline.confirm_window_ms {
            c.confirm_window = Duration::from_millis(ranged("pipeline.confirm_window_ms", ms, 10..=10_000)?);
        }
        if let Some(list) = self.watchlist.replace {
            c.watchlist = Some(list);
        }
        if let Some(list) = self.watchlist.extend {
            c.watchlist_extend = list;
        }
        // Compiled here so a bad pattern is refused naming its key, not later
        // at the sensor's start.
        if let Err(e) = Watchlist::new(c.watchlist.as_deref(), &c.watchlist_extend) {
            let key = if c.watchlist_extend.contains(&e.pattern) { "watchlist.extend" } else { "watchlist.replace" };
            return Err(SettingsError(format!("{key}: {:?} is not a valid pattern: {}", e.pattern, e.error)));
        }
        if let Some(on) = self.network.udp {
            c.network_udp = on;
        }
        if let Some(sec) = self.network.udp_idle_s {
            c.udp_idle = Duration::from_secs(ranged("network.udp_idle_s", sec, 1..=3_600)?);
        }
        if let Some(rate) = self.network.dns_rate_per_pid {
            c.dns_rate_per_pid = ranged("network.dns_rate_per_pid", u64::from(rate), 1..=100_000)? as u32;
        }
        if let Some(on) = self.file.op_end {
            c.file_op_end = on;
        }
        if let Some(on) = self.registry.value_reads {
            c.registry_value_reads = on;
        }
        if let Some(on) = self.seeding.on_start {
            c.seed_on_start = on;
        }
        if let Some(on) = self.seeding.on_miss {
            c.seed_on_miss = on;
        }

        if let Some(mib) = self.enrichment.size_cap_mib {
            s.services.hash_size_cap = ranged("enrichment.size_cap_mib", mib, 1..=1 << 20)? << 20;
        }
        if let Some(ms) = self.seeding.cpu_ms_per_minute {
            s.services.seeder_cpu = Duration::from_millis(ranged("seeding.cpu_ms_per_minute", ms, 1..=60_000)?);
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
        assert_eq!(Settings::parse("# only a comment\n").unwrap(), Settings::default());
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let s = Settings::load(Path::new("no-such-dir/agent.toml")).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn the_documented_example_parses() {
        let doc = include_str!("settings.rs");
        let example: String = doc
            .lines()
            .skip_while(|l| !l.starts_with("//! ```toml"))
            .skip(1)
            .take_while(|l| !l.starts_with("//! ```"))
            .map(|l| l.strip_prefix("//! ").or(l.strip_prefix("//!")).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let s = Settings::parse(&example).unwrap();
        assert_eq!(s.config.watchlist.as_deref(), Some(&[r"\Users\*\.ssh\*".to_owned()][..]));
        assert_eq!(s.config.watchlist_extend, [r"**\secrets.txt"]);
        // The rest of the example restates the defaults.
        let defaults = Settings::default();
        assert_eq!(Settings { config: defaults.config.clone(), ..s.clone() }, defaults);
        assert_eq!(Config { watchlist: None, watchlist_extend: Vec::new(), ..s.config }, defaults.config);
    }

    #[test]
    fn every_key_is_applied() {
        let s = Settings::parse(
            r#"
            log_level = "debug"
            [buffer]
            cap_mib = 256
            overflow = "drop_newest"
            [pipeline]
            hold_ms = 1000
            deadline_ms = 2000
            confirm_window_ms = 300
            [enrichment]
            size_cap_mib = 10
            [network]
            udp = false
            udp_idle_s = 30
            dns_rate_per_pid = 5
            [file]
            op_end = false
            [registry]
            value_reads = false
            [seeding]
            on_start = false
            on_miss = false
            cpu_ms_per_minute = 100
            "#,
        )
        .unwrap();
        assert_eq!(s.log_level, LogLevel::Debug);
        assert_eq!(s.buffer, BufferSettings { cap_bytes: 256 << 20, overflow: Overflow::DropNewest });
        let c = &s.config;
        assert_eq!(c.hold, Duration::from_secs(1));
        let two = Duration::from_secs(2);
        assert_eq!([c.enrich_deadline, c.join_deadline, c.value_read_deadline, c.expand_deadline], [two; 4]);
        assert_eq!(c.confirm_window, Duration::from_millis(300));
        assert_eq!((c.network_udp, c.udp_idle, c.dns_rate_per_pid), (false, Duration::from_secs(30), 5));
        assert!(!c.file_op_end && !c.registry_value_reads && !c.seed_on_start && !c.seed_on_miss);
        assert_eq!(s.services.hash_size_cap, 10 << 20);
        assert_eq!(s.services.seeder_cpu, Duration::from_millis(100));
    }

    #[test]
    fn unknown_keys_are_refused_by_name() {
        let e = Settings::parse("[network]\nudp = true\ntcp = false\n").unwrap_err();
        assert!(e.0.contains("tcp"), "{e}");
        let e = Settings::parse("[nonsense]\n").unwrap_err();
        assert!(e.0.contains("nonsense"), "{e}");
        let e = Settings::parse("hold_ms = 5\n").unwrap_err();
        assert!(e.0.contains("hold_ms"), "{e}");
    }

    #[test]
    fn wrong_types_and_values_are_refused() {
        for (text, needle) in [
            ("[network]\nudp = \"yes\"\n", "udp"),
            ("log_level = \"loud\"\n", "loud"),
            ("[buffer]\noverflow = \"keep_all\"\n", "keep_all"),
            ("[buffer]\ncap_mib = 16\n", "buffer.cap_mib"),
            ("[pipeline]\nhold_ms = 0\n", "pipeline.hold_ms"),
            ("[network]\ndns_rate_per_pid = 0\n", "network.dns_rate_per_pid"),
            ("[seeding]\ncpu_ms_per_minute = 0\n", "seeding.cpu_ms_per_minute"),
            ("[pipeline]\nhold_ms = -1\n", "hold_ms"),
            ("transport = \"grpc\"\n", "transport"),
            ("[network\n", "network"),
            ("[watchlist]\nextend = ['[']\n", "watchlist.extend: \"[\""),
            ("[watchlist]\nreplace = ['**', '[']\n", "watchlist.replace: \"[\""),
        ] {
            let e = Settings::parse(text).unwrap_err();
            assert!(e.0.contains(needle), "{text:?}: {e}");
        }
    }

    #[test]
    fn the_buffer_cap_matches_the_buffers_own_rule() {
        let s = Settings::parse(&format!("[buffer]\ncap_mib = {MIN_CAP_MIB}\n")).unwrap();
        let cfg = atlas_buffer::Config { cap_bytes: s.buffer.cap_bytes, ..atlas_buffer::Config::new("x") };
        assert_eq!(cfg.cap_bytes, 4 * cfg.segment_bytes);
    }
}
````

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -16,6 +16,7 @@ pub mod pipeline;
 pub mod process;
 pub mod recent;
 pub mod services;
+pub mod settings;
 pub mod time;
 pub mod watchlist;
 #[cfg(windows)]
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent settings
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 372 passed, 7 ignored (`atlas-agent` unit tests: 187).
```powershell
git add Cargo.toml Cargo.lock crates/atlas-agent
git commit -m "feat(agent): agent.toml settings over the built-in defaults"
```

### Task 4: Sensor Health reports from counter snapshots

**Files:**
- Modify: `crates/atlas-agent/src/counters.rs`, `crates/atlas-agent/src/lib.rs`
- Create: `crates/atlas-agent/src/health.rs`

- [ ] **Step 1: Plain copies of the shared counters.** `IntakeCounters::totals()` and `ServiceCounters::totals()` read every atomic once into a plain struct, so a snapshot can be compared and stored.

```diff
--- a/crates/atlas-agent/src/counters.rs
+++ b/crates/atlas-agent/src/counters.rs
@@ -102,6 +102,38 @@ impl IntakeCounters {
     pub fn get(c: &AtomicU64) -> u64 {
         c.load(Ordering::Relaxed)
     }
+
+    /// The values now, for a Sensor Health snapshot (plan 1b-4a).
+    pub fn totals(&self) -> IntakeTotals {
+        let g = Self::get;
+        IntakeTotals {
+            kernel_queue_drops: g(&self.kernel_queue_drops),
+            user_queue_drops: g(&self.user_queue_drops),
+            dns_rate_limit_drops: g(&self.dns_rate_limit_drops),
+            parse_errors: g(&self.parse_errors),
+            unknown_version: g(&self.unknown_version),
+            early_key_map_evictions: g(&self.early_key_map_evictions),
+            op_end_discarded: g(&self.op_end_discarded),
+            fast_reads: g(&self.fast_reads),
+            cleanup_unpaired: g(&self.cleanup_unpaired),
+            cleanup_outcome_late: g(&self.cleanup_outcome_late),
+        }
+    }
+}
+
+/// [`IntakeCounters`] read at one instant.
+#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
+pub struct IntakeTotals {
+    pub kernel_queue_drops: u64,
+    pub user_queue_drops: u64,
+    pub dns_rate_limit_drops: u64,
+    pub parse_errors: u64,
+    pub unknown_version: u64,
+    pub early_key_map_evictions: u64,
+    pub op_end_discarded: u64,
+    pub fast_reads: u64,
+    pub cleanup_unpaired: u64,
+    pub cleanup_outcome_late: u64,
 }
 
 /// Counters kept by the Windows services (plan 1b-3b), shared with their
@@ -126,3 +158,34 @@ pub struct ServiceCounters {
     /// Gauge, updated at the end of each snapshot.
     pub seeder_stuck_helpers: AtomicU64,
 }
+
+impl ServiceCounters {
+    /// The values now, for a Sensor Health snapshot (plan 1b-4a).
+    pub fn totals(&self) -> ServiceTotals {
+        let g = IntakeCounters::get;
+        ServiceTotals {
+            service_queue_drops: g(&self.service_queue_drops),
+            hash_cache_evictions: g(&self.hash_cache_evictions),
+            seeder_handles_named: g(&self.seeder_handles_named),
+            seeder_handles_failed: g(&self.seeder_handles_failed),
+            seeder_handles_timed_out: g(&self.seeder_handles_timed_out),
+            seeder_table_reads: g(&self.seeder_table_reads),
+            seeder_deferred_rereads: g(&self.seeder_deferred_rereads),
+            seeder_stuck_helpers: g(&self.seeder_stuck_helpers),
+        }
+    }
+}
+
+/// [`ServiceCounters`] read at one instant.
+#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
+pub struct ServiceTotals {
+    pub service_queue_drops: u64,
+    pub hash_cache_evictions: u64,
+    pub seeder_handles_named: u64,
+    pub seeder_handles_failed: u64,
+    pub seeder_handles_timed_out: u64,
+    pub seeder_table_reads: u64,
+    pub seeder_deferred_rereads: u64,
+    /// A gauge.
+    pub seeder_stuck_helpers: u64,
+}
```

- [ ] **Step 2: Snapshots and reports.** `Snapshot` holds every cumulative figure the agent has: the pipeline's `Counters` (`None` when it did not answer), the intake's and the services' totals, the callbacks' panics, both sessions' losses (`None` when the query failed), the sink's drops, the CPU time and the working set.
  - `report(prev, now)` gives the `HealthReport` of the differences. The sessions' counters are 32-bit and wrap, so they are differenced with `wrapping_sub`. A field whose source is `None` in either snapshot is absent.
  - `Snapshot::advance` keeps the previous value of a source that was not read, so the next report covers both intervals.
  - `file_cleanup_unpaired` is the sum of the intake's two parts; `Diagnostics` keeps them apart for the log, with the self-filtered events, the discarded OperationEnds and the fast reads.
  - Left absent: `buffer_invalid_records`, `seeder_negative_cache_size` (clarification 5) and `retention_evictions` (the writer fills it in, Task 5). `buffer_backlog_drops` is computed here from the sink's counter, which decides an early report; the writer replaces it with its own exact figure (Task 5).
  - `loss_rose` is true when any loss counter rose. `event` wraps a report in a Sensor Health `Event`; a test checks it passes 0a's validation.

`crates/atlas-agent/src/health.rs`:
```rust
//! Sensor Health reports (sensor spec §9.3, §10.3; plan 1b-4a, D1): the
//! counters of every thread, read into a [`Snapshot`], and a report made of
//! the differences between two snapshots.
//!
//! The health thread takes the snapshots: the pipeline's [`Counters`] through
//! the driver's control channel, the intake's and the services' atomics, the
//! sessions' losses, the process's CPU time and working set. It reports every
//! 60 s, sooner when a loss counter rose ([`loss_rose`]). The buffer writer
//! fills in the report's `buffer` group, `housekeeping.retention_evictions` and
//! the gap as it appends the report (`store`).

use std::collections::BTreeMap;

use atlas_schema::classes::sensor_health::{
    ClassCount, HealthReport, SensorHealthAction, SensorHealthActivity, SensorHousekeeping, SensorLoss, SensorQuality,
    SensorResources,
};
use atlas_schema::{Device, Event, EventId, EventKind, EventMeta, Sensor};

use crate::counters::{Class, Counters, IntakeTotals, ServiceTotals};

/// One ETW session's loss counters (`EVENT_TRACE_PROPERTIES`): 32-bit and
/// cumulative, so differences wrap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionLoss {
    pub events_lost: u32,
    pub buffers_lost: u32,
}

/// Every cumulative figure at one instant. `None` is "not read this time"
/// (the pipeline did not answer, a query failed); such fields are absent from
/// the report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub pipeline: Option<Counters>,
    pub intake: IntakeTotals,
    pub services: ServiceTotals,
    pub callback_panics: u64,
    pub sensor_session: Option<SessionLoss>,
    pub process_session: Option<SessionLoss>,
    /// Events the buffer could not take: the sink's channel was full, or the
    /// writer's backlog was. It decides an early report and the log line; the
    /// writer thread fills the report's `loss.buffer_backlog_drops` itself.
    pub buffer_drops: u64,
    /// Process CPU time (user + kernel), ns.
    pub cpu_time: Option<u64>,
    /// A gauge, bytes.
    pub working_set: Option<u64>,
    pub seeding: bool,
}

impl Snapshot {
    /// The state at start: everything zero, the sessions just created.
    pub fn at_start(seeding: bool) -> Snapshot {
        Snapshot {
            pipeline: Some(Counters::default()),
            sensor_session: Some(SessionLoss::default()),
            process_session: Some(SessionLoss::default()),
            cpu_time: Some(0),
            seeding,
            ..Snapshot::default()
        }
    }

    /// What the next report is measured from: `now`, keeping `self`'s values
    /// for what `now` could not read, so nothing is counted twice or lost.
    pub fn advance(&self, now: &Snapshot) -> Snapshot {
        Snapshot {
            pipeline: now.pipeline.clone().or_else(|| self.pipeline.clone()),
            sensor_session: now.sensor_session.or(self.sensor_session),
            process_session: now.process_session.or(self.process_session),
            cpu_time: now.cpu_time.or(self.cpu_time),
            ..now.clone()
        }
    }
}

/// Figures for the diagnostic log only: no schema field (plan 1b-4a, D5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// The two parts of `quality.file_cleanup_unpaired` (plan 1b-3c, R-M4).
    pub cleanup_unpaired: u64,
    pub cleanup_outcome_late: u64,
    /// Events the self-filter dropped, successful OperationEnds discarded in
    /// the callback, fast-path value reads sent.
    pub self_filtered: Option<u64>,
    pub op_end_discarded: u64,
    pub fast_reads: u64,
}

fn d(prev: u64, now: u64) -> u64 {
    now.saturating_sub(prev)
}

fn d32(prev: u32, now: u32) -> u64 {
    u64::from(now.wrapping_sub(prev))
}

/// Per-class differences, zero counts left out.
fn d_classes(prev: &BTreeMap<Class, u64>, now: &BTreeMap<Class, u64>) -> Vec<ClassCount> {
    now.iter()
        .map(|(class, &n)| ClassCount { class_uid: class.uid(), count: d(prev.get(class).copied().unwrap_or(0), n) })
        .filter(|c| c.count > 0)
        .collect()
}

/// The report for the interval from `prev` to `now`, without the buffer's
/// figures (the writer adds them), and the figures for the log.
pub fn report(prev: &Snapshot, now: &Snapshot) -> (HealthReport, Diagnostics) {
    let (i0, i1) = (&prev.intake, &now.intake);
    let (s0, s1) = (&prev.services, &now.services);
    let session = |p: Option<SessionLoss>, n: Option<SessionLoss>| match (p, n) {
        (Some(p), Some(n)) => (Some(d32(p.events_lost, n.events_lost)), Some(d32(p.buffers_lost, n.buffers_lost))),
        _ => (None, None),
    };
    let (sensor_events, sensor_buffers) = session(prev.sensor_session, now.sensor_session);
    let (process_events, process_buffers) = session(prev.process_session, now.process_session);
    let pipe = match (&prev.pipeline, &now.pipeline) {
        (Some(p), Some(n)) => Some((p, n)),
        _ => None,
    };
    // A pipeline counter's difference, absent when the pipeline was not read.
    let p = |f: fn(&Counters) -> u64| pipe.map(|(a, b)| d(f(a), f(b)));

    let loss = SensorLoss {
        sensor_session_events_lost: sensor_events,
        process_session_events_lost: process_events,
        sensor_session_buffers_lost: sensor_buffers,
        process_session_buffers_lost: process_buffers,
        kernel_queue_drops: Some(d(i0.kernel_queue_drops, i1.kernel_queue_drops)),
        user_queue_drops: Some(d(i0.user_queue_drops, i1.user_queue_drops)),
        dns_rate_limit_drops: Some(d(i0.dns_rate_limit_drops, i1.dns_rate_limit_drops)),
        actor_dropped: pipe.map(|(a, b)| d_classes(&a.actor_dropped, &b.actor_dropped)).unwrap_or_default(),
        buffer_backlog_drops: Some(d(prev.buffer_drops, now.buffer_drops)),
        callback_panics: Some(d(prev.callback_panics, now.callback_panics)),
    };
    let quality = SensorQuality {
        late_arrivals: p(|c| c.late_arrivals),
        parse_errors: Some(d(i0.parse_errors, i1.parse_errors)),
        unknown_version: Some(d(i0.unknown_version, i1.unknown_version)),
        launch_join_miss: p(|c| c.launch_join_miss),
        actor_unresolved: pipe.map(|(a, b)| d_classes(&a.actor_unresolved, &b.actor_unresolved)).unwrap_or_default(),
        unknown_file_object: p(|c| c.unknown_file_object),
        registry_unresolved: p(|c| c.registry_unresolved),
        value_read_failed: p(|c| c.value_read_failed),
        early_read_redone: p(|c| c.early_read_redone),
        reg_type_unusual: p(|c| c.reg_type_unusual),
        file_op_late_failure: p(|c| c.file_op_late_failure),
        writes_after_cleanup: p(|c| c.writes_after_cleanup),
        file_object_replaced: p(|c| c.file_object_replaced),
        // The agent never replays its buffer; `dump` and the transport do.
        buffer_invalid_records: None,
        enrichment_misses: p(|c| c.enrichment_misses),
        enrichment_errors: p(|c| c.enrichment_errors),
        reg_name_ambiguous: p(|c| c.reg_name_ambiguous),
        file_delete_outcome_unknown: p(|c| c.file_delete_outcome_unknown),
        file_cleanup_unpaired: Some(
            d(i0.cleanup_unpaired, i1.cleanup_unpaired) + d(i0.cleanup_outcome_late, i1.cleanup_outcome_late),
        ),
    };
    let housekeeping = SensorHousekeeping {
        process_cache_evictions: p(|c| c.process_cache_evictions),
        file_map_evictions: p(|c| c.file_map_evictions),
        key_map_evictions: p(|c| c.key_map_evictions),
        early_key_map_evictions: Some(d(i0.early_key_map_evictions, i1.early_key_map_evictions)),
        flow_table_evictions: p(|c| c.flow_table_evictions),
        hash_cache_evictions: Some(d(s0.hash_cache_evictions, s1.hash_cache_evictions)),
        // The writer's figure.
        retention_evictions: None,
        file_op_failed: p(|c| c.file_op_failed),
        pending_overflow: p(|c| c.pending_overflow),
        seeding_enabled: Some(now.seeding),
        seeder_handles_named: Some(d(s0.seeder_handles_named, s1.seeder_handles_named)),
        seeder_handles_failed: Some(d(s0.seeder_handles_failed, s1.seeder_handles_failed)),
        seeder_handles_timed_out: Some(d(s0.seeder_handles_timed_out, s1.seeder_handles_timed_out)),
        seeder_table_reads: Some(d(s0.seeder_table_reads, s1.seeder_table_reads)),
        seeder_deferred_rereads: Some(d(s0.seeder_deferred_rereads, s1.seeder_deferred_rereads)),
        seeder_stuck_helpers: Some(s1.seeder_stuck_helpers),
        // No source yet: the seeder keeps no count of its negative cache.
        seeder_negative_cache_size: None,
        service_queue_drops: Some(d(s0.service_queue_drops, s1.service_queue_drops)),
    };
    let resources = SensorResources {
        cpu_time: prev.cpu_time.zip(now.cpu_time).map(|(a, b)| d(a, b)),
        working_set: now.working_set,
    };
    let diagnostics = Diagnostics {
        cleanup_unpaired: d(i0.cleanup_unpaired, i1.cleanup_unpaired),
        cleanup_outcome_late: d(i0.cleanup_outcome_late, i1.cleanup_outcome_late),
        self_filtered: p(|c| c.self_filtered),
        op_end_discarded: d(i0.op_end_discarded, i1.op_end_discarded),
        fast_reads: d(i0.fast_reads, i1.fast_reads),
    };
    let report = HealthReport { loss, quality, housekeeping, resources, ..HealthReport::default() };
    (report, diagnostics)
}

/// Whether any loss counter rose between the two snapshots: a report is then
/// due at once (§9.3).
pub fn loss_rose(prev: &Snapshot, now: &Snapshot) -> bool {
    let l = report(prev, now).0.loss;
    let counts = [
        l.sensor_session_events_lost,
        l.process_session_events_lost,
        l.sensor_session_buffers_lost,
        l.process_session_buffers_lost,
        l.kernel_queue_drops,
        l.user_queue_drops,
        l.dns_rate_limit_drops,
        l.buffer_backlog_drops,
        l.callback_panics,
    ];
    counts.iter().any(|c| c.is_some_and(|n| n > 0)) || l.actor_dropped.iter().any(|c| c.count > 0)
}

/// The Sensor Health event: `interval_start` and `time` in Unix ns.
pub fn event(id: EventId, time: i64, device: Device, interval_start: i64, report: HealthReport) -> Event {
    Event {
        meta: EventMeta { event_id: id, time, sensor: Sensor::Etw },
        device,
        kind: EventKind::SensorHealth(SensorHealthActivity {
            interval_start,
            action: SensorHealthAction::Report(Box::new(report)),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_schema::{BootId, DeviceUid};

    fn busy() -> Snapshot {
        let mut c = Counters { late_arrivals: 5, file_op_failed: 2, self_filtered: 40, ..Counters::default() };
        c.actor_dropped.insert(Class::File, 3);
        c.actor_unresolved.insert(Class::Network, 1);
        Snapshot {
            pipeline: Some(c),
            intake: IntakeTotals {
                kernel_queue_drops: 7,
                cleanup_unpaired: 2,
                cleanup_outcome_late: 1,
                ..Default::default()
            },
            services: ServiceTotals { seeder_handles_named: 9, seeder_stuck_helpers: 1, ..Default::default() },
            callback_panics: 0,
            sensor_session: Some(SessionLoss { events_lost: 4, buffers_lost: 0 }),
            process_session: Some(SessionLoss::default()),
            buffer_drops: 6,
            cpu_time: Some(1_000),
            working_set: Some(50 << 20),
            seeding: true,
        }
    }

    #[test]
    fn a_report_is_the_difference_of_two_snapshots() {
        let (r, diag) = report(&Snapshot::at_start(true), &busy());
        assert_eq!(r.loss.kernel_queue_drops, Some(7));
        assert_eq!(r.loss.sensor_session_events_lost, Some(4));
        assert_eq!(r.loss.process_session_events_lost, Some(0));
        assert_eq!(r.loss.buffer_backlog_drops, Some(6));
        assert_eq!(r.loss.actor_dropped, [ClassCount { class_uid: 1001, count: 3 }]);
        assert_eq!(r.quality.late_arrivals, Some(5));
        assert_eq!(r.quality.actor_unresolved, [ClassCount { class_uid: 4001, count: 1 }]);
        assert_eq!(r.quality.file_cleanup_unpaired, Some(3));
        assert_eq!(r.housekeeping.file_op_failed, Some(2));
        assert_eq!(r.housekeeping.seeder_handles_named, Some(9));
        assert_eq!(r.housekeeping.seeding_enabled, Some(true));
        assert_eq!(r.resources.cpu_time, Some(1_000));
        assert_eq!(r.resources.working_set, Some(50 << 20));
        assert_eq!((diag.cleanup_unpaired, diag.cleanup_outcome_late, diag.self_filtered), (2, 1, Some(40)));

        // The next interval counts only what is new; gauges are taken as they are.
        let mut later = busy();
        later.intake.kernel_queue_drops = 10;
        later.pipeline.as_mut().unwrap().actor_dropped.insert(Class::File, 3);
        let (r, _) = report(&busy(), &later);
        assert_eq!(r.loss.kernel_queue_drops, Some(3));
        assert_eq!(r.loss.sensor_session_events_lost, Some(0));
        assert!(r.loss.actor_dropped.is_empty(), "zero counts are left out");
        assert_eq!(r.housekeeping.seeder_stuck_helpers, Some(1));
        assert_eq!(r.resources.working_set, Some(50 << 20));
    }

    #[test]
    fn session_counters_wrap() {
        let mut a = busy();
        a.sensor_session = Some(SessionLoss { events_lost: u32::MAX - 1, buffers_lost: u32::MAX });
        let mut b = busy();
        b.sensor_session = Some(SessionLoss { events_lost: 2, buffers_lost: 0 });
        let (r, _) = report(&a, &b);
        assert_eq!(r.loss.sensor_session_events_lost, Some(4));
        assert_eq!(r.loss.sensor_session_buffers_lost, Some(1));
    }

    #[test]
    fn what_was_not_read_is_absent_and_counted_later() {
        let start = Snapshot::at_start(true);
        let mut blind = busy();
        blind.pipeline = None;
        blind.sensor_session = None;
        let (r, diag) = report(&start, &blind);
        assert_eq!(r.quality.late_arrivals, None);
        assert!(r.loss.actor_dropped.is_empty());
        assert_eq!(r.loss.sensor_session_events_lost, None);
        assert_eq!(diag.self_filtered, None);
        assert_eq!(r.loss.kernel_queue_drops, Some(7), "the atomics are read regardless");

        // The next report covers both intervals for what was missed.
        let from = start.advance(&blind);
        let (r, _) = report(&from, &busy());
        assert_eq!(r.quality.late_arrivals, Some(5));
        assert_eq!(r.loss.sensor_session_events_lost, Some(4));
        assert_eq!(r.loss.kernel_queue_drops, Some(0));
    }

    #[test]
    fn a_rising_loss_counter_is_seen_and_other_counters_are_not() {
        let base = busy();
        assert!(!loss_rose(&base, &base));
        let mut quality_only = busy();
        quality_only.pipeline.as_mut().unwrap().late_arrivals += 100;
        quality_only.intake.cleanup_unpaired += 1;
        assert!(!loss_rose(&base, &quality_only));
        let rises: [fn(&mut Snapshot); 6] = [
            |s| s.intake.kernel_queue_drops += 1,
            |s| s.intake.dns_rate_limit_drops += 1,
            |s| s.buffer_drops += 1,
            |s| s.callback_panics += 1,
            |s| s.process_session.as_mut().unwrap().buffers_lost += 1,
            |s| *s.pipeline.as_mut().unwrap().actor_dropped.entry(Class::Dns).or_default() += 1,
        ];
        for (i, rise) in rises.iter().enumerate() {
            let mut s = busy();
            rise(&mut s);
            assert!(loss_rose(&base, &s), "rise {i}");
        }
    }

    #[test]
    fn the_event_carries_the_interval_and_validates() {
        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
        let (r, _) = report(&Snapshot::at_start(false), &busy());
        let e = event(EventId::new_v7(), 2_000, device, 1_000, r.clone());
        let back = atlas_schema::decode_event(&atlas_schema::encode_event(e.clone())).unwrap();
        assert_eq!(back, e);
        let EventKind::SensorHealth(a) = back.kind else { panic!("not a Sensor Health event") };
        assert_eq!(a.interval_start, 1_000);
        assert_eq!(a.action, SensorHealthAction::Report(Box::new(r)));
    }
}
```

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -7,6 +7,7 @@ pub mod counters;
 pub mod driver;
 pub mod evict;
 pub mod fakes;
+pub mod health;
 pub mod input;
 pub mod intake;
 pub mod keymap;
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent health
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 377 passed, 7 ignored (`atlas-agent` unit tests: 192).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): Sensor Health reports from counter snapshots"
```

### Task 5: The buffer writer thread and the pipeline's sink

**Files:**
- Modify: `crates/atlas-agent/src/lib.rs`, `crates/atlas-buffer/src/writer.rs`, `crates/atlas-buffer/tests/log.rs`
- Create: `crates/atlas-agent/src/store.rs`

- [ ] **Step 1: `Writer::close` returns its final counts** (F9), the records of its own last write included. The buffer's `close_writes_everything` checks them.

```diff
--- a/crates/atlas-buffer/src/writer.rs
+++ b/crates/atlas-buffer/src/writer.rs
@@ -183,9 +183,11 @@ impl Writer {
         }
     }
 
-    /// Clean stop: writes and flushes everything queued.
-    pub fn close(mut self) -> io::Result<()> {
-        self.flush_now()
+    /// Clean stop: writes and flushes everything queued. Returns the final
+    /// counts, this last write's records included.
+    pub fn close(mut self) -> io::Result<Stats> {
+        self.flush_now()?;
+        Ok(self.stats)
     }
 
     /// Persists `cursor` as delivered and deletes the segments wholly before it (§8.5).
```

```diff
--- a/crates/atlas-buffer/tests/log.rs
+++ b/crates/atlas-buffer/tests/log.rs
@@ -63,8 +63,9 @@ fn close_writes_everything() {
     for t in 0..3 {
         w.append(&rec(t)).unwrap();
     }
-    w.close().unwrap();
+    let stats = w.close().unwrap();
     assert_eq!(read_times(tmp.path(), Cursor::default()), vec![0, 1, 2]);
+    assert_eq!(stats.records_written, 3, "the records written by the close are counted");
 }
 
 #[test]
```

- [ ] **Step 2: The writer thread.**
  - `Store::start` opens the buffer on the calling thread, so an error stops start-up, then starts the thread `atlas-buffer`. It returns what recovery found.
  - `Sink::event` uses `try_send`: a full channel drops the event and counts it. The channel holds 65,536 records.
  - The thread receives `Record::Event` and `Record::Health`. It appends events as they come and calls `tick` about every 100 ms, also while records keep arriving (at most about 200: a wait for the first record, then a drain as long; the writer flushes every second). A backlog drop is counted in the same counter as the channel's.
  - For a report, `complete` fills in the `buffer` group and `retention_evictions` as differences of the writer's `Stats` since the last report, and `loss.buffer_backlog_drops` as the difference of the shared drop counter since the last report: everything queued ahead of the report has been handled by then, the final drain's drops included (review finding 3); `failing` and `disk_bytes` as gauges, the recovery in the first report only, and the gaps merged into one (the sum of the counts, the widest range).
  - A report the backlog cannot take is held, and the held reports are written before anything else once there is room; past 16 the oldest goes. The figures stay as they were when the report was made.
  - `Store::close` drops the last sender; the thread writes what is held, then the writer closes (flushing what is queued) and its final counts are returned (F9). If the thread panicked, `close` returns the panic's message as an error instead of panicking in turn; `Store::is_running` says whether the thread still runs (review finding 7). `crate::panic_message` reads a panic's message from a join.
  - The tests also run the thread over a writer that panics (`PanicsAfter`, test-only, also behind `StoreConfig::panic_after`, which exists only in test builds and serves Task 8's test), and the full-channel test sends from its own thread with a bounded wait.
  - The `Log` trait stands between the thread and `atlas_buffer::Writer`, so tests can fail the disk.

`crates/atlas-agent/src/store.rs`:
```rust
//! The buffer writer thread (sensor spec §3.2 [6], §8; plan 1b-4a, D1 and D5).
//!
//! - [`Sink`]: what the pipeline thread calls with each emitted event. It moves
//!   the event into a bounded channel and never blocks; a full channel drops
//!   the event and counts it (`loss.buffer_backlog_drops`, with the events the
//!   writer's backlog could not take).
//! - The thread owns the `atlas_buffer::Writer`. It encodes each event, appends
//!   it, and calls `tick` about every 100 ms (at most about 200: a wait for
//!   the first record, then a drain as long); `tick` may block on the disk
//!   while the channel absorbs events. The writer flushes every second, so
//!   neither bound matters to it.
//! - A Sensor Health report arrives from the health thread with everything but
//!   the buffer's figures, which the thread fills in as it appends it: the
//!   `buffer` group, `loss.buffer_backlog_drops`, `housekeeping.retention_evictions`,
//!   the gap, and in the first report what recovery found at open. Counted
//!   here, a report includes the drops of every event queued ahead of it, the
//!   final report's too. A report the backlog cannot
//!   take is held and written before anything else once there is room, so its
//!   counts are not lost.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use atlas_buffer::{Dropped, Gap, Recovery, Stats, Writer};
use atlas_schema::classes::sensor_health::{SensorGap, SensorHealthAction};
use atlas_schema::{Event, EventKind, decode_event, encode_event};

/// What the writer thread receives.
pub enum Record {
    Event(Event),
    /// A Sensor Health event whose buffer figures the thread fills in.
    Health(Event),
}

/// The pipeline's sink (plan 1b-3c's interface): never blocks.
#[derive(Clone)]
pub struct Sink {
    tx: SyncSender<Record>,
    drops: Arc<AtomicU64>,
}

impl Sink {
    pub fn event(&self, e: Event) {
        if self.tx.try_send(Record::Event(e)).is_err() {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The running writer thread.
pub struct Store {
    tx: SyncSender<Record>,
    drops: Arc<AtomicU64>,
    thread: JoinHandle<io::Result<Stats>>,
}

/// Settings of the writer thread.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub buffer: atlas_buffer::Config,
    /// Events waiting for the thread (65,536 by default).
    pub channel_cap: usize,
    /// Tests: the writer thread panics after this many appends.
    #[cfg(test)]
    pub panic_after: Option<u64>,
}

impl StoreConfig {
    pub fn new(buffer: atlas_buffer::Config) -> Self {
        StoreConfig {
            buffer,
            channel_cap: 65_536,
            #[cfg(test)]
            panic_after: None,
        }
    }
}

/// The wait for a record, and the drain after it, before each `tick`.
const TICK_EVERY: Duration = Duration::from_millis(100);
/// Held Sensor Health reports kept while the backlog is full; past it the
/// oldest goes.
const HELD_CAP: usize = 16;

/// A gap's time range comes from the record's event (plan 1b-1).
fn record_time(bytes: &[u8]) -> Option<i64> {
    decode_event(bytes).ok().map(|e| e.meta.time)
}

impl Store {
    /// Opens the buffer on the calling thread, so an error stops start-up,
    /// then starts the writer thread. Returns what recovery found.
    pub fn start(cfg: StoreConfig) -> io::Result<(Store, Recovery)> {
        let (writer, recovery) = Writer::open(cfg.buffer, record_time)?;
        #[cfg(test)]
        if let Some(left) = cfg.panic_after {
            let writer = tests::PanicsAfter { inner: writer, left };
            return Ok((Store::spawn(writer, recovery.clone(), cfg.channel_cap), recovery));
        }
        Ok((Store::spawn(writer, recovery.clone(), cfg.channel_cap), recovery))
    }

    /// The writer thread over `writer`.
    pub(crate) fn spawn<W: Log + Send + 'static>(writer: W, recovery: Recovery, channel_cap: usize) -> Store {
        let (tx, rx) = sync_channel(channel_cap);
        let drops = Arc::new(AtomicU64::new(0));
        let mut core = Core::new(writer, drops.clone(), recovery);
        let thread = std::thread::Builder::new()
            .name("atlas-buffer".into())
            .spawn(move || {
                core.run(&rx);
                core.close()
            })
            .expect("spawn the buffer writer thread");
        Store { tx, drops, thread }
    }

    /// Whether the writer thread still runs: once it ended (a panic), nothing
    /// more reaches the disk, reports included.
    pub fn is_running(&self) -> bool {
        !self.thread.is_finished()
    }

    pub fn sink(&self) -> Sink {
        Sink { tx: self.tx.clone(), drops: self.drops.clone() }
    }

    /// Events the buffer could not take (`loss.buffer_backlog_drops`): the
    /// channel or the writer's backlog was full.
    pub fn drops(&self) -> Arc<AtomicU64> {
        self.drops.clone()
    }

    /// Where the health thread sends its reports. It may block while the
    /// writer is stuck on the disk; the health thread can wait.
    pub fn health(&self) -> SyncSender<Record> {
        self.tx.clone()
    }

    /// The clean stop: once every other sender is gone (the sink's with the
    /// pipeline thread, the health thread's), writes and flushes what is left.
    pub fn close(self) -> io::Result<Stats> {
        drop(self.tx);
        self.thread.join().unwrap_or_else(|p| {
            Err(io::Error::other(format!("the buffer writer thread panicked: {}", crate::panic_message(p.as_ref()))))
        })
    }
}

/// What the thread does with the writer. A trait, so tests can fail the disk.
pub(crate) trait Log {
    fn append(&mut self, payload: &[u8]) -> Result<(), Dropped>;
    fn tick(&mut self, now: Instant) -> io::Result<()>;
    fn stats(&self) -> &Stats;
    fn take_gaps(&mut self) -> Vec<Gap>;
    fn is_failing(&self) -> bool;
    fn disk_bytes(&self) -> u64;
    /// The final counts.
    fn close(self) -> io::Result<Stats>;
}

impl Log for Writer {
    fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
        Writer::append(self, payload)
    }
    fn tick(&mut self, now: Instant) -> io::Result<()> {
        Writer::tick(self, now)
    }
    fn stats(&self) -> &Stats {
        Writer::stats(self)
    }
    fn take_gaps(&mut self) -> Vec<Gap> {
        Writer::take_gaps(self)
    }
    fn is_failing(&self) -> bool {
        Writer::is_failing(self)
    }
    fn disk_bytes(&self) -> u64 {
        Writer::disk_bytes(self)
    }
    fn close(self) -> io::Result<Stats> {
        Writer::close(self)
    }
}

pub(crate) struct Core<W: Log> {
    writer: W,
    drops: Arc<AtomicU64>,
    /// Reported once, in the first report.
    recovery: Option<Recovery>,
    /// The writer's counters at the last report.
    reported: Stats,
    /// `drops` at the last report.
    reported_drops: u64,
    held: VecDeque<Vec<u8>>,
    last_tick: Option<Instant>,
    /// Whether the last tick failed: the log says so once per failure.
    failing: bool,
}

impl<W: Log> Core<W> {
    pub(crate) fn new(writer: W, drops: Arc<AtomicU64>, recovery: Recovery) -> Self {
        Core {
            writer,
            drops,
            recovery: Some(recovery),
            reported: Stats::default(),
            reported_drops: 0,
            held: VecDeque::new(),
            last_tick: None,
            failing: false,
        }
    }

    /// Until every sender is gone.
    pub(crate) fn run(&mut self, rx: &Receiver<Record>) {
        loop {
            match rx.recv_timeout(TICK_EVERY) {
                Ok(r) => {
                    self.handle(r);
                    // Take what is queued, but tick on time.
                    let since = Instant::now();
                    while since.elapsed() < TICK_EVERY {
                        match rx.try_recv() {
                            Ok(r) => self.handle(r),
                            Err(_) => break,
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick(Instant::now());
        }
    }

    pub(crate) fn handle(&mut self, r: Record) {
        self.retry_held();
        match r {
            Record::Event(e) => {
                if let Err(Dropped::BacklogFull) = self.writer.append(&encode_event(e)) {
                    self.drops.fetch_add(1, Ordering::Relaxed);
                }
                // `Rejected` is counted by the writer (`buffer.rejected`).
            }
            Record::Health(e) => {
                let bytes = encode_event(self.complete(e));
                if !self.held.is_empty() || self.writer.append(&bytes).is_err() {
                    if self.held.len() == HELD_CAP {
                        self.held.pop_front();
                    }
                    self.held.push_back(bytes);
                }
            }
        }
    }

    fn retry_held(&mut self) {
        while let Some(bytes) = self.held.front() {
            if self.writer.append(bytes).is_err() {
                return;
            }
            self.held.pop_front();
        }
    }

    pub(crate) fn tick(&mut self, now: Instant) {
        self.retry_held();
        let result = self.writer.tick(now);
        self.last_tick = Some(now);
        match result {
            Err(e) if !self.failing => {
                self.failing = true;
                tracing::warn!("buffer: writes are failing, retrying every 10 s: {e}");
            }
            Ok(()) if self.failing && !self.writer.is_failing() => {
                self.failing = false;
                tracing::info!("buffer: writes recovered");
            }
            _ => {}
        }
    }

    /// Fills in the buffer's figures since the last report.
    fn complete(&mut self, mut e: Event) -> Event {
        let EventKind::SensorHealth(a) = &mut e.kind else { return e };
        let SensorHealthAction::Report(r) = &mut a.action;
        let now = self.writer.stats().clone();
        let (s0, s1) = (&self.reported, &now);
        let d = |f: fn(&Stats) -> u64| Some(f(s1).saturating_sub(f(s0)));
        let b = &mut r.buffer;
        b.write_errors = d(|s| s.write_errors);
        b.recoveries = d(|s| s.recoveries);
        b.rejected = d(|s| s.rejected);
        b.delete_failures = d(|s| s.delete_failures);
        b.failing = Some(self.writer.is_failing());
        b.disk_bytes = Some(self.writer.disk_bytes());
        r.housekeeping.retention_evictions = d(|s| s.retention_evictions);
        // Replaces the health thread's figure, read before the events queued
        // ahead of this report were appended.
        let drops = self.drops.load(Ordering::Relaxed);
        r.loss.buffer_backlog_drops = Some(drops.saturating_sub(self.reported_drops));
        self.reported_drops = drops;
        if let Some(rec) = self.recovery.take() {
            b.truncated_bytes = Some(rec.truncated_bytes);
            b.cursor_reset = Some(rec.cursor_reset);
            b.foreign_segments = Some(u64::from(rec.foreign_segment.is_some()));
        }
        r.gap = merge(self.writer.take_gaps());
        self.reported = now;
        e
    }

    /// The clean stop: what is held, then what is queued, is written; the
    /// counts are taken after that last write.
    pub(crate) fn close(mut self) -> io::Result<Stats> {
        self.retry_held();
        self.writer.close()
    }
}

/// A report carries one gap: the sum of the counts, the widest time range.
fn merge(gaps: Vec<Gap>) -> Option<SensorGap> {
    gaps.into_iter().map(|g| SensorGap { first_time: g.first_time, last_time: g.last_time, events: g.records }).reduce(
        |a, b| SensorGap {
            first_time: min_some(a.first_time, b.first_time),
            last_time: a.last_time.max(b.last_time),
            events: a.events + b.events,
        },
    )
}

fn min_some(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Snapshot, event, report};
    use atlas_buffer::{Cursor, Reader};
    use atlas_schema::classes::sensor_health::HealthReport;
    use atlas_schema::{BootId, Device, DeviceUid, EventId};

    fn device() -> Device {
        Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) }
    }

    fn health(time: i64) -> Event {
        let (r, _) = report(&Snapshot::at_start(true), &Snapshot::at_start(true));
        event(EventId::new_v7(), time, device(), 0, r)
    }

    /// Any small valid event: a Sensor Health one, sent as a plain event.
    fn plain(time: i64) -> Event {
        let mut e = health(time);
        let EventKind::SensorHealth(a) = &mut e.kind else { unreachable!() };
        a.interval_start = -1; // marks it as not passed through `complete`
        e
    }

    fn the_report(e: &Event) -> &HealthReport {
        let EventKind::SensorHealth(a) = &e.kind else { panic!("not Sensor Health") };
        let SensorHealthAction::Report(r) = &a.action;
        r
    }

    fn read_all(dir: &std::path::Path) -> Vec<Event> {
        let mut r = Reader::open(dir, Cursor::default());
        let mut out = Vec::new();
        while let Some(rec) = r.next_record().unwrap() {
            out.push(decode_event(&rec.payload).unwrap());
        }
        out
    }

    fn tempdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("atlas-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn events_and_reports_reach_the_disk_in_order_with_the_buffers_figures() {
        let dir = tempdir("order");
        let buf = dir.join("buffer");
        let (store, recovery) = Store::start(StoreConfig::new(atlas_buffer::Config::new(&buf))).unwrap();
        assert_eq!(recovery, Recovery::default());
        let sink = store.sink();
        for t in 0..1_000 {
            sink.event(plain(t));
        }
        store.health().send(Record::Health(health(5_000))).unwrap();
        sink.event(plain(5_001));
        drop(sink);
        let stats = store.close().unwrap();
        assert_eq!(stats.records_written, 1_002);

        let got = read_all(&buf);
        assert_eq!(got.len(), 1_002);
        assert!(got.iter().map(|e| e.meta.time).is_sorted());
        let r = the_report(&got[1_000]);
        assert_eq!(r.buffer.failing, Some(false));
        assert_eq!(r.buffer.write_errors, Some(0));
        assert_eq!(r.buffer.truncated_bytes, Some(0), "the first report carries the recovery");
        assert_eq!(r.buffer.cursor_reset, Some(false));
        assert!(r.buffer.disk_bytes.is_some());
        assert_eq!(r.housekeeping.retention_evictions, Some(0));
        assert_eq!(r.gap, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The counts `close` returns include what its own last write wrote
    /// (found by the binary test: `records=` fell short of the disk).
    #[test]
    fn close_counts_what_its_last_write_wrote() {
        let dir = tempdir("close-counts");
        let buf = dir.join("buffer");
        let (store, _) = Store::start(StoreConfig::new(atlas_buffer::Config::new(&buf))).unwrap();
        let sink = store.sink();
        sink.event(plain(0)); // written by the first tick, which is always due
        std::thread::sleep(Duration::from_millis(300));
        for t in 1..=10 {
            sink.event(plain(t)); // the next flush is a second away: left to close
        }
        drop(sink);
        let stats = store.close().unwrap();
        assert_eq!(read_all(&buf).len(), 11);
        assert_eq!(stats.records_written, 11);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_first_report_carries_the_recovery() {
        let dir = tempdir("recovery");
        let buf = dir.join("buffer");
        let (store, _) = Store::start(StoreConfig::new(atlas_buffer::Config::new(&buf))).unwrap();
        store.health().send(Record::Health(health(1))).unwrap();
        store.health().send(Record::Health(health(2))).unwrap();
        store.close().unwrap();
        let got = read_all(&buf);
        assert_eq!(the_report(&got[0]).buffer.cursor_reset, Some(false));
        assert_eq!(the_report(&got[1]).buffer.cursor_reset, None);
        assert_eq!(the_report(&got[1]).buffer.truncated_bytes, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_channel_drops_and_counts_never_blocks() {
        let (tx, rx) = sync_channel(2);
        let drops = Arc::new(AtomicU64::new(0));
        let sink = Sink { tx, drops: drops.clone() };
        // On its own thread: a send that blocked would fail the test, not hang
        // it (review finding 8); dropping `rx` at the end frees such a thread.
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for t in 0..10 {
                sink.event(plain(t));
            }
            let _ = done_tx.send(());
        });
        assert!(done.recv_timeout(Duration::from_secs(5)).is_ok(), "the sink blocked on a full channel");
        assert_eq!(drops.load(Ordering::Relaxed), 8);
        drop(rx);
    }

    /// A disk that refuses everything, with a backlog that holds `room` records.
    #[derive(Default)]
    struct FullDisk {
        room: usize,
        appended: Vec<Vec<u8>>,
        stats: Stats,
        gaps: Vec<Gap>,
    }

    impl Log for FullDisk {
        fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
            if self.appended.len() >= self.room {
                self.stats.backlog_drops += 1;
                return Err(Dropped::BacklogFull);
            }
            self.appended.push(payload.to_vec());
            Ok(())
        }
        fn tick(&mut self, _: Instant) -> io::Result<()> {
            self.stats.write_errors += 1;
            Err(io::Error::other("disk full"))
        }
        fn stats(&self) -> &Stats {
            &self.stats
        }
        fn take_gaps(&mut self) -> Vec<Gap> {
            std::mem::take(&mut self.gaps)
        }
        fn is_failing(&self) -> bool {
            true
        }
        fn disk_bytes(&self) -> u64 {
            0
        }
        fn close(self) -> io::Result<Stats> {
            Ok(self.stats)
        }
    }

    #[test]
    fn a_report_the_backlog_cannot_take_is_held_and_written_first() {
        let drops = Arc::new(AtomicU64::new(0));
        let mut core = Core::new(FullDisk { room: 1, ..FullDisk::default() }, drops.clone(), Recovery::default());
        core.handle(Record::Event(plain(1)));
        core.handle(Record::Event(plain(2))); // the backlog is full
        core.handle(Record::Health(health(3)));
        core.tick(Instant::now());
        assert_eq!(drops.load(Ordering::Relaxed), 1, "the event is counted as lost");
        assert_eq!(core.held.len(), 1, "the report is held");

        // Room again: the held report goes before the next event.
        core.writer.room = 3;
        core.handle(Record::Event(plain(4)));
        let times: Vec<i64> = core.writer.appended.iter().map(|b| decode_event(b).unwrap().meta.time).collect();
        assert_eq!(times, [1, 3, 4]);
        let held = decode_event(&core.writer.appended[1]).unwrap();
        assert_eq!(the_report(&held).buffer.write_errors, Some(0), "figures as of when it was made");
        assert_eq!(the_report(&held).buffer.failing, Some(true));
    }

    /// Each report counts the writer's figures since the report before it.
    #[test]
    fn each_report_counts_since_the_last_one() {
        let mut core =
            Core::new(FullDisk { room: 10, ..FullDisk::default() }, Arc::new(AtomicU64::new(0)), Recovery::default());
        core.tick(Instant::now());
        core.tick(Instant::now());
        core.handle(Record::Health(health(1)));
        core.tick(Instant::now());
        core.handle(Record::Health(health(2)));
        let reports: Vec<Event> = core.writer.appended.iter().map(|b| decode_event(b).unwrap()).collect();
        assert_eq!(the_report(&reports[0]).buffer.write_errors, Some(2));
        assert_eq!(the_report(&reports[1]).buffer.write_errors, Some(1));
    }

    /// A report counts the drops up to it: the writer's, of the events queued
    /// ahead of it, and the sink's (review finding 3: the final report missed
    /// those made while the writer drained at stop).
    #[test]
    fn a_report_counts_the_backlog_drops_of_everything_queued_before_it() {
        let drops = Arc::new(AtomicU64::new(0));
        let mut core = Core::new(FullDisk { room: 1, ..FullDisk::default() }, drops.clone(), Recovery::default());
        core.handle(Record::Event(plain(1)));
        core.handle(Record::Event(plain(2))); // the backlog is full: dropped
        drops.fetch_add(2, Ordering::Relaxed); // two the sink could not queue
        core.handle(Record::Health(health(3))); // made before any drop: says 0
        core.writer.room = 10;
        core.handle(Record::Event(plain(4))); // writes the held report first
        core.handle(Record::Health(health(5)));
        let reports: Vec<Event> = core
            .writer
            .appended
            .iter()
            .map(|b| decode_event(b).unwrap())
            .filter(|e| matches!(&e.kind, EventKind::SensorHealth(a) if a.interval_start == 0))
            .collect();
        assert_eq!(reports.len(), 2);
        assert_eq!(the_report(&reports[0]).loss.buffer_backlog_drops, Some(3));
        assert_eq!(the_report(&reports[1]).loss.buffer_backlog_drops, Some(0), "since the last report");
    }

    /// Tests: a writer that panics once it has taken `left` records.
    pub(super) struct PanicsAfter<W> {
        pub(super) inner: W,
        pub(super) left: u64,
    }

    impl<W: Log> Log for PanicsAfter<W> {
        fn append(&mut self, payload: &[u8]) -> Result<(), Dropped> {
            assert!(self.left > 0, "test: the buffer writer fails");
            self.left -= 1;
            self.inner.append(payload)
        }
        fn tick(&mut self, now: Instant) -> io::Result<()> {
            self.inner.tick(now)
        }
        fn stats(&self) -> &Stats {
            self.inner.stats()
        }
        fn take_gaps(&mut self) -> Vec<Gap> {
            self.inner.take_gaps()
        }
        fn is_failing(&self) -> bool {
            self.inner.is_failing()
        }
        fn disk_bytes(&self) -> u64 {
            self.inner.disk_bytes()
        }
        fn close(self) -> io::Result<Stats> {
            self.inner.close()
        }
    }

    /// Review finding 7: a writer thread that dies is seen, and `close` says
    /// why instead of panicking in turn.
    #[test]
    fn a_writer_that_dies_is_seen_and_close_says_why() {
        let writer = PanicsAfter { inner: FullDisk { room: 10, ..FullDisk::default() }, left: 1 };
        let store = Store::spawn(writer, Recovery::default(), 16);
        let sink = store.sink();
        sink.event(plain(1));
        sink.event(plain(2)); // the writer panics on it
        let t0 = Instant::now();
        while store.is_running() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!store.is_running(), "the writer's end is seen");
        drop(sink);
        let e = store.close().unwrap_err();
        assert!(e.to_string().contains("test: the buffer writer fails"), "{e}");
    }

    #[test]
    fn held_reports_are_bounded() {
        let mut core = Core::new(FullDisk::default(), Arc::new(AtomicU64::new(0)), Recovery::default());
        for t in 0..(HELD_CAP as i64 + 4) {
            core.handle(Record::Health(health(t)));
        }
        assert_eq!(core.held.len(), HELD_CAP);
        assert_eq!(decode_event(&core.held[0]).unwrap().meta.time, 4, "the oldest went");
    }

    #[test]
    fn gaps_are_merged_into_one() {
        let gaps = vec![
            Gap { records: 3, first_time: Some(50), last_time: Some(60) },
            Gap { records: 2, first_time: None, last_time: None },
            Gap { records: 1, first_time: Some(10), last_time: Some(20) },
        ];
        assert_eq!(merge(gaps), Some(SensorGap { first_time: Some(10), last_time: Some(60), events: 6 }));
        assert_eq!(merge(Vec::new()), None);
        let mut core = Core::new(FullDisk::default(), Arc::new(AtomicU64::new(0)), Recovery::default());
        core.writer.gaps = vec![Gap { records: 4, first_time: Some(1), last_time: Some(2) }];
        let e = core.complete(health(9));
        assert_eq!(the_report(&e).gap, Some(SensorGap { first_time: Some(1), last_time: Some(2), events: 4 }));
    }
}
```

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -18,7 +18,16 @@ pub mod process;
 pub mod recent;
 pub mod services;
 pub mod settings;
+pub mod store;
 pub mod time;
 pub mod watchlist;
 #[cfg(windows)]
 pub mod win;
+
+/// The message of a panic that ended a thread, from its join.
+pub(crate) fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
+    p.downcast_ref::<&str>()
+        .map(|s| (*s).to_string())
+        .or_else(|| p.downcast_ref::<String>().cloned())
+        .unwrap_or_else(|| "a panic without a message".into())
+}
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent store
cargo test -p atlas-buffer
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 387 passed, 7 ignored (`atlas-agent` unit tests: 202).
```powershell
git add crates/atlas-agent crates/atlas-buffer
git commit -m "feat(agent): the buffer writer thread and the pipeline's sink"
```

### Task 6: `dump`

**Files:**
- Modify: `crates/atlas-agent/src/lib.rs`, `crates/atlas-agent/tests/replay.rs`
- Create: `crates/atlas-agent/src/dump.rs`

- [ ] **Step 1: The reader and the rendering** (D3).
  - `render(bytes)` decodes a record with `prost-reflect` against `atlas_proto::FILE_DESCRIPTOR_SET` and prints protobuf's standard JSON mapping on one line. The descriptor is built once.
  - `dump(dir, options, out, stop)` reads from `--from` (a `segment:offset` cursor, `0:0` by default), validates every record with 0a's `decode_event` and skips an invalid one, counting it. `--class` keeps one class. `--follow` polls every 250 ms for new records until `stop` says so. It returns what it printed and skipped, the corrupt segments, and the cursor to continue from.

`crates/atlas-agent/src/dump.rs`:
```rust
//! `atlas-agent dump` (sensor spec §11.3; plan 1b-4a, D3): the buffer as JSON
//! lines, one event per line, in protobuf's standard JSON mapping (the same
//! rendering as the replay snapshot). Read-only: it never acks.
//!
//! Every record is validated first (0a's `decode_event`); an invalid one is
//! skipped and counted, never printed (§8.2). The reader follows the §8.5
//! rules, so `dump --follow` can run next to the agent.

use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use atlas_buffer::{Cursor, Reader};
use atlas_schema::{EventKind, decode_event};
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor};

/// The classes `--class` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassName {
    Process,
    Module,
    Network,
    File,
    RegistryKey,
    RegistryValue,
    Dns,
    EventLog,
    SensorHealth,
}

impl ClassName {
    pub const ALL: [(&'static str, ClassName); 9] = [
        ("process", ClassName::Process),
        ("module", ClassName::Module),
        ("network", ClassName::Network),
        ("file", ClassName::File),
        ("registry_key", ClassName::RegistryKey),
        ("registry_value", ClassName::RegistryValue),
        ("dns", ClassName::Dns),
        ("event_log", ClassName::EventLog),
        ("sensor_health", ClassName::SensorHealth),
    ];

    pub fn of(kind: &EventKind) -> ClassName {
        match kind {
            EventKind::Process(_) => ClassName::Process,
            EventKind::Module(_) => ClassName::Module,
            EventKind::Network(_) => ClassName::Network,
            EventKind::File(_) => ClassName::File,
            EventKind::RegistryKey(_) => ClassName::RegistryKey,
            EventKind::RegistryValue(_) => ClassName::RegistryValue,
            EventKind::Dns(_) => ClassName::Dns,
            EventKind::EventLog(_) => ClassName::EventLog,
            EventKind::SensorHealth(_) => ClassName::SensorHealth,
        }
    }
}

impl FromStr for ClassName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        ClassName::ALL.iter().find(|(n, _)| *n == s).map(|(_, c)| *c).ok_or_else(|| {
            let names: Vec<_> = ClassName::ALL.iter().map(|(n, _)| *n).collect();
            format!("unknown class {s:?}; one of {}", names.join(", "))
        })
    }
}

/// `--from <segment:offset>`, as `Record::next` cursors print.
pub fn parse_cursor(s: &str) -> Result<Cursor, String> {
    let (seg, off) = s.split_once(':').ok_or_else(|| format!("{s:?} is not <segment:offset>"))?;
    let n = |v: &str| v.parse::<u64>().map_err(|_| format!("{s:?} is not <segment:offset>"));
    Ok(Cursor { segment: n(seg)?, offset: n(off)? })
}

fn event_descriptor() -> &'static MessageDescriptor {
    static DESC: OnceLock<MessageDescriptor> = OnceLock::new();
    DESC.get_or_init(|| {
        let pool = DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET).expect("the built-in descriptor set");
        pool.get_message_by_name("atlas.events.v1.Event").expect("atlas.events.v1.Event")
    })
}

/// One encoded event as a line of protobuf JSON. The caller has validated it.
pub fn render(bytes: &[u8]) -> Result<String, String> {
    let msg = DynamicMessage::decode(event_descriptor().clone(), bytes).map_err(|e| e.to_string())?;
    serde_json::to_string(&msg).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Default)]
pub struct DumpOptions {
    pub from: Cursor,
    pub class: Option<ClassName>,
    pub follow: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DumpStats {
    pub printed: u64,
    /// Records that failed validation (`quality.buffer_invalid_records`).
    pub invalid: u64,
    /// Sealed segments whose remainder was skipped as corrupt.
    pub corrupt_segments: u64,
    /// Where a later `--from` continues.
    pub next: Cursor,
}

/// How long `--follow` waits when it has caught up.
const FOLLOW_POLL: Duration = Duration::from_millis(250);

/// Prints the buffer in `dir` to `out`. With `follow`, waits for new records
/// until `stop()` is true.
pub fn dump(
    dir: &Path,
    opts: &DumpOptions,
    out: &mut impl Write,
    mut stop: impl FnMut() -> bool,
) -> io::Result<DumpStats> {
    let mut reader = Reader::open(dir, opts.from);
    let mut stats = DumpStats { next: opts.from, ..DumpStats::default() };
    loop {
        match reader.next_record()? {
            Some(rec) => {
                stats.next = rec.next;
                let Ok(event) = decode_event(&rec.payload) else {
                    stats.invalid += 1;
                    continue;
                };
                if opts.class.is_some_and(|c| c != ClassName::of(&event.kind)) {
                    continue;
                }
                match render(&rec.payload) {
                    Ok(line) => {
                        writeln!(out, "{line}")?;
                        stats.printed += 1;
                    }
                    Err(_) => stats.invalid += 1,
                }
            }
            None if opts.follow && !stop() => {
                out.flush()?;
                std::thread::sleep(FOLLOW_POLL);
            }
            None => break,
        }
    }
    stats.corrupt_segments = reader.stats().corrupt_segments;
    out.flush()?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{Snapshot, event, report};
    use atlas_buffer::Writer;
    use atlas_schema::{BootId, Device, DeviceUid, Event, EventId, encode_event};
    use std::time::Instant;

    fn health(time: i64) -> Event {
        let (r, _) = report(&Snapshot::at_start(true), &Snapshot::at_start(true));
        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
        event(EventId::new_v7(), time, device, 0, r)
    }

    fn buffer(name: &str, records: &[Vec<u8>]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("atlas-dump-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut w, _) = Writer::open(atlas_buffer::Config::new(&dir), |_| None).unwrap();
        for r in records {
            w.append(r).unwrap();
        }
        w.tick(Instant::now()).unwrap();
        w.close().unwrap();
        dir
    }

    #[test]
    fn prints_protobuf_json_lines_and_skips_invalid_records() {
        let dir = buffer("lines", &[encode_event(health(1)), b"not an event".to_vec(), encode_event(health(2))]);
        let mut out = Vec::new();
        let stats = dump(&dir, &DumpOptions::default(), &mut out, || true).unwrap();
        assert_eq!((stats.printed, stats.invalid), (2, 1));
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["time"], "1", "int64 as a string, protobuf's mapping");
        assert_eq!(lines[1]["sensor"], "SENSOR_ETW");
        assert!(lines[0]["sensorHealth"]["report"].is_object());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_rendering_is_the_replay_snapshots() {
        let e = health(7);
        let pool = DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET).unwrap();
        let desc = pool.get_message_by_name("atlas.events.v1.Event").unwrap();
        let msg = DynamicMessage::decode(desc, encode_event(e.clone()).as_slice()).unwrap();
        assert_eq!(render(&encode_event(e)).unwrap(), serde_json::to_string(&msg).unwrap());
    }

    #[test]
    fn class_and_from_select_records() {
        let dir = buffer("select", &[encode_event(health(1)), encode_event(health(2))]);
        let mut out = Vec::new();
        let opts = DumpOptions { class: Some(ClassName::File), ..DumpOptions::default() };
        let all = dump(&dir, &opts, &mut out, || true).unwrap();
        assert_eq!(all.printed, 0);

        // Continue after the first record: from where it ends.
        let mut r = Reader::open(&dir, Cursor::default());
        let first = r.next_record().unwrap().unwrap();
        let opts = DumpOptions { from: first.next, class: Some(ClassName::SensorHealth), ..DumpOptions::default() };
        let mut out = Vec::new();
        let rest = dump(&dir, &opts, &mut out, || true).unwrap();
        assert_eq!(rest.printed, 1);
        assert!(String::from_utf8(out).unwrap().contains("\"time\":\"2\""));
        assert_eq!(rest.next, all.next);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn follow_waits_for_new_records_until_stopped() {
        let dir = buffer("follow", &[encode_event(health(1))]);
        let (mut w, _) = Writer::open(atlas_buffer::Config::new(&dir), |_| None).unwrap();
        let polls = std::cell::Cell::new(0);
        let mut out = Vec::new();
        let opts = DumpOptions { follow: true, ..DumpOptions::default() };
        let stats = dump(&dir, &opts, &mut out, || {
            polls.set(polls.get() + 1);
            if polls.get() == 1 {
                w.append(&encode_event(health(2))).unwrap();
                w.tick(Instant::now()).unwrap();
            }
            polls.get() > 2
        })
        .unwrap();
        assert_eq!(stats.printed, 2);
        w.close().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn options_parse() {
        assert_eq!(parse_cursor("3:128"), Ok(Cursor { segment: 3, offset: 128 }));
        assert!(parse_cursor("3").is_err() && parse_cursor("a:1").is_err() && parse_cursor("1:-2").is_err());
        assert_eq!("registry_value".parse(), Ok(ClassName::RegistryValue));
        let e = "registry".parse::<ClassName>().unwrap_err();
        assert!(e.contains("registry_key"), "{e}");
    }
}
```

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -5,6 +5,7 @@ pub mod completion;
 pub mod config;
 pub mod counters;
 pub mod driver;
+pub mod dump;
 pub mod evict;
 pub mod fakes;
 pub mod health;
```

- [ ] **Step 2: The replay snapshot renders through `dump::render`,** so the snapshot and `dump` cannot drift apart. The snapshot does not change.

```diff
--- a/crates/atlas-agent/tests/replay.rs
+++ b/crates/atlas-agent/tests/replay.rs
@@ -32,7 +32,6 @@ use atlas_schema::classes::network::{NetworkAction, NetworkProtocol};
 use atlas_schema::classes::process::ProcessActivity;
 use atlas_schema::classes::registry::{RegistryKeyAction, RegistryValueAction};
 use atlas_schema::{BootId, DeviceUid, Event, EventKind, Hashes, decode_event, encode_event};
-use prost_reflect::{DescriptorPool, DynamicMessage};
 use serde_json::Value;
 
 /// The runner's QPC frequency (10 MHz on every current Windows).
@@ -233,11 +232,9 @@ fn answer(r: &Request) -> Option<Reply> {
     }
 }
 
+/// `dump`'s rendering (plan 1b-4a, D3).
 fn snapshot_line(e: &Event) -> String {
-    let pool = DescriptorPool::decode(atlas_proto::FILE_DESCRIPTOR_SET).unwrap();
-    let desc = pool.get_message_by_name("atlas.events.v1.Event").unwrap();
-    let msg = DynamicMessage::decode(desc, encode_event(e.clone()).as_slice()).unwrap();
-    serde_json::to_string(&msg).unwrap()
+    atlas_agent::dump::render(&encode_event(e.clone())).unwrap()
 }
 
 #[test]
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent dump
cargo test -p atlas-agent --test replay
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass, the snapshot unchanged. Workspace: 392 passed, 7 ignored (`atlas-agent` unit tests: 207).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): dump: the buffer as protobuf JSON lines"
```

### Task 7: The data directory

**Files:**
- Modify: `crates/atlas-agent/src/win/mod.rs`, `crates/atlas-agent/src/win/util.rs`
- Create: `crates/atlas-agent/src/win/datadir.rs`

- [ ] **Step 1: Create or verify** (D2, §11.1).
  - `prepare(dir)` creates each of the directory, `buffer\` and `logs\` with `SDDL` (owner Administrators; a protected DACL, full control for SYSTEM and Administrators, inherited by what is created inside), or verifies it if it exists. A directory created a moment ago by someone else is verified too.
  - `verify(dir)` opens one handle with `READ_CONTROL` and `FILE_FLAG_OPEN_REPARSE_POINT` (never following a junction or link) and refuses a reparse point or a non-directory. It reads the owner and DACL through that same handle (`GetSecurityInfo`), so the directory cannot change between the checks.
  - **The folders above** (review finding 4). Before that, `prepare` checks every folder above the directory, from the volume root down, through the same kind of handle: a real directory, owned by SYSTEM, Administrators or TrustedInstaller, with no allow entry that applies to it giving anyone else a `SWAP_RIGHTS` right (`DELETE`, `WRITE_DAC`, `WRITE_OWNER`, `FILE_DELETE_CHILD`, `GENERIC_ALL`); inherit-only entries are skipped, deny entries accepted. The path must be absolute, without `..` (`Path` drops a `.` inside a path, as Windows does). Otherwise a user who could rename a folder above could swap in a directory of their own after the checks. `C:\` and `C:\ProgramData` pass; a user's profile or `%TEMP%` does not.
  - `SecurityDescriptor` (a descriptor freed with `LocalFree`) moves to `win::util`; Task 8's lock uses it too.
  - `check_descriptor` refuses: an owner other than Administrators or SYSTEM; a null DACL; a DACL without `SE_DACL_PROTECTED`; an allow entry for any other trustee, inherit-only ones included (files inside would inherit them); an entry of a type it does not know. Deny entries are accepted: they only take access away.
  - A refusal names the directory and the reason.
- [ ] **Step 2: The tests.**
  - Unelevated: the descriptor checks on SDDL cases, for the directory (an inherit-only allow for Users, a conditional and an object entry refused) and for the folders above (`C:\` and `C:\ProgramData` as Windows sets them accepted; each swapping right, a foreign owner, a null DACL, a conditional and an object entry refused); this machine's real folders above `C:\ProgramData\Atlas` pass; a directory below `%TEMP%` is refused at the user's folder and not created; a relative path or one with `..` is refused; a folder that inherits `%TEMP%`'s ACL is refused (its read-back shows inherited entries); a junction is refused without following it; a file is refused.
  - Elevated (`#[ignore]`; CI's `agent-live` and Task 10 run them), under an Administrators-only folder `C:\ProgramData\Atlas-Test-datadir-*` deleted on drop: `prepare` makes a directory it then accepts, and a file created inside inherits only SYSTEM and Administrators; directories made loose the way Windows makes them are refused: one granting Users, a null DACL, one owned by Users (set with `SeRestorePrivilege`), one that inherits (a plain create, its owner then set to Administrators: F2), while one with only a deny entry is accepted; and below real folders above, one owned by Users, one giving Users `DELETE`, `FILE_DELETE_CHILD` or `WRITE_DAC`, and a junction are refused with nothing created, while an inherit-only full-control entry for Users is accepted.

`crates/atlas-agent/src/win/datadir.rs`:
```rust
//! The data directory (sensor spec §11.1; plan 1b-4a, D2).
//!
//! `C:\ProgramData` lets any user create folders, and grants `BUILTIN\Users`
//! write access to subfolders that inherit its ACL. The agent deletes and
//! renames files in its directory as an administrator or SYSTEM, so a folder
//! a user created or can write into would let them aim those operations
//! elsewhere. Therefore the directory, and `buffer\` and `logs\` inside it:
//! - are created with owner Administrators and a **protected** DACL granting
//!   full control to SYSTEM and Administrators only ([`SDDL`]);
//! - when they exist, are verified through one handle opened without following
//!   a reparse point: a real directory, owned by Administrators or SYSTEM, a
//!   protected DACL (not a null one), and no access granted to anyone else.
//!
//! A user who could rename a folder above the directory could put one of
//! their own in its place after these checks (and with it an `agent.toml`
//! that turns the sensor off). So before it, every folder above it, from the
//! volume root down, is checked the same way (without following a reparse
//! point): a real directory, owned by SYSTEM, Administrators or
//! TrustedInstaller, with a DACL that grants no one else a right to rename,
//! delete or re-permission it, or to rename what it contains ([`SWAP_RIGHTS`];
//! entries that only pass to children are skipped). `C:\` and
//! `C:\ProgramData` pass; a user's profile or `%TEMP%` does not. The path must
//! be absolute, without `.` or `..`. On any failure the agent refuses to start.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce,
    GetAclInformation, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING, READ_CONTROL,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

use super::util::{Owned, SecurityDescriptor, wide};

/// Owner Administrators; a protected, auto-inherited DACL; full control to
/// SYSTEM and Administrators, inherited by files and folders inside.
pub const SDDL: &str = "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
/// The subdirectories the agent writes into.
pub const SUBDIRS: [&str; 2] = ["buffer", "logs"];
/// The rights on a folder above the directory that would let their holder
/// swap what the agent finds there: rename or delete it (`DELETE`), change
/// its permissions or owner (`WRITE_DAC`, `WRITE_OWNER`), rename or delete
/// what it contains (`FILE_DELETE_CHILD`), or all of these (`GENERIC_ALL`).
pub const SWAP_RIGHTS: u32 = 0x0001_0000 | 0x0004_0000 | 0x0008_0000 | 0x0000_0040 | 0x1000_0000;

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// Owns the volume root and the system's folders.
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const INHERIT_ONLY_ACE: u8 = 0x08;

/// Why the directory was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for DataDirError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "data directory {}: {}", self.path.display(), self.reason)
    }
}

impl std::error::Error for DataDirError {}

fn refuse(path: &Path, reason: impl Into<String>) -> DataDirError {
    DataDirError { path: path.to_owned(), reason: reason.into() }
}

/// Checks the folders above the directory, then creates the directory and its
/// subdirectories as needed, and verifies each. Its parent must exist.
pub fn prepare(dir: &Path) -> Result<(), DataDirError> {
    check_ancestors(dir)?;
    create_or_verify(dir)?;
    for sub in SUBDIRS {
        create_or_verify(&dir.join(sub))?;
    }
    Ok(())
}

/// Every folder above `dir`, from the volume root down (see the module docs).
fn check_ancestors(dir: &Path) -> Result<(), DataDirError> {
    if !dir.is_absolute() || dir.components().any(|c| matches!(c, Component::CurDir | Component::ParentDir)) {
        return Err(refuse(dir, "must be an absolute path without . or .."));
    }
    let mut above: Vec<&Path> = dir.ancestors().skip(1).collect();
    above.reverse();
    for a in above {
        descriptor(a)
            .and_then(|sd| check_ancestor_descriptor(&sd))
            .map_err(|e| refuse(dir, format!("{}, a folder above it, {e}", a.display())))?;
    }
    Ok(())
}

fn create_or_verify(dir: &Path) -> Result<(), DataDirError> {
    let sd = SecurityDescriptor::from_sddl(SDDL).map_err(|e| refuse(dir, e))?;
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.0,
        bInheritHandle: false.into(),
    };
    let path = wide(&dir.to_string_lossy());
    // SAFETY: `path` is NUL-terminated; `attrs` and the descriptor it points to
    // outlive the call.
    match unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&attrs)) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_ALREADY_EXISTS.to_hresult() => {}
        Err(e) => return Err(refuse(dir, format!("cannot be created: {e}"))),
    }
    verify(dir)
}

/// Checks one directory (see the module docs).
pub fn verify(dir: &Path) -> Result<(), DataDirError> {
    descriptor(dir).and_then(|sd| check_descriptor(&sd)).map_err(|e| refuse(dir, e))
}

/// A directory's owner and DACL, read through one handle opened without
/// following a reparse point; a reparse point or a file is refused.
fn descriptor(dir: &Path) -> Result<SecurityDescriptor, String> {
    let path = wide(&dir.to_string_lossy());
    // SAFETY: `path` is NUL-terminated. The handle reads attributes and the
    // security descriptor only, and is closed by `Owned`.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            (READ_CONTROL | FILE_READ_ATTRIBUTES).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .map_err(|e| format!("cannot be opened: {e}"))?;
    let handle = Owned(handle);

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: a valid handle and an out-structure.
    unsafe { GetFileInformationByHandle(handle.raw(), &mut info) }
        .map_err(|e| format!("attributes cannot be read: {e}"))?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err("is a reparse point (a junction or symbolic link)".into());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
        return Err("is not a directory".into());
    }

    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: a valid handle opened with READ_CONTROL; the descriptor is freed
    // by `SecurityDescriptor`'s drop.
    let err = unsafe {
        GetSecurityInfo(
            handle.raw(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut sd),
        )
    };
    if err.is_err() {
        return Err(format!("security cannot be read: {err:?}"));
    }
    Ok(SecurityDescriptor(sd))
}

/// The owner, the protection and the entries of a directory's descriptor.
fn check_descriptor(sd: &SecurityDescriptor) -> Result<(), String> {
    let owner = owner(sd)?;
    if owner != ADMINISTRATORS && owner != SYSTEM {
        return Err(format!("is owned by {owner}, not Administrators or SYSTEM"));
    }
    let (mut control, mut revision) = (0u16, 0u32);
    // SAFETY: `sd` is a valid descriptor.
    unsafe { GetSecurityDescriptorControl(sd.0, &mut control, &mut revision) }.map_err(unreadable)?;
    if control & SE_DACL_PROTECTED.0 == 0 {
        return Err("its DACL is not protected (it inherits from its parent)".into());
    }
    // Only allow entries for SYSTEM and Administrators; deny entries only take
    // access away and are accepted; any other kind is refused.
    for e in entries(sd)? {
        match e.kind {
            ACCESS_DENIED_ACE_TYPE => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                let who = e.sid.unwrap_or_default();
                if who != SYSTEM && who != ADMINISTRATORS {
                    return Err(format!("grants access to {who}"));
                }
            }
            other => return Err(unaccepted(other)),
        }
    }
    Ok(())
}

/// The owner and the entries of a folder above the directory: no one but
/// SYSTEM, Administrators and TrustedInstaller may swap it.
fn check_ancestor_descriptor(sd: &SecurityDescriptor) -> Result<(), String> {
    let trusted = |sid: &str| [SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER].contains(&sid);
    let owner = owner(sd)?;
    if !trusted(&owner) {
        return Err(format!("is owned by {owner}, not SYSTEM, Administrators or TrustedInstaller"));
    }
    for e in entries(sd)? {
        match e.kind {
            ACCESS_DENIED_ACE_TYPE => {}
            // Rights the folder itself does not get are not checked.
            ACCESS_ALLOWED_ACE_TYPE if e.flags & INHERIT_ONLY_ACE != 0 || e.mask & SWAP_RIGHTS == 0 => {}
            ACCESS_ALLOWED_ACE_TYPE => {
                let who = e.sid.unwrap_or_default();
                if !trusted(&who) {
                    return Err(format!("lets {who} rename or re-permission it or what it contains"));
                }
            }
            other => return Err(unaccepted(other)),
        }
    }
    Ok(())
}

fn unreadable(e: windows::core::Error) -> String {
    format!("security cannot be read: {e}")
}

fn unaccepted(kind: u8) -> String {
    format!("has an entry of type {kind}, which the agent does not accept")
}

fn owner(sd: &SecurityDescriptor) -> Result<String, String> {
    let mut owner = PSID::default();
    let mut defaulted = BOOL::default();
    // SAFETY: `sd` is a valid descriptor; `owner` points into it.
    unsafe { GetSecurityDescriptorOwner(sd.0, &mut owner, &mut defaulted) }.map_err(unreadable)?;
    if owner.0.is_null() {
        return Err("has no owner".into());
    }
    sid_string(owner)
}

/// One DACL entry: its type, flags and mask, and for an allow entry its SID.
struct Entry {
    kind: u8,
    flags: u8,
    mask: u32,
    sid: Option<String>,
}

/// The entries of the descriptor's DACL; a missing or null DACL is refused.
fn entries(sd: &SecurityDescriptor) -> Result<Vec<Entry>, String> {
    let (mut present, mut dacl, mut defaulted) = (BOOL::default(), std::ptr::null_mut::<ACL>(), BOOL::default());
    // SAFETY: `sd` is a valid descriptor; `dacl` points into it.
    unsafe { GetSecurityDescriptorDacl(sd.0, &mut present, &mut dacl, &mut defaulted) }.map_err(unreadable)?;
    if !present.as_bool() || dacl.is_null() {
        return Err("has a null DACL (everyone has full access)".into());
    }
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `dacl` points into a live security descriptor; `size` is the
    // structure `AclSizeInformation` asks for.
    unsafe {
        GetAclInformation(dacl, (&raw mut size).cast(), size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation)
    }
    .map_err(|e| format!("its DACL cannot be read: {e}"))?;
    let mut out = Vec::new();
    for i in 0..size.AceCount {
        let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `i` is below the ACL's count; `ace` points into the ACL.
        unsafe { GetAce(dacl, i, &mut ace) }.map_err(|e| format!("its DACL cannot be read: {e}"))?;
        // SAFETY: every ACE starts with an ACE_HEADER.
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        let (mut mask, mut sid) = (0, None);
        if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
            let allowed = ace as *const ACCESS_ALLOWED_ACE;
            // SAFETY: an allowed ACE: its mask follows the header, and its SID
            // starts at `SidStart`, inside the ACE.
            mask = unsafe { (*allowed).Mask };
            sid = Some(sid_string(PSID(unsafe { (&raw const (*allowed).SidStart) } as *mut _))?);
        }
        out.push(Entry { kind: header.AceType, flags: header.AceFlags, mask, sid });
    }
    Ok(out)
}

fn sid_string(sid: PSID) -> Result<String, String> {
    let mut s = PWSTR::null();
    // SAFETY: `sid` is a valid SID; the string is freed below with LocalFree.
    unsafe { ConvertSidToStringSidW(sid, &mut s) }.map_err(|e| format!("a SID cannot be read: {e}"))?;
    // SAFETY: a NUL-terminated string allocated by the call above.
    let out = unsafe { s.to_string() }.map_err(|e| e.to_string());
    // SAFETY: allocated with LocalAlloc by ConvertSidToStringSidW.
    unsafe { LocalFree(Some(HLOCAL(s.0.cast()))) };
    out
}

#[cfg(test)]
mod tests {
    use windows::Win32::Security::Authorization::SDDL_REVISION_1;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("atlas-datadir-{name}-{}", std::process::id()));
        let _ = std::process::Command::new("cmd").args(["/c", "rmdir", "/s", "/q"]).arg(&d).output();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Creates `dir` with `sddl` (needs administrator rights when it names an owner).
    fn create_with(dir: &Path, sddl: &str) {
        let sd = SecurityDescriptor::from_sddl(sddl).unwrap();
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0.0,
            bInheritHandle: false.into(),
        };
        let path = wide(&dir.to_string_lossy());
        // SAFETY: as in `create_or_verify`.
        unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&attrs)) }.unwrap();
    }

    /// The descriptor Windows actually gave `path`, as SDDL (owner and DACL).
    fn actual_sddl(path: &Path) -> String {
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
        };
        let name = wide(&path.to_string_lossy());
        let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a NUL-terminated name; the descriptor is freed by `SecurityDescriptor`.
        let err = unsafe {
            GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, info, None, None, None, None, &mut sd)
        };
        if err.is_err() {
            return format!("unreadable: {err:?}");
        }
        let sd = SecurityDescriptor(sd);
        let mut text = PWSTR::null();
        // SAFETY: a valid descriptor; the string is freed with LocalFree.
        unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd.0, SDDL_REVISION_1, info, &mut text, None) }
            .unwrap();
        // SAFETY: a NUL-terminated string allocated by the call above.
        let out = unsafe { text.to_string() }.unwrap();
        // SAFETY: allocated with LocalAlloc.
        unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        out
    }

    /// What an elevated `mkdir` makes: owner Administrators, the parent's ACL inherited.
    fn create_inheriting_owned_by_administrators(dir: &Path) {
        use windows::Win32::Security::Authorization::{ConvertStringSidToSidW, SetNamedSecurityInfoW};
        std::fs::create_dir(dir).unwrap();
        let mut sid = PSID::default();
        // SAFETY: a NUL-terminated SID string; the SID is freed with LocalFree.
        unsafe { ConvertStringSidToSidW(PCWSTR(wide(ADMINISTRATORS).as_ptr()), &mut sid) }.unwrap();
        let name = wide(&dir.to_string_lossy());
        // SAFETY: a NUL-terminated name and a valid SID; only the owner changes.
        let err = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(name.as_ptr()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(sid),
                None,
                None,
                None,
            )
        };
        // SAFETY: allocated with LocalAlloc by ConvertStringSidToSidW.
        unsafe { LocalFree(Some(HLOCAL(sid.0))) };
        assert!(err.is_ok(), "{err:?}");
    }

    fn reason(dir: &Path) -> String {
        verify(dir).unwrap_err().reason
    }

    /// A folder under C:\ProgramData that only SYSTEM and Administrators
    /// control (made elevated), deleted on drop, also on failure.
    struct AdminScratch(PathBuf);

    impl AdminScratch {
        fn new(name: &str) -> Self {
            let d = PathBuf::from(format!(r"C:\ProgramData\Atlas-Test-datadir-{name}-{}", std::process::id()));
            let _ = std::process::Command::new("cmd").args(["/c", "rmdir", "/s", "/q"]).arg(&d).output();
            create_with(&d, SDDL);
            AdminScratch(d)
        }
    }

    impl Drop for AdminScratch {
        fn drop(&mut self) {
            let _ = std::process::Command::new("cmd").args(["/c", "rmdir", "/s", "/q"]).arg(&self.0).output();
        }
    }

    /// The checks on descriptors built from SDDL: no directory, no elevation.
    #[test]
    fn descriptors_are_checked() {
        let check = |sddl: &str| check_descriptor(&SecurityDescriptor::from_sddl(sddl).unwrap());
        assert_eq!(check(SDDL), Ok(()));
        assert_eq!(check("O:SYD:P(A;OICI;FA;;;SY)"), Ok(()), "owned by SYSTEM");
        assert_eq!(check("O:BAD:P(D;OICI;FW;;;WD)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"), Ok(()), "a deny entry");
        for (sddl, needle) in [
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FR;;;BU)", "grants access to S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;WD)", "grants access to S-1-1-0"),
            ("O:BAD:PNO_ACCESS_CONTROL", "null DACL"),
            ("O:BAD:AI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", "not protected"),
            ("O:BUD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", "owned by S-1-5-32-545"),
            ("D:PAI(A;OICI;FA;;;SY)", "no owner"),
            // Review finding 8: what files inside would inherit, and kinds the
            // check does not read (a conditional and an object entry).
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;OICIIO;FA;;;BU)", "grants access to S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(XA;;FA;;;WD;(Member_of {SID(BA)}))", "entry of type 9"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(OA;;FA;bf967aba-0de6-11d0-a285-00aa003049e2;;BU)", "entry of type 5"),
        ] {
            let r = check(sddl);
            assert!(r.as_ref().is_err_and(|e| e.contains(needle)), "{sddl}: {r:?}");
        }
    }

    /// The checks on the folders above, on descriptors built from SDDL.
    #[test]
    fn ancestor_descriptors_are_checked() {
        let check = |sddl: &str| check_ancestor_descriptor(&SecurityDescriptor::from_sddl(sddl).unwrap());
        for (sddl, what) in [
            // C:\ as Windows sets it: Users read; Authenticated Users add
            // folders, and modify only what they create (inherit-only).
            (
                "O:S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464D:PAI(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)\
                 (A;OICI;0x1200a9;;;BU)(A;OICIIO;SDGXGWGR;;;AU)(A;;0x4;;;AU)",
                "the volume root",
            ),
            // C:\ProgramData: Users add files and folders, CREATOR OWNER inherit-only.
            (
                "O:SYD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICIIO;GA;;;CO)(A;OICI;0x1200a9;;;BU)(A;CI;0x116;;;BU)",
                "ProgramData",
            ),
            ("O:BAD:P(D;;FA;;;WD)(A;OICI;FA;;;SY)", "a deny entry"),
        ] {
            assert_eq!(check(sddl), Ok(()), "{what}");
        }
        for (sddl, needle) in [
            ("O:BUD:PAI(A;OICI;FA;;;SY)", "is owned by S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;FA;;;WD)", "lets S-1-1-0"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;SD;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;WD;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;WO;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;0x40;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(A;;GA;;;BU)", "lets S-1-5-32-545"),
            ("O:BAD:PNO_ACCESS_CONTROL", "null DACL"),
            ("D:PAI(A;OICI;FA;;;SY)", "no owner"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(XA;;FA;;;WD;(Member_of {SID(BA)}))", "entry of type 9"),
            ("O:BAD:PAI(A;OICI;FA;;;SY)(OA;;FA;bf967aba-0de6-11d0-a285-00aa003049e2;;BU)", "entry of type 5"),
        ] {
            let r = check(sddl);
            assert!(r.as_ref().is_err_and(|e| e.contains(needle)), "{sddl}: {r:?}");
        }
    }

    #[test]
    fn a_data_directory_below_the_users_temp_is_refused() {
        let base = scratch("below-temp");
        let dir = base.join("Atlas");
        let e = prepare(&dir).unwrap_err();
        // Refused at the user's own folder, not before (C:\ and C:\Users pass).
        assert!(e.reason.starts_with(r"C:\Users\") && e.reason.contains("a folder above it"), "{e}");
        assert!(!dir.exists(), "nothing is created below it");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The real folders above the default directory, as this machine has them.
    #[test]
    fn the_folders_above_the_default_pass() {
        check_ancestors(Path::new(r"C:\ProgramData\Atlas")).unwrap();
    }

    #[test]
    fn a_relative_or_dotted_path_is_refused() {
        // A `.` inside a path is dropped by `Path` and by Windows alike.
        for p in ["Atlas", r"C:\ProgramData\..\Users\Atlas", r"ProgramData\Atlas"] {
            let e = prepare(Path::new(p)).unwrap_err();
            assert!(e.reason.contains("absolute path without"), "{p}: {e}");
        }
    }

    #[test]
    fn a_folder_with_an_inherited_acl_is_refused() {
        let base = scratch("inherited");
        let dir = base.join("Atlas");
        std::fs::create_dir(&dir).unwrap();
        // Unelevated, the owner is the user; elevated, the DACL is inherited.
        let r = reason(&dir);
        assert!(r.contains("owned by") || r.contains("not protected"), "{r}");
        // The elevated tests' diagnostic reads it back: an owner and inherited entries.
        let sddl = actual_sddl(&dir);
        assert!(sddl.starts_with("O:") && sddl.contains("D:") && sddl.contains("ID;FA;"), "{sddl}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_junction_is_refused_without_following_it() {
        let base = scratch("junction");
        let target = base.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = base.join("Atlas");
        let out =
            std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(out.status.success(), "mklink: {out:?}");
        assert!(reason(&link).contains("reparse point"));
        let _ = std::process::Command::new("cmd").args(["/c", "rmdir"]).arg(&link).output();
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_file_is_refused() {
        let base = scratch("file");
        let f = base.join("Atlas");
        std::fs::write(&f, b"x").unwrap();
        assert!(reason(&f).contains("not a directory"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn prepare_creates_a_protected_directory_and_accepts_it_again() {
        let base = AdminScratch::new("prepare");
        let dir = base.0.join("Atlas");
        prepare(&dir).unwrap();
        for d in [dir.clone(), dir.join("buffer"), dir.join("logs")] {
            verify(&d).unwrap();
        }
        prepare(&dir).unwrap();
        // A file created inside inherits SYSTEM and Administrators only.
        let file = dir.join("buffer").join("probe");
        std::fs::write(&file, b"x").unwrap();
        let sddl = actual_sddl(&file);
        let dacl = sddl.split_once("D:").map_or("", |(_, d)| d);
        let trustees: Vec<&str> =
            dacl.split('(').skip(1).map(|ace| ace.trim_end_matches(')').rsplit(';').next().unwrap()).collect();
        assert!(!trustees.is_empty() && trustees.iter().all(|t| *t == "SY" || *t == "BA"), "{sddl}");
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn folders_above_that_others_control_are_refused() {
        let base = AdminScratch::new("above");
        // Making Users the owner takes the restore privilege.
        assert!(super::super::privilege::enable("SeRestorePrivilege"));
        let admin = "(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
        let mut wrong = Vec::new();
        for (name, sddl, needle) in [
            ("owned", format!("O:BUD:P{admin}"), Some("is owned by S-1-5-32-545")),
            ("delete", format!("O:BAD:P{admin}(A;;SD;;;BU)"), Some("lets S-1-5-32-545")),
            ("children", format!("O:BAD:P{admin}(A;;0x40;;;BU)"), Some("lets S-1-5-32-545")),
            ("dacl", format!("O:BAD:P{admin}(A;;WD;;;BU)"), Some("lets S-1-5-32-545")),
            ("inherit-only", format!("O:BAD:P{admin}(A;OICIIO;FA;;;BU)"), None),
        ] {
            let parent = base.0.join(name);
            create_with(&parent, &sddl);
            let dir = parent.join("Atlas");
            let r = prepare(&dir);
            let ok = match needle {
                Some(n) => r.as_ref().is_err_and(|e| e.reason.contains(n)) && !dir.exists(),
                None => r.is_ok(),
            };
            if !ok {
                wrong.push(format!("{name}: got {}: {r:?}", actual_sddl(&parent)));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");

        // A junction above is refused, not followed.
        let target = base.0.join("target");
        create_with(&target, &format!("O:BAD:P{admin}"));
        let link = base.0.join("link");
        let out =
            std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(&link).arg(&target).output().unwrap();
        assert!(out.status.success(), "mklink: {out:?}");
        let e = prepare(&link.join("Atlas")).unwrap_err();
        assert!(e.reason.contains("reparse point"), "{e}");
        assert!(!target.join("Atlas").exists(), "nothing is created through it");
        let _ = std::process::Command::new("cmd").args(["/c", "rmdir"]).arg(&link).output();
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn loose_protected_directories_are_refused() {
        let base = scratch("loose");
        // Making Users the owner takes the restore privilege.
        assert!(super::super::privilege::enable("SeRestorePrivilege"));
        let mut wrong = Vec::new();
        for (name, sddl, needle) in [
            ("users", "O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FR;;;BU)", "grants access to S-1-5-32-545"),
            ("null", "O:BAD:PNO_ACCESS_CONTROL", "null DACL"),
            ("owner", "O:BUD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", "owned by S-1-5-32-545"),
        ] {
            let dir = base.join(name);
            create_with(&dir, sddl);
            let r = verify(&dir);
            let actual = actual_sddl(&dir);
            println!("{name}: asked {sddl}, got {actual}: {r:?}");
            if !r.as_ref().is_err_and(|e| e.reason.contains(needle)) {
                wrong.push(format!("{name}: got {actual}: {r:?}"));
            }
        }
        // An explicit DACL given at creation is made protected (NTFS), so the
        // inheriting case is made the way an elevated `mkdir` makes it.
        let open = base.join("open");
        create_inheriting_owned_by_administrators(&open);
        let r = verify(&open);
        println!("open: got {}: {r:?}", actual_sddl(&open));
        if !r.as_ref().is_err_and(|e| e.reason.contains("not protected")) {
            wrong.push(format!("open: got {}: {r:?}", actual_sddl(&open)));
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
        // A deny entry only takes access away (here: adding files, so the
        // directory can still be deleted).
        let deny = base.join("deny");
        create_with(&deny, "O:BAD:P(D;OICI;0x2;;;WD)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        verify(&deny).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }
}
```

```diff
--- a/crates/atlas-agent/src/win/util.rs
+++ b/crates/atlas-agent/src/win/util.rs
@@ -1,8 +1,10 @@
 //! Small helpers shared by the Windows services.
 
-use windows::Win32::Foundation::{CloseHandle, HANDLE, UNICODE_STRING};
+use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree, UNICODE_STRING};
+use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
+use windows::Win32::Security::PSECURITY_DESCRIPTOR;
 use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
-use windows::core::PWSTR;
+use windows::core::{PCWSTR, PWSTR};
 
 /// A NUL-terminated UTF-16 copy of `s`.
 pub(crate) fn wide(s: &str) -> Vec<u16> {
@@ -33,6 +35,31 @@ impl Drop for Owned {
 // SAFETY: a kernel handle value can be used and closed from any thread.
 unsafe impl Send for Owned {}
 
+/// A security descriptor allocated by the system, freed with `LocalFree`.
+pub(crate) struct SecurityDescriptor(pub PSECURITY_DESCRIPTOR);
+
+impl SecurityDescriptor {
+    pub(crate) fn from_sddl(sddl: &str) -> Result<Self, String> {
+        let text = wide(sddl);
+        let mut sd = PSECURITY_DESCRIPTOR::default();
+        // SAFETY: `text` is NUL-terminated; the descriptor is freed on drop.
+        unsafe {
+            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None)
+        }
+        .map_err(|e| format!("security descriptor {sddl:?}: {e}"))?;
+        Ok(SecurityDescriptor(sd))
+    }
+}
+
+impl Drop for SecurityDescriptor {
+    fn drop(&mut self) {
+        if !self.0.0.is_null() {
+            // SAFETY: allocated with LocalAlloc by the system call that returned it.
+            unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
+        }
+    }
+}
+
 /// The current QPC value.
 pub(crate) fn qpc_now() -> i64 {
     let mut v = 0i64;
```

```diff
--- a/crates/atlas-agent/src/win/mod.rs
+++ b/crates/atlas-agent/src/win/mod.rs
@@ -14,6 +14,7 @@
 compile_error!("atlas-agent's Windows services support 64-bit Windows only");
 
 pub mod agent;
+pub mod datadir;
 mod expand;
 mod handles;
 mod hash;
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent datadir
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass (the three elevated tests ignored). Workspace: 400 passed, 10 ignored (`atlas-agent` unit tests: 215).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): the data directory, created protected or verified"
```

### Task 8: `atlas-agent run` and `dump`, and the health thread

**Files:**
- Modify: `crates/atlas-agent/Cargo.toml`, `crates/atlas-agent/src/win/agent.rs`, `crates/atlas-agent/src/win/mod.rs`, `crates/atlas-agent/tests/live.rs`, `crates/atlas-etw/src/session/consumer.rs`
- Create: `crates/atlas-agent/src/main.rs`, `crates/atlas-agent/src/win/health.rs`, `crates/atlas-agent/src/win/instance.rs`, `crates/atlas-agent/src/win/run.rs`

- [ ] **Step 1: What the health thread reads.** `Consumer::panic_counter` shares a consumer's panic count while it runs. `Running` keeps the session names and gives `health_sources()`: the control sender, the intake and service counters, the panic counters, the session names and whether seeding is on. `Stopped` gains the sessions' final figures from `Session::stop`, for the final report. `Stopped::pipeline` becomes a `Result`: a panicked pipeline thread is joined without re-panicking, and the panic's message is kept (review finding 2); the live test expects `Ok`.

```diff
--- a/crates/atlas-etw/src/session/consumer.rs
+++ b/crates/atlas-etw/src/session/consumer.rs
@@ -198,6 +198,12 @@ impl Consumer {
         self.panics.load(Ordering::Relaxed)
     }
 
+    /// The counter behind [`Consumer::panics`], for a thread that reports it
+    /// while the consumer runs (the agent's Sensor Health, plan 1b-4a).
+    pub fn panic_counter(&self) -> Arc<AtomicU64> {
+        self.panics.clone()
+    }
+
     /// `ProcessTrace` has returned: the session stopped or was destroyed (§9.1).
     pub fn is_finished(&self) -> bool {
         self.thread.as_ref().is_none_or(JoinHandle::is_finished)
```

```diff
--- a/crates/atlas-agent/src/win/agent.rs
+++ b/crates/atlas-agent/src/win/agent.rs
@@ -3,18 +3,19 @@
 //! callbacks' intakes, the Windows services, and the pipeline thread running
 //! [`crate::driver::Driver`] into a sink.
 //!
-//! Plan 1b-4 adds the buffer writer as the sink, the watchdog, Sensor Health,
-//! the service and the CLI around this.
+//! Plan 1b-4a runs it from `atlas-agent run` with the buffer writer as the
+//! sink and the health thread reading its counters ([`Running::health_sources`]).
 
 use std::fmt;
 use std::path::PathBuf;
 use std::sync::Arc;
+use std::sync::atomic::AtomicU64;
 use std::sync::mpsc::{Sender, channel, sync_channel};
 use std::thread::JoinHandle;
 use std::time::Duration;
 
 use atlas_etw::providers;
-use atlas_etw::session::{self, Consumer, EtwError, EventRecord, Session, VersionGate};
+use atlas_etw::session::{self, Consumer, EtwError, EventRecord, Session, SessionInfo, VersionGate};
 use atlas_schema::{Event, EventId};
 
 use super::identity::{self, IdentityError, Started};
@@ -83,16 +84,36 @@ pub struct Running {
     /// The pipeline's settings as started (seeding turned off without
     /// `SeDebugPrivilege`): what a replacement `Intake` needs (plan 1b-4).
     config: Config,
+    names: [String; 2],
 }
 
 /// What a stopped sensor counted.
 #[derive(Debug)]
 pub struct Stopped {
-    pub pipeline: Counters,
+    /// The pipeline's counters, or the message of the panic that ended its
+    /// thread (its counters died with it).
+    pub pipeline: Result<Counters, String>,
     pub intake: Arc<IntakeCounters>,
     pub services: Arc<ServiceCounters>,
     /// Callbacks that panicked, both sessions (`loss.callback_panics`).
     pub callback_panics: u64,
+    /// The sessions as they stopped: their final loss counters.
+    pub sensor_session: Option<SessionInfo>,
+    pub process_session: Option<SessionInfo>,
+}
+
+/// What the health thread reads while the sensor runs (plan 1b-4a, D1).
+#[derive(Clone)]
+pub struct HealthSources {
+    /// The pipeline's counters are read through it, between two passes.
+    pub control: Sender<Control<WinLookups>>,
+    pub intake: Arc<IntakeCounters>,
+    pub services: Arc<ServiceCounters>,
+    /// Both consumers' callback panics.
+    pub panics: Vec<Arc<AtomicU64>>,
+    /// Session A's and Session B's names, for their loss counters.
+    pub sessions: [String; 2],
+    pub seeding: bool,
 }
 
 /// Starts the sensor (§11.4): identity, the services, both sessions and their
@@ -162,6 +183,7 @@ pub fn start(opts: AgentOptions, sink: impl FnMut(Event) + Send + 'static) -> Re
         seeding,
         started,
         config,
+        names: [opts.sensor_session, opts.process_session],
     })
 }
 
@@ -204,6 +226,18 @@ impl Running {
         self.control.clone()
     }
 
+    /// The sources of the Sensor Health reports.
+    pub fn health_sources(&self) -> HealthSources {
+        HealthSources {
+            control: self.control.clone(),
+            intake: self.intake.clone(),
+            services: self.services.clone(),
+            panics: self.consumers.iter().map(Consumer::panic_counter).collect(),
+            sessions: self.names.clone(),
+            seeding: self.seeding,
+        }
+    }
+
     /// Whether the pipeline thread still runs: if it panicked, the callbacks'
     /// sends fail and nothing is emitted (the watchdog's check, plan 1b-4).
     pub fn pipeline_running(&self) -> bool {
@@ -222,9 +256,7 @@ impl Running {
     }
 
     fn finish(&mut self) -> Stopped {
-        for s in [self.sensor.take(), self.process.take()].into_iter().flatten() {
-            let _ = s.stop();
-        }
+        let mut last = [self.sensor.take(), self.process.take()].map(|s| s.and_then(|s| s.stop().ok()));
         // A stopped session's consumer delivers what is left and finishes; its
         // panic count is read after that (review R-m7).
         let t0 = std::time::Instant::now();
@@ -238,8 +270,21 @@ impl Running {
             c.close();
         }
         self.spare = None;
-        let pipeline = self.pipeline.take().map(|t| t.join().expect("the pipeline thread")).unwrap_or_default();
-        Stopped { pipeline, intake: self.intake.clone(), services: self.services.clone(), callback_panics }
+        // A panicked pipeline thread must not take the caller down with it:
+        // `run` still sends the final report and flushes the buffer.
+        let pipeline = match self.pipeline.take().map(JoinHandle::join) {
+            Some(Ok(c)) => Ok(c),
+            Some(Err(p)) => Err(crate::panic_message(p.as_ref())),
+            None => Ok(Counters::default()),
+        };
+        Stopped {
+            pipeline,
+            intake: self.intake.clone(),
+            services: self.services.clone(),
+            callback_panics,
+            sensor_session: last[0].take(),
+            process_session: last[1].take(),
+        }
     }
 }
 
```

```diff
--- a/crates/atlas-agent/tests/live.rs
+++ b/crates/atlas-agent/tests/live.rs
@@ -351,7 +351,7 @@ fn live_agent() {
     assert!(actor.wait().unwrap().success(), "the actor failed");
 
     analyse(&mut report, &events, &result, actor_pid, me);
-    let c = &stopped.pipeline;
+    let c = stopped.pipeline.as_ref().expect("the pipeline thread ended cleanly");
     let i = &stopped.intake;
     let s = &stopped.services;
     let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
```

```diff
--- a/crates/atlas-agent/Cargo.toml
+++ b/crates/atlas-agent/Cargo.toml
@@ -42,11 +42,13 @@ features = [
     "Win32_Security_Cryptography_Sip",
     "Win32_Security_WinTrust",
     "Win32_Storage_FileSystem",
+    "Win32_System_Console",
     "Win32_System_IO",
     "Win32_System_Ioctl",
     "Win32_System_Kernel",
     "Win32_System_Performance",
     "Win32_System_Pipes",
+    "Win32_System_ProcessStatus",
     "Win32_System_Registry",
     "Win32_System_SystemInformation",
     "Win32_System_Threading",
```

- [ ] **Step 2: The health thread** (D1, D5.6).
  - Every 5 s it takes a snapshot: the pipeline's counters through `control()` (waiting up to 2 s, else `None`), the atomics, the sessions by name (`query_by_name`), the sink's drops, `GetProcessTimes` and `GetProcessMemoryInfo`.
  - Every 60 s, or at once when `loss_rose`, it sends a report to the writer and logs one `health report` line with the main figures and the `Diagnostics`; the next interval starts where this one ended.
  - `stop()` ends the thread and returns the open interval, so the final report covers the time up to the stop; `final_snapshot` reads the stopped sensor's figures.
  - A report's time is the system clock at generation (`identity::anchor().unix_ns`, `GetSystemTimePreciseAsFileTime`); its id is a UUIDv7.
  - A unit test drives the real loop with plain counters, no pipeline and no sessions: a loss is reported at the next check, the period starts again from that report, and `stop()` returns where the last one ended (review finding 8).

`crates/atlas-agent/src/win/health.rs`:
```rust
//! The health thread (sensor spec §3.2 [7], §9.3; plan 1b-4a, D1): every 5 s
//! it takes a [`Snapshot`] of every counter; every 60 s, or at once when a
//! loss counter rose, it sends a Sensor Health report to the buffer writer and
//! logs its figures. Plan 1b-4b adds the watchdog's checks to this thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use atlas_etw::session::{SessionInfo, query_by_name};
use atlas_schema::{Device, EventId};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use super::agent::{HealthSources, Stopped};
use super::identity;
use crate::counters::Counters;
use crate::health::{self, Diagnostics, SessionLoss, Snapshot};
use crate::store::Record;

#[derive(Debug, Clone, Copy)]
pub struct HealthConfig {
    pub check_every: Duration,
    pub report_every: Duration,
    /// How long the pipeline may take to answer a counters request.
    pub pipeline_timeout: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        HealthConfig {
            check_every: Duration::from_secs(5),
            report_every: Duration::from_secs(60),
            pipeline_timeout: Duration::from_secs(2),
        }
    }
}

/// Where the next report starts: its base snapshot and its start time (Unix ns).
#[derive(Debug, Clone)]
pub struct Interval {
    pub from: Snapshot,
    pub start: i64,
}

/// The running health thread.
pub struct HealthThread {
    stop: Sender<()>,
    thread: JoinHandle<Interval>,
}

/// Everything a report needs besides the sources.
#[derive(Clone)]
pub struct Reporting {
    pub to: SyncSender<Record>,
    pub device: Device,
    /// `loss.buffer_backlog_drops` (`Store::drops`).
    pub buffer_drops: Arc<AtomicU64>,
}

impl HealthThread {
    pub fn start(src: HealthSources, out: Reporting, first: Interval, cfg: HealthConfig) -> HealthThread {
        let (stop, stop_rx) = channel();
        let thread = std::thread::Builder::new()
            .name("atlas-health".into())
            .spawn(move || run(&src, &out, first, cfg, &stop_rx))
            .expect("spawn the health thread");
        HealthThread { stop, thread }
    }

    /// Stops the thread; returns where the final report starts.
    pub fn stop(self) -> Interval {
        drop(self.stop);
        self.thread.join().expect("the health thread")
    }
}

fn run(src: &HealthSources, out: &Reporting, first: Interval, cfg: HealthConfig, stop: &Receiver<()>) -> Interval {
    let mut at = first;
    let mut next_report = Instant::now() + cfg.report_every;
    loop {
        match stop.recv_timeout(cfg.check_every) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return at,
        }
        let now = snapshot(src, &out.buffer_drops, pipeline_counters(src, cfg.pipeline_timeout));
        if Instant::now() >= next_report || health::loss_rose(&at.from, &now) {
            at = send(out, at, &now);
            next_report = Instant::now() + cfg.report_every;
        }
    }
}

/// Sends the report for `at` up to `now`; returns where the next one starts.
pub fn send(out: &Reporting, at: Interval, now: &Snapshot) -> Interval {
    let time = identity::anchor().unix_ns;
    let (report, diag) = health::report(&at.from, now);
    log(&report, &diag, time - at.start);
    let e = health::event(EventId::new_v7(), time, out.device, at.start, report);
    if out.to.send(Record::Health(e)).is_err() {
        tracing::warn!("health: the buffer writer is gone; report not written");
    }
    Interval { from: at.from.advance(now), start: time }
}

/// The figures of each report, on one line, for the host run's summary.
fn log(r: &atlas_schema::classes::sensor_health::HealthReport, d: &Diagnostics, interval_ns: i64) {
    let n = |v: Option<u64>| v.map_or(-1, |v| v as i64);
    let classes = |v: &[atlas_schema::classes::sensor_health::ClassCount]| v.iter().map(|c| c.count).sum::<u64>();
    tracing::info!(
        interval_ms = interval_ns / 1_000_000,
        sensor_events_lost = n(r.loss.sensor_session_events_lost),
        process_events_lost = n(r.loss.process_session_events_lost),
        sensor_buffers_lost = n(r.loss.sensor_session_buffers_lost),
        process_buffers_lost = n(r.loss.process_session_buffers_lost),
        kernel_queue_drops = n(r.loss.kernel_queue_drops),
        user_queue_drops = n(r.loss.user_queue_drops),
        buffer_drops = n(r.loss.buffer_backlog_drops),
        callback_panics = n(r.loss.callback_panics),
        actor_dropped = classes(&r.loss.actor_dropped),
        dns_rate_limit_drops = n(r.loss.dns_rate_limit_drops),
        cleanup_unpaired = d.cleanup_unpaired,
        cleanup_outcome_late = d.cleanup_outcome_late,
        file_delete_outcome_unknown = n(r.quality.file_delete_outcome_unknown),
        late_arrivals = n(r.quality.late_arrivals),
        service_queue_drops = n(r.housekeeping.service_queue_drops),
        pending_overflow = n(r.housekeeping.pending_overflow),
        self_filtered = n(d.self_filtered),
        cpu_ms = n(r.resources.cpu_time.map(|ns| ns / 1_000_000)),
        working_set_mb = n(r.resources.working_set.map(|b| b >> 20)),
        "health report"
    );
}

/// The pipeline's counters, read on its thread between two passes; `None` if
/// it does not answer in time (it ended, or is stuck).
fn pipeline_counters(src: &HealthSources, timeout: Duration) -> Option<Counters> {
    let (tx, rx) = channel();
    src.control.send(Box::new(move |p| drop(tx.send(p.counters())))).ok()?;
    rx.recv_timeout(timeout).ok()
}

fn session_loss(info: Option<SessionInfo>) -> Option<SessionLoss> {
    info.map(|i| SessionLoss { events_lost: i.events_lost, buffers_lost: i.realtime_buffers_lost })
}

/// Every source now. `pipeline` is read by the caller: through the control
/// channel while running, from `Stopped` at the end.
pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<Counters>) -> Snapshot {
    let query = |name: &str| session_loss(query_by_name(name).ok().flatten());
    let (cpu_time, working_set) = resources();
    Snapshot {
        pipeline,
        intake: src.intake.totals(),
        services: src.services.totals(),
        callback_panics: src.panics.iter().map(|p| p.load(Ordering::Relaxed)).sum(),
        sensor_session: query(&src.sessions[0]),
        process_session: query(&src.sessions[1]),
        buffer_drops: buffer_drops.load(Ordering::Relaxed),
        cpu_time,
        working_set,
        seeding: src.seeding,
    }
}

/// The snapshot for the final report, after the stop: the sessions' last
/// counters come from their stop; the pipeline's are absent if its thread
/// panicked.
pub fn final_snapshot(stopped: &Stopped, buffer_drops: &AtomicU64, seeding: bool) -> Snapshot {
    let (cpu_time, working_set) = resources();
    Snapshot {
        pipeline: stopped.pipeline.as_ref().ok().cloned(),
        intake: stopped.intake.totals(),
        services: stopped.services.totals(),
        callback_panics: stopped.callback_panics,
        sensor_session: session_loss(stopped.sensor_session),
        process_session: session_loss(stopped.process_session),
        buffer_drops: buffer_drops.load(Ordering::Relaxed),
        cpu_time,
        working_set,
        seeding,
    }
}

/// The process's CPU time (user + kernel, ns) and working set (bytes).
fn resources() -> (Option<u64>, Option<u64>) {
    let ft = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) * 100;
    let (mut c, mut e, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the current-process pseudo-handle and four out-structures.
    let cpu =
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) }.ok().map(|()| ft(k) + ft(u));
    let mut m = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    // SAFETY: the current-process pseudo-handle and a structure of the size given.
    let ws = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) }.ok().map(|()| m.WorkingSetSize as u64);
    (cpu, ws)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use atlas_schema::classes::sensor_health::SensorHealthAction;
    use atlas_schema::{BootId, DeviceUid, EventKind};

    use super::*;
    use crate::counters::{IntakeCounters, ServiceCounters};

    /// Sources with no pipeline (its channel is closed) and no sessions.
    fn sources(intake: &Arc<IntakeCounters>) -> HealthSources {
        let (control, _) = channel();
        let pid = std::process::id();
        HealthSources {
            control,
            intake: intake.clone(),
            services: Arc::new(ServiceCounters::default()),
            panics: Vec::new(),
            sessions: [format!("Atlas-Test-None-A-{pid}"), format!("Atlas-Test-None-B-{pid}")],
            seeding: false,
        }
    }

    /// The next report's time and `loss.kernel_queue_drops`.
    fn next_report(rx: &Receiver<Record>, within: Duration) -> Option<(i64, Option<u64>)> {
        let Record::Health(e) = rx.recv_timeout(within).ok()? else { panic!("not a report") };
        let EventKind::SensorHealth(a) = &e.kind else { panic!("not Sensor Health") };
        let SensorHealthAction::Report(r) = &a.action;
        Some((e.meta.time, r.loss.kernel_queue_drops))
    }

    /// Review finding 8: a loss is reported at the next check, the period
    /// starts again from that report, and `stop` returns where the last one
    /// ended.
    #[test]
    fn a_loss_is_reported_at_once_and_restarts_the_period() {
        let intake = Arc::new(IntakeCounters::default());
        let (to, rx) = sync_channel(64);
        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
        let out = Reporting { to, device, buffer_drops: Arc::new(AtomicU64::new(0)) };
        let period = Duration::from_millis(1_000);
        let cfg = HealthConfig {
            check_every: Duration::from_millis(20),
            report_every: period,
            pipeline_timeout: Duration::from_millis(1),
        };
        let first = Interval { from: Snapshot::at_start(false), start: identity::anchor().unix_ns };
        let health = HealthThread::start(sources(&intake), out, first, cfg);

        std::thread::sleep(Duration::from_millis(400));
        assert!(rx.try_recv().is_err(), "no report before the period without a loss");
        intake.kernel_queue_drops.fetch_add(1, Ordering::Relaxed);
        let (t1, drops) = next_report(&rx, Duration::from_millis(500)).expect("the loss reported at once");
        assert_eq!(drops, Some(1));
        // Times are taken as each report is sent: a slow test thread does not
        // shorten the gap.
        let (t2, drops) = next_report(&rx, Duration::from_secs(5)).expect("the periodic report");
        let gap = Duration::from_nanos((t2 - t1) as u64);
        assert!(gap + Duration::from_millis(50) >= period, "{gap:?}: the period did not restart at the loss");
        assert_eq!(drops, Some(0), "counted since the report before");
        let at = health.stop();
        assert_eq!(at.start, t2, "the final report starts where the last one ended");
    }

    #[test]
    fn resources_are_read() {
        let (cpu, ws) = resources();
        assert!(cpu.is_some() && ws.is_some_and(|b| b > 0));
        // Burn a little CPU: the time moves.
        let t0 = Instant::now();
        let mut x = 0u64;
        while t0.elapsed() < Duration::from_millis(50) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        assert!(resources().0 > cpu);
    }
}
```

- [ ] **Step 3: The single-instance lock** (review finding 1; §11.3). A mutex in `Global\` can be created by any user, so one could take the name first and keep the agent from ever starting (verified: an unelevated process created `Global\Atlas-Squat-Test`). `instance::acquire` takes a mutex `Atlas\Agent` inside a private namespace:
  - the boundary descriptor is named `Atlas` and holds the Administrators SID, so only a token with Administrators enabled can create the namespace; the namespace and the mutex get the DACL `D:P(A;;GA;;;SY)(A;;GA;;;BA)`, so the service and an elevated `run` can each open what the other made;
  - if the namespace exists it is opened; a holder that exits between the two is retried; a token without Administrators is refused at the create even when the namespace exists (`Refused::Denied`);
  - the mutex already there, or held under a DACL this token is not in, is another instance (`Refused::Running`);
  - the namespace is closed, not destroyed, so it goes with its last handle.

  Verified on the host before it was written (an unelevated squatter, an elevated holder, a SYSTEM probe in session 0): a name-only and a user-SID namespace of the same name and `Global\Atlas-Agent` blocked nothing; a non-admin could neither create nor open it; a second instance saw `ERROR_ALREADY_EXISTS`, in session 0 too; a holder that died without closing freed it. In one process an alias cannot be used twice at once (`ERROR_DUP_NAME`), so the tests take a second lock through `acquire_as` with another alias, as a second process would. Elevated tests: taken once until dropped; squats block nothing; a token with Administrators deny-only (by impersonation) is refused, held or not; a lock held under another DACL is another instance; both DACLs grant SYSTEM and Administrators only.

`crates/atlas-agent/src/win/instance.rs`:
```rust
//! The single-instance lock (sensor spec §11.3; plan 1b-4a): `run` cannot tear
//! down the service's sessions, nor a second `run` the first one's.
//!
//! A mutex in `Global\` will not do: creating a mutex there takes no
//! privilege, so any user could take the name first and keep the agent from
//! ever starting. The mutex ([`MUTEX`]) lives instead in a private namespace
//! whose boundary holds the Administrators SID:
//! - only a token with Administrators enabled (elevated, or SYSTEM) can create
//!   that namespace;
//! - its DACL ([`SDDL`]) lets only SYSTEM and Administrators open it;
//! - a namespace of the same name with another boundary (no SID, a user's SID)
//!   is a different namespace, so squatting the name blocks nothing;
//! - private namespaces are not per session: the service in session 0 and
//!   `run` in a user's session find the same one.
//!
//! The namespace goes away with its last handle, so a process that dies
//! without closing it does not block the next start.

use std::fmt;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_PATH_NOT_FOUND, GetLastError, HANDLE, WIN32_ERROR,
};
use windows::Win32::Security::{
    CreateWellKnownSid, PSID, SECURITY_ATTRIBUTES, SECURITY_MAX_SID_SIZE, WinBuiltinAdministratorsSid,
};
use windows::Win32::System::Threading::{
    AddSIDToBoundaryDescriptor, ClosePrivateNamespace, CreateBoundaryDescriptorW, CreateMutexW,
    CreatePrivateNamespaceW, DeleteBoundaryDescriptor, OpenPrivateNamespaceW,
};
use windows::core::PCWSTR;

use super::util::{Owned, SecurityDescriptor, wide};

/// The boundary's name, and the namespace's alias in this process.
pub const NAMESPACE: &str = "Atlas";
/// The mutex, inside the namespace (as named through [`NAMESPACE`]).
pub const MUTEX: &str = r"Atlas\Agent";
const MUTEX_LEAF: &str = "Agent";
/// The namespace's and the mutex's DACL: SYSTEM and Administrators only, so
/// the service and an elevated `run` can each open what the other created.
pub const SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)";

/// Held for the life of the agent; dropping it releases the lock.
#[derive(Debug)]
pub struct Instance {
    // Field order is drop order: the mutex, then the namespace it lives in.
    _mutex: Owned,
    _namespace: Namespace,
}

/// Why the lock was not taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Another instance holds it.
    Running,
    /// The token has no enabled Administrators SID.
    Denied,
    Failed(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Running => write!(f, "another instance is running ({MUTEX} is held)"),
            Refused::Denied => write!(f, "the single-instance lock ({MUTEX}) needs administrator rights"),
            Refused::Failed(e) => write!(f, "the single-instance lock ({MUTEX}) cannot be taken: {e}"),
        }
    }
}

/// Takes the lock, or says why not.
pub fn acquire() -> Result<Instance, Refused> {
    acquire_as(NAMESPACE)
}

/// [`acquire`] with the namespace under `alias`. An alias names the namespace
/// in one process only and cannot be used twice there at once
/// (`ERROR_DUP_NAME`), so tests that hold the lock in-process take it again
/// under another alias, as a second process would.
fn acquire_as(alias: &str) -> Result<Instance, Refused> {
    let namespace = namespace(alias)?;
    let sd = SecurityDescriptor::from_sddl(SDDL).map_err(Refused::Failed)?;
    let sa = attributes(&sd);
    let name = wide(&format!(r"{alias}\{MUTEX_LEAF}"));
    // SAFETY: `name` is NUL-terminated and `sa` outlives the call; the handle
    // is closed by `Owned`.
    let created = unsafe { CreateMutexW(Some(&sa), false, PCWSTR(name.as_ptr())) };
    // SAFETY: no arguments; read at once after the call above.
    let last = unsafe { GetLastError() };
    match created {
        Ok(h) if last == ERROR_ALREADY_EXISTS => {
            drop(Owned(h));
            Err(Refused::Running)
        }
        Ok(h) => Ok(Instance { _mutex: Owned(h), _namespace: namespace }),
        // Held by an instance whose DACL differs: still another instance.
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Err(Refused::Running),
        Err(e) => Err(Refused::Failed(format!("mutex: {e}"))),
    }
}

/// Creates the namespace, or opens the one another instance created.
///
/// A token without Administrators enabled is refused at the create, whether
/// the namespace exists or not: the boundary is checked first. The open is
/// reached only by a token the DACL ([`SDDL`]) admits.
fn namespace(alias: &str) -> Result<Namespace, Refused> {
    let boundary = Boundary::administrators()?;
    let sd = SecurityDescriptor::from_sddl(SDDL).map_err(Refused::Failed)?;
    let sa = attributes(&sd);
    let alias = wide(alias);
    // A holder that exits between a refused create and the open: try again.
    for _ in 0..3 {
        // SAFETY: the boundary is valid for the call; `alias` is NUL-terminated;
        // `sa` outlives the call. The handle is closed by `Namespace`.
        let h = unsafe { CreatePrivateNamespaceW(Some(&sa), boundary.0.0, PCWSTR(alias.as_ptr())) };
        if !h.is_invalid() && !h.0.is_null() {
            return Ok(Namespace(h));
        }
        // SAFETY: no arguments; read at once after the call above.
        match unsafe { GetLastError() } {
            ERROR_ALREADY_EXISTS => {}
            ERROR_ACCESS_DENIED => return Err(Refused::Denied),
            e => return Err(failed("create the namespace", e)),
        }
        // SAFETY: as above, without the attributes.
        let h = unsafe { OpenPrivateNamespaceW(boundary.0.0, PCWSTR(alias.as_ptr())) };
        if !h.is_invalid() && !h.0.is_null() {
            return Ok(Namespace(h));
        }
        // SAFETY: no arguments; read at once after the call above.
        match unsafe { GetLastError() } {
            ERROR_PATH_NOT_FOUND => continue,
            e => return Err(failed("open the namespace", e)),
        }
    }
    Err(Refused::Failed("the namespace kept vanishing between create and open".into()))
}

fn attributes(sd: &SecurityDescriptor) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.0,
        bInheritHandle: false.into(),
    }
}

fn failed(what: &str, e: WIN32_ERROR) -> Refused {
    Refused::Failed(format!("{what}: {}", windows::core::Error::from(e.to_hresult())))
}

/// A boundary descriptor, deleted on drop.
struct Boundary(HANDLE);

impl Boundary {
    /// Named [`NAMESPACE`], holding the Administrators SID.
    fn administrators() -> Result<Boundary, Refused> {
        let name = wide(NAMESPACE);
        // SAFETY: `name` is NUL-terminated; the descriptor is deleted on drop.
        let h = unsafe { CreateBoundaryDescriptorW(PCWSTR(name.as_ptr()), 0) };
        if h.is_invalid() || h.0.is_null() {
            // SAFETY: no arguments; read at once after the call above.
            return Err(failed("boundary descriptor", unsafe { GetLastError() }));
        }
        let mut boundary = Boundary(h);
        let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut len = sid.len() as u32;
        let psid = PSID(sid.as_mut_ptr().cast());
        // SAFETY: `sid` has room for any SID, `len` says so.
        unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(psid), &mut len) }
            .map_err(|e| Refused::Failed(format!("Administrators SID: {e}")))?;
        // SAFETY: the descriptor may be reallocated, which updates `boundary.0`;
        // the SID is copied into it.
        unsafe { AddSIDToBoundaryDescriptor(&mut boundary.0, psid) }
            .map_err(|e| Refused::Failed(format!("boundary SID: {e}")))?;
        Ok(boundary)
    }
}

impl Drop for Boundary {
    fn drop(&mut self) {
        // SAFETY: created by CreateBoundaryDescriptorW, deleted only here.
        unsafe { DeleteBoundaryDescriptor(self.0) };
    }
}

/// A private namespace handle, closed (not destroyed) on drop: the namespace
/// goes away with its last handle.
#[derive(Debug)]
struct Namespace(HANDLE);

impl Drop for Namespace {
    fn drop(&mut self) {
        // SAFETY: returned by Create/OpenPrivateNamespaceW, closed only here.
        unsafe { ClosePrivateNamespace(self.0, 0) };
    }
}

// SAFETY: a kernel handle value can be used and closed from any thread.
unsafe impl Send for Namespace {}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows::Win32::Security::{
        CREATE_RESTRICTED_TOKEN_FLAGS, CreateRestrictedToken, DACL_SECURITY_INFORMATION, ImpersonateLoggedOnUser,
        PSECURITY_DESCRIPTOR, RevertToSelf, SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
        TOKEN_IMPERSONATE, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::PWSTR;

    use super::*;

    /// Runs `f` on this thread as this token with Administrators deny-only:
    /// what an unelevated administrator, or a standard user, has.
    fn without_administrators<T>(f: impl FnOnce() -> T) -> T {
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: ends this thread's impersonation.
                unsafe { RevertToSelf() }.unwrap();
            }
        }
        let mut token = HANDLE::default();
        // SAFETY: the process pseudo-handle; the token is closed by `Owned`.
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_IMPERSONATE,
                &mut token,
            )
        }
        .unwrap();
        let token = Owned(token);
        let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut len = sid.len() as u32;
        let psid = PSID(sid.as_mut_ptr().cast());
        // SAFETY: `sid` has room for any SID.
        unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(psid), &mut len) }.unwrap();
        let disable = [SID_AND_ATTRIBUTES { Sid: psid, Attributes: 0 }];
        let mut restricted = HANDLE::default();
        // SAFETY: a valid token and SID; the new token is closed by `Owned`.
        unsafe {
            CreateRestrictedToken(
                token.raw(),
                CREATE_RESTRICTED_TOKEN_FLAGS(0),
                Some(&disable),
                None,
                None,
                &mut restricted,
            )
        }
        .unwrap();
        let restricted = Owned(restricted);
        // SAFETY: a token derived from this process's own.
        unsafe { ImpersonateLoggedOnUser(restricted.raw()) }.unwrap();
        let _revert = Revert;
        f()
    }

    fn dacl(h: HANDLE) -> String {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a valid handle; the descriptor is freed by `SecurityDescriptor`.
        let err = unsafe {
            GetSecurityInfo(h, SE_KERNEL_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, Some(&mut sd))
        };
        assert!(err.is_ok(), "GetSecurityInfo: {err:?}");
        let sd = SecurityDescriptor(sd);
        let mut text = PWSTR::null();
        // SAFETY: a valid descriptor; the string is freed with LocalFree.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd.0,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                None,
            )
        }
        .unwrap();
        // SAFETY: a NUL-terminated string allocated by the call above.
        let out = unsafe { text.to_string() }.unwrap();
        // SAFETY: allocated with LocalAlloc.
        unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        out
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn the_lock_is_taken_once_until_dropped() {
        let first = acquire().expect("the first instance");
        assert_eq!(acquire_as("Second").unwrap_err(), Refused::Running);
        drop(first);
        let again = acquire().expect("free again once dropped");
        drop(again);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn squatted_names_do_not_block_it() {
        // What a standard user can make: the old global mutex, and a namespace
        // of the same name with no SID in its boundary.
        let old = wide(r"Global\Atlas-Agent");
        // SAFETY: NUL-terminated; closed by `Owned`.
        let _old = Owned(unsafe { CreateMutexW(None, false, PCWSTR(old.as_ptr())) }.unwrap());
        let name = wide(NAMESPACE);
        // SAFETY: NUL-terminated; deleted by `Boundary`.
        let bare = Boundary(unsafe { CreateBoundaryDescriptorW(PCWSTR(name.as_ptr()), 0) });
        let alias = wide("Squat");
        // SAFETY: a valid boundary; closed by `Namespace`.
        let squat = Namespace(unsafe { CreatePrivateNamespaceW(None, bare.0.0, PCWSTR(alias.as_ptr())) });
        // SAFETY: no arguments; read at once after the call above.
        let last = unsafe { GetLastError() };
        assert!(!squat.0.0.is_null(), "the squat namespace: {last:?}");
        let inner = wide(r"Squat\Agent");
        // SAFETY: NUL-terminated; closed by `Owned`.
        let _inner = Owned(unsafe { CreateMutexW(None, false, PCWSTR(inner.as_ptr())) }.unwrap());

        let instance = acquire().expect("squats do not block the lock");
        drop(instance);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_token_without_administrators_is_refused_held_or_not() {
        // Nothing held: the create is refused.
        assert_eq!(without_administrators(acquire).unwrap_err(), Refused::Denied);
        // Held: still refused at the create, not reported as another instance.
        let held = acquire().expect("elevated");
        assert_eq!(without_administrators(|| acquire_as("Second")).unwrap_err(), Refused::Denied);
        drop(held);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_lock_held_under_another_dacl_is_another_instance() {
        // The namespace, and the mutex with a DACL this token is not in.
        let ns = namespace(NAMESPACE).expect("elevated");
        let sd = SecurityDescriptor::from_sddl("D:P(A;;GA;;;SY)").unwrap();
        let sa = attributes(&sd);
        let name = wide(MUTEX);
        // SAFETY: NUL-terminated; `sa` outlives the call; closed by `Owned`.
        let _other = Owned(unsafe { CreateMutexW(Some(&sa), false, PCWSTR(name.as_ptr())) }.unwrap());
        assert_eq!(acquire_as("Second").unwrap_err(), Refused::Running);
        drop(ns);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn the_namespace_and_the_mutex_grant_system_and_administrators_only() {
        let held = acquire().expect("elevated");
        for (what, h) in [("namespace", held._namespace.0), ("mutex", held._mutex.raw())] {
            let d = dacl(h);
            let aces: Vec<&str> = d.split('(').skip(1).collect();
            assert!(d.starts_with("D:P"), "{what} DACL not protected: {d}");
            assert_eq!(aces.len(), 2, "{what} DACL: {d}");
            assert!(aces.iter().any(|a| a.ends_with(";;;SY)")), "{what} DACL: {d}");
            assert!(aces.iter().any(|a| a.ends_with(";;;BA)")), "{what} DACL: {d}");
        }
    }
}
```

- [ ] **Step 4: `run`** (D5.2, D5.9; §11.3, §11.4).
  - **Start:** an elevated token, else exit 1 → the single-instance lock (Step 3; exit 1 when refused) → `datadir::prepare` → `agent.toml` → the log (console and `logs\`, daily, seven files, at the configured level) → the buffer → the console handler → the sensor with the store's sink → the health thread. The handler comes before the sessions start, so Ctrl+C during start-up waits in the stop channel and the clean stop follows at once (review finding 6).
  - **The wait:** every second it checks for a stop request, whether the pipeline thread still runs and whether the writer thread still runs; if either ended, it stops and exits 2 (review finding 7).
  - **The console handler** turns Ctrl+C, Ctrl+Break and a console close into a stop request, sent to the latest `run`'s channel (each `run` replaces it; the OS handler is installed once). For a close it waits up to 4.5 s for `STOPPED`, which `run` sets after the log file's writer is flushed, because Windows ends the process about 5 s after the event.
  - **Clean stop:** the health thread → `Running::stop` → a panicked pipeline's message to the log → the final snapshot and report (without the pipeline's counters if it died) → `Store::close`, logged with the records written and how long the stop took, or the writer's panic as an error.
  - **Test seams:** `run_with_sink` takes the function that makes the pipeline's sink from the buffer's; `RunOptions::store_panics_after` exists only in test builds. Elevated tests: a sink that panics on its 21st event gives exit 2, the 20 events on disk and the final report last, without the pipeline's counters; a writer that panics after 20 appends gives exit 2 within 60 s.

`crates/atlas-agent/src/win/run.rs`:
```rust
//! `atlas-agent run` (sensor spec §11.3, §11.4; plan 1b-4a): the sensor in a
//! console, until Ctrl+C, Ctrl+Break or the console closes, then a clean stop.
//!
//! Start: an elevated token → the single-instance lock → the data directory
//! (created or verified, §11.1) → `agent.toml` → the diagnostic log → the
//! buffer → the sensor ([`agent::start`]) with the buffer writer as its sink →
//! the health thread.
//!
//! Clean stop: the health thread → the sensor ([`agent::Running::stop`]) →
//! the final Sensor Health report → the buffer, flushed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use atlas_schema::{Device, Event};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::BOOL;

use super::agent::{self, AgentOptions};
use super::datadir;
use super::health::{self, HealthConfig, HealthThread, Interval, Reporting};
use super::instance;
use super::util::Owned;
use crate::config::Config;
use crate::driver::DriverConfig;
use crate::health::Snapshot;
use crate::settings::{LogLevel, Settings};
use crate::store::{Sink, Store, StoreConfig};

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub data_dir: PathBuf,
    pub sensor_session: String,
    pub process_session: String,
    pub health: HealthConfig,
    /// Tests: the buffer writer thread panics after this many appends.
    #[cfg(test)]
    pub store_panics_after: Option<u64>,
}

impl RunOptions {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        RunOptions {
            data_dir: data_dir.into(),
            sensor_session: "Atlas-Sensor".into(),
            process_session: "Atlas-Process".into(),
            health: HealthConfig::default(),
            #[cfg(test)]
            store_panics_after: None,
        }
    }
}

/// Exit codes.
pub const EXIT_OK: i32 = 0;
/// Start-up was refused (not elevated, another instance, the directory, the
/// settings, the buffer, the sessions).
pub const EXIT_START: i32 = 1;
/// The pipeline or the buffer writer thread ended while running.
pub const EXIT_PIPELINE: i32 = 2;

/// Runs until stopped; returns the process exit code.
pub fn run(opts: &RunOptions) -> i32 {
    if !elevated() {
        eprintln!("atlas-agent run needs an elevated (administrator) prompt");
        return EXIT_START;
    }
    let _instance = match instance::acquire() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("atlas-agent: {e}");
            return EXIT_START;
        }
    };
    if let Err(e) = datadir::prepare(&opts.data_dir) {
        eprintln!("atlas-agent: {e}");
        return EXIT_START;
    }
    let settings = Settings::load(&opts.data_dir.join("agent.toml"));
    let level = settings.as_ref().map_or(LogLevel::Info, |s| s.log_level);
    let _log = init_logging(&opts.data_dir.join("logs"), level);
    let settings = match settings {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("agent.toml refused: {e}");
            return EXIT_START;
        }
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), data_dir = %opts.data_dir.display(), "atlas-agent starting");
    let code = run_sensor(opts, &settings);
    tracing::info!(code, "atlas-agent stopped");
    // The log file is written last; only then may a console close end the
    // process (it waits for `STOPPED`).
    drop(_log);
    STOPPED.store(true, Ordering::SeqCst);
    code
}

fn run_sensor(opts: &RunOptions, settings: &Settings) -> i32 {
    run_with_sink(opts, settings, |sink| move |e| sink.event(e))
}

/// [`run_sensor`] with the pipeline's sink made from the buffer's by `sink`
/// (tests make it fail).
fn run_with_sink<S>(opts: &RunOptions, settings: &Settings, sink: impl FnOnce(Sink) -> S) -> i32
where
    S: FnMut(Event) + Send + 'static,
{
    let buffer_dir = opts.data_dir.join("buffer");
    let buffer = atlas_buffer::Config {
        cap_bytes: settings.buffer.cap_bytes,
        overflow: settings.buffer.overflow,
        ..atlas_buffer::Config::new(&buffer_dir)
    };
    #[allow(unused_mut)]
    let mut store_cfg = StoreConfig::new(buffer);
    #[cfg(test)]
    {
        store_cfg.panic_after = opts.store_panics_after;
    }
    let (store, recovery) = match Store::start(store_cfg) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("the buffer could not be opened: {e}");
            return EXIT_START;
        }
    };
    if recovery != atlas_buffer::Recovery::default() {
        tracing::warn!(?recovery, "the buffer was repaired at open");
    }

    let sink = sink(store.sink());
    let agent_opts = AgentOptions {
        sensor_session: opts.sensor_session.clone(),
        process_session: opts.process_session.clone(),
        config: settings.config.clone(),
        services: settings.services.clone(),
        driver: DriverConfig::default(),
        device_file: opts.data_dir.join("device.json"),
    };
    // Before the sessions start: Ctrl+C during start-up must not end the
    // process with them running. A stop asked for meanwhile waits in `stop`.
    let stop = install_ctrl_handler();
    tracing::info!("starting the sensor");
    let running = match agent::start(agent_opts, sink) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("the sensor could not start: {e}");
            let _ = store.close();
            return EXIT_START;
        }
    };
    let started = running.started().clone();
    if let Some(kusd) = started.boot_id_disagreement {
        tracing::warn!(kusd, "the boot id in KUSER_SHARED_DATA disagreed with the kernel's; the kernel's is used");
    }
    log_settings(&running.config().clone(), running.seeding());

    let seeding = running.seeding();
    let reporting = Reporting {
        to: store.health(),
        device: Device { uid: started.identity.device, boot_id: started.identity.boot },
        buffer_drops: store.drops(),
    };
    let first = Interval { from: Snapshot::at_start(seeding), start: started.anchor.unix_ns };
    let health = HealthThread::start(running.health_sources(), reporting.clone(), first, opts.health);

    tracing::info!("running; Ctrl+C stops");
    let mut code = EXIT_OK;
    loop {
        match stop.recv_timeout(Duration::from_secs(1)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) if !running.pipeline_running() => {
                tracing::error!("the pipeline thread ended; stopping");
                code = EXIT_PIPELINE;
                break;
            }
            Err(RecvTimeoutError::Timeout) if !store.is_running() => {
                tracing::error!("the buffer writer thread ended; stopping");
                code = EXIT_PIPELINE;
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }

    tracing::info!("stopping");
    let t0 = Instant::now();
    let at = health.stop();
    let drops = store.drops();
    let stopped = running.stop();
    if let Err(panic) = &stopped.pipeline {
        // The default hook printed it to stderr only; a service has none.
        tracing::error!("the pipeline thread panicked: {panic}");
    }
    let last = health::final_snapshot(&stopped, &drops, seeding);
    health::send(&reporting, at, &last);
    drop(reporting);
    match store.close() {
        Ok(stats) => tracing::info!(
            records = stats.records_written,
            bytes = stats.bytes_written,
            stop_ms = t0.elapsed().as_millis() as u64,
            "buffer closed"
        ),
        Err(e) => tracing::error!("the buffer could not be flushed at stop: {e}"),
    }
    code
}

fn log_settings(c: &Config, seeding: bool) {
    tracing::info!(
        hold_ms = c.hold.as_millis() as u64,
        udp = c.network_udp,
        op_end = c.file_op_end,
        value_reads = c.registry_value_reads,
        seeding,
        "sensor started"
    );
}

/// The diagnostic log (§11.4): the console, and `logs\` rolled daily, seven
/// files kept. The guard flushes the file at the end of `run`.
fn init_logging(dir: &Path, level: LogLevel) -> Option<WorkerGuard> {
    let level = match level {
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    };
    let console = fmt::layer().with_writer(std::io::stderr).with_target(false);
    let file = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("atlas-agent")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir);
    let (file_layer, guard) = match file {
        Ok(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            (Some(fmt::layer().with_writer(writer).with_ansi(false).with_target(false)), Some(guard))
        }
        Err(e) => {
            eprintln!("atlas-agent: the log file in {} cannot be opened: {e}", dir.display());
            (None, None)
        }
    };
    let _ = tracing_subscriber::registry().with(level).with(console).with(file_layer).try_init();
    guard
}

fn elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: opens our own token for query; closed by `Owned`.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let token = Owned(token);
    let mut e = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `e` is the structure TokenElevation returns, of the size given.
    let ok = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            Some((&raw mut e).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    ok.is_ok() && e.TokenIsElevated != 0
}

/// Where the console handler sends a stop: the latest `run`'s channel.
static STOP: Mutex<Option<Sender<()>>> = Mutex::new(None);
static HANDLER: Once = Once::new();
/// Set once the clean stop is done and the log file written: a console
/// close waits for it.
static STOPPED: AtomicBool = AtomicBool::new(false);
/// Windows ends the process about 5 s after a close event.
const CLOSE_WAIT: Duration = Duration::from_millis(4_500);

/// Ctrl+C, Ctrl+Break and closing the console all ask for the clean stop.
fn install_ctrl_handler() -> std::sync::mpsc::Receiver<()> {
    let (tx, rx) = channel();
    // A later `run` in one process (tests) replaces the channel.
    *STOP.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    STOPPED.store(false, Ordering::SeqCst);
    HANDLER.call_once(|| {
        // SAFETY: `on_ctrl` is a valid handler for the life of the process.
        if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(on_ctrl), true) } {
            tracing::warn!("the console handler could not be installed: {e}");
        }
    });
    rx
}

unsafe extern "system" fn on_ctrl(kind: u32) -> BOOL {
    match kind {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
            if let Some(tx) = STOP.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                let _ = tx.send(());
            }
            if kind == CTRL_CLOSE_EVENT {
                // Returning ends the process: wait for the clean stop.
                let t0 = Instant::now();
                while !STOPPED.load(Ordering::SeqCst) && t0.elapsed() < CLOSE_WAIT {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            true.into()
        }
        _ => false.into(),
    }
}

#[cfg(test)]
mod tests {
    use atlas_buffer::{Cursor, Reader};
    use atlas_schema::EventKind;
    use atlas_schema::classes::sensor_health::SensorHealthAction;

    use super::*;

    /// Removes the test's data directory, also on failure.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_pipeline_that_dies_still_gets_its_final_report_and_a_flushed_buffer() {
        const WRITTEN: u64 = 20;
        let pid = std::process::id();
        let dir = Scratch(std::env::temp_dir().join(format!("atlas-run-dies-{pid}")));
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).unwrap();
        let mut opts = RunOptions::new(&dir.0);
        opts.sensor_session = format!("Atlas-Test-Run-A-{pid}");
        opts.process_session = format!("Atlas-Test-Run-B-{pid}");

        // The start-up seeding alone makes far more than WRITTEN events.
        let code = run_with_sink(&opts, &Settings::default(), |sink| {
            let mut n = 0;
            move |e| {
                n += 1;
                assert!(n <= WRITTEN, "test: the sink fails on event {n}");
                sink.event(e);
            }
        });
        assert_eq!(code, EXIT_PIPELINE);

        let mut reader = Reader::open(&dir.0.join("buffer"), Cursor::default());
        let mut events = 0;
        let mut last = None;
        while let Some(rec) = reader.next_record().unwrap() {
            let e = atlas_schema::decode_event(&rec.payload).expect("a valid record");
            if !matches!(e.kind, EventKind::SensorHealth(_)) {
                events += 1;
            }
            last = Some(e);
        }
        assert_eq!(events, WRITTEN, "every event the sink took was flushed");
        let Some(EventKind::SensorHealth(health)) = last.map(|e| e.kind) else {
            panic!("the last record is not the final Sensor Health report");
        };
        let SensorHealthAction::Report(report) = health.action;
        // The pipeline's counters died with it: absent, not made up.
        assert_eq!(report.quality.late_arrivals, None);
    }

    #[test]
    #[ignore = "needs administrator rights; CI's agent-live job runs it"]
    fn a_buffer_writer_that_dies_stops_the_agent() {
        let pid = std::process::id();
        let dir = Scratch(std::env::temp_dir().join(format!("atlas-run-writer-dies-{pid}")));
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).unwrap();
        let mut opts = RunOptions::new(&dir.0);
        opts.sensor_session = format!("Atlas-Test-Run-C-{pid}");
        opts.process_session = format!("Atlas-Test-Run-D-{pid}");
        opts.store_panics_after = Some(20);

        let (tx, rx) = channel();
        let runner = std::thread::spawn(move || {
            let _ = tx.send(run_with_sink(&opts, &Settings::default(), |sink| move |e| sink.event(e)));
        });
        let code = rx.recv_timeout(Duration::from_secs(60)).ok();
        if code.is_none() {
            // Never noticed: stop it, so the test ends.
            if let Some(tx) = STOP.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                let _ = tx.send(());
            }
        }
        runner.join().unwrap();
        assert_eq!(code, Some(EXIT_PIPELINE), "the writer's end was noticed within 60 s");
    }
}
```

```diff
--- a/crates/atlas-agent/src/win/mod.rs
+++ b/crates/atlas-agent/src/win/mod.rs
@@ -18,9 +18,12 @@ pub mod datadir;
 mod expand;
 mod handles;
 mod hash;
+pub mod health;
 pub mod identity;
+pub mod instance;
 pub mod lookups;
 pub mod privilege;
+pub mod run;
 mod seeder;
 mod services;
 mod telemetry;
```

- [ ] **Step 5: The command line.** `clap` derive: `run [--data-dir]` and `dump [--data-dir] [--follow] [--from] [--class]`. `dump` prints its summary on stderr when it reaches the end; with `--follow` there is no end, and Ctrl+C ends it without one (it installs no handler). On other platforms `run` says it needs Windows; `dump` works anywhere.

`crates/atlas-agent/src/main.rs`:
```rust
//! The `atlas-agent` command line (sensor spec §11.3). Plan 1b-4a has `run`
//! and `dump`; 1b-4b adds `canary`, 1b-4c `service`.

use std::path::PathBuf;
use std::process::ExitCode;

use atlas_agent::dump::{self, ClassName, DumpOptions};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "atlas-agent", version, about = "The Atlas endpoint agent")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// `C:\ProgramData\Atlas` (§11.1).
const DATA_DIR: &str = r"C:\ProgramData\Atlas";

#[derive(Subcommand)]
enum Command {
    /// Runs the sensor in this console until Ctrl+C (administrator required).
    Run {
        /// The data directory: created protected, or verified (§11.1).
        #[arg(long, default_value = DATA_DIR)]
        data_dir: PathBuf,
    },
    /// Prints the buffer as JSON lines, one event per line. Never acks.
    Dump {
        #[arg(long, default_value = DATA_DIR)]
        data_dir: PathBuf,
        /// Keeps printing new events until Ctrl+C.
        #[arg(long)]
        follow: bool,
        /// Starts after this position, as `<segment:offset>`.
        #[arg(long, value_parser = dump::parse_cursor)]
        from: Option<atlas_buffer::Cursor>,
        /// Only this class: process, module, network, file, registry_key,
        /// registry_value, dns, event_log, sensor_health.
        #[arg(long)]
        class: Option<ClassName>,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Run { data_dir } => run(data_dir),
        Command::Dump { data_dir, follow, from, class } => {
            let opts = DumpOptions { from: from.unwrap_or_default(), class, follow };
            let out = std::io::stdout();
            match dump::dump(&data_dir.join("buffer"), &opts, &mut out.lock(), || false) {
                Ok(s) => {
                    eprintln!(
                        "{} events; {} invalid records skipped; {} corrupt segments; next: {}:{}",
                        s.printed, s.invalid, s.corrupt_segments, s.next.segment, s.next.offset
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("atlas-agent dump: {}: {e}", data_dir.display());
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        eprintln!("the data directory is readable by administrators only");
                    }
                    ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(windows)]
fn run(data_dir: PathBuf) -> ExitCode {
    let code = atlas_agent::win::run::run(&atlas_agent::win::run::RunOptions::new(data_dir));
    ExitCode::from(code as u8)
}

#[cfg(not(windows))]
fn run(_: PathBuf) -> ExitCode {
    eprintln!("atlas-agent run needs Windows");
    ExitCode::FAILURE
}
```

- [ ] **Step 6: Check and commit**

```powershell
cargo test -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p atlas-agent -- --help
cargo run -p atlas-agent -- run
```
Expected: all pass; the help lists `run` and `dump`; an unelevated `run` prints that it needs an elevated prompt and exits 1. Workspace: 402 passed, 17 ignored (`atlas-agent` unit tests: 217).
```powershell
git add crates
git commit -m "feat(agent): atlas-agent run and dump, the health thread"
```

### Task 9: The binary-level live test, and the CI job

**Files:**
- Create: `crates/atlas-agent/tests/binary.rs`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: The test** (D4). One `#[ignore]`d test, `binary_run_and_dump`, run as administrator. The same file is also the actor, started with `ATLAS_BINARY_ACTOR` set.
  - **Set-up:** the data directory `C:\ProgramData\Atlas-Test-binary-<pid>` (the folders above a `%TEMP%` one would be refused, Task 7); the actor's files stay under `%TEMP%`, in its long form (F3). A guard stops the agent if it still runs, stops the test sessions and deletes the directories, also on failure.
  - **Start:** `atlas-agent run` in a new process group (F6), its output drained by two threads (F7). A second `run` must be refused.
  - **The actor**, once the log says the agent runs: a file created, renamed and deleted; a volatile `HKCU` key with a value, deleted; a loopback TCP connection; lookups of `example.com` and a `.invalid` name; a `cmd /c exit 3` child.
  - **Stop:** once the first periodic report is logged (about 60 s after the start; the executable has no shorter interval), Ctrl+Break, then a clean exit within 30 s. The test then checks that the sessions are gone.
  - **Checks:** the ones listed in the Review Focus (item 9), plus the actor's classes (file create, rename, delete; registry key and value; TCP open; DNS; the child's Launch and its exit status), nothing attributed to the agent itself, and that a directory with a loose ACL is refused before anything is written into it. Review finding 8 made three stronger: `dump`'s lines are compared with the writer's own `records=` in the log file, not only with a second read of the same disk; the last record must be the final report, timed after the Ctrl+Break; the first report's interval must start between the agent's launch and its "running" line.
  - **After the main run:** a `--data-dir` below `%TEMP%` is refused, naming the user's folder, and not created; and a second `run` gets Ctrl+Break as soon as it logs "starting the sensor": it must exit 0 with no session left and a final report in its buffer (review finding 6).
  - With `ATLAS_AGENT_REPORT` set, it writes its checks and figures as JSON (CI uploads it).

`crates/atlas-agent/tests/binary.rs`:
```rust
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
    let periodic = wait_for(&lines, "health report", Duration::from_secs(75));
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
```

- [ ] **Step 2: CI runs it** in the `agent-live` job, after the live test, even if that one failed:

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -109,6 +109,14 @@ jobs:
         run: |
           mkdir -p "$RUNNER_TEMP/agent"
           cargo test -p atlas-agent --test live -- --ignored --test-threads=1 --nocapture
+      - name: Binary live test
+        if: ${{ !cancelled() }}
+        shell: bash
+        env:
+          ATLAS_AGENT_REPORT: ${{ runner.temp }}/agent/binary-report.json
+        run: |
+          mkdir -p "$RUNNER_TEMP/agent"
+          cargo test -p atlas-agent --test binary -- --ignored --test-threads=1 --nocapture
       - name: Upload the report
         if: always()
         uses: actions/upload-artifact@v7
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --test binary
cargo clippy --workspace --all-targets -- -D warnings
actionlint .github/workflows/ci.yml
```
Expected: the test is ignored; clippy and `actionlint` are clean. Workspace: 402 passed, 18 ignored (`atlas-agent` unit tests: 217).
```powershell
git add crates/atlas-agent .github
git commit -m "test(agent): the binary-level live test, in CI's agent-live job"
```

### Task 10: The host runs (the user)

Two runs, both from scripts in the git-ignored `spikes/`, both by the user in an elevated PowerShell. Claude writes the scripts, checks that they parse in `powershell.exe` and `pwsh`, and reads the results.

- [ ] **Step 1: The elevated tests.** About 8 minutes the first time (a debug build), then about 4. It changes nothing lasting:
  - ETW sessions `Atlas-Test-Agent-*`, and for about two minutes the real names `Atlas-Sensor` and `Atlas-Process` (the binary test); all stopped at the end, also on failure;
  - protected test data directories `C:\ProgramData\Atlas-Test-*`, deleted by the tests (and any left over deleted by the script at the end);
  - test ETW sessions `Atlas-Test-Run-*` (the `run` tests), stopped at the end;
  - volatile `HKCU` test keys, `%TEMP%` test directories, short-lived `cmd.exe` children, loopback TCP, lookups of `example.com` and a `.invalid` name;
  - build output in the checkout.

  It runs atlas-agent's elevated unit tests (the services, the directory, the lock and the `run` tests), the agent live test, and the binary test. It runs from the repo by default; `-wt` names another checkout. `-mutations` names revert lists (the verification's reverts), `-mutationsOnly` runs only those.

`spikes/run-1b4a-elevated.ps1`:
```powershell
# Plan 1b-4a: the elevated tests, in an elevated shell, from the repo (or
# the checkout given as -wt).
# Runs on the HOST, about 8 minutes the first time (a debug build in the
# checkout), then about 4. Changes nothing lasting:
#   - ETW sessions: Atlas-Test-Agent-* (the 1b-3c test), and for about two
#     minutes the real names Atlas-Sensor and Atlas-Process (the binary test);
#     all stopped at the end, also on failure;
#   - protected test data directories C:\ProgramData\Atlas-Test-* (deleted
#     by the tests, and any left over deleted at the end);
#   - volatile HKCU test keys, %TEMP% test directories, short-lived cmd.exe
#     children, loopback TCP, DNS lookups of example.com and a .invalid name;
#   - build output in the worktree's target directory.
# 1. atlas-agent's elevated lib tests: the services (1b-3b) and the data
#    directory (1b-4a: created protected; loose, null, open and wrongly owned
#    directories refused);
# 2. the agent live test (1b-3c), with the file.op_end change;
# 3. the binary live test (1b-4a): atlas-agent run and dump end to end;
# 4. with -mutations: each listed file's fixes reverted one at a time
#    (spikes\1b4a-plan-src\mutate.py); each named test must then fail. The
#    source is restored after each one, also on failure. A bare name (m5)
#    means spikes\1b4a-plan-src\m5.json. -mutationsOnly skips 1 to 3.
# Run from an elevated PowerShell (Windows PowerShell 5.1 or pwsh 7):
#   & C:\Users\jakef\Desktop\atlas-edr\spikes\run-1b4a-elevated.ps1
# (verify-first, from the scratch worktree: add -wt C:\Users\jakef\Desktop\atlas-wt-1b4a)
param([string]$wt = 'C:\Users\jakef\Desktop\atlas-edr', [string[]]$mutations = @(), [switch]$mutationsOnly)

$ErrorActionPreference = 'Stop'
try { Stop-Transcript | Out-Null } catch { }

$out = 'C:\Users\jakef\Desktop\atlas-edr\spikes\results\1b4a-elevated'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
New-Item -ItemType Directory -Force -Path $out | Out-Null
Start-Transcript -Path (Join-Path $out "transcript-$stamp.txt") | Out-Null

function Invoke-Cargo([string]$name, [string[]]$cargoArgs, [string]$report) {
    $log = Join-Path $out "$name-$stamp.txt"
    Write-Host "== ${name}: cargo $($cargoArgs -join ' ')"
    if ($report) { $env:ATLAS_AGENT_REPORT = $report } else { Remove-Item Env:ATLAS_AGENT_REPORT -ErrorAction SilentlyContinue }
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
    if (Get-Process atlas-agent -ErrorAction SilentlyContinue) { throw 'An atlas-agent process is running: stop it first (Ctrl+C in its window).' }
    Write-Host "Windows build: $([Environment]::OSVersion.Version)"
    Set-Location $wt
    if (-not $mutationsOnly) {
        $results += Invoke-Cargo 'agent-elevated' @('test', '-p', 'atlas-agent', '--lib', '--', '--include-ignored', '--test-threads=1') ''
        $results += Invoke-Cargo 'agent-live' @('test', '-p', 'atlas-agent', '--test', 'live', '--', '--ignored', '--test-threads=1', '--nocapture') (Join-Path $out "agent-report-$stamp.json")
        $results += Invoke-Cargo 'binary-live' @('test', '-p', 'atlas-agent', '--test', 'binary', '--', '--ignored', '--test-threads=1', '--nocapture') (Join-Path $out "binary-report-$stamp.json")
    }
    foreach ($m in $mutations) {
        if (-not [IO.Path]::IsPathRooted($m)) { $m = Join-Path 'C:\Users\jakef\Desktop\atlas-edr\spikes\1b4a-plan-src' "$m.json" }
        if (-not (Test-Path $m -PathType Leaf)) { $results += "FAIL mutations: no file $m"; continue }
        $name = 'mutate-' + [IO.Path]::GetFileNameWithoutExtension($m)
        $log = Join-Path $out "$name-$stamp.txt"
        Write-Host "== ${name}: $m"
        $ErrorActionPreference = 'Continue'
        & python -I 'C:\Users\jakef\Desktop\atlas-edr\spikes\1b4a-plan-src\mutate.py' $m 2>&1 | ForEach-Object { $line = "$_"; Write-Host $line; Add-Content -Path $log -Value $line -Encoding UTF8 }
        $code = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        $hit = @(Select-String -Path $log -Pattern '^ok ').Count
        $miss = @(Select-String -Path $log -Pattern '^MISS ').Count
        $results += $(if ($code -eq 0) { "PASS ${name}: $hit reverts caught" } else { "FAIL ${name}: exit $code, $hit caught, $miss missed" })
    }
}
catch {
    $results += "FAIL: $_"
}
finally {
    Remove-Item Env:ATLAS_AGENT_REPORT -ErrorAction SilentlyContinue
    $sessions = @(logman query -ets)
    foreach ($s in $sessions | Select-String -Pattern 'Atlas-Test-\S+' -AllMatches) {
        foreach ($m in $s.Matches) { logman stop $m.Value -ets | Out-Null; "stopped leftover session $($m.Value)" }
    }
    if (-not (Get-Process atlas-agent -ErrorAction SilentlyContinue)) {
        foreach ($name in 'Atlas-Sensor', 'Atlas-Process') {
            if ($sessions | Select-String -SimpleMatch $name) { logman stop $name -ets | Out-Null; "stopped leftover session $name" }
        }
    }
    foreach ($d in Get-ChildItem 'C:\ProgramData' -Directory -Filter 'Atlas-Test-*' -ErrorAction SilentlyContinue) {
        cmd /c rmdir /s /q "$($d.FullName)"
        "removed leftover directory $($d.FullName)"
    }
    foreach ($r in $results) { Write-Host $r -ForegroundColor $(if ($r -like 'PASS*') { 'Green' } else { 'Red' }) }
    Stop-Transcript | Out-Null
    Write-Host "Results in $out"
}
```

Expected: three `PASS` lines (`agent-elevated`, `agent-live`, `binary-live`).

- [ ] **Step 2: About an hour of `run` during normal use** (D4). First `cargo build --release -p atlas-agent`. Before handing it over, Claude tells the user that it runs on their main machine and what it changes there:
  - it creates `C:\ProgramData\Atlas` (SYSTEM and Administrators only) with `buffer\`, `logs\` and `device.json`, which stay for 1b-4b and 1b-4c; removing it needs an elevated prompt;
  - it writes roughly 150–235 MB of buffer per hour (capped at 1 GiB);
  - while it runs, it holds the ETW sessions `Atlas-Sensor` and `Atlas-Process` and the single-instance lock.

  The user presses Ctrl+C once after about an hour. The script then summarises the run; if no summary prints, the user runs the summary script alone (it only reads).

`spikes/run-1b4a-hour.ps1`:
```powershell
# Plan 1b-4a: about an hour of atlas-agent run on the HOST during normal use,
# then a summary (plan 1b-4a, D4). Elevated shell; Windows PowerShell 5.1 or pwsh 7.
#   & C:\Users\jakef\Desktop\atlas-edr\spikes\run-1b4a-hour.ps1
# The agent runs in this window and logs here. Use the machine as usual; after
# about an hour press Ctrl+C ONCE in this window. The agent stops cleanly, then
# this script summarises the run (summarize-1b4a.ps1).
# What it changes on the host:
#   - creates C:\ProgramData\Atlas (owner Administrators; only SYSTEM and
#     Administrators have access) with buffer\, logs\ and device.json. It stays
#     afterwards (1b-4b and 1b-4c use it); removing it needs an elevated prompt;
#   - writes roughly 150-235 MB of buffer per hour (capped at 1 GiB);
#   - runs the ETW sessions Atlas-Sensor and Atlas-Process while it runs;
#   - takes the mutex Global\Atlas-Agent while it runs.
# Build it first: cargo build --release -p atlas-agent (in the repo, or pass -exe).
param([string]$exe = 'C:\Users\jakef\Desktop\atlas-edr\target\release\atlas-agent.exe')

$ErrorActionPreference = 'Stop'
try { Stop-Transcript | Out-Null } catch { }
$out = 'C:\Users\jakef\Desktop\atlas-edr\spikes\results\1b4a-hour'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
New-Item -ItemType Directory -Force -Path $out | Out-Null

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Not elevated: open PowerShell with "Run as administrator" and run this line again.'
}
if (-not (Test-Path $exe)) { throw "Agent not built: $exe" }
if (Get-Process atlas-agent -ErrorAction SilentlyContinue) { throw 'An atlas-agent process is already running.' }

$started = Get-Date
Write-Host "Started $started. Use the machine normally; after about an hour press Ctrl+C once." -ForegroundColor Cyan
$code = $null
try {
    # The agent writes its log to this console and to C:\ProgramData\Atlas\logs.
    & $exe run
    $code = $LASTEXITCODE
}
finally {
    $minutes = [math]::Round(((Get-Date) - $started).TotalMinutes, 1)
    # On 2026-10-12 the summary step never ran after the note was written
    # (cause not reproduced; the note no longer goes through the pipeline).
    # If no summary prints, run summarize-1b4a.ps1 alone in an elevated window.
    $note = "agent exit code: $code after $minutes minutes"
    Write-Host $note
    Set-Content -Path (Join-Path $out "run-$stamp.txt") -Value $note
    & (Join-Path $PSScriptRoot 'summarize-1b4a.ps1') -exe $exe -out $out
}
```

`spikes/summarize-1b4a.ps1`:
```powershell
# Plan 1b-4a: summarises the newest run of atlas-agent in a data directory,
# from its Sensor Health records (atlas-agent dump) and its log. Read-only.
# run-1b4a-hour.ps1 calls it at the end; it can also be run alone, elevated:
#   & C:\Users\jakef\Desktop\atlas-edr\spikes\summarize-1b4a.ps1
param(
    [string]$exe = 'C:\Users\jakef\Desktop\atlas-edr\target\release\atlas-agent.exe',
    [string]$data = 'C:\ProgramData\Atlas',
    [string]$out = 'C:\Users\jakef\Desktop\atlas-edr\spikes\results\1b4a-hour'
)

$ErrorActionPreference = 'Stop'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
New-Item -ItemType Directory -Force -Path $out | Out-Null

# ---- the Sensor Health records ----
$jsonl = Join-Path $out "health-$stamp.jsonl"
$ErrorActionPreference = 'Continue'
& $exe dump --data-dir $data --class sensor_health 1> $jsonl 2> (Join-Path $out "dump-$stamp.txt")
$ErrorActionPreference = 'Stop'
$all = @(Get-Content $jsonl | Where-Object { $_ } | ForEach-Object { $_ | ConvertFrom-Json })
if ($all.Count -eq 0) { throw "No Sensor Health records in $data" }
# A run's first report carries what buffer recovery found (cursorReset): the
# newest run starts at the last one that has it.
$first = 0
for ($i = 0; $i -lt $all.Count; $i++) {
    if ($null -ne $all[$i].sensorHealth.report.buffer.cursorReset) { $first = $i }
}
$reports = $all[$first..($all.Count - 1)]

function Get-Field($obj, [string]$path) {
    $v = $obj
    foreach ($p in $path.Split('.')) { if ($null -eq $v) { return $null }; $v = $v.$p }
    return $v
}
function Get-Sum([string]$path) {
    $sum = [uint64]0
    $seen = $false
    foreach ($r in $reports) {
        $v = Get-Field $r.sensorHealth.report $path
        if ($null -ne $v) { $sum += [uint64]$v; $seen = $true }
    }
    if ($seen) { return $sum } else { return 'not measured' }
}
function Get-ClassSum([string]$path) {
    $sum = [uint64]0
    foreach ($r in $reports) { foreach ($c in @(Get-Field $r.sensorHealth.report $path)) { if ($c) { $sum += [uint64]$c.count } } }
    return $sum
}

$startNs = [int64]$reports[0].sensorHealth.intervalStart
$endNs = [int64]$reports[-1].time
$seconds = ($endNs - $startNs) / 1e9
$cpuNs = [double](Get-Sum 'resources.cpuTime')
$ws = @($reports | ForEach-Object { Get-Field $_.sensorHealth.report 'resources.workingSet' } | Where-Object { $null -ne $_ } | ForEach-Object { [uint64]$_ })
$wsMax = ($ws | Measure-Object -Maximum).Maximum
$wsLast = $ws[-1]

$summary = [ordered]@{
    reports = $reports.Count
    minutes = [math]::Round($seconds / 60, 1)
    loss = [ordered]@{
        sensor_session_events_lost = Get-Sum 'loss.sensorSessionEventsLost'
        process_session_events_lost = Get-Sum 'loss.processSessionEventsLost'
        sensor_session_buffers_lost = Get-Sum 'loss.sensorSessionBuffersLost'
        process_session_buffers_lost = Get-Sum 'loss.processSessionBuffersLost'
        kernel_queue_drops = Get-Sum 'loss.kernelQueueDrops'
        user_queue_drops = Get-Sum 'loss.userQueueDrops'
        buffer_backlog_drops = Get-Sum 'loss.bufferBacklogDrops'
        callback_panics = Get-Sum 'loss.callbackPanics'
        dns_rate_limit_drops = Get-Sum 'loss.dnsRateLimitDrops'
        actor_dropped = Get-ClassSum 'loss.actorDropped'
    }
    quality = [ordered]@{
        file_cleanup_unpaired = Get-Sum 'quality.fileCleanupUnpaired'
        file_delete_outcome_unknown = Get-Sum 'quality.fileDeleteOutcomeUnknown'
        late_arrivals = Get-Sum 'quality.lateArrivals'
        launch_join_miss = Get-Sum 'quality.launchJoinMiss'
        actor_unresolved = Get-ClassSum 'quality.actorUnresolved'
        unknown_file_object = Get-Sum 'quality.unknownFileObject'
        registry_unresolved = Get-Sum 'quality.registryUnresolved'
        value_read_failed = Get-Sum 'quality.valueReadFailed'
        enrichment_misses = Get-Sum 'quality.enrichmentMisses'
        parse_errors = Get-Sum 'quality.parseErrors'
    }
    housekeeping = [ordered]@{
        pending_overflow = Get-Sum 'housekeeping.pendingOverflow'
        service_queue_drops = Get-Sum 'housekeeping.serviceQueueDrops'
        retention_evictions = Get-Sum 'housekeeping.retentionEvictions'
        process_cache_evictions = Get-Sum 'housekeeping.processCacheEvictions'
        file_map_evictions = Get-Sum 'housekeeping.fileMapEvictions'
        key_map_evictions = Get-Sum 'housekeeping.keyMapEvictions'
    }
    resources = [ordered]@{
        cpu_percent_of_one_core = if ($seconds -gt 0) { [math]::Round(100 * $cpuNs / 1e9 / $seconds, 3) } else { 'n/a' }
        working_set_mb_max = [math]::Round($wsMax / 1MB, 1)
        working_set_mb_last = [math]::Round($wsLast / 1MB, 1)
        buffer_disk_mb_last = [math]::Round([double](Get-Field $reports[-1].sensorHealth.report 'buffer.diskBytes') / 1MB, 1)
    }
}

# ---- the log: the two unpaired-Cleanup parts, and the records written ----
$startUtc = [DateTimeOffset]::FromUnixTimeMilliseconds([int64]($startNs / 1e6)).UtcDateTime
$parts = [ordered]@{ cleanup_unpaired = [uint64]0; cleanup_outcome_late = [uint64]0; records_written = 'not found'; stop_ms = 'not found' }
foreach ($f in Get-ChildItem (Join-Path $data 'logs') -Filter '*.log' | Sort-Object LastWriteTime) {
    foreach ($line in Get-Content $f.FullName) {
        if ($line -notmatch '^(\S+)') { continue }
        $t = [datetime]::MinValue
        if (-not [datetime]::TryParse($Matches[1], [ref]$t)) { continue }
        if ($t.ToUniversalTime() -lt $startUtc.AddSeconds(-5)) { continue }
        if ($line -match 'health report') {
            if ($line -match 'cleanup_unpaired=(\d+)') { $parts.cleanup_unpaired += [uint64]$Matches[1] }
            if ($line -match 'cleanup_outcome_late=(\d+)') { $parts.cleanup_outcome_late += [uint64]$Matches[1] }
        }
        if ($line -match 'buffer closed.*records=(\d+)') { $parts.records_written = [uint64]$Matches[1] }
        if ($line -match 'buffer closed.*stop_ms=(\d+)') { $parts.stop_ms = [uint64]$Matches[1] }
    }
}
$summary.log = $parts

$text = $summary | ConvertTo-Json -Depth 4
$text | Set-Content -Path (Join-Path $out "summary-$stamp.json") -Encoding UTF8
Write-Host $text
Write-Host "Summary in $(Join-Path $out "summary-$stamp.json")"
```

Expected: zero loss of every kind, both unpaired-Cleanup parts reported, CPU and working set against §13. Claude reads `spikes\results\1b4a-hour\` and records the figures in the PR (verification: F4). Any loss is investigated before the PR merges; a budget overrun is recorded and handled as D6 says.

### Task 11: Documentation

- [ ] **Step 1: The spec, the schema reference and the widened comment.** In `docs/specs/2026-10-01-etw-sensor-design.md`:
  - the status line (its date is the build's merge day: change `2026-10-13` if it differs);
  - §3.2: clarifications 2 and 3;
  - §4.2: clarification 6;
  - §9.1: clarification 1 (1b-4b decides on Kernel-EventTracing);
  - §9.3: clarifications 4 and 5;
  - §10.3: clarification 2;
  - §11.1: clarification 7;
  - §11.2: clarification 8;
  - §11.3: clarification 9;
  - §11.4: clarification 10;
  - §12.3: clarification 11;
  - §13: clarification 12;
  - §15.3: a "Plan 1b-4a" addendum with F1–F5;
  - §17: a "Plan 1b-4a clarifications" paragraph, and a "Plan 1b-4a review fixes" one (clarification 13).

  Also:
  - `docs/schema-reference.md`: `loss` names the events the buffer could not take;
  - `sensor_health.proto` and `sensor_health.rs`: `buffer_backlog_drops`' comment covers the channel (no field changes);
  - `docs/architecture-overview.md`: roadmap row 1 ("plan 1b-4a done; 1b-4b next, after the profiling step", with a link to this plan), and a decision-log row for the build.

````diff
--- a/docs/specs/2026-10-01-etw-sensor-design.md
+++ b/docs/specs/2026-10-01-etw-sensor-design.md
@@ -1,6 +1,6 @@
 # Sub-project 1 — ETW Sensor Design (Agent Core)
 
-**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is next. Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
+**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13), and plan 1b-4b (blinding detection) is next, then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
 **Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
 **Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).
 
@@ -94,6 +94,7 @@ ETW (Session A: manifest providers; Session B: system logger, process events)
 [6] buffer writer thread → segment log (§8)
 
 [7] watchdog thread: session/provider checks, canaries, counters, restart (§9)
+      (plan 1b-4a: the counters and Sensor Health reports; 1b-4b adds the rest)
 ```
 
 **Order before state.** The stateful pipeline [3] must see events in timestamp order. Otherwise an ImageLoad that arrives before its ProcessStart misses the cache, and a Cleanup that arrives before its Write loses the Update. Stage [2] provides that order. Real-time ETW delivers per-CPU buffers at the session flush timer (250 ms, §4.1), so an event older than flush timer + scheduling slack has almost certainly arrived. The 750 ms hold is three flush periods. A late event (one older than stream time, the watermark; plan 1b-3a) is processed immediately, out of order, and counted. It is never dropped.
@@ -122,6 +123,8 @@ ETW (Session A: manifest providers; Session B: system logger, process events)
 - Sweeps over whole tables (UDP idle, cache retention) run once per second of stream time.
 - A pending event's request bookkeeping is freed when the event leaves [5], whether or not its reply came; a later reply is ignored. Each request gets at most one reply.
 
+**The sink and the health thread** (plan 1b-4a, D1, D5). The sink moves each emitted event into a bounded channel (65,536 entries) to the buffer writer thread [6] and never blocks: a full channel drops the event and counts it in `loss.buffer_backlog_drops` (§10.3). [6] encodes and appends each event and calls the writer's `tick` about every 100 ms (at most about 200: a wait for the first record, then a drain as long; the writer flushes every second); `tick` may block on the disk while the channel absorbs events. Thread [7] is, for now, the health thread: every 5 s it takes a snapshot of every counter (the pipeline's through the driver's control channel, between two passes), and every 60 s, or at once when a loss counter rose, it sends [6] a Sensor Health report (§9.3). Plan 1b-4b adds the watchdog's checks to it.
+
 **Backpressure.** The ETW callback must stay fast: a slow consumer makes ETW drop events in the kernel. The callback parses, enqueues and keeps the small early registry map (a hash-map operation per CreateKey, OpenKey or CloseKey). Successful OperationEnds, about half of Kernel-File's volume, are discarded right there: they cost parse time but never reach [2]. The exceptions are a Cleanup's OperationEnd that reports a delete, and a delete request's (§5.1, plan 1b-3c): the callback remembers each Cleanup's and each request's `Irp` until its OperationEnd, one or two hash-map operations per Kernel-File event. Queues never block (kernel queue default 65,536 entries; user-mode queue 8,192); drops are counted per queue. User-mode providers get their own queue so a process flooding forged DNS-Client events (§4.4) cannot push kernel events out.
 
 ### 3.3 Time base
@@ -160,6 +163,8 @@ Each provider is enabled with the listed keywords and an `EVENT_FILTER_TYPE_EVEN
 
 `FILEIO` is needed because Cleanup, Close and SetInformation sit under it. `OP_END` adds one OperationEnd (24) per operation of the enabled keywords, roughly doubling Kernel-File's kernel-side volume. Session A's callback forwards only those with a failure status and discards the rest on arrival (§3.2), so the extra cost stops at parsing. `OpenKey` is the largest single source (S9: about 5 600/s average, 32 000/s peaks), and `CloseKey` adds about 1 100/s (S7). The key map needs both (§7.4). S10 measured the set without `OP_END` and `CloseKey`, so DoD item 4 re-checks the §13 budget with them.
 
+With `file.op_end` off (§11.2, the second §13 fallback), Kernel-File is enabled without 24 and `OP_END` (mask `0x1EA0`), and the callback pairs no Cleanup with an outcome, so none is counted unpaired (plan 1b-4a). A Delete then comes from its request (§5.5).
+
 ### 4.3 Parsers
 
 - One parser per (provider, event ID, version) that we consume. Pointer-sized fields take the pointer size from **each event's** header flags (32-bit processes log 32-bit pointers from user-mode providers). Kernel providers and Session B log with the kernel's pointer size even for WOW64 processes (S8). The field layouts found by the spikes are in §15.3.
@@ -471,7 +476,7 @@ The writer stops appending, keeps events in a bounded in-memory backlog, retries
 - **`ProcessTrace` returning** on either consumer thread also means Stop.
 - **Provider state:** each provider in Session A is still enabled with our keywords and level (`EnumerateTraceGuidsEx` / `TraceGuidQueryInfo`). Missing or changed → **Disable** (with the provider name), then re-enable.
   - The event-ID filter cannot be read back: `TraceGuidQueryInfo` returns level, keywords, enable property and LoggerId only, and ETW adds bits of its own (property 0x80 reads back as 0xC0). So the check is that our bits are present, not that the values are equal (plan 1b-2, F3).
-  - `Microsoft-Windows-Kernel-EventTracing` logs every enable (event 14) and disable (15) of a provider in our session, a filter change included, with the **caller's** PID and start key in the header (F2). It is the candidate for detecting a narrowed filter and naming who changed it; plan 1b-4 decides.
+  - `Microsoft-Windows-Kernel-EventTracing` logs every enable (event 14) and disable (15) of a provider in our session, a filter change included, with the **caller's** PID and start key in the header (F2). It is the candidate for detecting a narrowed filter and naming who changed it; plan 1b-4b decides.
 - **Counters:** session events-lost and real-time-buffers-lost, plus all pipeline counters, sampled for Sensor Health.
 
 ### 9.2 Canaries
@@ -493,6 +498,13 @@ Emitted every 60 s (and immediately on a non-zero change in a loss counter):
 - **Housekeeping:** cache, map and flow-table evictions, `retention_evictions`, `file_op_failed` (failed creates, deletes and renames dropped, §5.5), `pending_overflow` (§3.2), seeder results (handles named, failed, timed out; stuck helpers outstanding; negative-cache size; table reads; deferred re-reads; whether seeding is on).
 - **Self-measurement:** agent CPU time and working set for the interval (`GetProcessTimes`, `GetProcessMemoryInfo`), so long runs measure themselves (§13).
 
+**How a report is made** (plan 1b-4a, D1, D5):
+- **Snapshots and differences:** the health thread [7] reads every cumulative counter into a snapshot: the pipeline's (through the driver's control channel, waiting at most 2 s), the intake's and the services' atomics, the callbacks' panics, both sessions' losses (`ControlTraceW` query), the sink's drops, and the process's CPU time and working set. A report holds the differences between two snapshots. The sessions' counters are 32-bit, so their differences wrap. A source that could not be read is absent from the report, and the next report covers its interval too.
+- **When:** every 60 s; at once when a loss counter rose (checked every 5 s); and a final report at the clean stop, taken after the sessions stopped, so everything counted during the run is reported. The intervals chain: each starts where the last ended, the first at the anchor taken at start. A report's time is the system clock at generation (`GetSystemTimePreciseAsFileTime`, the clock the anchor reads, §3.3); its id is a UUIDv7. If the pipeline thread died, the final report is still sent, without the pipeline's counters (they died with it).
+- **The buffer's figures:** [6] fills in the `buffer` group, `loss.buffer_backlog_drops`, `housekeeping.retention_evictions` and the gap as it appends the report, each since the report before; so the drops of the events queued ahead of a report, the final drain's included, are in it (review finding 3). The health thread's own reading of the drops only decides an early report; the first report of a run carries what recovery found at open. A report the writer's backlog cannot take is held (up to 16) and written before anything else once there is room.
+- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums.
+- **Not measured yet** (absent): `quality.buffer_invalid_records` and `buffer.corrupt_segments` (the agent never reads its own buffer back; `dump` counts both), and `housekeeping.seeder_negative_cache_size` (the seeder exposes no size).
+
 ## 10. Schema Additions (`atlas.events.v1`, additive)
 
 All additions pass `buf breaking`, get golden fixtures, are documented in `docs/schema-reference.md`, and keep the 0a rules: no `Unknown`/`Other` activities emitted; unknown enums rejected. Two deliberate deviations from 0a are recorded in the decision log: classes without an actor (0a D5 says every event carries one), and an Atlas extension *class* (0a §5.9 reserved the `atlas` namespace for fields).
@@ -521,6 +533,8 @@ One activity, `Report`, carrying the §9.3 counters (`u64`, all optional), the i
 
 **Plan 1b-3b addition:** `housekeeping.service_queue_drops`, for requests dropped because a service lane was full (§3.2).
 
+**Plan 1b-4a:** no new field. `loss.buffer_backlog_drops` also counts events dropped because the channel to the buffer writer thread was full (§3.2).
+
 **Plan 1b-3c additions:** `quality.file_delete_outcome_unknown`, for Deletes reported from the request alone because the file system reported no Cleanup outcome, and `quality.file_cleanup_unpaired`, for Cleanups whose outcome could not be paired with them (§5.1).
 
 ### 10.4 Registry fields (additive, from S4 and S7)
@@ -550,25 +564,37 @@ Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9
 - **Data:** `C:\ProgramData\Atlas\` holds `buffer\` (segments and the `cursor` file), `canary\`, `agent.toml`, `device.json`, and `logs\`. `C:\ProgramData` lets ordinary users create folders, so a non-admin could pre-create `Atlas\` with their own ACL or plant a junction, turning the agent's deletes and renames into SYSTEM-level primitives. Therefore:
   - `service install` creates the directory with owner Administrators and a **protected** DACL (no inheritance): SYSTEM and Administrators full control, nothing else.
   - **On every start** the agent verifies the owner, the protected DACL, and that neither the directory nor anything it opens inside is a reparse point (files opened with `FILE_FLAG_OPEN_REPARSE_POINT` and checked). On failure it refuses to start and reports to the Windows Event Log (its own log directory is untrusted at that point).
+- **Plan 1b-4a (D2):** `run` creates the directory, `buffer\` and `logs\` with the SDDL `O:BAD:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)`, or verifies them; `service install` (plan 1b-4c) calls the same code. Each is opened through one handle with `FILE_FLAG_OPEN_REPARSE_POINT` and refused if it is a reparse point or not a directory, its owner is not Administrators or SYSTEM, its DACL is null or not protected, or an allow entry names anyone but SYSTEM and Administrators (deny entries are accepted). Files inside are not checked one by one: in a verified directory only SYSTEM and Administrators can create them, and the buffer refuses segments that are reparse points (§8.2). Until the service exists (1b-4c), a refusal is printed on the console, which is the elevated user's own.
+- **The folders above it** (plan 1b-4a, review finding 4): a user who could rename one could swap in a directory of their own after the checks, with an `agent.toml` that turns the sensor off. So first, every folder above the directory, from the volume root down, is opened the same way and refused if it is a reparse point or not a directory, if its owner is not SYSTEM, Administrators or TrustedInstaller, or if an allow entry that applies to it gives anyone else `DELETE`, `WRITE_DAC`, `WRITE_OWNER`, `FILE_DELETE_CHILD` or `GENERIC_ALL` (inherit-only entries are skipped; deny entries accepted). The path must be absolute, without `..`. `C:\` and `C:\ProgramData` pass; a user's profile or `%TEMP%` does not, so `--data-dir` cannot point there. The default directory is unchanged.
 
 ### 11.2 `agent.toml`
 
 - Keys: buffer cap, retention/overflow policy, `transport` (`none` in this sub-project), ordering hold, completion deadline, enrichment size cap, watchlist (replace or extend the default), per-class enable, `network.udp`, UDP idle timeout, DNS per-PID rate, the failure-confirm window (§5.5), `file.op_end` (on by default; off emits Create, Delete and Rename without the window), seeding on start and on miss (two settings, both on; without `SeDebugPrivilege` both are off; with on-miss off, events on unknown handles wait only for the start-up pass), the seeder CPU budget (§7.4), `registry.value_reads` (on by default), log level.
 - Built-in defaults; the file only overrides them. Validated at start: an invalid file stops startup with a clear error in the Windows Event Log (Application, source `Atlas`). No hot reload in v1.
+- **Plan 1b-4a (D5):** the file's sections, keys and ranges are documented in `atlas-agent`'s `settings` module. Unknown keys and sections are refused, and every error names its key. Every key above exists except per-class enable, which is deferred: it is the last §13 fallback, and a later plan adds it if the fallbacks reach it. Until the service exists (1b-4c), an invalid file is reported on the console and in the diagnostic log.
 
 ### 11.3 CLI
 
 | Command | Behaviour |
 |---|---|
-| `atlas-agent run` | Console mode (admin required); Ctrl+C triggers a clean stop. Refuses to start if another instance holds `Global\Atlas-Agent` (single-instance mutex), so it cannot tear down the service's sessions. |
+| `atlas-agent run` | Console mode (admin required); Ctrl+C triggers a clean stop. Refuses to start if another instance holds the single-instance lock, so it cannot tear down the service's sessions. |
 | `atlas-agent service install \| uninstall \| start \| stop` | Install per §11.1; LocalSystem, auto-start. Recovery: restart after 5 s, then 60 s, then every 10 min, so a bad config does not loop hot. Uses the `windows-service` crate. |
 | `atlas-agent dump [--follow] [--from <cursor>] [--class <name>]` | JSON lines from the buffer; read-only (never acks); §8.5 concurrency rules. |
 
+**Plan 1b-4a (D3, D5):**
+- `run` takes `--data-dir` (default `C:\ProgramData\Atlas`) and names its sessions `Atlas-Sensor` and `Atlas-Process`. Ctrl+Break and closing the console also stop it cleanly. It exits with 0 after a clean stop, 1 when start-up is refused, and 2 when the pipeline thread or the buffer writer thread ended on its own (after the final report and the flush, as at a clean stop; a panic's message goes to the log).
+- **The single-instance lock** (review finding 1): a mutex `Atlas\Agent` inside a private namespace whose boundary holds the Administrators SID, with a DACL for SYSTEM and Administrators only. A mutex in `Global\` would not do: any user can create one there first. Only a token with Administrators enabled can create the namespace; one of the same name with another boundary is a different namespace; private namespaces are not per session, so the service in session 0 and `run` in a user's session find the same one; and it goes with its last handle, so a process that died does not block the next start. Verified on the host before it was built: squats on the old name and on the namespace's name blocked nothing, a non-admin could neither create nor open it, SYSTEM in session 0 saw a holder in session 1, and a holder that died freed it.
+- `dump` prints protobuf's standard JSON mapping, the replay snapshot's, one event per line. `--from` takes `segment:offset`, and `--class` one of `process`, `module`, `network`, `file`, `registry_key`, `registry_value`, `dns`, `event_log` and `sensor_health`. Invalid records are skipped and counted (§8.2). A summary goes to stderr at the end: events printed, invalid records, corrupt segments, and the cursor to continue from. With `--follow` there is no end: Ctrl+C ends it without the summary.
+
 ### 11.4 Lifecycle
 
-- **Start:** take the mutex → verify the data directory (§11.1) → load config → load `device.json` → compute `boot_id` → start sessions (recreating stale ones) → seed the process cache from the rundown → start threads → seed the key and file maps from the handle table (§7.4).
+- **Start:** take the single-instance lock (§11.3) → verify the data directory (§11.1) → load config → load `device.json` → compute `boot_id` → start sessions (recreating stale ones) → seed the process cache from the rundown → start threads → seed the key and file maps from the handle table (§7.4).
 - **Clean stop:** flush both sessions → wait out the hold → stop the sessions, so the consumers deliver the rest and finish → drain the ordering and completion stages → flush the buffer (plan 1b-3c: the queues close when the consumers do, which ends the pipeline loop). No in-flight event is lost on a clean stop. Pending events go out as at their deadlines, so one marked "drop at deadline" is dropped (plan 1b-3a).
 - **Diagnostic log:** `tracing` with a rolling file in `logs\`; its writes are self-filtered (§5.5).
+- **Plan 1b-4a:**
+  - **Start:** `run` checks for an elevated token first. The diagnostic log starts once the config is loaded, at its level. The buffer opens before the sessions start, so the sink is ready for the first event, and the health thread starts last. The console handler is installed just before the sessions start, so Ctrl+C during start-up waits and then stops cleanly, instead of ending the process with the sessions running (review finding 6).
+  - **Clean stop:** the health thread, then the sensor (as above), then a final Sensor Health report covering the stop, then the buffer flushed and closed. On the host it took 1.0 s (§15.3). The same stop follows when the pipeline thread or the buffer writer thread died (review findings 2 and 7): neither's panic takes `run` down with it. A console close waits for the stop and for the log file's last lines.
+  - **The log** goes to the console (stderr) and to `logs\atlas-agent.<date>.log`, rolled daily with seven files kept, through a non-blocking writer.
 
 ## 12. Testing
 
@@ -591,6 +617,7 @@ Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9
 - Tests start real sessions on `windows-latest` (the runner is admin), run the scripted scenario, and assert the expected normalized events, filtered to the test's own process tree, with timeouts. The scenario: spawn a process with a known command line; create, write, overwrite, rename and delete a file; a failed delete (no event); a delete-on-close file; an undelete (disposition set, then cleared) and a delete-on-close cleared through `FileDispositionInformationEx`, which must produce no Delete (result documented either way); open a watchlisted path, also through its 8.3 name; create, set and delete a registry key and value (with the value data read after); a handle opened **before** the agent starts that is then written to (file) or created under (registry), named by seeding; open a TCP connection; send UDP; resolve a name.
 - Marked `#[ignore]` locally; CI runs them explicitly.
 - **Split by crate** (plan 1b-2): `atlas-etw`'s live test covers the ETW level of this scenario (process, files, failed operations, delete-on-close, registry including embedded NULs and the CloseKey handle test, TCP and UDP over IPv4 and IPv6, DNS), plus the session primitives and the event-ID filters. It runs the scenario in a second process (the actor), so the observer's own TDH lookups are not recorded. The agent-level parts (watchlist and 8.3, undelete, seeding, value reads) are `atlas-agent`'s live test (plan 1b-3c, D2): the real sessions, services and pipeline thread against an actor that opens a file and a key handle before the agent starts and is then told, over its stdin, to run the scenario. It also checks the real enrichment of the `cmd.exe` Launch (SHA-256, catalog signature, account, DOS path), that nothing of the agent's own (hashing, value reads, seeding) is emitted, and that no queue dropped. `atlas-etw`'s live test also checks the Cleanup outcomes of a delete, a delete-on-close, an undelete, a hard-link delete and a stream delete, and `SetDelete` on set and clear. CI's `agent-live` job runs the service tests that need `SeDebugPrivilege` or `SeBackupPrivilege` (plan 1b-3b) and the agent live test. Test sessions are named `Atlas-Test-*`.
+- **The binary-level test** (plan 1b-4a, D4), also in `agent-live`, runs the `atlas-agent` executable: `run` against an actor (a file created, renamed and deleted; a registry key and value; loopback TCP; two name lookups; a child `cmd`), then Ctrl+Break, then `dump`. It checks every class the actor produces; Sensor Health reports with no loss whose intervals chain, the first starting at the agent's start and the last after the stop; as many lines as the writer logged as written (`records=`), not only as a second read of the disk finds; the class filter; the protected directories and a log file; that a second instance, a directory with a loose ACL and one below `%TEMP%` are refused; and that Ctrl+Break while the sensor starts still stops it cleanly, with no session left and a final report. Its zero-loss check fails on a busy host (bursts of 18–36k events a minute; §13), pending the profiling before 1b-4b.
 
 ### 12.4 Known gaps
 
@@ -618,6 +645,8 @@ Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gate
 
 `OP_END`, `CloseKey`, seeding, the fast path and value reads were added after these measurements; DoD item 4 checks the budget with them.
 
+**Measured by plan 1b-4a's host run** (2026-10-12, 103 minutes of normal use; the agent's own Sensor Health records; §15.3). With everything above on, nothing was lost, but the agent is **over budget**: CPU averaged 10.1% of one core (3–4% in quiet minutes, 40–70% in the busiest), and the working set grew to 200 MB, then held. The cause is profiled before plan 1b-4b, ahead of any fallback (decision log, 2026-10-12).
+
 **Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.
 
 ## 14. Definition of Done
@@ -722,6 +751,13 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **F7, 1b-3b's symbolic-link test leaked its volatile test key** on every run (`RegDeleteTreeW` follows a link instead of deleting it), and a reused PID then failed the test. The link is now deleted through its own handle.
 - **F8, key names keep their hive's case on the runner too:** the CI runner's `HKU\<SID>\SOFTWARE` is upper case, which only a case-sensitive test noticed; paths are compared ignoring case (§5.5).
 
+**Plan 1b-4a (2026-10-12): the runnable agent** (verified on the host, build 26200, unelevated, in one elevated test run and a 103-minute run of normal use; and on the CI runner, 26100):
+- **F1, `file.op_end` off left OperationEnd enabled:** only the pipeline's rule changed, and the callback still waited for every Cleanup's outcome, so with the fallback applied every Cleanup would have been counted unpaired. Session A now leaves out 24 and `OP_END`, and the callback pairs nothing (§4.2).
+- **F2, NTFS protects an explicit DACL given at creation:** a directory created with a security descriptor reads back with its DACL protected, even without the `P` flag. An inheriting directory is what a plain create makes (its owner then set), so that is how the test builds the case the check must refuse.
+- **F3, the CI runner's `%TEMP%` is an 8.3 path** (`RUNNER~1`); the agent reports paths in their long form (§7.2), so the binary test's actor works under the long path.
+- **F4, the host run** (103 minutes, normal use, `C:\ProgramData\Atlas`): 103 reports, chained without gaps; nothing lost of any kind; both unpaired-Cleanup parts 0; 447,287 records (150 MB); no invalid records or corrupt segments; the clean stop took 1.0 s. CPU averaged 10.1% of one core: 3–4% in quiet minutes, 40–70% in the busiest, which were also those with the most file activity (`housekeeping.file_op_failed` at 40,000–65,000 a minute against about 8,000 when quiet). The working set grew from 122 to 200 MB and then held, with no cache or map evictions. Over the §13 budget (§13).
+- **F5, on the CI runner:** the binary-level test wrote 851 events and two reports with nothing lost, and stopped in 0.9 s.
+
 **Plan 1b-3b (2026-10-06): building the Windows services** (verified on the host, unelevated and in four elevated runs):
 - **F1, `GetLongPathNameW` cannot expand NT paths:** it rejects `\\?\GLOBALROOT\Device\…` (`ERROR_INVALID_NAME`) and costs 1.55 ms for a path with 7 short components. Per-component `NtQueryDirectoryFile` takes about 54 µs per short component (§7.2).
 - **F2, a handle can change between the table read and the duplicate;** names are verified by a second read (§7.4).
@@ -856,3 +892,27 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 8. the agent-level live test, and `atlas-etw`'s checks of the outcomes; the delete rules are covered by scenario tests and the replay of the CI recording, not by a property test;
 9. known limitations: SMB and ReFS, a file replaced by a rename, a Cleanup and its outcome delivered far apart; the delete-on-close limitation is gone;
 10. the path from the request's `DeletePath`; a delete request's successful OperationEnd passed on; pairing by the next operation on the `Irp`, within 1 s, in either delivery order; `quality.file_cleanup_unpaired`; a delete-on-close clear keeps another handle's disposition; the driver's control channel (review, Review Log of plan 1b-3c).
+
+**Plan 1b-4a clarifications (2026-10-12).** Applied to §3.2, §4.2, §9.1, §9.3, §10.3, §11.1–§11.4, §12.3, §13 and §15.3:
+1. plan 1b-4 is split in three: 1b-4a, a runnable agent; 1b-4b, blinding detection; 1b-4c, the service and acceptance;
+2. the sink: a bounded channel to the writer thread, never blocking, its drops in `loss.buffer_backlog_drops`;
+3. thread [7] starts as the health thread: snapshots every 5 s, a report every 60 s, early on a loss, and at the clean stop;
+4. how a report is made: differences of snapshots, wrapping session counters, absent sources carried over, the buffer's figures filled in by the writer, chained intervals;
+5. the two unpaired-Cleanup parts in the diagnostic log; two counters not measured yet;
+6. `file.op_end` off leaves 24 and `OP_END` out of Session A, and the callback pairs no Cleanup;
+7. the data directory created or verified by `run`, through one handle, deny entries accepted, files inside covered by the directory's DACL;
+8. `agent.toml`: unknown keys refused, per-class enable deferred; errors on the console and in the log until the service exists;
+9. `run`'s options, stop signals and exit codes; `dump`'s format, cursor, classes and summary;
+10. the start and clean-stop order, and the diagnostic log's files;
+11. the binary-level test in `agent-live`;
+12. the host run's figures, over the §13 budget, and the profiling that comes before 1b-4b.
+
+**Plan 1b-4a review fixes (2026-10-13).** The plan's independent review found two major and six minor issues; each fix has a test, and each test was checked by reverting the fix. Applied to §3.2, §9.3, §11.1, §11.3, §11.4 and §12.3:
+1. the single-instance lock is a private namespace bounded by the Administrators SID, not `Global\Atlas-Agent`, which any user could take first;
+2. a panicked pipeline thread no longer panics `run`: the final report (without the pipeline's counters) and the flush still happen, and `run` exits 2;
+3. the writer fills `loss.buffer_backlog_drops`, so a report counts the drops of the events queued ahead of it, the final drain's included;
+4. the folders above the data directory are checked, so a user cannot swap one;
+5. a bad watchlist pattern is refused when `agent.toml` is read, naming its key;
+6. the console handler is installed before the sessions start;
+7. a dead buffer writer thread stops `run` (exit 2), and closing the buffer reports its panic instead of panicking;
+8. tests that could not catch their bug or would hang were fixed; the binary test's new check against the writer's count found that the `records=` logged at stop missed the last write's records (the buffer's `close` now returns its final counts).
````

```diff
--- a/docs/schema-reference.md
+++ b/docs/schema-reference.md
@@ -157,7 +157,7 @@ The agent's own loss, quality, housekeeping, resource and buffer figures, every
 
 | Group | Holds |
 |---|---|
-| `loss` | ETW events and buffers lost per session, queue drops, DNS rate-limit drops, `actor_dropped` per class, buffer backlog drops, events skipped because the ETW callback panicked |
+| `loss` | ETW events and buffers lost per session, queue drops, DNS rate-limit drops, `actor_dropped` per class, events the buffer could not take (its writer's channel or backlog full), events skipped because the ETW callback panicked |
 | `quality` | late arrivals, parse errors, unknown versions, join misses, `actor_unresolved` per class, unresolved file objects and registry paths, value-read failures, invalid buffer records, enrichment misses, ambiguous registry value names, file deletes reported without a Cleanup outcome (SMB), Cleanups whose outcome could not be paired |
 | `housekeeping` | evictions from every bounded map and cache, retention evictions, dropped failed file creates and renames, seeding results, requests dropped because a service queue was full |
 | `resources` | agent CPU time in the interval (ns), working set (bytes) |
```

```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
@@ -48,7 +48,8 @@ message SensorLoss {
   optional uint64 dns_rate_limit_drops = 7;
   // Events dropped because no process uid could be computed, per class.
   repeated ClassCount actor_dropped = 8;
-  // Events dropped because the buffer's in-memory backlog was full (disk full or I/O errors).
+  // Events dropped because the buffer could not take them: the channel to its writer thread was
+  // full, or its in-memory backlog was (disk full or I/O errors).
   optional uint64 buffer_backlog_drops = 9;
   // Events skipped because the ETW callback panicked on them (caught, never unwound into ETW).
   optional uint64 callback_panics = 10;
```

```diff
--- a/crates/atlas-schema/src/classes/sensor_health.rs
+++ b/crates/atlas-schema/src/classes/sensor_health.rs
@@ -138,7 +138,7 @@ counter_group!(
         dns_rate_limit_drops: Option<u64>,
         /// Dropped because no process uid could be computed, per class.
         actor_dropped: Vec<ClassCount>,
-        /// Dropped because the buffer's in-memory backlog was full.
+        /// Dropped because the buffer could not take them (its writer's channel or backlog was full).
         buffer_backlog_drops: Option<u64>,
         /// Skipped because the ETW callback panicked on them.
         callback_panics: Option<u64>,
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-schema -p atlas-proto
buf lint crates/atlas-proto/proto
buf breaking crates/atlas-proto/proto --against '.git#branch=main,subdir=crates/atlas-proto/proto'
$env:ATLAS_UPDATE_FIXTURES = '1'; cargo test -p atlas-schema --test golden; Remove-Item Env:ATLAS_UPDATE_FIXTURES
git diff --ignore-cr-at-eol --stat crates/atlas-schema/tests/fixtures
```
Expected: all pass, `buf` reports nothing, the fixture diff is empty. Workspace: 402 passed, 18 ignored (`atlas-agent` unit tests: 217).
```powershell
git add docs crates/atlas-proto crates/atlas-schema
git commit -m "docs(1): plan 1b-4a clarifications, findings and schema reference"
```

### Task 12: PR and merge

- [ ] **Step 1:** Push `feat/1b-4a-agent` and open the PR. The body summarises:
  - `run` and `dump`, `agent.toml`, the data directory, the writer and the health thread;
  - the `file.op_end` fix (F1);
  - the binary test;
  - the review's fixes (the lock, a dead thread, the data directory's folders, the final report's drops, the watchlist, the start-up stop, the tests) and F9;
  - the test counts, the elevated run's results and the hour run's figures, with the budget overrun (D6).

  It ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto` (with `buf breaking`), `powershell` and `audit`. If a test fails only on the runner, fix it with a test, never by skipping.
- [ ] **Step 3:** When every check on the PR's current head is green, merge. Then:
  - update the decision log's build row if anything changed on the way;
  - delete the remote `scratch/1b-4a` branch (D5.11).

## Review Log

### Independent review (2026-10-12, of the draft at `7893dae`)

An independent agent read the draft against the spec and the code, compiled it, and ran the unit tests (208 passed). The plan's code matched the branch. It found two major issues, six minor ones and ten nits. Each fix was made in the scratch worktree, with a test; each test was checked by reverting its fix (the verification's reverts) and, where it needs elevation, run by the user on the host and on the CI runner.

**Major:**
1. **Any user could squat the single-instance mutex.** `SeCreateGlobalPrivilege` gates file mappings and symbolic links in `Global\`, not mutexes: an unelevated process created `Global\Atlas-Squat-Test` (verified). A squat would have kept `run`, and later the service, from ever starting. **Fixed:** the lock is a mutex in a private namespace bounded by the Administrators SID (Task 8, Step 3; §11.3), probed on the host first. Five elevated tests; six reverts.
2. **A panicked pipeline thread panicked `run`:** `Running::finish` joined it with `expect`, so `run` exited 101 instead of 2, sent no final report and skipped `Store::close`, losing the queued records. **Fixed:** the join keeps the panic's message (`Stopped::pipeline` is a `Result`), and `run` logs it, sends the final report without the pipeline's counters, flushes and exits 2. An elevated `run` test with a sink that panics; three reverts.

**Minor:**
3. **The final report missed the writer's drops during the stop's drain:** the health side read the drop counter before the writer handled what was queued ahead of the report. **Fixed:** the writer fills `loss.buffer_backlog_drops` as it appends each report. A unit test with a full disk; two reverts.
4. **Only the data directory itself was checked:** a `--data-dir` below a folder its user controls could be swapped after the checks (with an `agent.toml` that turns the sensor off), and the draft said "nothing can be swapped". The default directory was sound. **Fixed** (the user chose to check, over documenting `--data-dir` as admin-only): every folder above is checked (Task 7; §11.1). Unit and elevated tests, and the binary test; seven reverts. On the way, a test against this machine's real folders caught a wrong TrustedInstaller SID written from memory.
5. **A bad watchlist pattern failed at the sensor's start,** as the `Debug` form of `BadPattern`, without its key. **Fixed:** settings compiles the list and names the key. Two reverts.
6. **Ctrl+C during start-up killed the process with the sessions running:** the console handler came after `agent::start`. **Fixed:** it comes just before the sessions start. The binary test sends Ctrl+Break as soon as `run` says it is starting the sensor; one revert, checked by that check's own result.
7. **A dead writer thread went unnoticed:** every event was counted as dropped and no report reached the disk, while `run` went on. **Fixed:** the run loop checks `Store::is_running` and stops with exit 2; `Store::close` returns the panic instead of panicking. A unit test and an elevated `run` test; three reverts. On the way, a second `run` in one test process was found to get a dead stop channel; each `run` now replaces it.
8. **Tests that could not catch their bug, or would hang:**
   - the anchor test measured its bound from before the thread started: now the gaps between refreshes;
   - the first slow account lookup had no bound and would hang if the timeout were ignored: now bounded, and a timer opens the gate;
   - the full-channel sink test would hang on a blocking send: now it sends from its own thread;
   - the binary test compared `dump` with a second read of the same disk: now with the writer's `records=`. This found F9 on its first host run;
   - the binary test's intervals did not check where the first starts or that the last follows the stop: now they do;
   - the descriptor checks did not cover an unknown entry type or an inherit-only allow for others: now they do;
   - the health thread's loop had no test: now one drives it.

   Thirteen reverts, none hanging.

**Nits:** the code ones are fixed (a console close now waits for the log file's last lines; one SAFETY comment per `unsafe` call; the writer's tick documented as "about every 100 ms, at most about 200", which is what the code does and all the writer needs). The prose ones are fixed in this document and the spec: `dump --follow` on Ctrl+C prints no summary; the deny-only case is accepted; the binary test checks TCP open only; a report's time is the system clock, not QPC; six timing tests, not five; `buffer.corrupt_segments` is never measured either. One was not reproduced and is left: the hour script's summary after Ctrl+C (the summary script runs alone, Task 10).

**What held up:** 1b-3c's interfaces honoured; the health differences, wrap, absence and `advance`; the store's held reports, gaps and order; `dump`'s cursor and filter. **For 1b-4b** it noted that a restarted session's counters start again from 0, so the next difference would wrap (Interfaces).
