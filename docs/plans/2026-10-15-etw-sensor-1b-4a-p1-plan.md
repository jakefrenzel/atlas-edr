# Sub-project 1b-4a-p1 — The Profiling Step's First Fix Implementation Plan

> **Status:** Approved by the user (2026-10-15), after an independent review (Review Log). **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Task 3 is a run on the host by the user, from a script. Nothing in this plan needs the VM or a kernel driver.

**Goal:** Find where the agent's CPU goes and fix the largest cause (decision log, 2026-10-12, D6: the hour run used 10.1% of one core against §13's 2%), and leave the agent able to tell its own cost from then on.
- **The fix:** a SetAttributes no longer makes the hash and 8.3 caches forget the file (Q4). The System process sets the basic information of the DLLs every starting process loads; each such event made the next load of that DLL pay a full SHA-256 and signature check. Under the same load, normal use went from 19.0% to 7.9% of one core (F4);
- **events received:** a new Sensor Health group, `throughput`, counts the ETW events each session delivered to the agent (Q5), so CPU reads per event and a silent session shows; the counters are the agent's, so a restarted session counts on into them (Q8);
- **CPU per thread:** the diagnostic log gives the CPU each agent thread used, listed at most once a report period (Q5, Q7);
- **the fixes from this plan's independent review** (Review Log): one major and seven minor findings, each with a test checked by reverting its fix.

This is the first fix of the profiling step that comes before plan 1b-4b (D6). The rest of the step (the pipeline, on-miss seeding, the hash workers' first-time checks, the working set, then the hour run) follows in later plans, each fix or §13 fallback a decision-log entry.

**Architecture:**
- **`pipeline::file`** (portable): `forgets_cached(action)` says whether an action makes the caches forget a path: every action but SetAttributes does. The two senders of `InvalidateHash` that can carry a SetAttributes ask it: an action on a known handle, and the seeder's answer for an event that waited for it.
- **`atlas-etw`'s consumer:** `ConsumerCounters` (panics, events delivered) are the caller's, handed in through `consume_with`; the trampoline counts every event before it calls the callback.
- **`health`** (portable): the snapshot gains the two sessions' delivered counts and, when listed, a `ThreadList` (each thread's name and CPU by id and creation time); the report gains `throughput`, and the log-only `Diagnostics` each thread's difference summed by name, with the listings' interval.
- **`win::agent`:** `Running` owns each session's `ConsumerCounters`, created once, read by the health sources and at the stop.
- **`win::health`:** `threads()` lists this process's threads (a thread snapshot, their descriptions and times), about 45 ms, so at most once a report period, for a report of either kind; `run` lists them for the final report before the sensor stops. The health sources carry the lister as a closure, so a test can count its calls.
- **The schema:** `SensorThroughput` (`sensor_session_events_received`, `process_session_events_received`), additive (field 7 of `SensorHealthReport`).

**Tech stack:** Rust 1.97 (edition 2024). No new dependency; `atlas-agent` adds the `windows` feature `Win32_System_Diagnostics_ToolHelp`. Measurement tools (git-ignored, Task 4): Windows Performance Recorder (`wpr`, in Windows), the `d6load` helper (Rust), and a TraceProcessor analyser (.NET 9, `Microsoft.Windows.EventTracing.Processing.All` 1.12.10).

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2 to 1b-4a. Section numbers (§) refer to it. Plan 1b-4a (`docs/plans/2026-10-13-etw-sensor-1b-4a-plan.md`), its "Interfaces for later plans", is the contract this step started from.

**Verification note (2026-10-15, host, Windows 11 build 26200, and the CI runner, build 26100):** every code block below was compiled and run in a scratch worktree of `main` (901c085), the review's fixes included.
- **Lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean at the end of every task; `atlas-agent` and `atlas-etw` are also clippy-clean for `x86_64-unknown-linux-gnu`, checked from Windows at every task.
- **Unelevated:** `cargo test --workspace` passes 411 tests and ignores 18 (the live tests, the binary test and the elevated unit tests, unchanged in number). `atlas-agent` has 225 unit tests (1b-4a had 217). Test counts per task were measured at each task's commit. The agent's unit tests passed 20 runs in a row after Task 2 (the timing lesson of plan 1b-4a's F8).
- **On the CI runner** (the scratch branch pushed, no PR):
  - run 38760947417 passed every job but `etw-live`, whose new check found F6 (a test bug);
  - runs 38766153448 and 38766664860 (the draft's final code) passed every job: `etw-live`'s `received_counts_every_delivered_event`; `agent-live`'s `both_sessions_delivered_events` (25,747 and 663 events), `events_received_measured` (66,774 and 677 over the binary test's two reports) and `a_report_logs_cpu_per_thread`;
  - after the review's fixes: run 38772171920 (the fixes) passed every job; run 38778683997 failed the binary test's final-listing check (made a note since) and the agent live test's launch hash, run 38779918757 the launch hash alone (F9); run 38780755145 (the final code; later commits change docs only) passed every job in each of its six attempts, its notes giving 0 enrichment errors and 0–74 misses.
- **On the host:** the before and after traces (F2, F4), each about 10 minutes, run by the user with Task 3's script; a third run got no trace (F8); the cache probe and the replay of F3 (ignored tests in the scratch worktree, not part of this plan).
- **Reverts:** every rule in the Review Focus was checked by reverting it in the source and running its test, which must then fail (`spikes/p1-plan-src/mutate.py`; the source restored after each). 26 reverts, all caught on the final code, 2026-10-15:
  - **the SetAttributes fix (4):** either sender forgetting again, `forgets_cached` saying yes to everything, an Update no longer forgetting;
  - **events received (6):** the trampoline not counting, counting only when the callback did not panic, the report taking totals, the sessions swapped, the snapshot reading Session A's counter twice, the schema dropping the group on the wire;
  - **CPU per thread (6):** every process's threads listed, threads sharing a name collapsed into one entry, totals instead of differences, idle names kept, an unlisted interval not carried, the report never carrying them;
  - **when the threads are listed (3):** at every check, never, for the early loss report too;
  - **the review's fixes (7):** a consumer counting into its own counters rather than the caller's (M1), counting after the callback (a nit), threads matched by id only and differenced by name (m1, two), listed only for a periodic report and at every report (m3, two), a Delete no longer forgetting (m4).

  On the first run the new sustained-loss test hung under one revert (its report channel was full): the channel was enlarged and the runner given a timeout.
- **Round trip:** the code was rebuilt from this document alone in a fresh LF worktree of `main`: every diff applied, and every task's tree is identical to the verified one (no task changes `Cargo.lock`).
- **Other checks:** `buf lint` and `buf breaking` (against `main`) pass on the host (`buf` 1.73.0; on pushes CI only lints, and checks breaking changes on the PR). Regenerating the golden fixtures on the final code changes nothing; `sensor_health_report.json` is the only fixture this plan changes. The trace script parses in Windows PowerShell 5.1 and PowerShell 7.
- **Not run yet:** Task 3 on the final code (Q7's and the review's listing changes were measured by timing the listing, 45 ms, not by a trace); the Linux build of the tests is run by CI only.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-4a-p1-plan`. The build runs on `feat/1b-4a-p1`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Windows code only in `win`** and `#[cfg(windows)]` tests; `pipeline` and `health` stay portable and Linux CI builds and tests them.
- **Every `unsafe` block has a `// SAFETY:` comment.**
- **The ETW callback stays fast** (§3.2): the new count is one relaxed atomic increment per event.
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). The live tests are `#[ignore]`d and run in CI's `etw-live` and `agent-live` jobs; Task 3's trace is the user's.
- **The host's data stays on the host:** the traces and buffer dumps stay in the git-ignored `spikes\results\`; only summary figures enter the docs.
- **Additive schema only:** one new group; `buf breaking` must pass. The golden fixture `sensor_health_report.json` gains the group (regenerated; no other fixture changes).
- **Formatting and lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` at the end of every task.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash). Edits that contain backslashes are made with an editor, never a heredoc.
- Every commit message ends with the attribution lines the session's system reminder specifies.

## Decisions

### Q1, how to measure (chosen 2026-10-15: C)

- **(C) Chosen: a WPR trace now, with no code change; per-thread CPU and an event rate built into the agent with the fix.** The trace gives CPU per thread and per function in one run; the built-in figures let the 24-hour run (1b-4c) tell which thread spends the CPU without another trace.
- (A) The trace only. (B) Built-in figures first, then another hour run: a plan cycle before any data, and no function-level view.

### Q2, the load during the trace (chosen 2026-10-15: C)

- **(C) Chosen: both, in one run:** 2 minutes of a fixed synthetic load on a quiet machine (repeatable, with known counts), then 5 minutes of normal use (realistic).
- (A) Synthetic only: may miss what real use stresses. (B) Normal use only: not repeatable.

### Q3, how to proceed once the cause was found (chosen 2026-10-15: A)

- **(A) Chosen: fix the largest cause first, as its own small plan, with the built-in figures, then re-measure and profile the rest against a clean baseline.**
- (B) Profile everything first, then one plan: the remaining profile would carry the hash workers' noise and backlog. (C) Profile the rest from the first trace: the same blur.

### Q4, which file events make the caches forget a path (chosen 2026-10-15: A)

- **(A) Chosen: Update, Rename and Delete; not SetAttributes.** Attributes and timestamps change neither content (so no hash or signature) nor names (so no short name is freed); the USN in the hash cache's key already separates versions.
- (B) The hash cache never forgets by path, only the 8.3 cache on Delete and Rename: also correct, but gives up the memory hygiene the spec chose, for little gain. (C) Ignore the System process's SetAttributes only: a special case; another program touching attributes would cause the same churn.

### Q5, where the new figures go (chosen 2026-10-15: C)

- **(C) Chosen: events received per session in the schema; CPU per thread in the diagnostic log.** Events received is a lasting signal (cost per event; a session gone silent, which 1b-4b's blinding detection watches); thread names are implementation details that should not be frozen in the schema.
- (A) Both in the log only: the server never sees the event rate. (B) Both in the schema: freezes thread names.

### Q6, the defaults (chosen 2026-10-15, accepted together)

1. this plan's name, 1b-4a-p1, built and reviewed like the others;
2. the fix covers both places that send the invalidation; the SetAttributes events themselves are still emitted;
3. events received counted in `atlas-etw`'s trampoline, before parsing (as ETW's own "Events Logged per sec" counts); two fields in a new `SensorThroughput` group; 64-bit, differenced per report;
4. per-thread CPU in milliseconds per thread name, hash workers listed separately; a thread snapshot with `GetThreadDescription` and `GetThreadTimes`;
5. the re-measure with the same trace script, compared phase by phase; the hour run once, at the end of the whole profiling step;
6. the spec's §6.3, §7.2, §9.3, §10.3, §13 and §15.3, and the decision log;
7. left for later: what the System process's SetAttributes events are and whether to filter them; the pipeline; the seeder; the working set.

### Q7, when the threads are listed (chosen 2026-10-15: A)

The re-measure found the listing itself costly (F5): a snapshot of every thread in the system, 45 ms, at every 5 s check: 0.86% of one core.
- **(A) Chosen: only for a periodic report** (about 0.08%), with documented APIs. An early report (a loss) leaves the threads out and the next periodic one covers both intervals, the health code's existing rule for a source not read.
- (B) Every 5 s with `NtGetNextThread`, which walks only this process's threads: undocumented, not in the `windows` crate. (C) Each thread registers its handle at start: touches every place that starts a thread, `atlas-etw` included, and a thread that forgets goes missing.

The review then found the rule "only for a periodic report" left the threads unlisted under sustained loss (every report then a loss report): they are listed at most once a period, for a report of either kind (review m3).

### Q8, counters across a session restart (chosen 2026-10-15: A)

The review found (M1) that the health thread held each consumer's own counters: after 1b-4b restarts a session, its new consumer would count into new ones while the health thread kept reading the dead consumer's, and `throughput` would read 0 for good, the very signal 1b-4b watches.
- **(A) Chosen: the agent owns one set of counters per session and hands it to every consumer it starts** (`consume_with`), so counts never go back across a restart: no base reset, no stale counter. It fixes the panic counter's same flaw (plan 1b-4a).
- (B) Document that 1b-4b must swap the counters in the health sources and reset the base at a restart: a two-part rule, easy to get half right, whose failure is a false "silent session".

### Q9, launches that miss their hash at start-up (chosen 2026-10-15: A)

F9: on the runner, a launch early in a run sometimes went out without its hash, past the 1 s enrichment deadline, while the hash workers checked every file for the first time.
- **(A) Chosen: record it, and fix it in the next profiling round,** where the hash workers' first-time checks are the item (Interfaces): launched images ahead of module loads, and/or a longer enrichment deadline while the cache is cold (as the seeder has at start). The live check stays as it is and keeps catching it; a failure there gets a re-run with its counters read, never a skip.
- (B) A priority lane for launches in this plan: a new design, not brainstormed or reviewed. (C) The test waits after the start: it would hide a real gap.

## Findings

- **F1, the method** (Task 3): a WPR CPU trace (`wpr -start CPU -filemode`: sampling and context switches, with stacks) of the release agent built with line tables, in four phases: 45 s settling; 30 s quiet; 2 minutes of `d6load`'s fixed load (each second: 1,000 opens of missing files, 100 file create-write-delete cycles, 200 opens of missing registry keys, 100 value sets, 5 launches of freshly copied `whoami.exe`, 5 DNS lookups); 5 minutes of normal use. `d6load` also records each agent thread's CPU at every phase boundary; `typeperf` records each session's "Events Logged per sec". `xperf -i` could not read the trace (`0x80070032`, an event it does not support); Microsoft's TraceProcessor library could. The trace is about 4–6 GB, and WPR needs about twice that while it stops.
- **F2, where the CPU went** (2026-10-15, before the fix; normal use, 14,480 events/s): 19.0% of one core: the hash workers 12.6%, the pipeline 4.0%, the seeder 1.1%, Session A's callback 0.9% (about 0.6 µs per event, so the callback, plan 1b-4a's first suspect, was not the cause), the buffer writer 0.3%. 55% of the hash workers' time was inside `WinVerifyTrust` (the Authenticode digest, certificate chains and stores); 60% of it was kernel time. The seeder's time is all handle-table snapshots (`NtQuerySystemInformation`). The pipeline's is spread: the ordering stage (18%), the 10 ms loop's sleep (14%), the registry key map (14%), allocation (about 25%).
- **F3, the cause:** the System process sets `FileBasicInformation` on the DLLs that starting processes load: about 1,300 SetAttributes events on module files in the five minutes (2,006 in the whole run; `KERNEL32.DLL` alone 85 times, as `C:\WINDOWS\SYSTEM32\KERNEL32.DLL`). Each sent `InvalidateHash`, so the next load of the DLL missed the cache: a full SHA-256 and signature check, 15–80 ms, instead of a hit, about 0.12 ms. Checked three ways:
  - the cache works for these files when called directly (`writer_open` false, the USN read and stable; the second call a hit);
  - replayed in order through the same `Enricher` with one cache, the phase's 29,476 module loads (392 distinct files) cost 7.5 s: 394 misses, 4.5 s; 29,082 hits, 3.0 s. Live, the hash workers spent 37 s;
  - the agent's own SHA-256 ran 1.9 s live, about four times what the distinct files' 780 MB (those under the 100 MB cap) need.
  No enrichment errors, cache evictions or queue drops, so nothing was being retried; 240 module loads in one busy minute went out past the 1 s deadline without a hash.
- **F4, after the fix** (2026-10-15, the same script, `2379538`; the other Claude session's commands were the normal use):

  | Phase | Events/s | Agent CPU | CPU per 1,000 events | Hash CPU per module load |
  |---|---|---|---|---|
  | synthetic, before → after | 19,061 → 21,980 | 17.6% → 12.9% | 9.2 → 5.9 ms | 0.77 → 0.52 ms |
  | normal use, before → after | 14,480 → 14,161 | 19.0% → 7.9% | 13.1 → 5.6 ms | 1.28 → 0.46 ms |

  After: hash workers 3.6%, pipeline 2.0%, seeder 0.8%, callback 0.5% in normal use. What is left of the hash workers' time is mostly first-time checks (668 distinct module files in the five minutes, against 418 before). The SetAttributes events are still emitted (2,469 on module files). Nothing was lost in either run.
- **F5, the thread listing's cost:** the same run showed the health thread at 0.86% of one core, against about 0 before: `threads()` takes a snapshot of every thread in the system, 45 ms per call on the host, and ran at every 5 s check. Q7: only for periodic reports.
- **F6, CI found a test bug** (run 38760947417): the `etw-live` test's new check read the consumer's count before closing it, while a stopped session's consumer still delivers the rest of its buffers until `close` (10,703 counted against 14,681 callback calls). Both counts are now read after `close`; the agent's `Stopped` already waits for the consumers to finish before reading them.
- **F7, the working set:** about 92–122 MB private in both runs; not changed by this fix.
- **F8, the disk:** a third trace run (2026-10-15, 09:23) got no trace: WPR stopped recording on its own about 1½ minutes in, with 1.6 GB left on C: (two earlier traces, 4 and 6 GB, and build caches). The script now refuses to start below 16 GB free, and stops the agent cleanly and saves its figures even when WPR fails.
- **F9, launches at start-up can miss their hash:** on the CI runner the agent live test's `cmd.exe` launch went out without its hash and signature in 2 of 9 runs on the same code (0 in the 33 runs before this plan). The live test's notes now record the enrichment counters: in four more runs of the same code, 0 errors every time and 0, 74, 50 and 0 enrichment misses (results past the 1 s deadline). So the hash workers sometimes fall over a second behind early in a run, when every file is checked for the first time (10–80 ms each, two workers), and a launch then goes out without its hash (the late result is cached for the next event on that file). This plan's fix only lowers that work; why it never showed before is not known (runner load; the earlier runs recorded no miss counts).

## Deliberate clarifications of the spec (applied to the spec text in Task 4)

1. **§6.3, §7.2:** a SetAttributes no longer invalidates the hash and 8.3 caches; Update, Rename and Delete still do (Q4).
2. **§9.3, §10.3:** Sensor Health's `throughput` group (Q5), from counters the agent owns (Q8).
3. **§9.3:** the diagnostic log gives each thread's CPU, listed at most once a report period (Q5, Q7, review m1–m3).
4. **§13, §15.3:** the profiling figures (F1–F9).
5. **§17:** the review's fixes (Review Log).

## Review Focus

1. **A SetAttributes never makes the caches forget, by either path; every other action does** (`a_set_attributes_leaves_the_caches_alone_and_an_update_does_not`, `a_set_attributes_the_seeder_answers_leaves_the_caches_alone`, and the table test `only_a_set_attributes_leaves_the_caches_alone`).
2. **Every delivered event is counted, before its callback, a panic included, into the caller's counters** (`the_trampoline_counts_every_event_and_every_panic`); live, the count equals the callback's calls (`etw-live`: `received_counts_every_delivered_event`) and Session A delivers more than B, B more than none (`agent-live`: `both_sessions_delivered_events`).
3. **Throughput is a difference, per session, in its place** (`a_report_is_the_difference_of_two_snapshots`, `each_sessions_received_count_is_read_into_its_place`), on the wire (`full_report_round_trips`, the golden fixture, the property tests), in every report of the binary test with A's sum the larger, and in its final report (`events_received_measured`, `the_final_report_counts_events_received`).
4. **Per-thread CPU:** each thread on its own, this process only, ended threads gone (`threads_are_listed_one_by_one`, which burns measured CPU time, not wall time); each thread's difference summed by name, a reused id and an ended thread handled, most first, zero left out, an unlisted interval carried, the listings' interval (`thread_cpu_is_differenced_by_thread_then_summed_by_name`); the log line (`the_thread_line_is_name_equals_ms`; the binary test's `a_report_logs_cpu_per_thread`).
5. **Threads are listed at most once a period, for a report of either kind** (Q7, review m3): none at the checks or for an early loss report, once for the periodic one (`a_loss_is_reported_at_once_and_restarts_the_period`), and still about once a period under sustained loss (`threads_are_listed_under_sustained_loss`); each test counts the lister's calls.

Every one of these was checked by reverting it: its test fails (verification note). Two fixes have no revert check. The agent's counters handed to its consumers are seen only on the runner (the live tests' counts). The final listing's place (review m2) cannot be checked: a thread shows in a listing only if it used a clock tick (about 15.6 ms) since the last one, a few seconds before; the binary test records the final line instead, and CI run 38778683997's named `atlas-seeder`, a thread that ends at the stop, so that listing came before it.

## File Structure

```
crates/atlas-etw/
  src/session/consumer.rs          + ConsumerCounters, consume_with; received counted in the trampoline; a unit test
  src/session/mod.rs               exports them
  tests/live.rs                    received_counts_every_delivered_event
crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto   + SensorThroughput (field 7)
crates/atlas-schema/
  src/classes/sensor_health.rs     + SensorThroughput group
  tests/common/mod.rs              the sample and the property strategy
  tests/fixtures/sensor_health_report.json                      regenerated
crates/atlas-agent/
  Cargo.toml                       + Win32_System_Diagnostics_ToolHelp
  src/pipeline/file.rs             forgets_cached; file_action_on asks it
  src/pipeline/seed.rs             the seeder's answer asks it
  src/pipeline/tests.rs            three tests
  src/health.rs                    Snapshot::received, threads (ThreadList); throughput; Diagnostics::threads, threads_interval
  src/dump.rs, src/store.rs        tests: at_start's start time
  src/win/agent.rs                 Running::counters; HealthSources::counters, threads; Stopped::received
  src/win/health.rs                threads(), listed once a period for a report; the log line; tests
  src/win/run.rs                   the final listing before the stop
  tests/live.rs                    both_sessions_delivered_events
  tests/binary.rs                  events_received_measured, a_report_logs_cpu_per_thread
spikes/                            git-ignored; Task 3: run-d6-trace.ps1, d6load/, d6trace/, p1-plan-src/compare.py
docs/…                             Task 4
```

## Interfaces for later plans

- **The rest of the profiling step** (before 1b-4b, D6), measured with Task 3's script against F4's figures, then the hour run:
  - **the pipeline** (2.0%): F2's spread; its kernel time (half of it before the fix) is the 10 ms loop's sleeps (`std::thread::sleep` creates a waitable timer each time) and allocation;
  - **on-miss seeding** (0.8%): every snapshot reads the whole handle table; the first §13 fallback turns it off;
  - **the hash workers' first-time checks** (3.6% while many new files load): `WinVerifyTrust` opens the system certificate stores on each call; while the cache is cold they can fall over a second behind, so a launch then goes out without its hash (F9, Q9: launched images ahead of module loads, or a longer deadline while cold);
  - **the System process's SetAttributes events on DLLs** (250–500 a minute while processes start, F3 and F4): what they set, and whether to filter them, is open (Q6.7);
  - **the working set** (F7);
  - each fix or fallback is a decision-log entry.
- **Plan 1b-4b (blinding detection):** `throughput` shows a session that delivers nothing. A restarted session's consumer is started with `session::consume_with(name, callback, running.counters[i].clone())`, so its counts go on from the old ones and the health thread needs no reset for them (Q8); the sessions' own loss counters still restart at 0 and need the base reset (plan 1b-4a's interfaces).
- **Found by the review, older than this plan** (not fixed here): `on_file_set_info` ignores `FileShortNameInformation` (class 40), so `SetFileShortName` changes an 8.3 name with no Rename and the expander can keep a stale mapping until its expiry; a written handle with a name but no opener emits no Update (`cleanup_handle`), so the hash cache's real guard against changed content is the USN in its key, not the Update's invalidation.
- **Plan 1b-4c:** the 24-hour run's summary reads `throughput` from the reports and the per-thread line from the log.

---

### Task 1: A SetAttributes leaves the hash and 8.3 caches alone

**Files:**
- Modify: `crates/atlas-agent/src/pipeline/file.rs`, `crates/atlas-agent/src/pipeline/seed.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: The tests** (Q4, F3). The first drives a known handle: Create, SetAttributes, Write, Cleanup; it expects both events and exactly one `InvalidateHash`, the Update's. The second drives a handle from before the agent: the SetAttributes and the Update both wait for the seeder, whose answer names the handle; again both events and one `InvalidateHash`. Before Step 2 both fail: two invalidations each. A table test pins `forgets_cached` for every action (review m4: the Deletes that ask it had no test).

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -587,6 +587,91 @@ fn one_update_per_written_handle_with_the_opener_as_actor() {
     valid(&out);
 }
 
+/// The paths the hash and 8.3 caches were told to forget, in order.
+fn forgotten(t: &mut T) -> Vec<String> {
+    t.requests()
+        .into_iter()
+        .filter_map(|r| match r {
+            Request::InvalidateHash { nt_path } => Some(nt_path),
+            _ => None,
+        })
+        .collect()
+}
+
+/// D6: the System process sets the basic information of the DLLs starting
+/// processes load; forgetting their hashes then re-hashed them on every launch.
+#[test]
+fn a_set_attributes_leaves_the_caches_alone_and_an_update_does_not() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\Windows\System32\kernel32.dll";
+    t.ev(5, 100, Some(key(10)), create(1, 0xA, f, 0));
+    t.ev(6, 100, Some(key(10)), set_info(0xA, 4));
+    t.ev(7, 100, Some(key(10)), write(0xA));
+    t.ev(8, 100, Some(key(10)), RawEvent::FileCleanup(handle(0xA)));
+    let out = t.settle();
+    assert_eq!(
+        files(&out),
+        [
+            ("setattr", r"C:\Windows\System32\kernel32.dll".into(), 100),
+            ("update", r"C:\Windows\System32\kernel32.dll".into(), 100)
+        ]
+    );
+    assert_eq!(forgotten(&mut t), [f], "the Update only");
+    valid(&out);
+}
+
+/// The same for a handle the seeder names: the waiting SetAttributes is
+/// answered without forgetting, the waiting Update forgets.
+#[test]
+fn a_set_attributes_the_seeder_answers_leaves_the_caches_alone() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    let f = r"\Device\HarddiskVolume3\Windows\System32\kernel32.dll";
+    t.ev(5, 100, Some(key(10)), set_info(0xB, 4)); // a handle from before the agent
+    t.ev(6, 100, Some(key(10)), write(0xB));
+    t.ev(7, 4, None, RawEvent::FileCleanup(handle(0xB)));
+    t.at(1_000);
+    assert_eq!(seed_asks(&mut t, HandleKind::File), [vec![0xB]]);
+    t.reply(Reply::Snapshot(Snapshot {
+        kind: HandleKind::File,
+        taken: ms(1_000),
+        asked: vec![0xB],
+        named: vec![Named { address: 0xB, owner_pid: 100, name: f.into() }],
+        unnamable: vec![],
+    }));
+    let out = t.settle();
+    assert_eq!(
+        files(&out),
+        [
+            ("setattr", r"C:\Windows\System32\kernel32.dll".into(), 100),
+            ("update", r"C:\Windows\System32\kernel32.dll".into(), 100)
+        ]
+    );
+    assert_eq!(forgotten(&mut t), [f], "the Update only");
+    valid(&out);
+}
+
+/// Review m4: every action but SetAttributes makes the caches forget, Delete
+/// and Rename included (a Delete on a known handle, or at a delete-on-close
+/// Cleanup, asks `forgets_cached`).
+#[test]
+fn only_a_set_attributes_leaves_the_caches_alone() {
+    let file_result = atlas_schema::File { name: String::new(), path: String::new(), hashes: None, signature: None };
+    let actions = [
+        (FileAction::Create, true),
+        (FileAction::Read, true),
+        (FileAction::Update, true),
+        (FileAction::Delete, true),
+        (FileAction::Rename { file_result }, true),
+        (FileAction::Open, true),
+        (FileAction::SetAttributes, false),
+    ];
+    for (action, forgets) in actions {
+        assert_eq!(super::file::forgets_cached(&action), forgets, "{action:?}");
+    }
+}
+
 /// The renames' results, the only part a rename of an unknown handle carries.
 fn rename_results(events: &[Event]) -> Vec<String> {
     events
```

- [ ] **Step 2: `forgets_cached`, asked by the two senders that can carry a SetAttributes.** On a known handle, `file_action_on` asks it before sending. The seeder's answer reads the waiting event's action while it fills the event in, and sends only if it forgets (an event that is not a file event keeps the old behaviour). The other senders (a Delete from its request's path, a Rename's source) are unchanged.

```diff
--- a/crates/atlas-agent/src/pipeline/file.rs
+++ b/crates/atlas-agent/src/pipeline/file.rs
@@ -380,7 +380,9 @@ impl<L: Lookups> Pipeline<L> {
         let known = self.files.touch(fo).and_then(|e| e.nt.clone());
         match known {
             Some(nt) => {
-                self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
+                if forgets_cached(&action) {
+                    self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
+                }
                 let (file, expand) = self.emit_file(&nt, Some(fo));
                 let ev = self.file_event(ts, actor, file, action);
                 self.push_with(ev, vec![], expansion(expand, Expansion::file(Some(fo))));
@@ -682,6 +684,15 @@ pub(crate) fn seeded_entry(nt: &str, opener: Option<ProcessRef>, taken: i64) ->
     entry(Some(nt.to_string()), opener, taken)
 }
 
+/// Whether an action makes the hash and 8.3 caches forget the file's path
+/// (§6.3, §7.2): a change of content or of names. A SetAttributes changes
+/// neither, and the System process sets the basic information of the DLLs
+/// that starting processes load, so forgetting there re-hashed them on
+/// every launch (plan 1b-4a-p1, D6).
+pub(super) fn forgets_cached(action: &FileAction) -> bool {
+    !matches!(action, FileAction::SetAttributes)
+}
+
 /// Fills a file event waiting for a seeded name.
 pub(crate) fn fill_file_event(e: &mut Event, fill: Fill, file: File, actor: Option<ProcessRef>) {
     if let EventKind::File(f) = &mut e.kind {
```

```diff
--- a/crates/atlas-agent/src/pipeline/seed.rs
+++ b/crates/atlas-agent/src/pipeline/seed.rs
@@ -12,10 +12,10 @@
 
 use std::collections::{HashMap, HashSet};
 
-use atlas_schema::File;
+use atlas_schema::{EventKind, File};
 
 use super::Pipeline;
-use super::file::{Fill, fill_file_event};
+use super::file::{Fill, fill_file_event, forgets_cached};
 use crate::completion::Reason;
 use crate::config::Ticks;
 use crate::process::file_name;
@@ -244,9 +244,17 @@ impl<L: Lookups> Pipeline<L> {
                 continue; // left to its deadline
             }
             let (f, a) = (file.clone(), opener.clone());
-            self.completion.update(id, |e| fill_file_event(e, fill, f, a));
+            let mut forget = true;
+            self.completion.update(id, |e| {
+                if let EventKind::File(x) = &e.kind {
+                    forget = forgets_cached(&x.action);
+                }
+                fill_file_event(e, fill, f, a)
+            });
             self.completion.resolve(id, Reason::Seeder);
-            self.requests.push(Request::InvalidateHash { nt_path: nt.to_string() });
+            if forget {
+                self.requests.push(Request::InvalidateHash { nt_path: nt.to_string() });
+            }
         }
     }
 
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 405 passed, 18 ignored (`atlas-agent` unit tests: 220).
```powershell
git add crates/atlas-agent
git commit -m "fix(agent): a SetAttributes leaves the hash and 8.3 caches alone"
```

### Task 2: Sensor Health counts each session's events, the log each thread's CPU

**Files:**
- Modify: `crates/atlas-etw/src/session/consumer.rs`, `crates/atlas-etw/src/session/mod.rs`, `crates/atlas-etw/tests/live.rs`
- Modify: `crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto`
- Modify: `crates/atlas-schema/src/classes/sensor_health.rs`, `crates/atlas-schema/tests/common/mod.rs`, `crates/atlas-schema/tests/fixtures/sensor_health_report.json`
- Modify: `crates/atlas-agent/Cargo.toml`, `crates/atlas-agent/src/health.rs`, `crates/atlas-agent/src/dump.rs`, `crates/atlas-agent/src/store.rs`, `crates/atlas-agent/src/win/agent.rs`, `crates/atlas-agent/src/win/health.rs`, `crates/atlas-agent/src/win/run.rs`, `crates/atlas-agent/tests/live.rs`, `crates/atlas-agent/tests/binary.rs`

- [ ] **Step 1: The consumer's counters are the caller's** (Q5, review M1). `ConsumerCounters` holds a consumer's panic and delivered-event counters; `consume_with` takes them from the caller (`consume` keeps its old behaviour with new ones). The trampoline adds one to `received` before it calls the callback, so an event the callback panics on is counted too. A unit test calls the trampoline directly with a zeroed record (three events, the second panicking) and checks each event is counted before its callback runs. The live test wraps Session A's callback to count its calls, and compares after `close` (F6).

```diff
--- a/crates/atlas-etw/src/session/consumer.rs
+++ b/crates/atlas-etw/src/session/consumer.rs
@@ -110,11 +110,22 @@ impl<'a> EventRecord<'a> {
 
 type Callback = Box<dyn FnMut(&EventRecord) + Send>;
 
+/// A consumer's counters, owned by the caller so they outlive one consumer: a
+/// restarted session's new consumer takes the same ones, and the counts never
+/// go back (plan 1b-4a-p1, review M1).
+#[derive(Debug, Clone, Default)]
+pub struct ConsumerCounters {
+    /// Callbacks that panicked (each was caught and the event skipped).
+    pub panics: Arc<AtomicU64>,
+    /// Events delivered, counted before the callback runs.
+    pub received: Arc<AtomicU64>,
+}
+
 /// A consumer thread. Close it after stopping its session; dropping it closes it too.
 pub struct Consumer {
     handle: PROCESSTRACE_HANDLE,
     thread: Option<JoinHandle<u32>>,
-    panics: Arc<AtomicU64>,
+    counters: ConsumerCounters,
     qpc_frequency: i64,
     closed: bool,
 }
@@ -124,7 +135,11 @@ const INVALID: u64 = u64::MAX;
 
 struct Context {
     callback: Callback,
-    panics: Arc<AtomicU64>,
+    counters: ConsumerCounters,
+}
+
+fn context(callback: Callback, counters: &ConsumerCounters) -> Context {
+    Context { callback, counters: counters.clone() }
 }
 
 unsafe extern "system" fn trampoline(rec: *mut EVENT_RECORD) {
@@ -134,9 +149,10 @@ unsafe extern "system" fn trampoline(rec: *mut EVENT_RECORD) {
     unsafe {
         let rec = &*rec;
         let ctx = &mut *rec.UserContext.cast::<Context>();
+        ctx.counters.received.fetch_add(1, Ordering::Relaxed);
         let cb = &mut ctx.callback;
         if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(&EventRecord { rec }))).is_err() {
-            ctx.panics.fetch_add(1, Ordering::Relaxed);
+            ctx.counters.panics.fetch_add(1, Ordering::Relaxed);
         }
     }
 }
@@ -144,9 +160,18 @@ unsafe extern "system" fn trampoline(rec: *mut EVENT_RECORD) {
 /// Opens a real-time consumer on the session `name` and runs `ProcessTrace` on
 /// a new thread, calling `callback` for every event, the session's own header
 /// event included (its `meta()` is `None`). The callback must stay fast (§3.2).
+/// Its counters start at 0; see [`consume_with`] for counters that outlive it.
 pub fn consume(name: &str, callback: impl FnMut(&EventRecord) + Send + 'static) -> Result<Consumer, EtwError> {
-    let panics = Arc::new(AtomicU64::new(0));
-    let ctx = Box::into_raw(Box::new(Context { callback: Box::new(callback), panics: panics.clone() }));
+    consume_with(name, callback, ConsumerCounters::default())
+}
+
+/// [`consume`], counting into the caller's `counters` (review M1).
+pub fn consume_with(
+    name: &str,
+    callback: impl FnMut(&EventRecord) + Send + 'static,
+    counters: ConsumerCounters,
+) -> Result<Consumer, EtwError> {
+    let ctx = Box::into_raw(Box::new(context(Box::new(callback), &counters)));
     let mut name_w: Vec<u16> = name.encode_utf16().chain([0]).collect();
     // SAFETY: an all-zero EVENT_TRACE_LOGFILEW is valid; the fields set below are
     // the ones a real-time EVENT_RECORD consumer needs.
@@ -183,7 +208,7 @@ pub fn consume(name: &str, callback: impl FnMut(&EventRecord) + Send + 'static)
             }
             EtwError { op: "spawn consumer thread", code: 0 }
         })?;
-    Ok(Consumer { handle, thread: Some(thread), panics, qpc_frequency, closed: false })
+    Ok(Consumer { handle, thread: Some(thread), counters, qpc_frequency, closed: false })
 }
 
 impl Consumer {
@@ -195,13 +220,26 @@ impl Consumer {
 
     /// Callbacks that panicked (each was caught and the event skipped).
     pub fn panics(&self) -> u64 {
-        self.panics.load(Ordering::Relaxed)
+        self.counters.panics.load(Ordering::Relaxed)
     }
 
     /// The counter behind [`Consumer::panics`], for a thread that reports it
     /// while the consumer runs (the agent's Sensor Health, plan 1b-4a).
     pub fn panic_counter(&self) -> Arc<AtomicU64> {
-        self.panics.clone()
+        self.counters.panics.clone()
+    }
+
+    /// Events delivered to the callback, counted before it runs: every event
+    /// the session delivered, the session's header event and any the callback
+    /// discards or panics on included.
+    pub fn received(&self) -> u64 {
+        self.counters.received.load(Ordering::Relaxed)
+    }
+
+    /// The counter behind [`Consumer::received`], for a thread that reports it
+    /// while the consumer runs (the agent's Sensor Health, plan 1b-4a-p1).
+    pub fn received_counter(&self) -> Arc<AtomicU64> {
+        self.counters.received.clone()
     }
 
     /// `ProcessTrace` has returned: the session stopped or was destroyed (§9.1).
@@ -230,3 +268,40 @@ impl Drop for Consumer {
         self.close_inner();
     }
 }
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    /// The trampoline counts every event before the callback runs (the
+    /// callback sees its own event counted), those the callback panics on as
+    /// well, into the caller's counters (plan 1b-4a-p1, review M1).
+    #[test]
+    fn the_trampoline_counts_every_event_and_every_panic() {
+        let counters = ConsumerCounters::default();
+        let (calls, before) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
+        let (c, b, r) = (calls.clone(), before.clone(), counters.received.clone());
+        let mut ctx = context(
+            Box::new(move |_| {
+                let n = c.fetch_add(1, Ordering::Relaxed);
+                if r.load(Ordering::Relaxed) == n + 1 {
+                    b.fetch_add(1, Ordering::Relaxed);
+                }
+                if n == 1 {
+                    panic!("the second event");
+                }
+            }),
+            &counters,
+        );
+        // SAFETY: an all-zero EVENT_RECORD is valid; the callback never reads it.
+        let mut rec: EVENT_RECORD = unsafe { std::mem::zeroed() };
+        rec.UserContext = (&raw mut ctx).cast();
+        for _ in 0..3 {
+            // SAFETY: `rec` is valid and its UserContext is the live `ctx`.
+            unsafe { trampoline(&mut rec) };
+        }
+        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
+        assert_eq!((n(&counters.received), n(&calls), n(&counters.panics)), (3, 3, 1));
+        assert_eq!(n(&before), 3, "each event counted before its callback");
+    }
+}
```

```diff
--- a/crates/atlas-etw/src/session/mod.rs
+++ b/crates/atlas-etw/src/session/mod.rs
@@ -7,7 +7,7 @@ mod consumer;
 mod gate;
 pub mod tdh;
 
-pub use consumer::{Consumer, EventRecord, consume};
+pub use consumer::{Consumer, ConsumerCounters, EventRecord, consume, consume_with};
 pub use gate::{Verdict, VersionGate, verdict};
 
 use crate::Provider;
```

```diff
--- a/crates/atlas-etw/tests/live.rs
+++ b/crates/atlas-etw/tests/live.rs
@@ -26,6 +26,7 @@ use std::collections::{BTreeMap, BTreeSet, HashSet};
 use std::io::{Read, Write};
 use std::net::{TcpListener, TcpStream, UdpSocket};
 use std::os::windows::fs::OpenOptionsExt;
+use std::sync::atomic::{AtomicU64, Ordering};
 use std::sync::{Arc, Mutex};
 use std::time::{Duration, Instant};
 use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
@@ -654,7 +655,17 @@ fn live_scenario() {
 
     let sa = Session::start(&Config::session_a(SA)).expect("start A");
     let sb = Session::start(&Config::session_b(SB)).expect("start B");
-    let ca = session::consume(SA, tree_callback(tree.clone(), sink.clone(), unrequested.clone())).expect("consume A");
+    // Session A's callback counts its calls, for the consumer's own count.
+    let delivered_a = Arc::new(AtomicU64::new(0));
+    let ca = {
+        let (delivered, mut inner) =
+            (delivered_a.clone(), tree_callback(tree.clone(), sink.clone(), unrequested.clone()));
+        session::consume(SA, move |rec: &EventRecord| {
+            delivered.fetch_add(1, Ordering::Relaxed);
+            inner(rec);
+        })
+        .expect("consume A")
+    };
     let cb = session::consume(SB, tree_callback(tree.clone(), sink.clone(), unrequested.clone())).expect("consume B");
     let mut freq = 0;
     // SAFETY: writes one i64.
@@ -741,7 +752,16 @@ fn live_scenario() {
     let info_a = sa.stop().expect("stop A");
     report.check("no_events_lost", info_a.events_lost == 0 && info_a.realtime_buffers_lost == 0, format!("{info_a:?}"));
     let panics = ca.panics();
+    // Both counts after `close`: a stopped session's consumer delivers the
+    // rest of its buffers until then.
+    let received = ca.received_counter();
     report.note("process_trace_status_after_stop", json!(ca.close()));
+    let (received, delivered) = (received.load(Ordering::Relaxed), delivered_a.load(Ordering::Relaxed));
+    report.check(
+        "received_counts_every_delivered_event",
+        received > 0 && received == delivered,
+        format!("received {received}, the callback called {delivered} times"),
+    );
     let _ = sw.stop();
     let _ = cw.close();
     report.check("no_callback_panics", panics == 0, format!("{panics} panics"));
```

- [ ] **Step 2: The schema.** `SensorThroughput`, field 7 of `SensorHealthReport`, through the `counter_group!` macro like the other groups. The round-trip test, the golden sample and the property strategy carry it; regenerate the fixture:

```powershell
$env:ATLAS_UPDATE_FIXTURES = '1'; cargo test -p atlas-schema --test golden; Remove-Item Env:ATLAS_UPDATE_FIXTURES
```

```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
@@ -24,6 +24,7 @@ message SensorHealthReport {
   // Set when buffered events were deleted before delivery (overflow).
   SensorGap gap = 5;
   SensorBuffer buffer = 6;
+  SensorThroughput throughput = 7;
 }
 
 // A count for one event class.
@@ -113,6 +114,15 @@ message SensorHousekeeping {
   optional uint64 service_queue_drops = 18;
 }
 
+// What the agent's ETW sessions delivered to it.
+message SensorThroughput {
+  // ETW events the `Atlas-Sensor` session delivered to the agent during the interval, counted
+  // before parsing: every event, those the agent discards at once included.
+  optional uint64 sensor_session_events_received = 1;
+  // The same for the `Atlas-Process` session.
+  optional uint64 process_session_events_received = 2;
+}
+
 // The agent's own resource use.
 message SensorResources {
   // Process CPU time (user + kernel) during the interval, in nanoseconds.
```

```diff
--- a/crates/atlas-schema/src/classes/sensor_health.rs
+++ b/crates/atlas-schema/src/classes/sensor_health.rs
@@ -34,6 +34,7 @@ pub struct HealthReport {
     /// Set when buffered events were deleted before delivery (overflow).
     pub gap: Option<SensorGap>,
     pub buffer: SensorBuffer,
+    pub throughput: SensorThroughput,
 }
 
 /// A count for one event class.
@@ -204,6 +205,17 @@ counter_group!(
     }
 );
 
+counter_group!(
+    /// What the agent's ETW sessions delivered to it.
+    SensorThroughput, "throughput" {
+        /// ETW events the `Atlas-Sensor` session delivered, counted before
+        /// parsing: those the agent discards at once included.
+        sensor_session_events_received: Option<u64>,
+        /// The same for the `Atlas-Process` session.
+        process_session_events_received: Option<u64>,
+    }
+);
+
 counter_group!(
     /// The agent's own resource use.
     SensorResources, "resources" {
@@ -263,6 +275,7 @@ impl From<SensorHealthActivity> for wire::SensorHealth {
                 resources: r.resources.into_wire(),
                 gap: r.gap.map(Into::into),
                 buffer: r.buffer.into_wire(),
+                throughput: r.throughput.into_wire(),
             }),
         };
         Self { interval_start: v.interval_start, activity: Some(activity) }
@@ -283,6 +296,7 @@ impl SensorHealthActivity {
                     None => None,
                 },
                 buffer: SensorBuffer::from_wire(r.buffer)?,
+                throughput: SensorThroughput::from_wire(r.throughput)?,
             })),
         };
         Ok(Self { interval_start: w.interval_start, action })
@@ -340,6 +354,10 @@ mod tests {
             resources: SensorResources { cpu_time: Some(250_000_000), working_set: Some(40 << 20) },
             gap: Some(SensorGap { first_time: Some(1), last_time: Some(2), events: 9 }),
             buffer: SensorBuffer { failing: Some(true), write_errors: Some(3), ..Default::default() },
+            throughput: SensorThroughput {
+                sensor_session_events_received: Some(600_000),
+                process_session_events_received: Some(0),
+            },
         });
         assert_eq!(SensorHealthActivity::from_wire(a.clone().into()).unwrap(), a);
     }
```

```diff
--- a/crates/atlas-schema/tests/common/mod.rs
+++ b/crates/atlas-schema/tests/common/mod.rs
@@ -16,7 +16,7 @@ use atlas_schema::classes::registry::{
 };
 use atlas_schema::classes::sensor_health::{
     ClassCount, HealthReport, SensorBuffer, SensorGap, SensorHealthAction, SensorHealthActivity, SensorHousekeeping,
-    SensorLoss, SensorQuality, SensorResources,
+    SensorLoss, SensorQuality, SensorResources, SensorThroughput,
 };
 use atlas_schema::*;
 use proptest::prelude::*;
@@ -257,6 +257,10 @@ pub fn samples() -> Vec<(&'static str, Event)> {
                         disk_bytes: Some(734_003_200),
                         ..Default::default()
                     },
+                    throughput: SensorThroughput {
+                        sensor_session_events_received: Some(612_408),
+                        process_session_events_received: Some(1_120),
+                    },
                 })),
             })),
         ),
@@ -439,6 +443,10 @@ fn arb_health_report() -> BoxedStrategy<HealthReport> {
         });
     let resources =
         (counter(), counter()).prop_map(|(cpu_time, working_set)| SensorResources { cpu_time, working_set });
+    let throughput =
+        (counter(), counter()).prop_map(|(sensor_session_events_received, process_session_events_received)| {
+            SensorThroughput { sensor_session_events_received, process_session_events_received }
+        });
     // Gap times are both present (and ordered) or both absent.
     let times = prop::option::of((any::<i64>(), any::<i64>())).prop_map(|t| match t {
         Some((a, b)) => (Some(a.min(b)), Some(a.max(b))),
@@ -465,14 +473,15 @@ fn arb_health_report() -> BoxedStrategy<HealthReport> {
             corrupt_segments: b.3,
             disk_bytes: b.4,
         });
-    (loss, quality, housekeeping, resources, gap, buffer)
-        .prop_map(|(loss, quality, housekeeping, resources, gap, buffer)| HealthReport {
+    (loss, quality, housekeeping, resources, gap, buffer, throughput)
+        .prop_map(|(loss, quality, housekeeping, resources, gap, buffer, throughput)| HealthReport {
             loss,
             quality,
             housekeeping,
             resources,
             gap,
             buffer,
+            throughput,
         })
         .boxed()
 }
```

```diff
--- a/crates/atlas-schema/tests/fixtures/sensor_health_report.json
+++ b/crates/atlas-schema/tests/fixtures/sensor_health_report.json
@@ -46,6 +46,10 @@
         "writeErrors": "0",
         "failing": false,
         "diskBytes": "734003200"
+      },
+      "throughput": {
+        "sensorSessionEventsReceived": "612408",
+        "processSessionEventsReceived": "1120"
       }
     }
   }
```

- [ ] **Step 3: The report's portable part.** `Snapshot::received` (Session A's, then B's); the report's `throughput` is their difference. `Snapshot::threads` is a `ThreadList`, each thread's name and CPU by (id, creation time), `None` when not listed; `advance` carries the last listing. `Diagnostics::threads` sums each thread's difference by name (review m1: a thread that ends loses only its time since the last listing), most first, zero left out, with the listings' own interval. `at_start` takes the start time, the first listing's base. `Diagnostics` is no longer `Copy`.

```diff
--- a/crates/atlas-agent/src/health.rs
+++ b/crates/atlas-agent/src/health.rs
@@ -13,7 +13,7 @@ use std::collections::BTreeMap;
 
 use atlas_schema::classes::sensor_health::{
     ClassCount, HealthReport, SensorHealthAction, SensorHealthActivity, SensorHousekeeping, SensorLoss, SensorQuality,
-    SensorResources,
+    SensorResources, SensorThroughput,
 };
 use atlas_schema::{Device, Event, EventId, EventKind, EventMeta, Sensor};
 
@@ -36,6 +36,9 @@ pub struct Snapshot {
     pub intake: IntakeTotals,
     pub services: ServiceTotals,
     pub callback_panics: u64,
+    /// ETW events each consumer was delivered, Session A's then Session B's
+    /// (`throughput`, plan 1b-4a-p1).
+    pub received: [u64; 2],
     pub sensor_session: Option<SessionLoss>,
     pub process_session: Option<SessionLoss>,
     /// Events the buffer could not take: the sink's channel was full, or the
@@ -46,17 +49,32 @@ pub struct Snapshot {
     pub cpu_time: Option<u64>,
     /// A gauge, bytes.
     pub working_set: Option<u64>,
+    /// The last listing of the agent's threads, for the diagnostic log (plan
+    /// 1b-4a-p1); `None` when not listed this time.
+    pub threads: Option<ThreadList>,
     pub seeding: bool,
 }
 
+/// One listing of the agent's threads (plan 1b-4a-p1).
+#[derive(Debug, Clone, Default, PartialEq, Eq)]
+pub struct ThreadList {
+    /// When it was taken, Unix ns.
+    pub at: i64,
+    /// Each thread's name and CPU time (user + kernel, ns), by thread id and
+    /// creation time (a thread id is reused once its thread ends).
+    pub threads: BTreeMap<(u32, u64), (String, u64)>,
+}
+
 impl Snapshot {
-    /// The state at start: everything zero, the sessions just created.
-    pub fn at_start(seeding: bool) -> Snapshot {
+    /// The state at start (`start`, Unix ns): everything zero, the sessions
+    /// just created, no thread listed yet (each thread then counts in full).
+    pub fn at_start(seeding: bool, start: i64) -> Snapshot {
         Snapshot {
             pipeline: Some(Counters::default()),
             sensor_session: Some(SessionLoss::default()),
             process_session: Some(SessionLoss::default()),
             cpu_time: Some(0),
+            threads: Some(ThreadList { at: start, threads: BTreeMap::new() }),
             seeding,
             ..Snapshot::default()
         }
@@ -70,13 +88,14 @@ impl Snapshot {
             sensor_session: now.sensor_session.or(self.sensor_session),
             process_session: now.process_session.or(self.process_session),
             cpu_time: now.cpu_time.or(self.cpu_time),
+            threads: now.threads.clone().or_else(|| self.threads.clone()),
             ..now.clone()
         }
     }
 }
 
 /// Figures for the diagnostic log only: no schema field (plan 1b-4a, D5).
-#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
+#[derive(Debug, Clone, Default, PartialEq, Eq)]
 pub struct Diagnostics {
     /// The two parts of `quality.file_cleanup_unpaired` (plan 1b-3c, R-M4).
     pub cleanup_unpaired: u64,
@@ -86,6 +105,14 @@ pub struct Diagnostics {
     pub self_filtered: Option<u64>,
     pub op_end_discarded: u64,
     pub fast_reads: u64,
+    /// CPU time (ns) each thread name used since the last listing, threads
+    /// sharing a name summed, most first, those that used none left out (plan
+    /// 1b-4a-p1). A thread that ended since loses its time since the last
+    /// listing; one that started counts in full.
+    pub threads: Option<Vec<(String, u64)>>,
+    /// The time between the two listings, ns: it differs from the report's
+    /// interval when the threads were last listed before the last report.
+    pub threads_interval: Option<i64>,
 }
 
 fn d(prev: u64, now: u64) -> u64 {
@@ -104,6 +131,18 @@ fn d_classes(prev: &BTreeMap<Class, u64>, now: &BTreeMap<Class, u64>) -> Vec<Cla
         .collect()
 }
 
+/// Each thread's difference, then summed by name, most first, zero left out.
+fn d_threads(prev: &ThreadList, now: &ThreadList) -> Vec<(String, u64)> {
+    let mut by_name: BTreeMap<&str, u64> = BTreeMap::new();
+    for (key, (name, cpu)) in &now.threads {
+        let base = prev.threads.get(key).map_or(0, |(_, c)| *c);
+        *by_name.entry(name).or_default() += d(base, *cpu);
+    }
+    let mut v: Vec<_> = by_name.into_iter().filter(|(_, n)| *n > 0).map(|(k, n)| (k.to_owned(), n)).collect();
+    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
+    v
+}
+
 /// The report for the interval from `prev` to `now`, without the buffer's
 /// figures (the writer adds them), and the figures for the log.
 pub fn report(prev: &Snapshot, now: &Snapshot) -> (HealthReport, Diagnostics) {
@@ -184,14 +223,20 @@ pub fn report(prev: &Snapshot, now: &Snapshot) -> (HealthReport, Diagnostics) {
         cpu_time: prev.cpu_time.zip(now.cpu_time).map(|(a, b)| d(a, b)),
         working_set: now.working_set,
     };
+    let throughput = SensorThroughput {
+        sensor_session_events_received: Some(d(prev.received[0], now.received[0])),
+        process_session_events_received: Some(d(prev.received[1], now.received[1])),
+    };
     let diagnostics = Diagnostics {
         cleanup_unpaired: d(i0.cleanup_unpaired, i1.cleanup_unpaired),
         cleanup_outcome_late: d(i0.cleanup_outcome_late, i1.cleanup_outcome_late),
         self_filtered: p(|c| c.self_filtered),
         op_end_discarded: d(i0.op_end_discarded, i1.op_end_discarded),
         fast_reads: d(i0.fast_reads, i1.fast_reads),
+        threads: prev.threads.as_ref().zip(now.threads.as_ref()).map(|(a, b)| d_threads(a, b)),
+        threads_interval: prev.threads.as_ref().zip(now.threads.as_ref()).map(|(a, b)| b.at - a.at),
     };
-    let report = HealthReport { loss, quality, housekeeping, resources, ..HealthReport::default() };
+    let report = HealthReport { loss, quality, housekeeping, resources, throughput, ..HealthReport::default() };
     (report, diagnostics)
 }
 
@@ -244,18 +289,76 @@ mod tests {
             },
             services: ServiceTotals { seeder_handles_named: 9, seeder_stuck_helpers: 1, ..Default::default() },
             callback_panics: 0,
+            received: [120_000, 300],
             sensor_session: Some(SessionLoss { events_lost: 4, buffers_lost: 0 }),
             process_session: Some(SessionLoss::default()),
             buffer_drops: 6,
             cpu_time: Some(1_000),
             working_set: Some(50 << 20),
+            threads: Some(listing(1_000, &[(1, 10, "atlas-hash-0", 900), (2, 10, "atlas-pipeline", 100)])),
             seeding: true,
         }
     }
 
+    /// A thread listing: (thread id, creation time, name, CPU ns).
+    fn listing(at: i64, threads: &[(u32, u64, &str, u64)]) -> ThreadList {
+        ThreadList { at, threads: threads.iter().map(|&(t, c, n, cpu)| ((t, c), (n.to_owned(), cpu))).collect() }
+    }
+
+    /// Per-thread CPU (plan 1b-4a-p1, review m1): each thread's difference,
+    /// summed by name, most first, zero left out; a thread that ended loses
+    /// only its time since the last listing, even when another thread shares
+    /// its name; a new thread (or a reused id) counts in full; an unlisted
+    /// interval is absent, then covered; the listings' own interval.
+    #[test]
+    fn thread_cpu_is_differenced_by_thread_then_summed_by_name() {
+        let (_, diag) = report(&Snapshot::at_start(true, 0), &busy());
+        assert_eq!(diag.threads, Some(vec![("atlas-hash-0".into(), 900), ("atlas-pipeline".into(), 100)]));
+        assert_eq!(diag.threads_interval, Some(1_000));
+
+        // Name X: T7 (10 s so far) ends, T8 (1 s) runs on; a reader starts;
+        // thread 2's id is reused by a new pipeline thread (creation 20).
+        let mut prev = busy();
+        prev.threads = Some(listing(
+            1_000,
+            &[(1, 10, "atlas-hash-0", 900), (2, 10, "atlas-pipeline", 100), (7, 1, "X", 10_000), (8, 2, "X", 1_000)],
+        ));
+        let mut now = busy();
+        now.threads = Some(listing(
+            61_000,
+            &[
+                (1, 10, "atlas-hash-0", 950),
+                (2, 20, "atlas-pipeline", 40),
+                (8, 2, "X", 1_500),
+                (9, 30, "atlas-reader", 7),
+            ],
+        ));
+        let (_, diag) = report(&prev, &now);
+        assert_eq!(
+            diag.threads,
+            Some(vec![
+                ("X".into(), 500),
+                ("atlas-hash-0".into(), 50),
+                ("atlas-pipeline".into(), 40),
+                ("atlas-reader".into(), 7),
+            ])
+        );
+        assert_eq!(diag.threads_interval, Some(60_000));
+        let mut idle = prev.clone();
+        idle.threads.as_mut().unwrap().at = 2_000;
+        assert_eq!(report(&prev, &idle).1.threads, Some(vec![]), "nothing used, nothing listed");
+
+        let mut blind = now.clone();
+        blind.threads = None;
+        assert_eq!(report(&prev, &blind).1.threads, None);
+        assert_eq!(report(&prev, &blind).1.threads_interval, None);
+        let from = prev.advance(&blind);
+        assert_eq!(report(&from, &now).1.threads.unwrap()[0], ("X".into(), 500));
+    }
+
     #[test]
     fn a_report_is_the_difference_of_two_snapshots() {
-        let (r, diag) = report(&Snapshot::at_start(true), &busy());
+        let (r, diag) = report(&Snapshot::at_start(true, 0), &busy());
         assert_eq!(r.loss.kernel_queue_drops, Some(7));
         assert_eq!(r.loss.sensor_session_events_lost, Some(4));
         assert_eq!(r.loss.process_session_events_lost, Some(0));
@@ -269,13 +372,18 @@ mod tests {
         assert_eq!(r.housekeeping.seeding_enabled, Some(true));
         assert_eq!(r.resources.cpu_time, Some(1_000));
         assert_eq!(r.resources.working_set, Some(50 << 20));
+        assert_eq!(r.throughput.sensor_session_events_received, Some(120_000));
+        assert_eq!(r.throughput.process_session_events_received, Some(300));
         assert_eq!((diag.cleanup_unpaired, diag.cleanup_outcome_late, diag.self_filtered), (2, 1, Some(40)));
 
         // The next interval counts only what is new; gauges are taken as they are.
         let mut later = busy();
         later.intake.kernel_queue_drops = 10;
         later.pipeline.as_mut().unwrap().actor_dropped.insert(Class::File, 3);
+        later.received[0] += 5_000;
         let (r, _) = report(&busy(), &later);
+        assert_eq!(r.throughput.sensor_session_events_received, Some(5_000));
+        assert_eq!(r.throughput.process_session_events_received, Some(0));
         assert_eq!(r.loss.kernel_queue_drops, Some(3));
         assert_eq!(r.loss.sensor_session_events_lost, Some(0));
         assert!(r.loss.actor_dropped.is_empty(), "zero counts are left out");
@@ -296,7 +404,7 @@ mod tests {
 
     #[test]
     fn what_was_not_read_is_absent_and_counted_later() {
-        let start = Snapshot::at_start(true);
+        let start = Snapshot::at_start(true, 0);
         let mut blind = busy();
         blind.pipeline = None;
         blind.sensor_session = None;
@@ -341,7 +449,7 @@ mod tests {
     #[test]
     fn the_event_carries_the_interval_and_validates() {
         let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
-        let (r, _) = report(&Snapshot::at_start(false), &busy());
+        let (r, _) = report(&Snapshot::at_start(false, 0), &busy());
         let e = event(EventId::new_v7(), 2_000, device, 1_000, r.clone());
         let back = atlas_schema::decode_event(&atlas_schema::encode_event(e.clone())).unwrap();
         assert_eq!(back, e);
```

```diff
--- a/crates/atlas-agent/src/dump.rs
+++ b/crates/atlas-agent/src/dump.rs
@@ -160,7 +160,7 @@ mod tests {
     use std::time::Instant;
 
     fn health(time: i64) -> Event {
-        let (r, _) = report(&Snapshot::at_start(true), &Snapshot::at_start(true));
+        let (r, _) = report(&Snapshot::at_start(true, 0), &Snapshot::at_start(true, 0));
         let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
         event(EventId::new_v7(), time, device, 0, r)
     }
```

```diff
--- a/crates/atlas-agent/src/store.rs
+++ b/crates/atlas-agent/src/store.rs
@@ -355,7 +355,7 @@ mod tests {
     }
 
     fn health(time: i64) -> Event {
-        let (r, _) = report(&Snapshot::at_start(true), &Snapshot::at_start(true));
+        let (r, _) = report(&Snapshot::at_start(true, 0), &Snapshot::at_start(true, 0));
         event(EventId::new_v7(), time, device(), 0, r)
     }
 
```

- [ ] **Step 4: The Windows part.** `Running` creates each session's `ConsumerCounters` once and hands them to `consume_with`; the health sources and `Stopped` read those (a restarted session, plan 1b-4b, takes the same ones). `threads()` takes a thread snapshot, keeps this process's threads (a reused id is checked with `GetProcessIdOfThread`), and reads each one's description and times; it costs about 45 ms (F5), so the health thread lists them at most once a report period, for a report of either kind (Q7, review m3), and `run` lists them for the final report just before the sensor stops (review m2). The sources carry the lister as a closure, so a test can count its calls. The log line gets both delivered counts, `threads_ms` and `threads` (`name=ms`, most first, or `-`).

```diff
--- a/crates/atlas-agent/Cargo.toml
+++ b/crates/atlas-agent/Cargo.toml
@@ -43,6 +43,7 @@ features = [
     "Win32_Security_WinTrust",
     "Win32_Storage_FileSystem",
     "Win32_System_Console",
+    "Win32_System_Diagnostics_ToolHelp",
     "Win32_System_IO",
     "Win32_System_Ioctl",
     "Win32_System_Kernel",
```

```diff
--- a/crates/atlas-agent/src/win/agent.rs
+++ b/crates/atlas-agent/src/win/agent.rs
@@ -15,7 +15,7 @@ use std::thread::JoinHandle;
 use std::time::Duration;
 
 use atlas_etw::providers;
-use atlas_etw::session::{self, Consumer, EtwError, EventRecord, Session, SessionInfo, VersionGate};
+use atlas_etw::session::{self, Consumer, ConsumerCounters, EtwError, EventRecord, Session, SessionInfo, VersionGate};
 use atlas_schema::{Event, EventId};
 
 use super::identity::{self, IdentityError, Started};
@@ -25,6 +25,7 @@ use super::util::qpc_now;
 use crate::config::{Config, ServiceConfig};
 use crate::counters::{Counters, IntakeCounters, ServiceCounters};
 use crate::driver::{Control, Driver, DriverConfig};
+use crate::health::ThreadList;
 use crate::input::{Header, Session as Which};
 use crate::intake::{Intake, Queues};
 use crate::pipeline::{Pipeline, Setup};
@@ -72,6 +73,9 @@ pub struct Running {
     sensor: Option<Session>,
     process: Option<Session>,
     consumers: Vec<Consumer>,
+    /// Each session's callback counters, Session A's then B's: owned here, so
+    /// a restarted session's consumer counts on into the same ones (review M1).
+    counters: [ConsumerCounters; 2],
     /// Senders kept so the queues stay open while a session is restarted
     /// (plan 1b-4's watchdog); dropped at stop, which ends the pipeline loop.
     spare: Option<Queues>,
@@ -97,6 +101,8 @@ pub struct Stopped {
     pub services: Arc<ServiceCounters>,
     /// Callbacks that panicked, both sessions (`loss.callback_panics`).
     pub callback_panics: u64,
+    /// Events each consumer was delivered, Session A's then B's (`throughput`).
+    pub received: [u64; 2],
     /// The sessions as they stopped: their final loss counters.
     pub sensor_session: Option<SessionInfo>,
     pub process_session: Option<SessionInfo>,
@@ -109,11 +115,16 @@ pub struct HealthSources {
     pub control: Sender<Control<WinLookups>>,
     pub intake: Arc<IntakeCounters>,
     pub services: Arc<ServiceCounters>,
-    /// Both consumers' callback panics.
-    pub panics: Vec<Arc<AtomicU64>>,
+    /// Each session's callback counters (panics, events delivered), Session
+    /// A's then B's: the agent's, so they survive a session restart.
+    pub counters: [ConsumerCounters; 2],
     /// Session A's and Session B's names, for their loss counters.
     pub sessions: [String; 2],
     pub seeding: bool,
+    /// Lists the agent's threads with their CPU time (`win::health::threads`;
+    /// a test counts its calls). It costs ~45 ms, so it runs at most once a
+    /// report period, for a report (plan 1b-4a-p1, Q7, review m3).
+    pub threads: Arc<dyn Fn() -> Option<ThreadList> + Send + Sync>,
 }
 
 /// Starts the sensor (§11.4): identity, the services, both sessions and their
@@ -156,9 +167,12 @@ pub fn start(opts: AgentOptions, sink: impl FnMut(Event) + Send + 'static) -> Re
         sensor.enable(&e).map_err(AgentError::Etw)?;
     }
     let process = Session::start(&session::Config::session_b(&opts.process_session)).map_err(AgentError::Etw)?;
+    let counters = [ConsumerCounters::default(), ConsumerCounters::default()];
     let consumers = vec![
-        session::consume(&opts.sensor_session, callback(Which::Sensor, sensor_intake)).map_err(AgentError::Etw)?,
-        session::consume(&opts.process_session, callback(Which::Process, process_intake)).map_err(AgentError::Etw)?,
+        session::consume_with(&opts.sensor_session, callback(Which::Sensor, sensor_intake), counters[0].clone())
+            .map_err(AgentError::Etw)?,
+        session::consume_with(&opts.process_session, callback(Which::Process, process_intake), counters[1].clone())
+            .map_err(AgentError::Etw)?,
     ];
 
     let service_counters = services.counters();
@@ -175,6 +189,7 @@ pub fn start(opts: AgentOptions, sink: impl FnMut(Event) + Send + 'static) -> Re
         sensor: Some(sensor),
         process: Some(process),
         consumers,
+        counters,
         spare: Some(queues()),
         pipeline: Some(pipeline),
         control,
@@ -232,9 +247,10 @@ impl Running {
             control: self.control.clone(),
             intake: self.intake.clone(),
             services: self.services.clone(),
-            panics: self.consumers.iter().map(Consumer::panic_counter).collect(),
+            counters: self.counters.clone(),
             sessions: self.names.clone(),
             seeding: self.seeding,
+            threads: Arc::new(super::health::threads),
         }
     }
 
@@ -263,7 +279,9 @@ impl Running {
         while self.consumers.iter().any(|c| !c.is_finished()) && t0.elapsed() < Duration::from_secs(10) {
             std::thread::sleep(Duration::from_millis(10));
         }
-        let callback_panics = self.consumers.iter().map(Consumer::panics).sum();
+        let load = |a: &AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
+        let callback_panics = self.counters.iter().map(|c| load(&c.panics)).sum();
+        let received = [0, 1].map(|i| load(&self.counters[i].received));
         // Closing a consumer drops its callback, and with it the intake's senders
         // and Session A's `FastRead`: before the services go (R-m11).
         for c in self.consumers.drain(..) {
@@ -282,6 +300,7 @@ impl Running {
             intake: self.intake.clone(),
             services: self.services.clone(),
             callback_panics,
+            received,
             sensor_session: last[0].take(),
             process_session: last[1].take(),
         }
```

```diff
--- a/crates/atlas-agent/src/win/health.rs
+++ b/crates/atlas-agent/src/win/health.rs
@@ -3,6 +3,7 @@
 //! loss counter rose, it sends a Sensor Health report to the buffer writer and
 //! logs its figures. Plan 1b-4b adds the watchdog's checks to this thread.
 
+use std::collections::BTreeMap;
 use std::sync::Arc;
 use std::sync::atomic::{AtomicU64, Ordering};
 use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel};
@@ -11,14 +12,21 @@ use std::time::{Duration, Instant};
 
 use atlas_etw::session::{SessionInfo, query_by_name};
 use atlas_schema::{Device, EventId};
-use windows::Win32::Foundation::FILETIME;
+use windows::Win32::Foundation::{FILETIME, HLOCAL, LocalFree};
+use windows::Win32::System::Diagnostics::ToolHelp::{
+    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
+};
 use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
-use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
+use windows::Win32::System::Threading::{
+    GetCurrentProcess, GetCurrentProcessId, GetProcessIdOfThread, GetProcessTimes, GetThreadDescription,
+    GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
+};
 
 use super::agent::{HealthSources, Stopped};
 use super::identity;
+use super::util::Owned;
 use crate::counters::Counters;
-use crate::health::{self, Diagnostics, SessionLoss, Snapshot};
+use crate::health::{self, Diagnostics, SessionLoss, Snapshot, ThreadList};
 use crate::store::Record;
 
 #[derive(Debug, Clone, Copy)]
@@ -81,13 +89,21 @@ impl HealthThread {
 fn run(src: &HealthSources, out: &Reporting, first: Interval, cfg: HealthConfig, stop: &Receiver<()>) -> Interval {
     let mut at = first;
     let mut next_report = Instant::now() + cfg.report_every;
+    let mut next_listing = next_report;
     loop {
         match stop.recv_timeout(cfg.check_every) {
             Err(RecvTimeoutError::Timeout) => {}
             Ok(()) | Err(RecvTimeoutError::Disconnected) => return at,
         }
-        let now = snapshot(src, &out.buffer_drops, pipeline_counters(src, cfg.pipeline_timeout));
+        let mut now = snapshot(src, &out.buffer_drops, pipeline_counters(src, cfg.pipeline_timeout));
         if Instant::now() >= next_report || health::loss_rose(&at.from, &now) {
+            // Listing the threads costs ~45 ms (Q7): at most once a period,
+            // for a report of either kind (review m3). A report without them
+            // leaves them to the next listing, which covers the time since.
+            if Instant::now() >= next_listing {
+                now.threads = (src.threads)();
+                next_listing = Instant::now() + cfg.report_every;
+            }
             at = send(out, at, &now);
             next_report = Instant::now() + cfg.report_every;
         }
@@ -129,12 +145,22 @@ fn log(r: &atlas_schema::classes::sensor_health::HealthReport, d: &Diagnostics,
         service_queue_drops = n(r.housekeeping.service_queue_drops),
         pending_overflow = n(r.housekeeping.pending_overflow),
         self_filtered = n(d.self_filtered),
+        sensor_events_received = n(r.throughput.sensor_session_events_received),
+        process_events_received = n(r.throughput.process_session_events_received),
         cpu_ms = n(r.resources.cpu_time.map(|ns| ns / 1_000_000)),
         working_set_mb = n(r.resources.working_set.map(|b| b >> 20)),
+        threads_ms = d.threads_interval.map_or(-1, |ns| ns / 1_000_000),
+        threads = d.threads.as_deref().map_or_else(|| "-".to_owned(), thread_line),
         "health report"
     );
 }
 
+/// `name=ms` for each thread name, most first, as the log line carries them.
+fn thread_line(threads: &[(String, u64)]) -> String {
+    let parts: Vec<String> = threads.iter().map(|(name, ns)| format!("{name}={}", ns / 1_000_000)).collect();
+    parts.join(" ")
+}
+
 /// The pipeline's counters, read on its thread between two passes; `None` if
 /// it does not answer in time (it ended, or is stuck).
 fn pipeline_counters(src: &HealthSources, timeout: Duration) -> Option<Counters> {
@@ -147,16 +173,19 @@ fn session_loss(info: Option<SessionInfo>) -> Option<SessionLoss> {
     info.map(|i| SessionLoss { events_lost: i.events_lost, buffers_lost: i.realtime_buffers_lost })
 }
 
-/// Every source now. `pipeline` is read by the caller: through the control
-/// channel while running, from `Stopped` at the end.
+/// Every source now but the threads (the caller lists them when due).
+/// `pipeline` is read by the caller: through the control channel while
+/// running, from `Stopped` at the end.
 pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<Counters>) -> Snapshot {
     let query = |name: &str| session_loss(query_by_name(name).ok().flatten());
     let (cpu_time, working_set) = resources();
     Snapshot {
+        threads: None,
         pipeline,
         intake: src.intake.totals(),
         services: src.services.totals(),
-        callback_panics: src.panics.iter().map(|p| p.load(Ordering::Relaxed)).sum(),
+        callback_panics: src.counters.iter().map(|c| c.panics.load(Ordering::Relaxed)).sum(),
+        received: [0, 1].map(|i| src.counters[i].received.load(Ordering::Relaxed)),
         sensor_session: query(&src.sessions[0]),
         process_session: query(&src.sessions[1]),
         buffer_drops: buffer_drops.load(Ordering::Relaxed),
@@ -168,14 +197,22 @@ pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<
 
 /// The snapshot for the final report, after the stop: the sessions' last
 /// counters come from their stop; the pipeline's are absent if its thread
-/// panicked.
-pub fn final_snapshot(stopped: &Stopped, buffer_drops: &AtomicU64, seeding: bool) -> Snapshot {
+/// panicked. `threads` was listed just before the stop, while the sensor's
+/// threads still ran (review m2).
+pub fn final_snapshot(
+    stopped: &Stopped,
+    buffer_drops: &AtomicU64,
+    seeding: bool,
+    threads: Option<ThreadList>,
+) -> Snapshot {
     let (cpu_time, working_set) = resources();
     Snapshot {
+        threads,
         pipeline: stopped.pipeline.as_ref().ok().cloned(),
         intake: stopped.intake.totals(),
         services: stopped.services.totals(),
         callback_panics: stopped.callback_panics,
+        received: stopped.received,
         sensor_session: session_loss(stopped.sensor_session),
         process_session: session_loss(stopped.process_session),
         buffer_drops: buffer_drops.load(Ordering::Relaxed),
@@ -199,6 +236,59 @@ fn resources() -> (Option<u64>, Option<u64>) {
     (cpu, ws)
 }
 
+/// This process's threads: each one's name (`(unnamed)` when empty) and CPU
+/// time (user + kernel, ns), by thread id and creation time. `None` if the
+/// threads could not be listed; a thread that cannot be opened, or whose id
+/// now belongs to another process, is left out. It takes a snapshot of every
+/// thread in the system: ~45 ms on the host.
+pub(crate) fn threads() -> Option<ThreadList> {
+    let at = identity::anchor().unix_ns;
+    // SAFETY: a snapshot of every thread in the system; closed by `Owned`.
+    let snap = Owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.ok()?);
+    // SAFETY: reads the calling process's id.
+    let me = unsafe { GetCurrentProcessId() };
+    let mut threads = BTreeMap::new();
+    let mut te = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
+    // SAFETY: `te` is a THREADENTRY32 with its size set.
+    let mut more = unsafe { Thread32First(snap.raw(), &mut te) }.is_ok();
+    while more {
+        if te.th32OwnerProcessID == me
+            && let Some((key, entry)) = thread(te.th32ThreadID, me)
+        {
+            threads.insert(key, entry);
+        }
+        // SAFETY: as for Thread32First.
+        more = unsafe { Thread32Next(snap.raw(), &mut te) }.is_ok();
+    }
+    Some(ThreadList { at, threads })
+}
+
+/// One of `me`'s threads: (id, creation time) and (name, CPU time); `None` if
+/// it cannot be opened or its id now belongs to another process's thread.
+fn thread(tid: u32, me: u32) -> Option<((u32, u64), (String, u64))> {
+    let ft = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) * 100;
+    // SAFETY: opens a thread for querying; closed by `Owned`.
+    let h = Owned(unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) }.ok()?);
+    // SAFETY: an open thread handle. The id may have been reused since the snapshot.
+    if unsafe { GetProcessIdOfThread(h.raw()) } != me {
+        return None;
+    }
+    // SAFETY: the returned string is ours to free with LocalFree.
+    let name = unsafe { GetThreadDescription(h.raw()) }.ok().map(|p| {
+        // SAFETY: a NUL-terminated string from GetThreadDescription.
+        let s = unsafe { p.to_string() }.unwrap_or_default();
+        // SAFETY: frees the string GetThreadDescription allocated.
+        unsafe { LocalFree(Some(HLOCAL(p.0.cast()))) };
+        s
+    });
+    let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| "(unnamed)".to_owned());
+    let (mut c, mut e, mut k, mut u) =
+        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
+    // SAFETY: an open thread handle and four out-structures.
+    unsafe { GetThreadTimes(h.raw(), &mut c, &mut e, &mut k, &mut u) }.ok()?;
+    Some(((tid, ft(c)), (name, ft(k) + ft(u))))
+}
+
 #[cfg(test)]
 mod tests {
     use std::sync::mpsc::sync_channel;
@@ -207,22 +297,38 @@ mod tests {
     use atlas_schema::{BootId, DeviceUid, EventKind};
 
     use super::*;
+    use windows::Win32::System::Threading::GetCurrentThread;
+
     use crate::counters::{IntakeCounters, ServiceCounters};
 
-    /// Sources with no pipeline (its channel is closed) and no sessions.
-    fn sources(intake: &Arc<IntakeCounters>) -> HealthSources {
+    /// Sources with no pipeline (its channel is closed) and no sessions; the
+    /// threads' lister counts its calls in `lists`.
+    fn sources(intake: &Arc<IntakeCounters>, lists: &Arc<AtomicU64>) -> HealthSources {
         let (control, _) = channel();
         let pid = std::process::id();
+        let lists = lists.clone();
         HealthSources {
             control,
             intake: intake.clone(),
             services: Arc::new(ServiceCounters::default()),
-            panics: Vec::new(),
+            counters: Default::default(),
             sessions: [format!("Atlas-Test-None-A-{pid}"), format!("Atlas-Test-None-B-{pid}")],
             seeding: false,
+            threads: Arc::new(move || {
+                lists.fetch_add(1, Ordering::Relaxed);
+                Some(ThreadList::default())
+            }),
         }
     }
 
+    fn reporting() -> (Reporting, Receiver<Record>) {
+        // Room for a report at every check of a test (a loss at each one):
+        // a full channel would block the health thread and hang the test.
+        let (to, rx) = sync_channel(4_096);
+        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
+        (Reporting { to, device, buffer_drops: Arc::new(AtomicU64::new(0)) }, rx)
+    }
+
     /// The next report's time and `loss.kernel_queue_drops`.
     fn next_report(rx: &Receiver<Record>, within: Duration) -> Option<(i64, Option<u64>)> {
         let Record::Health(e) = rx.recv_timeout(within).ok()? else { panic!("not a report") };
@@ -236,34 +342,148 @@ mod tests {
     /// ended.
     #[test]
     fn a_loss_is_reported_at_once_and_restarts_the_period() {
-        let intake = Arc::new(IntakeCounters::default());
-        let (to, rx) = sync_channel(64);
-        let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
-        let out = Reporting { to, device, buffer_drops: Arc::new(AtomicU64::new(0)) };
+        let (intake, lists) = (Arc::new(IntakeCounters::default()), Arc::new(AtomicU64::new(0)));
+        let (out, rx) = reporting();
         let period = Duration::from_millis(1_000);
         let cfg = HealthConfig {
             check_every: Duration::from_millis(20),
             report_every: period,
             pipeline_timeout: Duration::from_millis(1),
         };
-        let first = Interval { from: Snapshot::at_start(false), start: identity::anchor().unix_ns };
-        let health = HealthThread::start(sources(&intake), out, first, cfg);
+        let first =
+            Interval { from: Snapshot::at_start(false, identity::anchor().unix_ns), start: identity::anchor().unix_ns };
+        let health = HealthThread::start(sources(&intake, &lists), out, first, cfg);
 
         std::thread::sleep(Duration::from_millis(400));
         assert!(rx.try_recv().is_err(), "no report before the period without a loss");
         intake.kernel_queue_drops.fetch_add(1, Ordering::Relaxed);
         let (t1, drops) = next_report(&rx, Duration::from_millis(500)).expect("the loss reported at once");
         assert_eq!(drops, Some(1));
+        // Threads are listed at most once a period, for a report (plan
+        // 1b-4a-p1, Q7): none for the ~20 checks and the early loss report.
+        assert_eq!(lists.load(Ordering::Relaxed), 0, "threads listed at a check or too soon");
         // Times are taken as each report is sent: a slow test thread does not
         // shorten the gap.
         let (t2, drops) = next_report(&rx, Duration::from_secs(5)).expect("the periodic report");
         let gap = Duration::from_nanos((t2 - t1) as u64);
         assert!(gap + Duration::from_millis(50) >= period, "{gap:?}: the period did not restart at the loss");
         assert_eq!(drops, Some(0), "counted since the report before");
+        assert_eq!(lists.load(Ordering::Relaxed), 1, "listed once, for the periodic report");
         let at = health.stop();
         assert_eq!(at.start, t2, "the final report starts where the last one ended");
     }
 
+    /// Review m3: under a loss at every check (a report each time), the
+    /// threads are still listed about once a period, not at every report.
+    #[test]
+    fn threads_are_listed_under_sustained_loss() {
+        let (intake, lists) = (Arc::new(IntakeCounters::default()), Arc::new(AtomicU64::new(0)));
+        let (out, rx) = reporting();
+        let cfg = HealthConfig {
+            check_every: Duration::from_millis(20),
+            report_every: Duration::from_millis(300),
+            pipeline_timeout: Duration::from_millis(1),
+        };
+        let start = identity::anchor().unix_ns;
+        let first = Interval { from: Snapshot::at_start(false, start), start };
+        let health = HealthThread::start(sources(&intake, &lists), out, first, cfg);
+        let t0 = Instant::now();
+        while t0.elapsed() < Duration::from_millis(1_500) {
+            intake.kernel_queue_drops.fetch_add(1, Ordering::Relaxed);
+            std::thread::sleep(Duration::from_millis(5));
+        }
+        health.stop();
+        let reports = rx.try_iter().count() as u64;
+        let listed = lists.load(Ordering::Relaxed);
+        assert!(listed >= 2, "{listed} listings in 1.5 s of loss ({reports} reports)");
+        assert!(reports >= 2 * listed, "{listed} listings for {reports} reports: listed at every report");
+    }
+
+    /// Each session's delivered count goes to its own place (plan 1b-4a-p1).
+    #[test]
+    fn each_sessions_received_count_is_read_into_its_place() {
+        let intake = Arc::new(IntakeCounters::default());
+        let src = sources(&intake, &Arc::new(AtomicU64::new(0)));
+        src.counters[0].received.store(5, Ordering::Relaxed);
+        src.counters[1].received.store(7, Ordering::Relaxed);
+        assert_eq!(snapshot(&src, &AtomicU64::new(0), None).received, [5, 7]);
+    }
+
+    /// The calling thread's CPU time (user + kernel), ns.
+    fn own_cpu() -> u64 {
+        let ft = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) * 100;
+        let (mut c, mut e, mut k, mut u) =
+            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
+        // SAFETY: the current-thread pseudo-handle and four out-structures.
+        unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) }.expect("GetThreadTimes");
+        ft(k) + ft(u)
+    }
+
+    /// Burns CPU until this thread used `ns` more: CPU time, not wall time,
+    /// so a busy machine only makes it slower.
+    fn burn(ns: u64) {
+        let end = own_cpu() + ns;
+        let mut x = 0u64;
+        while own_cpu() < end {
+            for _ in 0..10_000 {
+                x = std::hint::black_box(x.wrapping_add(1));
+            }
+        }
+    }
+
+    /// Each thread is listed on its own (two sharing a name are two entries),
+    /// only this process's threads are listed, and ended ones are gone (plan
+    /// 1b-4a-p1, review m1).
+    #[test]
+    fn threads_are_listed_one_by_one() {
+        let name = format!("atlas-test-burn-{}", std::process::id());
+        let (done, burnt) = channel::<()>();
+        let (mut go, mut handles) = (Vec::new(), Vec::new());
+        for _ in 0..2 {
+            let (g, wait) = channel::<()>();
+            let done = done.clone();
+            go.push(g);
+            let t = std::thread::Builder::new().name(name.clone()).spawn(move || {
+                burn(60_000_000);
+                done.send(()).unwrap();
+                wait.recv().unwrap();
+                burn(60_000_000);
+                done.send(()).unwrap();
+                wait.recv().unwrap(); // stays until read
+            });
+            handles.push(t.unwrap());
+        }
+        let step = |go: &[Sender<()>]| {
+            for _ in 0..2 {
+                burnt.recv().unwrap();
+            }
+            let all = threads().expect("listed");
+            let process = resources().0.expect("the process's CPU");
+            assert!(all.threads.values().map(|(_, c)| c).sum::<u64>() <= process, "only this process's threads");
+            for g in go {
+                g.send(()).unwrap();
+            }
+            let ours: Vec<u64> = all.threads.values().filter(|(n, _)| *n == name).map(|(_, c)| *c).collect();
+            assert_eq!(ours.len(), 2, "two threads, two entries");
+            assert!(ours.iter().all(|&c| c >= 60_000_000), "{ours:?}");
+            ours.iter().sum::<u64>()
+        };
+        let first = step(&go);
+        assert!(first >= 120_000_000, "{first} ns: both threads");
+        let second = step(&go);
+        assert!(second >= first + 120_000_000, "{first} then {second} ns");
+        for h in handles {
+            h.join().unwrap();
+        }
+        assert!(!threads().expect("listed").threads.values().any(|(n, _)| *n == name), "ended threads are gone");
+    }
+
+    #[test]
+    fn the_thread_line_is_name_equals_ms() {
+        let line = thread_line(&[("atlas-hash-0".into(), 1_234_000_000), ("atlas-pipeline".into(), 999_999)]);
+        assert_eq!(line, "atlas-hash-0=1234 atlas-pipeline=0");
+    }
+
     #[test]
     fn resources_are_read() {
         let (cpu, ws) = resources();
```

```diff
--- a/crates/atlas-agent/src/win/run.rs
+++ b/crates/atlas-agent/src/win/run.rs
@@ -173,7 +173,7 @@ where
         device: Device { uid: started.identity.device, boot_id: started.identity.boot },
         buffer_drops: store.drops(),
     };
-    let first = Interval { from: Snapshot::at_start(seeding), start: started.anchor.unix_ns };
+    let first = Interval { from: Snapshot::at_start(seeding, started.anchor.unix_ns), start: started.anchor.unix_ns };
     let health = HealthThread::start(running.health_sources(), reporting.clone(), first, opts.health);
 
     tracing::info!("running; Ctrl+C stops");
@@ -199,12 +199,14 @@ where
     let t0 = Instant::now();
     let at = health.stop();
     let drops = store.drops();
+    // Listed before the sensor stops, while its threads still run (review m2).
+    let listing = health::threads();
     let stopped = running.stop();
     if let Err(panic) = &stopped.pipeline {
         // The default hook printed it to stderr only; a service has none.
         tracing::error!("the pipeline thread panicked: {panic}");
     }
-    let last = health::final_snapshot(&stopped, &drops, seeding);
+    let last = health::final_snapshot(&stopped, &drops, seeding, listing);
     health::send(&reporting, at, &last);
     drop(reporting);
     match store.close() {
```

- [ ] **Step 5: The live checks.** The agent live test checks Session A delivered more than B and B more than none, and notes the enrichment counters (F9); the binary test, that every report measures both (A's sum the larger), that the final report counts, that a report lists the threads (and waits for one rather than for any report, review m6); it records the final report's thread line (a thread shows only if it used a clock tick since the last listing, so this is a note, not a check).

```diff
--- a/crates/atlas-agent/tests/live.rs
+++ b/crates/atlas-agent/tests/live.rs
@@ -358,6 +358,11 @@ fn live_agent() {
     report.check("no_queue_drops", get(&i.kernel_queue_drops) == 0 && get(&i.user_queue_drops) == 0, "");
     report.check("no_service_queue_drops", get(&s.service_queue_drops) == 0, "");
     report.check("no_callback_panics", stopped.callback_panics == 0, "");
+    report.check(
+        "both_sessions_delivered_events",
+        stopped.received[0] > stopped.received[1] && stopped.received[1] > 0,
+        format!("{:?}", stopped.received),
+    );
     // The agent's own hashing, value reads and seeding make events; the
     // self-filter must have dropped some, or the check above proves nothing
     // (review R-m6).
@@ -379,6 +384,9 @@ fn live_agent() {
             "op_end_discarded": get(&i.op_end_discarded),
             "seeder_handles_named": get(&s.seeder_handles_named),
             "seeder_table_reads": get(&s.seeder_table_reads),
+            // A launch without a hash: past its deadline, or an error.
+            "enrichment_misses": c.enrichment_misses,
+            "enrichment_errors": c.enrichment_errors,
         }),
     );
     finish(report);
```

```diff
--- a/crates/atlas-agent/tests/binary.rs
+++ b/crates/atlas-agent/tests/binary.rs
@@ -303,7 +303,9 @@ fn binary_run_and_dump() {
     let actor_pid = result["pid"].as_u64().unwrap() as u32;
 
     // ---- a periodic report, then the clean stop ----
-    let periodic = wait_for(&lines, "health report", Duration::from_secs(75));
+    // A report that lists the threads: the first one after a minute, a loss
+    // report included (review m6).
+    let periodic = wait_for(&lines, "atlas-pipeline=", Duration::from_secs(75));
     report.check("a_periodic_report_while_running", periodic, "");
     std::thread::sleep(Duration::from_secs(3));
     let stop_ns = unix_now();
@@ -351,6 +353,18 @@ fn binary_run_and_dump() {
         })
         .unwrap_or(false);
     report.check("the_log_file_has_the_reports", logged, "");
+    // A periodic report's line names the agent's threads with their CPU (plan 1b-4a-p1).
+    let log_text: String = std::fs::read_dir(data.join("logs"))
+        .map(|r| r.filter_map(Result::ok).filter_map(|e| std::fs::read_to_string(e.path()).ok()).collect())
+        .unwrap_or_default();
+    let thread_line = log_text.lines().find(|l| l.contains("health report") && l.contains("atlas-pipeline="));
+    report.check("a_report_logs_cpu_per_thread", thread_line.is_some(), thread_line.unwrap_or("").trim());
+    // The final report's threads are listed before the sensor stops (review
+    // m2). Not a check: a thread shows only if it used a clock tick (~15.6 ms)
+    // since the last listing, a few seconds before; its line is kept instead.
+    let last = log_text.lines().rfind(|l| l.contains("health report")).unwrap_or("");
+    let threads = last.split("threads=").nth(1).unwrap_or("").trim();
+    report.note("final_report_threads", json!(threads));
 
     // ---- the buffer: through dump, and directly ----
     let (code, out, err) = run_to_end(&["dump", "--data-dir", &data.to_string_lossy()], Duration::from_secs(60));
@@ -642,6 +656,27 @@ fn analyse_health(r: &mut Report, reports: &[(&Event, &HealthReport, i64)]) {
         "",
     );
     r.check("seeding_reported", reports.iter().all(|(_, h, _)| h.housekeeping.seeding_enabled == Some(true)), "");
+    // Every report measures what each session delivered; the actor's launches
+    // reach Session B, its files Session A.
+    let received =
+        |f: fn(&HealthReport) -> Option<u64>| reports.iter().map(|(_, h, _)| f(h)).collect::<Option<Vec<_>>>();
+    let sensor = received(|h| h.throughput.sensor_session_events_received);
+    let process = received(|h| h.throughput.process_session_events_received);
+    let sums = (sensor.as_ref().map(|v| v.iter().sum::<u64>()), process.as_ref().map(|v| v.iter().sum::<u64>()));
+    // Session A (files, registry, network) delivers far more than Session B
+    // (processes): a swap of the two shows (review m5). The final report,
+    // made after the stop, counts too.
+    r.check(
+        "events_received_measured",
+        matches!(sums, (Some(s), Some(p)) if s > p && p > 0),
+        format!("{sensor:?} {process:?}"),
+    );
+    r.check(
+        "the_final_report_counts_events_received",
+        sensor.as_ref().and_then(|v| v.last()).is_some_and(|&n| n > 0),
+        format!("{sensor:?}"),
+    );
+    r.note("events_received", json!({ "sensor": sums.0, "process": sums.1 }));
     let unpaired: u64 = reports.iter().filter_map(|(_, h, _)| h.quality.file_cleanup_unpaired).sum();
     r.note("file_cleanup_unpaired", json!(unpaired));
     let cpu: u64 = reports.iter().filter_map(|(_, h, _)| h.resources.cpu_time).sum();
```

- [ ] **Step 6: Check and commit**

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
buf lint crates/atlas-proto/proto
buf breaking crates/atlas-proto/proto --against '.git#branch=main,subdir=crates/atlas-proto/proto'
```
Expected: all pass; `buf breaking` finds nothing. Run the agent's unit tests repeatedly (verification: 20 runs in a row passed). The elevated live checks run in CI's `etw-live` and `agent-live` jobs (Task 5). Workspace: 411 passed, 18 ignored (`atlas-agent` unit tests: 225).
```powershell
git add crates
git commit -m "feat(etw,agent): Sensor Health counts each session's events, the log each thread's CPU"
```

### Task 3: The host trace (the user)

The measurement F2–F5 came from; it is repeated on the build's code. It needs elevation, so the user runs it. The tools are git-ignored under `spikes\`.

- [ ] **Step 1: Build the tools and the agent** (Claude, unelevated):

```powershell
cargo build --release --manifest-path spikes/d6load/Cargo.toml
$env:CARGO_PROFILE_RELEASE_DEBUG = 'line-tables-only'; $env:CARGO_TARGET_DIR = 'spikes/d6-target-p1'
cargo build --release -p atlas-agent
Remove-Item Env:CARGO_PROFILE_RELEASE_DEBUG, Env:CARGO_TARGET_DIR
```

`spikes/d6load/Cargo.toml` and `src/main.rs` (`load`, `threads`, `stop`, `cleanup`, and the `probe` used for F3):

```toml
[package]
name = "d6load"
version = "0.1.0"
edition = "2024"
publish = false

# Not part of the Atlas workspace (spikes/ is git-ignored).
[workspace]

[dependencies]
windows = { version = "0.62", features = [
    "Win32_Foundation",
    "Win32_System_Console",
    "Win32_System_Diagnostics_ToolHelp",
    "Win32_System_Registry",
    "Win32_System_Threading",
    "Win32_System_WindowsProgramming",
    "Win32_Security",
    "Win32_Storage_FileSystem",
    "Win32_System_IO",
    "Win32_System_Ioctl",
] }

[profile.release]
debug = "line-tables-only"
```

```rust
//! D6 profiling helper (spikes only).
//!
//!   d6load load <dir> <seconds>          a fixed, paced mix of file, registry,
//!                                        launch and DNS activity; prints JSON counts
//!   d6load threads <pid> <csv> <label>   appends each thread's name and CPU times
//!   d6load stop <pid>                    sends Ctrl+C to the console <pid> runs in
//!   d6load cleanup <dir>                 removes the load's files and registry key

use std::fs;
use std::io::Write;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Console::{
    AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, KEY_READ, REG_DWORD, REG_OPTION_NON_VOLATILE, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{
    GetThreadDescription, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;
use windows::core::{HSTRING, PCWSTR};

const REG_KEY: &str = r"Software\AtlasLoad";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some("load") if args.len() == 4 => load(Path::new(&args[2]), args[3].parse().expect("seconds")),
        Some("threads") if args.len() == 5 => threads(args[2].parse().expect("pid"), Path::new(&args[3]), &args[4]),
        Some("stop") if args.len() == 3 => stop(args[2].parse().expect("pid")),
        Some("cleanup") if args.len() == 3 => cleanup(Path::new(&args[2])),
        Some("probe") if args.len() >= 3 => {
            for a in &args[2..] {
                probe(a);
            }
            0
        }
        _ => {
            eprintln!("usage: d6load load <dir> <seconds> | threads <pid> <csv> <label> | stop <pid> | cleanup <dir>");
            2
        }
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------- load

/// Per 10 ms tick: 10 failed opens, 1 file cycle (create, write 4 KiB, close,
/// delete), 2 failed registry opens, 1 registry value set. Every 20th tick a
/// launch of a fresh copy of whoami.exe (a new file, so a hash miss). A
/// separate thread resolves 5 new names a second.
fn load(dir: &Path, seconds: u64) -> i32 {
    let files = dir.join("files");
    let bins = dir.join("bin");
    fs::create_dir_all(&files).expect("create the files dir");
    fs::create_dir_all(&bins).expect("create the bin dir");
    let whoami = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
        .join(r"System32\whoami.exe");
    let key = create_key();

    let stop = Arc::new(AtomicBool::new(false));
    let dns = Arc::new(AtomicU64::new(0));
    let dns_thread = {
        let (stop, dns) = (stop.clone(), dns.clone());
        std::thread::spawn(move || {
            let tag = std::process::id();
            let mut n = 0u64;
            let start = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let _ = format!("d6-{tag}-{n}.example.com:80").to_socket_addrs();
                n += 1;
                dns.store(n, Ordering::Relaxed);
                let due = start + Duration::from_millis(200 * n);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
        })
    };

    let (mut failed_opens, mut cycles, mut reg_missing, mut reg_sets, mut launches) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut late_ticks = 0u64;
    let mut children: Vec<Child> = Vec::new();
    let payload = vec![0x5Au8; 4096];
    let start = Instant::now();
    let ticks = seconds * 100;
    for t in 0..ticks {
        for i in 0..10 {
            let p = files.join(format!("missing-{t}-{i}.dat"));
            if fs::File::open(&p).is_err() {
                failed_opens += 1;
            }
        }
        let p = files.join(format!("f-{}.dat", t % 1000));
        if let Ok(mut f) = fs::File::create(&p) {
            let _ = f.write_all(&payload);
            drop(f);
            let _ = fs::remove_file(&p);
            cycles += 1;
        }
        for i in 0..2 {
            if open_missing_key(&format!("{REG_KEY}\\missing-{t}-{i}")) {
                reg_missing += 1;
            }
        }
        if set_value(key, &format!("v{}", t % 10), t as u32) {
            reg_sets += 1;
        }
        if t % 20 == 0 {
            let exe = bins.join(format!("w-{t}.exe"));
            if fs::copy(&whoami, &exe).is_ok() {
                if let Ok(c) = Command::new(&exe).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                    children.push(c);
                    launches += 1;
                }
            }
        }
        children.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));

        let due = start + Duration::from_millis(10 * (t + 1));
        match due.checked_duration_since(Instant::now()) {
            Some(wait) => std::thread::sleep(wait),
            None => late_ticks += 1,
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    let _ = dns_thread.join();
    for mut c in children {
        let _ = c.wait();
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    println!(
        "{{\"seconds\":{elapsed:.1},\"failed_opens\":{failed_opens},\"file_cycles\":{cycles},\"reg_missing_opens\":{reg_missing},\"reg_sets\":{reg_sets},\"launches\":{launches},\"dns\":{},\"late_ticks\":{late_ticks}}}",
        dns.load(Ordering::Relaxed)
    );
    0
}

fn create_key() -> HKEY {
    let mut key = HKEY::default();
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(REG_KEY),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS,
            None,
            &mut key,
            None,
        )
    };
    assert!(r.is_ok(), "RegCreateKeyExW: {r:?}");
    key
}

/// True when the open failed, as intended.
fn open_missing_key(path: &str) -> bool {
    let mut key = HKEY::default();
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, KEY_READ, &mut key) };
    if r.is_ok() {
        unsafe {
            let _ = RegCloseKey(key);
        }
        return false;
    }
    true
}

fn set_value(key: HKEY, name: &str, v: u32) -> bool {
    unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_DWORD, Some(&v.to_le_bytes())) }.is_ok()
}

fn cleanup(dir: &Path) -> i32 {
    let mut code = 0;
    if dir.exists() {
        if let Err(e) = fs::remove_dir_all(dir) {
            eprintln!("remove {}: {e}", dir.display());
            code = 1;
        }
    }
    let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(REG_KEY)) };
    if r.is_err() && r.0 != 2 {
        eprintln!("RegDeleteTreeW: {r:?}");
        code = 1;
    }
    code
}

// ---------------------------------------------------------------- threads

fn ft(f: FILETIME) -> u64 {
    (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
}

/// Appends `label,unix_ms,tid,name,kernel_100ns,user_100ns,cycles` per thread.
fn threads(pid: u32, csv: &Path, label: &str) -> i32 {
    let new = !csv.exists();
    let mut out = fs::OpenOptions::new().create(true).append(true).open(csv).expect("open the csv");
    if new {
        writeln!(out, "label,unix_ms,tid,name,kernel_100ns,user_100ns,cycles").unwrap();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.expect("thread snapshot");
    let mut te = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
    let mut n = 0;
    let mut ok = unsafe { Thread32First(snap, &mut te) }.is_ok();
    while ok {
        if te.th32OwnerProcessID == pid {
            if let Some(row) = thread_row(te.th32ThreadID) {
                writeln!(out, "{label},{now},{},{}", te.th32ThreadID, row).unwrap();
                n += 1;
            }
        }
        ok = unsafe { Thread32Next(snap, &mut te) }.is_ok();
    }
    unsafe {
        let _ = CloseHandle(snap);
    }
    println!("threads {label}: {n} rows");
    if n == 0 { 1 } else { 0 }
}

fn thread_row(tid: u32) -> Option<String> {
    let h: HANDLE = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) }.ok()?;
    let name = unsafe { GetThreadDescription(h) }
        .ok()
        .map(|p| {
            let s = unsafe { p.to_string() }.unwrap_or_default();
            unsafe {
                let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(p.0.cast())));
            }
            s
        })
        .unwrap_or_default();
    let (mut c, mut e, mut k, mut u) = Default::default();
    let times = unsafe { GetThreadTimes(h, &mut c, &mut e, &mut k, &mut u) }.is_ok();
    let mut cycles = 0u64;
    let _ = unsafe { QueryThreadCycleTime(h, &mut cycles) };
    unsafe {
        let _ = CloseHandle(h);
    }
    if !times {
        return None;
    }
    let name = if name.is_empty() { "(unnamed)".to_string() } else { name.replace(',', ";") };
    Some(format!("{name},{},{},{cycles}", ft(k), ft(u)))
}

// ---------------------------------------------------------------- stop

/// Ctrl+C to every process on `pid`'s console, as if typed there.
fn stop(pid: u32) -> i32 {
    unsafe {
        let _ = FreeConsole();
        if let Err(e) = AttachConsole(pid) {
            eprintln!("AttachConsole({pid}): {e}");
            return 1;
        }
        let _ = SetConsoleCtrlHandler(None, true);
        if let Err(e) = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) {
            eprintln!("GenerateConsoleCtrlEvent: {e}");
            return 1;
        }
        std::thread::sleep(Duration::from_millis(500));
        let _ = FreeConsole();
    }
    0
}

// ---------------------------------------------------------------- probe

/// The hash worker's open, writer check and cache key (atlas-agent win::hash), twice.
fn probe(path: &str) {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_SEQUENTIAL_SCAN, FILE_GENERIC_READ, FILE_ID_INFO,
        FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo,
        GetFileInformationByHandleEx, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{FSCTL_READ_FILE_USN_DATA, READ_FILE_USN_DATA};
    let p = HSTRING::from(format!(r"\\?\{path}"));
    let h = match unsafe {
        CreateFileW(
            &p,
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_SEQUENTIAL_SCAN,
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            println!("{path}: open failed {e}");
            return;
        }
    };
    let writer = match unsafe {
        CreateFileW(&p, FILE_READ_DATA.0, FILE_SHARE_READ | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, None)
    } {
        Ok(w) => {
            unsafe {
                let _ = CloseHandle(w);
            }
            "no".to_string()
        }
        Err(e) => format!("err {:#x}", e.code().0),
    };
    let mut id = FILE_ID_INFO::default();
    let idok = unsafe {
        GetFileInformationByHandleEx(h, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), size_of::<FILE_ID_INFO>() as u32)
    }
    .is_ok();
    let input = READ_FILE_USN_DATA { MinMajorVersion: 2, MaxMajorVersion: 3 };
    let mut out = vec![0u64; 128];
    let mut ret = 0u32;
    let r = unsafe {
        DeviceIoControl(
            h,
            FSCTL_READ_FILE_USN_DATA,
            Some((&input as *const READ_FILE_USN_DATA).cast()),
            size_of::<READ_FILE_USN_DATA>() as u32,
            Some(out.as_mut_ptr().cast()),
            1024,
            Some(&mut ret),
            None,
        )
    };
    let usn = match r {
        Ok(()) => {
            let b: &[u8] = unsafe { std::slice::from_raw_parts(out.as_ptr().cast(), 1024) };
            let major = u16::from_le_bytes([b[4], b[5]]);
            let off = if major == 2 { 24 } else { 40 };
            format!("v{major} usn={}", i64::from_le_bytes(b[off..off + 8].try_into().unwrap()))
        }
        Err(e) => format!("fsctl err {:#x}", e.code().0),
    };
    unsafe {
        let _ = CloseHandle(h);
    }
    println!("{path}: writer_open={writer} id_ok={idok} vol={:#x} {usn}", id.VolumeSerialNumber);
}
```

- [ ] **Step 2: The run** (the user, in an elevated window; about 10 minutes, the first 3½ without starting builds or test runs):

```powershell
& C:\Users\jakef\Desktop\atlas-edr\spikes\run-d6-trace.ps1 -build d6-target-p1
```

It changes on the host: the two Atlas sessions and a WPR trace while it runs; `C:\ProgramData\Atlas-D6`, `%TEMP%\atlas-d6-load` and `HKCU\Software\AtlasLoad`, all removed at the end; about 600 lookups of made-up `*.example.com` names; a 4–6 GB trace in `spikes\results\d6-trace\`. It refuses to start with less than 16 GB free: WPR needs about twice the trace's size while it stops, and stops recording on its own when the disk runs low (F8). If WPR fails, the script still stops the agent cleanly and dumps its buffer.

```powershell
# D6 profiling step: one WPR CPU trace of atlas-agent on the HOST.
# Elevated shell; Windows PowerShell 5.1 or pwsh 7.
#   & C:\Users\jakef\Desktop\atlas-edr\spikes\run-d6-trace.ps1
# Phases (about 9 minutes in all):
#   settle 45 s   agent started, initial seeding (do not use the machine)
#   quiet  30 s   WPR recording, nothing running (do not use the machine)
#   synth 120 s   d6load's fixed mix (do not use the machine)
#   normal 300 s  use the machine as usual (browse, open apps, build, ...)
# then WPR stops, the agent gets Ctrl+C, its buffer is dumped, and the test
# data directory and the load's files and registry key are removed.
# What it changes on the host while it runs:
#   - runs the ETW sessions Atlas-Sensor and Atlas-Process, and a WPR trace
#     (CPU sampling + context switches, with stacks);
#   - creates C:\ProgramData\Atlas-D6 (removed at the end; C:\ProgramData\Atlas
#     is not touched);
#   - d6load writes under %TEMP%\atlas-d6-load and HKCU\Software\AtlasLoad
#     (both removed at the end) and resolves about 600 *.example.com names;
#   - writes the results (trace about 0.5-1 GB) to spikes\results\d6-trace\<stamp>.
# Ctrl+C at any time stops everything and cleans up.
# -build: the agent build under spikes\ (d6-target = before the fix,
# d6-target-p1 = plan 1b-4a-p1). The trace is about 4 GB.
param([string]$build = 'd6-target')

$ErrorActionPreference = 'Stop'
try { Stop-Transcript | Out-Null } catch { }

$root = 'C:\Users\jakef\Desktop\atlas-edr\spikes'
$exe = Join-Path $root "$build\release\atlas-agent.exe"
$load = Join-Path $root 'd6load\target\release\d6load.exe'
$data = 'C:\ProgramData\Atlas-D6'
$work = Join-Path $env:TEMP 'atlas-d6-load'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$out = Join-Path $root "results\d6-trace\$stamp-$build"
New-Item -ItemType Directory -Force -Path $out | Out-Null
Start-Transcript -Path (Join-Path $out 'transcript.txt') | Out-Null

function Say([string]$text, [string]$color = 'Cyan') {
    Write-Host ("[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $text) -ForegroundColor $color
}
function Threads([string]$label) {
    & $load threads $agent.Id (Join-Path $out 'threads.csv') $label
    if ($LASTEXITCODE -ne 0) { throw "d6load threads $label failed" }
}
function Wait-Phase([int]$seconds, [string]$what) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        if ($agent.HasExited) { throw "the agent exited during $what (code $($agent.ExitCode))" }
        $left = [int]($end - (Get-Date)).TotalSeconds
        Write-Progress -Activity $what -SecondsRemaining $left
        Start-Sleep -Seconds 1
    }
    Write-Progress -Activity $what -Completed
}

$agent = $null
$typeperf = $null
$wprOn = $false
try {
    $principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Not elevated: open PowerShell with "Run as administrator" and run this line again.'
    }
    foreach ($f in @($exe, $load)) { if (-not (Test-Path $f)) { throw "missing: $f" } }
    if (Get-Process atlas-agent -ErrorAction SilentlyContinue) { throw 'An atlas-agent process is already running.' }
    $status = (& wpr -status) -join ' '
    if ($status -notmatch 'not recording') { throw "WPR is already recording ($status). Run: wpr -cancel" }
    # WPR needs about twice the trace (4-6 GB) while it stops, and stops
    # recording on its own when the disk runs low (2026-10-15, at 1.6 GB free).
    $freeGB = [math]::Round((Get-PSDrive C).Free / 1GB, 1)
    if ($freeGB -lt 16) { throw "only $freeGB GB free on C:; the run needs 16 GB. Free some space first." }
    if (Test-Path $data) { Remove-Item -Recurse -Force $data }
    & $load cleanup $work | Out-Null

    Say "Agent: $exe"
    Say "Results: $out"
    Say 'Phase settle (45 s): starting the agent. Please do not use the machine until told.' 'Yellow'
    $agent = Start-Process -FilePath $exe -ArgumentList @('run', '--data-dir', $data) -WindowStyle Minimized -PassThru
    $null = $agent.Handle
    $up = $false
    for ($i = 0; $i -lt 60 -and -not $up; $i++) {
        Start-Sleep -Milliseconds 500
        if ($agent.HasExited) { throw "the agent exited at start (code $($agent.ExitCode)); see $data\logs" }
        & cmd.exe /c 'logman query Atlas-Sensor -ets >nul 2>&1'
        $up = ($LASTEXITCODE -eq 0)
    }
    if (-not $up) { throw 'Atlas-Sensor did not start within 30 s' }
    Say "agent pid $($agent.Id), sessions up"
    Wait-Phase 45 'settle: initial seeding'
    Threads 'settled'

    $counters = @(
        '\Event Tracing for Windows Session(Atlas-Sensor)\Events Logged per sec',
        '\Event Tracing for Windows Session(Atlas-Process)\Events Logged per sec',
        '\Event Tracing for Windows Session(Atlas-Sensor)\Events Lost',
        '\Process(atlas-agent)\% Processor Time',
        '\Process(atlas-agent)\Working Set - Private',
        '\Processor(_Total)\% Processor Time'
    )
    $tpArgs = @($counters | ForEach-Object { '"' + $_ + '"' }) + @('-si', '1', '-f', 'CSV', '-y', '-o', ('"' + (Join-Path $out 'counters.csv') + '"'))
    $typeperf = Start-Process -FilePath typeperf -ArgumentList $tpArgs -WindowStyle Hidden -PassThru

    Say 'starting WPR (CPU profile, file mode)'
    & wpr -start CPU -filemode -recordtempto $out
    if ($LASTEXITCODE -ne 0) { throw "wpr -start failed ($LASTEXITCODE)" }
    $wprOn = $true

    Say 'Phase quiet (30 s): still please do not use the machine.' 'Yellow'
    & wpr -marker 'd6-quiet'
    Threads 'quiet-start'
    Wait-Phase 30 'quiet'
    Threads 'quiet-end'

    Say 'Phase synth (120 s): d6load running. Still please do not use the machine.' 'Yellow'
    & wpr -marker 'd6-synth'
    Threads 'synth-start'
    $loadJson = & $load load $work 120
    if ($LASTEXITCODE -ne 0) { throw 'd6load load failed' }
    Set-Content -Path (Join-Path $out 'load.json') -Value $loadJson
    Say "load: $loadJson"
    Threads 'synth-end'

    [console]::Beep(880, 300)
    Say 'Phase normal (300 s): NOW use the machine as you usually do (browser, apps, a build...).' 'Green'
    & wpr -marker 'd6-normal'
    Threads 'normal-start'
    Wait-Phase 300 'normal use'
    Threads 'normal-end'
    [console]::Beep(660, 300)
    Say 'Done with normal use; you can stop now. Stopping WPR (this can take a minute)...' 'Yellow'

    & wpr -marker 'd6-end'
    & wpr -stop (Join-Path $out 'trace.etl') -skipPdbGen
    # No trace is not the end of the run: the agent is still stopped cleanly
    # and its buffer, logs and thread figures saved.
    if ($LASTEXITCODE -ne 0) { Say "wpr -stop failed ($LASTEXITCODE): no trace this time" 'Red' }
    $wprOn = $false

    Say 'stopping the agent (Ctrl+C)'
    & $load stop $agent.Id
    if (-not $agent.WaitForExit(30000)) { throw 'the agent did not stop within 30 s' }
    Say "agent exit code $($agent.ExitCode)"
    if ($typeperf -and -not $typeperf.HasExited) { Stop-Process -Id $typeperf.Id -Force }

    Say 'dumping the buffer'
    $p = Start-Process -FilePath $exe -ArgumentList @('dump', '--data-dir', $data) -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput (Join-Path $out 'dump.jsonl') -RedirectStandardError (Join-Path $out 'dump-stderr.txt')
    Say "dump exit code $($p.ExitCode)"
    Copy-Item -Recurse -Path (Join-Path $data 'logs') -Destination (Join-Path $out 'logs')
    Say 'All done.' 'Green'
}
catch {
    Say "FAILED: $_" 'Red'
}
finally {
    if ($wprOn) { & wpr -cancel | Out-Null; Say 'WPR cancelled' }
    if ($agent -and -not $agent.HasExited) {
        & $load stop $agent.Id | Out-Null
        if (-not $agent.WaitForExit(30000)) { Stop-Process -Id $agent.Id -Force; Say 'agent killed' 'Red' }
    }
    if ($typeperf -and -not $typeperf.HasExited) { Stop-Process -Id $typeperf.Id -Force }
    & $load cleanup $work | Out-Null
    if ((Test-Path $data) -and (Test-Path (Join-Path $out 'logs'))) { Remove-Item -Recurse -Force $data }
    elseif (Test-Path $data) { Say "kept $data for its logs (remove it afterwards)" 'Yellow' }
    Get-ChildItem $out | Select-Object Name, Length | Format-Table -AutoSize | Out-String | Write-Host
    try { Stop-Transcript | Out-Null } catch { }
}
```

- [ ] **Step 3: The comparison** (Claude): per phase, events per second, module loads, CPU per thread, normalised; and, from the trace, CPU by function per thread (the TraceProcessor analyser, with `_NT_SYMBOL_PATH` set to the agent's PDB folder and Microsoft's symbol server).

```powershell
python -I spikes/p1-plan-src/compare.py spikes/results/d6-trace/<before> spikes/results/d6-trace/<after>
dotnet spikes/d6trace/bin/Release/net9.0/d6trace.dll <trace.etl> <agent pid> <threads.csv> <out dir> "quiet=<start>..<end>;synth=…;normal=…"
```

```
"""Side-by-side of D6 trace runs: per phase, events, module loads, agent CPU by
thread, normalised. Usage: python -I compare.py RUN_DIR [RUN_DIR ...]"""
import collections
import csv
import json
import re
import sys
from datetime import datetime, timedelta, timezone

TZ = timezone(timedelta(hours=-7))
MARKS = [('quiet', 'Phase quiet'), ('synth', 'Phase synth'), ('normal', 'Phase normal'), ('end', 'Done with normal use')]


def phases(run):
    raw = open(f'{run}/transcript.txt', 'rb').read()
    text = raw.decode('utf-16') if raw[:2] in (b'\xff\xfe', b'\xfe\xff') else raw.decode('utf-8-sig')
    day = re.search(r'Start time: (\d{8})', text).group(1)
    at = {}
    for key, label in MARKS:
        m = re.search(r'\[(\d\d:\d\d:\d\d)\] ' + label, text)
        at[key] = datetime.strptime(day + m.group(1), '%Y%m%d%H:%M:%S').replace(tzinfo=TZ).timestamp()
    return [('quiet', at['quiet'], at['synth']), ('synth', at['synth'], at['normal']), ('normal', at['normal'], at['end'])]


def thread_cpu(run):
    rows = list(csv.DictReader(open(f'{run}/threads.csv', encoding='utf-8')))
    by = collections.defaultdict(dict)
    for r in rows:
        by[r['label']][r['tid']] = r
    out = {}
    for name in ('quiet', 'synth', 'normal'):
        a, b = by[f'{name}-start'], by[f'{name}-end']
        agg = collections.Counter()
        for tid, e in b.items():
            s = a.get(tid)
            n = e['name']
            n = 'hash' if n.startswith('atlas-hash') else n.replace('atlas-', '').replace('etw-Atlas-', 'etw-')
            agg[n] += (int(e['kernel_100ns']) + int(e['user_100ns']) - (int(s['kernel_100ns']) + int(s['user_100ns']) if s else 0)) / 1e7
        out[name] = agg
    return out


def counters(run, ph):
    rows = list(csv.reader(open(f'{run}/counters.csv', encoding='utf-8')))
    hdr, data = rows[0], rows[1:]
    col = {h.split('\\')[-2] + '\\' + h.split('\\')[-1]: i for i, h in enumerate(hdr) if i > 0}
    out = {}
    for name, a, b in ph:
        vals = collections.defaultdict(list)
        for r in data:
            t = datetime.strptime(r[0], '%m/%d/%Y %H:%M:%S.%f').replace(tzinfo=TZ).timestamp()
            if a <= t < b:
                for k, i in col.items():
                    try:
                        vals[k].append(float(r[i]))
                    except ValueError:
                        pass
        out[name] = {k: sum(v) / len(v) for k, v in vals.items() if v}
    return out


def dump(run, ph):
    out = {p[0]: collections.Counter() for p in ph}
    mods = collections.defaultdict(set)
    seen_setattr = {p[0]: 0 for p in ph}
    module_paths = {p[0]: set() for p in ph}
    setattr_paths = {p[0]: collections.Counter() for p in ph}
    for line in open(f'{run}/dump.jsonl', encoding='utf-8'):
        d = json.loads(line)
        t = int(d['time']) / 1e9
        name = next((p[0] for p in ph if p[1] <= t < p[2]), None)
        if not name:
            continue
        c = out[name]
        if 'module' in d:
            c['module_loads'] += 1
            module_paths[name].add(d['module']['load']['file'].get('path', '?').lower())
        elif 'process' in d and 'launch' in d['process']:
            c['launches'] += 1
        elif 'file' in d and 'setAttributes' in d['file']:
            setattr_paths[name][d['file']['file'].get('path', '').lower()] += 1
        elif 'sensorHealth' in d:
            th = d['sensorHealth']['report'].get('throughput', {})
            c['reports_with_throughput'] += 1 if th else 0
        c['events'] += 1
    for name in out:
        out[name]['distinct_modules'] = len(module_paths[name])
        out[name]['setattr_on_modules'] = sum(n for p, n in setattr_paths[name].items() if p in module_paths[name])
    return out


for run in sys.argv[1:]:
    ph = phases(run)
    cpu = thread_cpu(run)
    cnt = counters(run, ph)
    dm = dump(run, ph)
    print(f'##### {run}')
    for name, a, b in ph:
        secs = b - a
        c = cnt[name]
        ev = c.get('Event Tracing for Windows Session(Atlas-Sensor)\\Events Logged per sec', 0)
        tot_cpu = sum(cpu[name].values())
        d = dm[name]
        hash_ms_per_load = 1000 * cpu[name]['hash'] / d['module_loads'] if d['module_loads'] else 0
        print(f'== {name} {secs:.0f}s  sensor ev/s={ev:,.0f}  agent cpu={100 * tot_cpu / secs:.2f}%core  '
              f'cpu per 1k ev={1000 * tot_cpu / (ev * secs) * 1000 if ev else 0:.2f} ms  machine cpu={c.get("Processor(_Total)\\% Processor Time", 0):.1f}%')
        print(f'   module loads={d["module_loads"]} ({d["module_loads"] / secs:.0f}/s) distinct={d["distinct_modules"]} '
              f'launches={d["launches"]} setattr_on_modules={d["setattr_on_modules"]}  hash ms per module load={hash_ms_per_load:.2f}')
        print('   threads %core: ' + '  '.join(f'{k}={100 * v / secs:.2f}' for k, v in cpu[name].most_common(8) if v > 0.005))
        print(f'   working set private={c.get("Process(atlas-agent)\\Working Set - Private", 0) / 2**20:.0f} MB')
```

```
// D6: CPU samples of one process from a WPR trace, by thread, phase and function.
//   d6trace <trace.etl> <pid> <threads.csv> <outdir> <phases: name=startLocal..endLocal;...>
//   d6trace --sig   prints the LoadSymbolsAsync parameter names
using System.Globalization;
using System.Reflection;
using System.Text;
using Microsoft.Windows.EventTracing;
using Microsoft.Windows.EventTracing.Cpu;
using Microsoft.Windows.EventTracing.Symbols;

if (args.Length == 1 && args[0] == "--sig")
{
    foreach (var m in typeof(ISymbolDataSource).GetMethods())
        Console.WriteLine(m.Name + "(" + string.Join(", ", m.GetParameters().Select(p => p.ParameterType.Name + " " + p.Name)) + ")");
    return;
}

string etl = args[0];
int pid = int.Parse(args[1]);
string threadsCsv = args[2];
string outDir = args[3];
var phases = args[4].Split(';', StringSplitOptions.RemoveEmptyEntries).Select(p =>
{
    var kv = p.Split('=');
    var se = kv[1].Split("..");
    return (Name: kv[0], Start: DateTimeOffset.Parse(se[0], CultureInfo.InvariantCulture), End: DateTimeOffset.Parse(se[1], CultureInfo.InvariantCulture));
}).ToList();

// tid -> name, from d6load's snapshots.
var tname = new Dictionary<int, string>();
foreach (var line in File.ReadLines(threadsCsv).Skip(1))
{
    var c = line.Split(',');
    var n = c[3].StartsWith("atlas-hash-") ? "atlas-hash-*" : c[3];
    tname[int.Parse(c[2])] = n;
}

var settings = new TraceProcessorSettings { AllowLostEvents = true, AllowTimeInversion = true };
using var trace = TraceProcessor.Create(etl, settings);
var pendingCpu = trace.UseCpuSamplingData();
var pendingSymbols = trace.UseSymbols();
Console.Error.WriteLine("processing...");
trace.Process();
Console.Error.WriteLine("loading symbols...");
var symPath = Environment.GetEnvironmentVariable("_NT_SYMBOL_PATH") ?? "";
var symCache = Path.Combine(outDir, "symcache");
await pendingSymbols.Result.LoadSymbolsAsync(new SymCachePath(symCache), new SymbolPath(symPath.Split(';')), null, new[] { "atlas-agent.exe" }, Array.Empty<string>());
Console.Error.WriteLine("aggregating...");

// key: phase|thread -> totals; per phase|thread: exclusive and inclusive by function.
var total = new Dictionary<string, double>();
var excl = new Dictionary<string, Dictionary<string, double>>();
var incl = new Dictionary<string, Dictionary<string, double>>();
var exclModule = new Dictionary<string, Dictionary<string, double>>();
void Add(Dictionary<string, Dictionary<string, double>> d, string k, string f, double w)
{
    if (!d.TryGetValue(k, out var m)) d[k] = m = new();
    m[f] = m.GetValueOrDefault(f) + w;
}

long count = 0;
foreach (ICpuSample s in pendingCpu.Result.Samples)
{
    if (s.Process?.Id != pid) continue;
    var ts = s.Timestamp.DateTimeOffset;
    var ph = phases.FirstOrDefault(p => ts >= p.Start && ts < p.End);
    if (ph.Name == null) continue;
    int tid = s.Thread?.Id ?? -1;
    string th = tname.GetValueOrDefault(tid, "tid" + tid);
    double w = (double)s.Weight.TotalMilliseconds;
    count++;
    foreach (var key in new[] { ph.Name + "|" + th, ph.Name + "|ALL" })
    {
        total[key] = total.GetValueOrDefault(key) + w;
        var frames = s.Stack?.Frames;
        string top = s.TopStackFrame is StackFrame tf ? Fn(tf) : "?";
        Add(excl, key, top, w);
        Add(exclModule, key, s.TopStackFrame is StackFrame tm ? (tm.Image?.FileName ?? "?") : "?", w);
        if (frames != null)
        {
            var seen = new HashSet<string>();
            foreach (var fr in frames)
            {
                var f = Fn(fr);
                if (seen.Add(f)) Add(incl, key, f, w);
            }
        }
    }
}
Console.Error.WriteLine($"{count} samples of pid {pid}");

static string Fn(StackFrame f)
{
    var img = f.Image?.FileName ?? "?";
    var fn = f.Symbol?.FunctionName;
    return img + "!" + (fn ?? "?");
}

var sb = new StringBuilder();
foreach (var ph in phases)
{
    double secs = (ph.End - ph.Start).TotalSeconds;
    var keys = total.Keys.Where(k => k.StartsWith(ph.Name + "|")).OrderByDescending(k => total[k]).ToList();
    sb.AppendLine($"######## phase {ph.Name} ({secs:F0} s)");
    foreach (var k in keys)
        sb.AppendLine($"  {k.Split('|')[1],-24} {total[k] / 1000,8:F2} s  {100 * total[k] / 1000 / secs,6:F2}% core");
    foreach (var k in keys)
    {
        if (total[k] / 1000 / secs < 0.002) continue; // under 0.2% of a core
        sb.AppendLine($"=== {k}  {total[k] / 1000:F2} s");
        sb.AppendLine("  -- exclusive by module");
        foreach (var (f, w) in exclModule[k].OrderByDescending(x => x.Value).Take(12))
            sb.AppendLine($"    {100 * w / total[k],6:F1}%  {f}");
        sb.AppendLine("  -- exclusive by function");
        foreach (var (f, w) in excl[k].OrderByDescending(x => x.Value).Take(25))
            sb.AppendLine($"    {100 * w / total[k],6:F1}%  {f}");
        sb.AppendLine("  -- inclusive by function");
        foreach (var (f, w) in incl.GetValueOrDefault(k, new()).OrderByDescending(x => x.Value).Take(60))
            sb.AppendLine($"    {100 * w / total[k],6:F1}%  {f}");
    }
}
File.WriteAllText(Path.Combine(outDir, "cpu-by-function.txt"), sb.ToString());
Console.Error.WriteLine("written " + Path.Combine(outDir, "cpu-by-function.txt"));
```

Expected: in normal use, the hash workers' CPU per module load well under F2's 1.28 ms, and the health thread near 0 (Q7). The figures go to §15.3 (Task 4).

### Task 4: Documentation

- [ ] **Step 1: The spec and the schema reference.** In `docs/specs/2026-10-01-etw-sensor-design.md`:
  - the status line (its date is the build's merge day: change `2026-10-15` if it differs);
  - §6.3 and §7.2: clarification 1;
  - §9.3: clarifications 2 and 3, and the review's fixes;
  - §10.3: clarification 2;
  - §13: the profiled figures;
  - §15.3: a "Plan 1b-4a-p1" addendum with F1–F9 (Task 3's final figures replace F4's if they differ);
  - §17: "Plan 1b-4a-p1 clarifications" and "Plan 1b-4a-p1 review fixes" paragraphs.

  Also `docs/schema-reference.md` (six groups, `throughput`). `docs/architecture-overview.md` (roadmap row 1, "plan 1b-4a-p1 done", and a decision-log row for the build) is a second commit, `docs(1): plan 1b-4a-p1 built: roadmap and decision log`, made once Task 3's figures are in.

```diff
--- a/docs/specs/2026-10-01-etw-sensor-design.md
+++ b/docs/specs/2026-10-01-etw-sensor-design.md
@@ -1,6 +1,6 @@
 # Sub-project 1 — ETW Sensor Design (Agent Core)
 
-**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13), and plan 1b-4b (blinding detection) is next, then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
+**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15); plan 1b-4b (blinding detection) is next, then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
 **Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
 **Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).
 
@@ -290,7 +290,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 ### 6.3 Hashes and signatures
 
 - Applies to `process.file` on Launch and `module.file` on Module Load.
-- **Cache key:** volume serial + 128-bit file ID (`FILE_ID_INFO`) + **the file's USN** (`FSCTL_READ_FILE_USN_DATA`). The USN changes on every modification and, unlike last-write time, cannot be set back by a user. On volumes without a USN journal, results are not cached. Entries for a path are also invalidated when the pipeline sees an Update, Rename, SetAttributes or Delete on it.
+- **Cache key:** volume serial + 128-bit file ID (`FILE_ID_INFO`) + **the file's USN** (`FSCTL_READ_FILE_USN_DATA`). The USN changes on every modification and, unlike last-write time, cannot be set back by a user. On volumes without a USN journal, results are not cached. Entries for a path are also invalidated when the pipeline sees an Update, Rename or Delete on it. A SetAttributes does not invalidate (plan 1b-4a-p1, D6): attributes and timestamps change neither a file's content nor its name, and the System process sets the basic information of the DLLs that starting processes load, so invalidating there re-hashed them on every launch.
 - **Work** runs on 2 below-normal-priority worker threads, never on an ETW or pipeline thread. Files are opened with full sharing (`FILE_SHARE_READ | WRITE | DELETE`) and read sequentially.
 - **SHA-256** for files up to the size cap (default 100 MB); larger files skip hashing.
 - **Signature:** `WinVerifyTrust` with `WTD_UI_NONE`, `WTD_REVOKE_NONE`, `WTD_CACHE_ONLY_URL_RETRIEVAL` (no network calls from the sensor), and catalog lookup (`CryptCATAdmin*`) for catalog-signed OS files. `signer` = the leaf certificate's subject CN. Mapping: valid chain → `Valid`; no signature (`TRUST_E_NOSIGNATURE`) → `Unsigned`; a signature that fails verification (bad digest, untrusted root, explicit distrust, revoked per local cache) → `Invalid`; any operational error (file locked, CryptSvc unavailable, timeout) → signature **absent**, counted.
@@ -345,7 +345,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 - **8.3 names.** A Create opened through a short name logs the short form (S6), for example `…\ATLASS~1\…\LONG-F~1.TXT`.
   - A path with any component matching the 8.3 pattern (`^[^.~]{1,6}~[0-9]+(\.[^.]{0,3})?$`, case-insensitive) is expanded by a worker [4]. The Open is pending meanwhile (deadline §3.2).
   - Each short component is looked up in its parent directory, opened by NT path, with `NtQueryDirectoryFile` (`FileBothDirectoryInformation`) and the short name as the filter, about 54 µs each; the answer counts only if its short or long name equals the component exactly. `GetLongPathNameW` cannot do this: it rejects `\\?\GLOBALROOT\Device\…` paths (plan 1b-3b, F1). This works for shadow copies too. Only `\Device\HarddiskVolume…` paths are expanded, on a lane of their own (the expander lane).
-  - Results are cached by (parent directory, short name). Short names are reused after a delete or rename, so the pipeline's `InvalidateHash` for a path drops the entries for that path and everything under it. Short components in that path are resolved from the cache, and one it cannot resolve clears the whole cache. An invalidation dropped because the lane was full also clears it. Entries expire after 60 s (plan 1b-3b, D4).
+  - Results are cached by (parent directory, short name). Short names are reused after a delete or rename, so the pipeline's `InvalidateHash` for a path drops the entries for that path and everything under it (sent on an Update, Rename or Delete; not on a SetAttributes, which frees no name, plan 1b-4a-p1). Short components in that path are resolved from the cache, and one it cannot resolve clears the whole cache. An invalidation dropped because the lane was full also clears it. Entries expire after 60 s (plan 1b-3b, D4).
   - The logged path is matched at once. An Open that matches neither form is dropped only after the expansion.
   - The emitted `file.path` is the expanded form when expansion succeeded, otherwise the logged one.
   - **Every emitted file path is expanded the same way** (plan 1b-3a, D4), not only a watchlist Open's: Create, Update, Delete, SetAttributes, both paths of a Rename, and the images of Launch and Module Load. An event with a short component waits for its expansion (deadline §3.2). The handle map keeps the expanded path, and a Launch's expanded image also updates the cached process; actor references built before the reply keep the short form. On the CI runner 13% of file events had a short component (`C:\Users\RUNNER~1\…`, from `TEMP`).
@@ -497,12 +497,13 @@ Emitted every 60 s (and immediately on a non-zero change in a loss counter):
 - **Quality:** late arrivals, parse errors, `unknown_version`, `launch_join_miss`, `actor_unresolved`, `unknown_file_object`, `registry_unresolved`, `value_read_failed`, `early_read_redone`, `reg_type_unusual`, `file_op_late_failure`, `writes_after_cleanup`, `file_object_replaced`, invalid records found on replay (§8.2), enrichment misses and errors.
 - **Housekeeping:** cache, map and flow-table evictions, `retention_evictions`, `file_op_failed` (failed creates, deletes and renames dropped, §5.5), `pending_overflow` (§3.2), seeder results (handles named, failed, timed out; stuck helpers outstanding; negative-cache size; table reads; deferred re-reads; whether seeding is on).
 - **Self-measurement:** agent CPU time and working set for the interval (`GetProcessTimes`, `GetProcessMemoryInfo`), so long runs measure themselves (§13).
+- **Throughput** (plan 1b-4a-p1): the ETW events each session delivered to the agent, counted in the callback before parsing, so CPU can be read per event and a session that falls silent shows.
 
 **How a report is made** (plan 1b-4a, D1, D5):
-- **Snapshots and differences:** the health thread [7] reads every cumulative counter into a snapshot: the pipeline's (through the driver's control channel, waiting at most 2 s), the intake's and the services' atomics, the callbacks' panics, both sessions' losses (`ControlTraceW` query), the sink's drops, and the process's CPU time and working set. A report holds the differences between two snapshots. The sessions' counters are 32-bit, so their differences wrap. A source that could not be read is absent from the report, and the next report covers its interval too.
+- **Snapshots and differences:** the health thread [7] reads every cumulative counter into a snapshot: the pipeline's (through the driver's control channel, waiting at most 2 s), the intake's and the services' atomics, the callbacks' panics and delivered events (counters the agent owns and hands to each consumer it starts, so a restarted session counts on into them; plan 1b-4a-p1, review M1), both sessions' losses (`ControlTraceW` query), the sink's drops, and the process's CPU time and working set. A report holds the differences between two snapshots. The sessions' counters are 32-bit, so their differences wrap. A source that could not be read is absent from the report, and the next report covers its interval too.
 - **When:** every 60 s; at once when a loss counter rose (checked every 5 s); and a final report at the clean stop, taken after the sessions stopped, so everything counted during the run is reported. The intervals chain: each starts where the last ended, the first at the anchor taken at start. A report's time is the system clock at generation (`GetSystemTimePreciseAsFileTime`, the clock the anchor reads, §3.3); its id is a UUIDv7. If the pipeline thread died, the final report is still sent, without the pipeline's counters (they died with it).
 - **The buffer's figures:** [6] fills in the `buffer` group, `loss.buffer_backlog_drops`, `housekeeping.retention_evictions` and the gap as it appends the report, each since the report before; so the drops of the events queued ahead of a report, the final drain's included, are in it (review finding 3). The health thread's own reading of the drops only decides an early report; the first report of a run carries what recovery found at open. A report the writer's backlog cannot take is held (up to 16) and written before anything else once there is room.
-- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums.
+- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums, and (plan 1b-4a-p1) the CPU each agent thread used since the last listing: each thread's difference (a thread is its id and creation time), summed by thread name, most first, with the listings' own interval. Threads are listed with a snapshot of every thread in the system (about 45 ms on the host), so at most once a report period, for a report of either kind; a report without them leaves them to the next listing. The final report's listing is taken just before the sensor stops, while its threads still run.
 - **Not measured yet** (absent): `quality.buffer_invalid_records` and `buffer.corrupt_segments` (the agent never reads its own buffer back; `dump` counts both), and `housekeeping.seeder_negative_cache_size` (the seeder exposes no size).
 
 ## 10. Schema Additions (`atlas.events.v1`, additive)
@@ -527,7 +528,7 @@ Fields: `log_name` (session name, required, so non-empty, ≤ 256 B), `log_provi
 
 One activity, `Report`, carrying the §9.3 counters (`u64`, all optional), the interval, and for gap reports the lost time range and count. No actor. Category: Application Activity (6). Its `class_uid` follows OCSF's extension rule (`extension_uid × 100000 + category_uid × 1000 + n`, as Windows' `201001`), with an Atlas extension UID the plan picks from outside OCSF's registered extensions: **500** (decision log 2026-10-04, D2), so `class_uid` 50006001.
 
-**Counter semantics (D1, plan 1b-1):** typed `optional uint64` fields in five groups (`loss`, `quality`, `housekeeping`, `resources`, `buffer`) plus an optional `gap`. Counters count occurrences during `[interval_start, time]`; gauges are sampled at the end; absent means not measured. Per-class counts are `{class_uid, count}` lists of at most 32 entries.
+**Counter semantics (D1, plan 1b-1):** typed `optional uint64` fields in six groups (`loss`, `quality`, `housekeeping`, `resources`, `buffer`, `throughput`) plus an optional `gap`. Counters count occurrences during `[interval_start, time]`; gauges are sampled at the end; absent means not measured. Per-class counts are `{class_uid, count}` lists of at most 32 entries.
 
 **Plan 1b-3a additions:** `loss.callback_panics`, for events skipped because the ETW callback panicked on them (caught, never unwound into ETW), and `quality.reg_name_ambiguous`, for value names that could end in more than one place (§7.5).
 
@@ -537,6 +538,8 @@ One activity, `Report`, carrying the §9.3 counters (`u64`, all optional), the i
 
 **Plan 1b-3c additions:** `quality.file_delete_outcome_unknown`, for Deletes reported from the request alone because the file system reported no Cleanup outcome, and `quality.file_cleanup_unpaired`, for Cleanups whose outcome could not be paired with them (§5.1).
 
+**Plan 1b-4a-p1 additions:** a `throughput` group with `sensor_session_events_received` and `process_session_events_received`, the ETW events each session delivered to the agent in the interval, counted before parsing (§9.3).
+
 ### 10.4 Registry fields (additive, from S4 and S7)
 
 | Message | Field | Meaning |
@@ -647,6 +650,8 @@ Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gate
 
 **Measured by plan 1b-4a's host run** (2026-10-12, 103 minutes of normal use; the agent's own Sensor Health records; §15.3). With everything above on, nothing was lost, but the agent is **over budget**: CPU averaged 10.1% of one core (3–4% in quiet minutes, 40–70% in the busiest), and the working set grew to 200 MB, then held. The cause is profiled before plan 1b-4b, ahead of any fallback (decision log, 2026-10-12).
 
+**Profiled** (D6, 2026-10-15; a WPR CPU trace on the host, §15.3): the hash workers took two-thirds of the agent's CPU, most of it re-checking DLLs they had already checked (about half the agent's CPU), because each SetAttributes the System process makes on a loaded DLL made the cache forget it. Plan 1b-4a-p1 removed that invalidation (§6.3). Under the same trace script, five minutes of normal use at about 14,000 events/s went from 19.0% to 7.9% of one core (13.1 to 5.6 ms of CPU per 1,000 events), and the synthetic load from 17.6% to 12.9%. Still over budget: the hash workers' first-time checks, the pipeline (2.0%, spread over the ordering stage, allocation and the key map), on-miss seeding (0.8%, all handle-table snapshots) and the working set are profiled next, and the first-time checks can leave a launch early in a run without its hash (F9, §15.3); the hour run measures the result at the end of the profiling step.
+
 **Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.
 
 ## 14. Definition of Done
@@ -768,6 +773,17 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **F7, an upper-case `\DEVICE\`** was not recognised as a device path; fixed.
 - **Elevated, on the host:** the start-up pass named 10,111–10,158 key handles in 205–363 ms and 3,382–3,428 disk files in 0.5–1.0 s. A read of `HKLM\SAM\SAM\Domains\Account` succeeded only with backup semantics.
 
+**Plan 1b-4a-p1 (2026-10-15): the profiling step's first fix** (the host, build 26200; the CI runner, 26100):
+- **F1, the method:** a WPR CPU trace (sampling and context switches, with stacks) of the release agent, built with line tables, during 30 s of quiet, 2 minutes of a fixed synthetic load (1,000 failed opens, 100 file cycles, 300 registry operations, 5 launches of fresh binary copies and 5 DNS lookups per second) and 5 minutes of normal use; each agent thread's CPU at every phase boundary (thread descriptions and times); each session's events per second from the `Event Tracing for Windows Session` performance counters. `xperf` could not read the trace (`0x80070032`); Microsoft's TraceProcessor library could.
+- **F2, where the CPU went** (normal use, 14,480 events/s, 19.0% of one core): hash workers 12.6%, pipeline 4.0%, seeder 1.1%, Session A's callback 0.9% (about 0.6 µs per event, so the callback was not the cause), the buffer writer 0.3%. 55% of the hash workers' time was inside `WinVerifyTrust`.
+- **F3, the cause:** the System process sets `FileBasicInformation` on the DLLs that starting processes load (about 1,300 SetAttributes events on module files in the five minutes; over the whole run 2,006, `KERNEL32.DLL` alone 85 times). Each one invalidated the file's cached hash, so the next load paid a full SHA-256 and signature check (11 ms on average in the replay below, 15–80 ms for the system DLLs probed) instead of a cache hit (about 0.1 ms). Replayed through the same code with one cache, the phase's 29,476 module loads cost 7.5 s against the 37 s spent live, and the agent's own SHA-256 had hashed about four times the bytes of its distinct files.
+- **F4, after the fix** (the same script): normal use 7.9% of one core at 14,161 events/s (hash workers 3.6%, pipeline 2.0%, seeder 0.8%, callback 0.5%), 5.6 ms of CPU per 1,000 events against 13.1; the synthetic load 12.9% against 17.6%; hash CPU per module load 0.46 ms against 1.28. What remains of the hash workers' time is mostly first-time checks (668 distinct module files in the five minutes).
+- **F5, the thread listing's cost:** listing the threads takes a snapshot of every thread in the system, 45 ms on the host; at every 5 s check it cost 0.86% of one core, so threads are listed only for periodic reports (about 0.08%).
+- **F6, a test bug found on the CI runner:** the live test read a consumer's count before closing it, while a stopped session's consumer still delivers the rest of its buffers until `close`; both counts are now read after `close`.
+- **F7, the working set:** about 92–122 MB private in both runs; not changed by the fix, profiled next.
+- **F8, the disk:** a third trace run got no trace: WPR stopped recording on its own with 1.6 GB left on the drive. The trace script now refuses to start below 16 GB free.
+- **F9, launches at start-up can miss their hash:** on the CI runner the agent live test's `cmd.exe` launch went out without its hash and signature in 2 of 9 runs on the same code (0 in the 33 runs before this plan). The live test's notes now record the enrichment counters: in four more runs of the same code, 0 errors every time and 0, 74, 50 and 0 enrichment misses (results past the 1 s deadline). So the hash workers sometimes fall over a second behind early in a run, when every file is checked for the first time (10–80 ms each, two workers), and a launch then goes out without its hash (the late result is cached for the next event on that file). This plan's fix only lowers that work; why it never showed before is not known (runner load; the earlier runs recorded no miss counts).
+
 ## 16. Known Limitations
 
 - **No boot-time coverage:** events before the agent starts (early boot, or while the service is stopped) are not captured. An AutoLogger session is sub-project 8.
@@ -916,3 +932,13 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 6. the console handler is installed before the sessions start;
 7. a dead buffer writer thread stops `run` (exit 2), and closing the buffer reports its panic instead of panicking;
 8. tests that could not catch their bug or would hang were fixed; the binary test's new check against the writer's count found that the `records=` logged at stop missed the last write's records (the buffer's `close` now returns its final counts).
+
+**Plan 1b-4a-p1 clarifications (2026-10-15).** The profiling step's first fix (decision log, 2026-10-15). Applied to §6.3, §7.2, §9.3, §10.3, §13 and §15.3:
+1. a SetAttributes no longer invalidates the hash and 8.3 caches; Update, Rename and Delete still do;
+2. Sensor Health's new `throughput` group counts the events each session delivered;
+3. the diagnostic log gives each thread's CPU per periodic report.
+
+**Plan 1b-4a-p1 review fixes (2026-10-15).** The plan's independent review found one major and seven minor issues; each fix has a test checked by reverting it. Applied to §9.3:
+1. each session's callback counters are the agent's and are handed to every consumer it starts, so a restarted session (plan 1b-4b) counts on into them instead of leaving the health thread reading a dead consumer's;
+2. per-thread CPU is differenced per thread, then summed by name, so a thread that ends no longer takes its whole lifetime out of its name's sum; the final report's threads are listed before the sensor stops; the threads are listed once a period for a report of either kind, with the listings' own interval;
+3. tests: every file action's invalidation, the sessions' order, the final report's count, and a report that lists the threads.
```

```diff
--- a/docs/schema-reference.md
+++ b/docs/schema-reference.md
@@ -151,8 +151,8 @@ Tampering seen on the agent's own ETW sessions (sensor spec §9).
 
 ### Sensor Health
 
-The agent's own loss, quality, housekeeping, resource and buffer figures, every 60 s (sensor spec §9.3). No actor.
-`interval_start` (i64 ns) starts the interval; the event's `time` ends it. One activity, Report, with five groups
+The agent's own loss, quality, housekeeping, resource, buffer and throughput figures, every 60 s (sensor spec §9.3). No actor.
+`interval_start` (i64 ns) starts the interval; the event's `time` ends it. One activity, Report, with six groups
 (field lists in `sensor_health.proto`) and an optional gap:
 
 | Group | Holds |
@@ -161,6 +161,7 @@ The agent's own loss, quality, housekeeping, resource and buffer figures, every
 | `quality` | late arrivals, parse errors, unknown versions, join misses, `actor_unresolved` per class, unresolved file objects and registry paths, value-read failures, invalid buffer records, enrichment misses, ambiguous registry value names, file deletes reported without a Cleanup outcome (SMB), Cleanups whose outcome could not be paired |
 | `housekeeping` | evictions from every bounded map and cache, retention evictions, dropped failed file creates and renames, seeding results, requests dropped because a service queue was full |
 | `resources` | agent CPU time in the interval (ns), working set (bytes) |
+| `throughput` | ETW events each session delivered to the agent, counted before parsing |
 | `buffer` | write errors, recoveries, `failing`, rejected records, segments that could not be deleted, startup recovery (truncated bytes, cursor reset, foreign segments), corrupt segments, disk bytes |
 | `gap` | events deleted from the buffer before delivery: `events`, and `first_time` / `last_time` (both or neither, ordered) |
 
```

- [ ] **Step 2: Commit**

```powershell
git add docs
git commit -m "docs(1): plan 1b-4a-p1 clarifications, findings and schema reference"
```

### Task 5: PR and merge

- [ ] **Step 1:** Push `feat/1b-4a-p1` and open the PR. The body summarises the cause (F3), the fix, the two new figures, the review's fixes, the before and after (F4 and Task 3), the test counts, and ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto` (with `buf breaking`), `powershell` and `audit`. If a test fails only on the runner, fix it with a test, never by skipping.
- [ ] **Step 3:** When every check on the PR's current head is green, merge. Then update the decision log's build row if anything changed on the way, and delete the remote `scratch/d6` branch.

## Review Log

### Independent review (2026-10-15, of the draft at `7368ba0`)

An independent agent read the draft against the spec, plan 1b-4a's interfaces and the code, built it in its own target directory, and ran the unelevated tests (409 passed, 18 ignored; clippy and fmt clean). It found the SetAttributes fix complete and safe, and one major issue, seven minor ones and some nits. Each fix was made in the scratch worktree with a test, and each test was checked by reverting its fix (the verification's reverts).

**On the fix's safety:** the hash cache's correctness never depended on the path invalidation. Its key is (volume, 128-bit file id, USN), with no caching when the USN is 0 or the volume keeps none; it is bypassed while any handle can write the file (`writer_open`; writable mapped sections count through share access); and a version that changed during the read is not cached. Any content change bumps the USN at its first write, and a SetAttributes that really changes something bumps it too, so it misses anyway. The fix only stops the re-hashing after basic-information sets that change nothing, which F3's probe (the USN stable) supports. As a side benefit, a user who may write a file's attributes can no longer make the agent hash and verify it over and over with a `SetFileTime` loop.

**Major:**
1. **A restarted session would read 0 for good (M1).** The health sources held each consumer's own counters; a session that 1b-4b restarts gets a new consumer with new ones, and the health thread would keep reading the dead consumer's: `throughput` would say 0 at every later report, a permanent false "silent session", the signal 1b-4b watches. The panic counter had the same flaw (plan 1b-4a). **Fixed** (the user chose A, Q8): the agent owns each session's `ConsumerCounters` and hands them to every consumer it starts (`consume_with`). The trampoline's unit test now counts into the caller's counters; one revert.

**Minor:**
2. **A thread that ended took its whole lifetime out of its name's sum (m1).** Sums by name were differenced as a whole, so a seeder helper replaced after a stall, or the OS's unnamed pool threads, could report 0 for a name that worked. **Fixed:** each thread (id and creation time) is differenced, then the differences are summed by name; a reused id counts in full. Two reverts.
3. **The final report's thread line was nearly empty (m2):** listed after the sensor stopped, when its threads had ended. **Fixed:** `run` lists them just before the stop. A binary-test check that the final line names the pipeline thread failed on the runner (the pipeline had used less than a clock tick since the last listing), so the test records the line instead; that run's named `atlas-seeder`, which ends at the stop.
4. **Under sustained loss the threads were never listed (m3)**, since every report was then a loss report, and a line's thread figures could cover more than its interval. **Fixed:** listed at most once a period for a report of either kind, with the listings' own interval logged (`threads_ms`). A new test under a loss at every check; two reverts.
5. **The Deletes that ask `forgets_cached` had no test (m4)**; the one cited goes through the request-path sender, which does not ask it. **Fixed:** a table test over every action; one revert.
6. **The live checks did not pin the sessions' order or the final count (m5).** **Fixed:** Session A's count must exceed B's, and the binary test's final report must count.
7. **The binary test's new check depended on the first report being periodic (m6).** **Fixed:** it waits for a report that lists the threads.
8. **§13 overstated the cause (m7):** two-thirds of the CPU was the hash workers in all; the re-checks were most of that, about half the agent's CPU. **Fixed** in the spec.
9. **The decision log had no row for Q1–Q7 (m8):** the plan PR adds the drafting row, as plan 1b-4a's did.

**Nits, fixed:** a doc comment moved onto the wrong function; the findings' numbers now match between this plan and the spec (F6 the CI test bug, F7 the working set); the schema reference's opening line names `throughput`; the trampoline test pins counting before the callback; a thread id reused by another process between the snapshot and the open is left out (`GetProcessIdOfThread`); F3's figures (11 ms per miss on average in the replay, 0.1 ms per hit) and the SetAttributes rate (250–500 a minute) made exact; the architecture's "both senders" says which two; the loop test's lister counts per test, not in a shared static. The new sustained-loss test hung on its first revert run (the test's report channel held 64 and every check sent one): its channel now holds 4,096, and the revert runner has a 10-minute timeout.

**Found, older than this plan, recorded in the Interfaces:** `SetFileShortName` (class 40) changes an 8.3 name with no Rename; a written handle with a name but no opener emits no Update.

**What held up:** the SetAttributes fix is complete (only `FileBasicInformation` maps to SetAttributes; end-of-file sets become Updates, which still forget); the delivered count is one relaxed increment before `catch_unwind`, read at the stop after the consumers finish; the trampoline test is sound (a zeroed plain-data record, a context alive throughout, a callback that never reads the record); `threads()` releases its snapshot, handles and strings on every path; the schema change is additive and no other code reads the groups; the plan's figures add up (F2's parts to 19.0%, F4's per-event costs, the per-task counts).
