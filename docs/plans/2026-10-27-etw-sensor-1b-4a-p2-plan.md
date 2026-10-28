# Sub-project 1b-4a-p2 — The Profiling Step's Remaining Fixes Implementation Plan

> **Status:** Approved by the user (2026-10-27), after an independent review (Review Log; its fixes are in). **For agentic workers:** steps use checkbox (`- [ ] `) syntax for tracking. Task 14 is a run on the host by the user, from a script. Nothing in this plan needs the VM or a kernel driver.

**Goal:** Finish the profiling step that comes before plan 1b-4b (decision log, 2026-10-12, D6): every item plan 1b-4a-p1 left over budget is fixed or decided, each from measured facts (the D6 notes, R1–R8), and the result is measured on the host against the old build under the same conditions.
- **On-miss seeding off by default** (R2, §13's first fallback): it spent its whole budget, 1% of one core, every minute, for about 1% of the answers;
- **the pipeline's measured fixes** (R3), each kept only for a gain on a checked-in replay benchmark (0.573 → 0.385 µs per event), and a loop that waits 40 ms after finding nothing;
- **invalidations that wake no thread** (R3a): a worker wake costs about 22 µs;
- **Launch images hashed ahead of module loads** (R4), closing plan 1b-4a-p1's F9 without a longer deadline;
- **each bounded structure's size in the diagnostic log** (R6), and **memory that follows contents** (R6b): the working set from 109–130 MB to 40–66 MB;
- **no power throttling** (R8): Windows had starved the agent's threads for most of 40 s under load.

After this plan, the hour run of normal use (R7.5) judges §13's budget and ends the profiling step; anything still over comes back as a decision.

**Architecture:**
- **`config`, `settings`:** `seed_on_miss` defaults to false; `agent.toml`'s `[seeding] on_miss` still turns it on.
- **`pipeline::seed`:** with on-miss seeding off, `Seeding::changed` records address history only while a start-up pass is outstanding (no later snapshot can need it).
- **`foldhash`** (a new dependency) hashes the maps keyed by kernel addresses, PIDs and ids; maps whose keys an outsider chooses (paths, endpoints) keep SipHash.
- **`ordering`:** the heap holds `(timestamp, arrival, slot)` keys; events wait in a slot vector with a free list; once fewer than an eighth of the slots hold an event, they are compacted (never below 8,192).
- **`watchlist`:** an Aho-Corasick scan (`aho-corasick`, a new direct dependency already in the tree) for each pattern's last literal run rejects most paths before the `GlobSet`.
- **`atlas-etw`'s `WStr::to_string_lossy`:** an all-ASCII name is narrowed into one `String`.
- **`driver`:** `next_wait` gives `cadence` (10 ms) after a pass that pushed events, `idle_cadence` (40 ms) after one that found none.
- **`win::services`:** `InvalidateHash` wakes no thread: the hash cache's paths go on a channel the hash workers apply before their next lookup, the 8.3 cache's into a shared list the expander applies before its next job. The hash lane is `HashLane`, two bounded queues behind a mutex and a condvar, Launch images popped first.
- **`health`, `win::health`:** `Sizes`, each structure's `(name, entries, capacity)`, read from the pipeline (between two passes) and the services' and intake's gauges; the log line's `sizes="…"`.
- **`queue`** (new): `bounded(cap)`, std's unbounded channel with a counter cap, for the sink and the kernel and user queues; `QueueGauge` reads one's fill for `sizes=`.
- **`win::power`** (new): `run_at_full_speed` and `runs_at_full_speed`; `run` calls the first before the sensor starts.

**Tech stack:** Rust 1.97 (edition 2024). New dependencies: `foldhash` 0.1.5 and `aho-corasick` 1.1.5 (workspace; both already in the lock file, through `hashbrown` and `globset`). Measurement tools (git-ignored, Task 14): the trace script and `d6load` of plan 1b-4a-p1, with `d6load qos`; a wrapper for back-to-back runs (`run-d6-ab.ps1`); `d6wait`, a TraceProcessor tool for waits, ready time and quality of service per thread.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with the clarifications of plans 1b-2 to 1b-4a-p1. Section numbers (§) refer to it. Plan 1b-4a-p1's "Interfaces for later plans" is the list this plan works through; the D6 notes (`docs/specs/2026-10-15-etw-sensor-d6-rest-brainstorm-notes.md`) have the facts behind each decision, and this plan the benchmark's steps.

**Verification note (2026-10-15 to 2026-10-27, host, Windows 11 build 26200, and the CI runner, build 26100):** every code block below was compiled and run in a scratch worktree of `main` (`45eac1d`): `scratch/p2`, then `scratch/p2-review` (the review's fixes folded into their tasks); the task commits on `tasks/1b-4a-p2` rebuild its tree exactly.
- **Lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean at the end of every task; `atlas-agent` and `atlas-etw` are also clippy-clean for `x86_64-unknown-linux-gnu`, checked from Windows at every task.
- **Unelevated:** `cargo test --workspace` passes 441 tests and ignores 19 (the live tests, the binary test, the elevated unit tests and, new, the benchmark). `atlas-agent` has 252 unit tests (plan 1b-4a-p1 left 225). Test counts per task were measured at each task's commit.
- **The benchmark** (Task 2, release, on the host): 0.573 µs per event before Task 3, 0.385 after Task 7, each step measured against the one before in the same sitting (F7: on battery with the Silent plan the same code ran 1.7 times slower).
- **On the CI runner** (the scratch branch pushed, no PR): run 38853020970 (Tasks 1–11 but the binary test's one-line note fix) passed every job, the binary test's 50 checks and the agent live test's 20 among them; run 38859494741 (Task 12) passed every job; run 40613724094 (Task 13, before the review) passed every job, the binary test's new `the_agent_runs_at_full_speed` included; run 40626102503 (the reviewed code, `98ceac3`, the final code) passed every job.
- **On the host:** two runs of the trace script on the verify-first build (F6, F7), then two A/B pairs of the old and the new build, back to back with Task 14's wrapper (F7, F8); the worker-wake and sleep probes (F2, F3; scratchpad programs, not part of this plan).
- **Reverts:** every rule in the Review Focus was checked by reverting it in the source and running its test, which must then fail (`spikes/p2-plan-src/mutate.py`, plan 1b-4a-p1's; the source restored after each). 50 reverts, all caught on the final code (2026-10-27; 48 unelevated, 2 in the user's elevated run, where the binary test's named check had to fail):
  - **on-miss seeding (1):** on by default;
  - **the address history (2):** recorded after the start-up passes; kept when they end;
  - **the ordering stage (2):** freed slots never reused; a released slot not freed;
  - **the watchlist pre-check (5):** the scan case-sensitive; a pattern without a run keeping the pre-check; alternatives taken as literal; the first run instead of the last; the pre-check rejecting everything;
  - **the ASCII names (2):** Latin-1 taken as ASCII; a lone surrogate not replaced;
  - **the loop (3):** an idle pass waiting the busy cadence; a busy pass the idle one; `drain` counting nothing;
  - **invalidations (4):** waiting invalidations never applied; an overflow forgetting without clearing; the expander ignoring a lost list; invalidations back in the expander's lane;
  - **the Launch lane (4):** module loads taken first; Launch images routed to the module queue; one bound for both queues; closing not waking the workers;
  - **the sizes (8):** a report's sizes from the previous snapshot; the services' gauges left out; the pipeline's file map left out; the ordering stage's entries as its capacity; the early map's gauge not updated on a close; the hash cache's gauge not set; the expansion cache's gauge not set; the log line's format;
  - **memory follows contents (7):** the cap not kept; a receive making no room; `send` refused at the cap; a failed send leaving the count up; the slots never compacted; compaction keeping the old slot numbers; compaction below the floor;
  - **power throttling (3):** the agent not turning it off (elevated: `FAIL the_agent_runs_at_full_speed`); throttling turned on instead of off; the reader ignoring the state;
  - **the review's fixes (9):** a user's pattern's runs taken raw, a `]` first in a class ending it (1); an overflow dropping the newest instead of clearing (2); the list collecting after an overflow, the hash invalidations not capped (3); the workers never applying the hash invalidations (4); the queues left out of the sizes, the gauge reading the cap as the fill, the sink's gauge not added (7; the last elevated: `FAIL a_report_logs_structure_sizes`).

  Two definitions went stale with the review's fixes and were corrected before the final run: a renamed test made one revert pass vacuously (cargo ran no test), and the watchlist's new class parser moved another's target.
- **Round trip:** the code was rebuilt from this document alone in a fresh LF worktree of `main`: every diff applied, and every task's tree is identical to the verified one (Tasks 4 and 6 each add one dependency line to `Cargo.lock`, shown as a diff). Its first run found those two lines missing from the plan.
- **Elevated, on the host:** the user's run of plan 1b-4a's elevated runner on the reviewed code (2026-10-27, `spikes/run-p2-elevated.ps1`): the elevated unit tests (267 passed), the agent live test (20 of 20 checks), the binary test (51 of 51, its `sizes=` line ending `kernel_queue=0/65536 user_queue=0/8192 sink=0/65536`), and the two reverts only the binary test can catch. The runner leaves its report path set during the reverts, so the JSON report holds the last revert's run; the checks above are from the run's own output.
- **Not run yet:** Task 14 on the build branch (its figures are the verify-first code's, F8, which the build branch's code equals); the Linux build of the tests is run by CI only.

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-4a-p2-plan`. The build runs on `feat/1b-4a-p2`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **Windows code only in `win`** and `#[cfg(windows)]` tests; `pipeline`, `ordering`, `queue`, `driver`, `health` and `watchlist` stay portable and Linux CI builds and tests them.
- **Every `unsafe` block has a `// SAFETY:` comment.**
- **The ETW callback stays fast** (§3.2): the event queues' `try_send` gains an atomic add (and a block allocation every 31 events, std's list channel), and the early key map two atomic stores per registry Create, Open or Close (its gauge); nothing else.
- **No behaviour change but the decided ones:** the same events, fields and counters as before for the same input; the replay tests still turn the CI recording into the same snapshot, through the driver too.
- **Elevation:** Claude never runs elevated on the host (plan 1a, D2). The live tests are `#[ignore]`d and run in CI's `etw-live` and `agent-live` jobs; Task 14's runs and the elevated revert checks are the user's.
- **The host's data stays on the host:** traces and buffer dumps stay in the git-ignored `spikes\results\`; only summary figures enter the docs.
- **No schema change.**
- **Formatting and lint:** `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` at the end of every task; `atlas-agent` and `atlas-etw` also for `x86_64-unknown-linux-gnu`.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash). Edits that contain backslashes are made with an editor, never a heredoc.
- Every commit message ends with the attribution lines the session's system reminder specifies.

## Decisions

The brainstorm (2026-10-15) decided R1–R7; verify-first re-decided R3a, R4 and R6b on probes and host runs, and added R8 (2026-10-27). The D6 notes have the facts behind each.

### R1, how to run the rest of the step (chosen 2026-10-15: A)

- **(A) Chosen: analyse every item from the kept final-code trace and the dumps, decide them one at a time, then one plan with all chosen fixes, one re-measure with the trace script, then the hour run.**
- (B) One plan per item, re-measured after each (4–5 plan cycles). (C) §13's fallbacks in order until under budget: trades telemetry without looking for a real fix.

### R2, on-miss seeding (chosen 2026-10-15: A)

- **(A) Chosen: off by default** (§13's first fallback; `[seeding] on_miss = true` turns it on). After start-up it spent its whole budget (600 ms per 60 s) every minute, and named about 20 handles in 9 minutes against about 1,500 dropped file events and 180 unresolved registry events. Start-up seeding stays.
- (B) Keep it with a 0.1% budget: answers would come after the 2 s wait, so effectively off, still paid. (C) Keep it as is.

### R3, the pipeline (chosen 2026-10-15: A)

- **(A) Chosen: a measured set of fixes, each kept only if a deterministic, unelevated replay benchmark shows a gain** (Task 2's `bench_pipeline_per_event`): the loop waits longer after a pass that found little; a fast hasher for the internal maps; the seeding address history stopped once start-up seeding is applied; fewer allocations and a cheaper watchlist pre-check where the bench points. Kept (Tasks 3–8): the history (no gain on the bench, which has no on-miss seeding after start; kept for memory, R6), foldhash (0.573 → 0.500 µs per event), the ordering stage's keys and slots (→ 0.448), the watchlist pre-check (→ 0.437; 220 → 95 ns per typical path), the ASCII names (→ 0.385), the 40 ms idle wait (about 25 wakes a second when idle instead of 100).
- (B) Only the adaptive loop and the history. (C) Leave the pipeline to sub-project 8.

### R3a, worker wakes (re-decided in verify-first, 2026-10-15: A)

R3 also chose "one worker wake per pass, not per request". A probe measured a wake at about 22 µs of CPU (F3), and under load each lane gets about one job per pass anyway, so batching saves little.
- **(A) Chosen: invalidations wake no thread.** As first built, the pipeline thread emptied the hash cache's entries itself, under the cache's lock; the review (finding 4) moved them to a channel the hash workers apply before their next lookup, so the normal-priority pipeline thread never waits on a lock a below-normal worker holds. The 8.3 cache's invalidations wait in a shared list the expander applies before its next expansion (a list that overflows its 8,192 clears the expansion cache, as a dropped invalidation did). The other requests keep one wake each and today's latency.
- (B) A, plus lanes flushed every 50 ms: another 0.2–0.3% of one core under heavy load, for up to 50 ms more latency. (C) Drop the item.

### R4, the hash workers and plan 1b-4a-p1's F9 (re-decided in verify-first, 2026-10-15: A')

- **(A') Chosen: Launch images get a priority queue ahead of module loads**; the deadline stays 1 s. A Launch then waits at most for the two jobs in progress and the Launches queued ahead of it. Module loads early in a run can still miss their hash; the miss is counted and the late result is cached for the file's next load.
- (A) As first decided: also a 5 s enrichment deadline for the first 5 minutes. Verify-first found the completion stage emits strictly in order (F4), so one waiting event holds every later one: up to 5 s of all events, about 80,000 at 16,000 events/s, near the 100,000 pending cap, exactly when the workers fall behind. (B) 2 s for the first 60 s: the same, smaller. Rejected at the first decision: the hash cache saved across restarts (a file changed offline, from an OS that does not keep the USN, would keep its old hash).

### R5, the SetAttributes events (chosen 2026-10-15: A)

- **(A) Chosen: keep them all; plan 1b-4a-p1's Q6.7 is closed.** Since p1 they cost about 2 µs each in the pipeline (0.001% of one core); Windows' SMB server works in the System process, so a filter on PID 4 would hide a remote client's timestomping on this machine's shares (F10). No code.
- (B) Drop the System process's SetAttributes on image files. (C) An elevated probe of what they set first.

### R6, the working set (chosen 2026-10-15: A)

- **(A) Chosen: measure, then right-size.** Every bounded structure's entries and capacity in the diagnostic log at every report (Task 11), then caps from the data, each a decision. The data (F6) showed the memory was not in any capped structure's contents, but in channels that allocate every slot at creation and a stage that kept a burst's slots: R6b. With R6b no cap needed changing.
- (B) Lower the largest caps by estimate. (C) Raise the budget.

### R6b, the memory fix (chosen 2026-10-15, from the first host run: A)

- **(A) Chosen: memory follows contents.** The sink and the kernel and user queues become `queue::bounded`, std's unbounded (block-list) channel with the same caps kept by a counter: the same drops, counted the same way. The ordering stage compacts its slots when fewer than an eighth hold an event (floor 8,192).
- (B) Box the sink's events only (about 44 MB less; the kernel queue and the ordering stage stay). (C) Lower the caps: counted loss in bursts.

### R7, the defaults (chosen 2026-10-15, accepted together)

1. p1's method: verify-first in a scratch worktree, the scratch branch pushed for CI, per-task commits, independent review, every fix checked by reverting it, the round trip, approval;
2. task order R2, R3, R4, R6's size line, then caps from its data (R3a, R6b and R8 joined in place);
3. R3's benchmark checked in as an `#[ignore]`d `atlas-agent` test replaying the CI recording, printing µs per event;
4. R6's line `sizes="…"` in the log only, next to `threads=`;
5. the re-measure with the trace script against the old build, then the hour run of normal use judged against §13 from the Sensor Health reports, which ends the profiling step;
6. if still over budget, the next §13 fallbacks come back as decisions;
7. docs: spec §3.2, §6.3, §7.2, §7.4, §9.3, §11.2, §11.4, §13, §15.3, §17, the decision log, the roadmap.

### R8, power throttling (chosen 2026-10-27, from the first A/B pair: A)

- **(A) Chosen: the agent opts out at start** (`SetProcessInformation(ProcessPowerThrottling)`, the execution-speed bit controlled and off, "HighQoS"), so its timeliness does not depend on Windows' window-based heuristics, and much less on the power plan, whose frequency caps still apply (F7). Cost: Windows can no longer move the agent's 1–2% of one core onto slow cores. The trace script gives every build HighQoS from outside (`d6load qos`), so the old and the new build compare.
- (B) Only the pipeline and ETW threads: the hash workers, the ones that starved, stay throttled. (C) Nothing, trusting that a service is never throttled: not verified.

## Findings

From verify-first (2026-10-15 to 2026-10-27; the host, build 26200; the CI runner, 26100). They go to §15.3 as a "Plan 1b-4a-p2" addendum (Task 15).
- **F1, the seeder's yield:** on the final code of plan 1b-4a-p1, on-miss seeding spent its whole budget every minute, 16–18 table reads a minute (two per snapshot, about 70 ms each), 68% inside the kernel's `ExpSnapShotHandleTables`, and named about 20 handles in 9 minutes against about 1,500 dropped file events and 180 unresolved registry events (R2).
- **F2, the pipeline:** quiet, half its time was `tick` and 29% the 10 ms loop's sleep (`std::thread::sleep` makes a waitable timer per call: 18 µs per wake, 0.17% of one core; 0.05% at 50 ms); under load, allocation about 25%, the ordering stage 16%, worker wakes about 10%, the watchlist 7%, SipHash 4–6%, the address history about 5%. Live it costs about 1.9 µs per event against the benchmark's 0.58: the benchmark has no real lookups, worker wakes, sink or tracing, and the recording is 45% OperationEnd (R3).
- **F3, a worker wake** (an unelevated probe, 2,000 jobs 5 ms apart): handing a job to a parked worker over `sync_channel` costs about 22 µs of CPU (the sender's `try_send` 6–7 µs, the worker's wake and park 15–16 µs). The synthetic load sent about 622 jobs a second (408 of them hash and 8.3 invalidations): about 1.4% of one core (R3a).
- **F4, the completion stage holds the line:** it emits strictly in order, so a longer deadline for some events delays all of them (R4).
- **F5, the kernel's own signature verdict** (the MOF image-load event's `SignatureLevel` and `SignatureType`; the manifest event the agent uses has neither) cannot replace `WinVerifyTrust`: it names no signer.
- **F6, the working set** (the first host run of the verify-first build, with the `sizes=` line): 107 → 151 MB, 175–182 MB after a heavy minute. std's bounded channels allocate and stamp every slot when made: the sink's 65,536 × 688 B (about 45 MB) and the kernel queue's 65,536 × 176 B (about 11.5 MB) were resident from start-up; the ordering stage's slots grew to 65,536, then 131,072, and stayed. With R6b: 43 MB at the first report, about 75 after five minutes, the ordering stage at 8,192 slots (R6b).
- **F7, power plans and quality of service:** the laptop's vendor tool switches between its Silent and Turbo plans by itself. The event log shows plan 1b-4a-p1's F10 ran on AC with Silent; the first run of the verify-first build ran mostly on Turbo (so its figures flattered it), the second on battery with Silent (the benchmark 1.7 times slower that hour). So the old and the new build were then run back to back, each refused unless on AC with Silent. In that first pair Windows ran the new build's whole process at its Low quality of service (EcoQoS) for the whole run, by its own heuristics (the old build's mostly at High; the earlier runs switched between them). No code touches priorities or QoS; which window was in front is the likely trigger. On the Silent plan a few slow cores were then the agent's: in a 40 s span of the synthetic load each hash worker was ready to run but not running for 30.5 s, the pipeline for 13.8 s, while Defender scanned each freshly copied `whoami.exe` (65–100% of one core). 3,471 enrichment misses and 143 Launches without a hash followed, cache hits included (R8).
- **F8, the final pair** (Task 14's script with R8; both builds at high QoS for the whole run): under the synthetic load the agent took 11.4% → 10.5% of one core (without the hash workers 5.3% → 3.4%; the seeder 1.0% → 0; the pipeline 2.7% → 2.3%), with 0 and 1 enrichment misses and no Launch without a hash; the working set 109–130 MB → 40–66 MB. The hash workers vary from run to run (6.1% and 6.1% for the old build in the two pairs, 5.3% and 7.2% for the new), driven by first-time checks and Defender. The old build's quiet and normal phases received 17,000–25,000 events/s of background activity it filtered out (11–13 emitted a second), and the user's Wi-Fi dropped during the new build's, so normal use does not compare; the new build's took 2.2% of one core (0.9% of it first-time hashes of 347 DLLs new to it), quiet 1.4%.
- **F9, not chased:** in both of the new build's runs that day the System process set the attributes of the same 28 system files (`ntdll.dll`, `kernel32.dll`, the `.nls` tables, the volume root) at every launch, about 12,000–13,600 times in the synthetic load, against 430–530 in the old build's runs. The new build always ran second, after the user's normal use, and its first host run showed 210: the order is as likely a cause as the build. About 2 µs each in the pipeline.
- **F10, SetAttributes from PID 4** (R5): Windows' SMB server (`srv2.sys`) works in the System process, so a remote client's changes on this machine's shares also show as PID 4.

## Deliberate clarifications of the spec (applied to the spec text in Task 15)

1. **§7.4, §11.2:** on-miss seeding is off by default; with it off, the address history ends with the start-up passes (R2, R3).
2. **§3.2:** the loop waits 40 ms after a pass that found no event; the benchmark and its figures (R3).
3. **§3.2, §6.3, §7.2:** invalidations wake no thread (R3a).
4. **§3.2, §6.3:** Launch images first; the deadline stays 1 s (R4).
5. **§9.3:** the `sizes=` line (R6).
6. **§3.2:** the queues and the sink hold memory for what they hold; the ordering stage gives a burst's slots back (R6b).
7. **§11.4:** no power throttling (R8).
8. **§13, §15.3:** the figures (F1–F10).
9. **§17:** the clarifications and the review's fixes.

## Review Focus

1. **On-miss seeding is off by default and still settable** (`defaults_follow_the_spec`; the seeder's own tests now turn it on with `T::seeding_on_miss`).
2. **With on-miss off, the address history ends with the start-up passes, and not before** (`without_on_miss_seeding_the_address_history_ends_with_the_start_up_passes`).
3. **The ordering stage keeps its order through reuse and compaction, and gives slots back** (`freed_slots_are_reused`, `after_a_burst_the_slots_shrink_and_the_order_holds`, and the existing ordering tests).
4. **The watchlist pre-check never hides a match** (`the_pre_check_never_hides_a_match`, `the_pre_check_never_hides_a_match_of_a_users_pattern` (review 1), which compares the pre-checked match with the `GlobSet` alone on matching and non-matching paths, case and stream variants included; `the_glob_set_folds_ascii_case_only_as_the_pre_check_does`; `a_pattern_without_a_literal_run_turns_the_pre_check_off`; `the_last_literal_runs`).
5. **`to_string_lossy` equals `String::from_utf16_lossy` on every input** (`to_string_lossy_is_from_utf16_lossy`, a property test; `to_string_lossy_of_latin_1_is_from_utf16_lossy`).
6. **The loop waits 10 ms after a busy pass, 40 ms after an empty one, and counts what it pushed** (`the_next_pass_waits_longer_after_finding_nothing`, `drain_counts_what_it_pushed`, `an_idle_loop_passes_at_the_idle_cadence`).
7. **An invalidation reaches each cache before its next lookup; an overflow clears the expansion cache, and past the cap nothing more is kept** (`an_invalidation_reaches_the_hash_cache_before_the_next_lookup`, `an_invalidation_waits_for_the_expanders_next_job`, `too_many_waiting_invalidations_clear_the_expansion_cache`, `invalidations_past_the_cap_are_not_kept`).
8. **Launch images go first; each queue has its own bound; closing drains, then ends** (`launch_images_are_taken_before_module_loads`, `launch_images_go_to_the_priority_queue`, `each_hash_queue_is_bounded_and_a_closed_lane_drains_then_ends`, `closing_the_lane_wakes_a_parked_worker_once_it_is_empty`).
9. **Each listed structure's size, from the current snapshot, in the log:** the pipeline's, the early key map, both caches and the three queues (`the_sizes_name_each_bounded_structure`, `a_report_carries_every_structures_size_from_now`, `the_early_maps_size_is_a_gauge`, `the_sizes_line_is_name_equals_entries_over_capacity`, `the_gauge_reads_the_fill_and_the_cap`; the binary test's `a_report_logs_structure_sizes`). Not listed: the services' lanes and a few small `RecentMap`s of the file stage.
10. **`queue::bounded` refuses at the cap like the channel it replaces, and its count stays right** (`the_cap_refuses_a_try_send_and_a_receive_makes_room`, `send_queues_past_the_cap`, `a_closed_end_is_reported_and_leaves_the_count_right`).
11. **The agent runs at full speed; the reader tells a throttled process from one that opted out** (`a_process_runs_at_full_speed_once_it_opts_out`; the binary test's `the_agent_runs_at_full_speed`).
12. **Nothing else changes:** the CI recording still gives the scenario and the snapshot (`the_ci_recording_produces_the_scenario`, `the_output_matches_the_snapshot`), the driver loop the synchronous replay's events (`the_driver_loop_gives_the_same_events`), and every earlier test passes unchanged except where on-miss seeding is turned on explicitly.

Every one of these was checked by reverting it: its test fails (verification note). Three fixes have no revert check: foldhash (Task 4) changes no behaviour and every map test runs on it; which hasher the watchlist Opens use (review 5) is a type; and tolerating a poisoned lock (review 9) would need a worker to panic holding one.

## File Structure

```
Cargo.toml, Cargo.lock                  + foldhash (workspace); aho-corasick for atlas-agent
crates/atlas-etw/src/parse/mod.rs       WStr::to_string_lossy's ASCII path; tests in parse/tests.rs
crates/atlas-agent/
  Cargo.toml                            + foldhash, aho-corasick
  src/config.rs, src/settings.rs        seed_on_miss false by default
  src/fast.rs (+ lib.rs)                FastMap, FastSet (foldhash)
  src/completion.rs, intake.rs, keymap.rs, process.rs, recent.rs,
  src/pipeline/{expand,file,mod,proc,reg,seed}.rs        the maps on foldhash; seed: the history rule; sizes
  src/ordering.rs                       keys and slots; compaction; sizes
  src/watchlist.rs                      the literal-run pre-check
  src/driver.rs                         idle_cadence; next_wait; the queues as queue::bounded
  src/queue.rs (+ lib.rs)               new: bounded queues whose memory follows contents
  src/counters.rs, src/health.rs        Sizes; the services' and intake's gauges
  src/store.rs                          the sink on queue::bounded
  src/win/services.rs                   invalidations by the caller; HashLane; gauges
  src/win/expand.rs, win/hash.rs        lengths for the gauges
  src/win/health.rs                     the sizes= line
  src/win/agent.rs                      the queues
  src/win/power.rs (+ win/mod.rs)       new: power throttling
  src/win/run.rs                        run_at_full_speed before the sensor starts; the sink's gauge
  src/pipeline/tests.rs                 seeding_on_miss; history; sizes
  tests/replay.rs                       bench_pipeline_per_event
  tests/binary.rs                       a_report_logs_structure_sizes, the_agent_runs_at_full_speed
spikes/                                 git-ignored; Task 14: run-d6-ab.ps1, run-d6-trace.ps1, d6load (qos), d6wait
docs/specs/2026-10-01-etw-sensor-design.md   Task 15
```

## Interfaces for later plans

- **The hour run** (R7.5, next): normal use for an hour with the build's agent, judged against §13 from its Sensor Health reports and the `threads=` and `sizes=` lines; it ends the profiling step. If still over budget, the next §13 fallback (`file.op_end` off) comes back as a decision. Start it after a minute of the machine being plugged in on its usual plan; HighQoS makes the plan matter less, not nothing.
- **Plan 1b-4b (blinding detection):** unchanged from plan 1b-4a-p1's interfaces. A restarted session goes through `consume_with` with the agent's counters; the sessions' own loss counters still need the base reset. The `sizes=` line shows whether any structure creeps over a long run.
- **Plan 1b-4c (the service):** call `win::power::run_at_full_speed` there too if the service does not go through `run_with_sink` (it does today), and check `the_agent_runs_at_full_speed` holds for the service process.
- **Open, not chased** (F9): whether the System process's per-launch SetAttributes depend on the agent; a run with the order of the builds swapped answers it.
- **Sub-project 8:** the benchmark is the baseline for pipeline work; the live per-event cost (about 1.9 µs before this plan) is what it does not model.

---

### Task 1: On-miss seeding off by default

**Files:**
- Modify: `crates/atlas-agent/src/config.rs`, `crates/atlas-agent/src/settings.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: The default** (R2, F1). `Config::default()`'s `seed_on_miss` is false; the defaults test says so, and `settings`' documented example shows `on_miss = false`. Start-up seeding is unchanged.

```diff
--- a/crates/atlas-agent/src/config.rs
+++ b/crates/atlas-agent/src/config.rs
@@ -53,8 +53,9 @@ pub struct Config {
     pub file_op_end: bool,
     /// Seed the key and file maps from the handle table at start (§7.4).
     pub seed_on_start: bool,
-    /// Ask the seeder about unknown handles (§7.4). Off, an event on an unknown
-    /// handle waits only for the start-up pass, while it is outstanding.
+    /// Ask the seeder about unknown handles (§7.4). Off by default, the first
+    /// §13 fallback (plan 1b-4a-p2, R2): an event on an unknown handle waits
+    /// only for the start-up pass, while it is outstanding.
     pub seed_on_miss: bool,
 }
 
@@ -87,7 +88,7 @@ impl Default for Config {
             registry_value_reads: true,
             file_op_end: true,
             seed_on_start: true,
-            seed_on_miss: true,
+            seed_on_miss: false,
         }
     }
 }
@@ -201,6 +202,7 @@ mod tests {
         let c = Config::default();
         assert_eq!((c.hold, c.confirm_window), (Duration::from_millis(750), Duration::from_millis(250)));
         assert_eq!(c.dns_rate_per_pid, 100);
-        assert!(c.network_udp && c.registry_value_reads && c.seed_on_miss);
+        assert!(c.network_udp && c.registry_value_reads && c.seed_on_start);
+        assert!(!c.seed_on_miss, "on-miss seeding is off by default (R2)");
     }
 }
```

````diff
--- a/crates/atlas-agent/src/settings.rs
+++ b/crates/atlas-agent/src/settings.rs
@@ -36,7 +36,7 @@
 //!
 //! [seeding]
 //! on_start = true
-//! on_miss = true
+//! on_miss = false            # on: the seeder answers unknown handles (§7.4)
 //! cpu_ms_per_minute = 600     # the seeder's CPU budget (§7.4)
 //! ```
 
````

- [ ] **Step 2: The seeder's tests turn it on.** The three pipeline tests that exercise on-miss seeding build their pipeline with `T::seeding_on_miss()`; every other test keeps the default.

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -50,6 +50,11 @@ impl T {
         T::with(Config::default(), FakeLookups::default())
     }
 
+    /// With on-miss seeding on (off by default since plan 1b-4a-p2, R2).
+    fn seeding_on_miss() -> T {
+        T::with(Config { seed_on_miss: true, ..Config::default() }, FakeLookups::default())
+    }
+
     fn with(config: Config, lookups: FakeLookups) -> T {
         let lookups = lookups.with_device(r"\Device\HarddiskVolume3", "C:");
         let mut accounts = lookups.accounts.clone();
@@ -625,7 +630,7 @@ fn a_set_attributes_leaves_the_caches_alone_and_an_update_does_not() {
 /// answered without forgetting, the waiting Update forgets.
 #[test]
 fn a_set_attributes_the_seeder_answers_leaves_the_caches_alone() {
-    let mut t = T::new();
+    let mut t = T::seeding_on_miss();
     running(&mut t, 100, 10, EXPLORER);
     let f = r"\Device\HarddiskVolume3\Windows\System32\kernel32.dll";
     t.ev(5, 100, Some(key(10)), set_info(0xB, 4)); // a handle from before the agent
@@ -1006,7 +1011,7 @@ fn short_names_in_emitted_paths_are_expanded() {
 
 #[test]
 fn an_unknown_handle_is_named_by_the_seeder_or_dropped() {
-    let mut t = T::new();
+    let mut t = T::seeding_on_miss();
     running(&mut t, 100, 10, EXPLORER);
     t.ev(5, 100, Some(key(10)), write(0xA)); // opened before we watched
     t.ev(6, 4, None, RawEvent::FileCleanup(handle(0xA)));
@@ -1433,7 +1438,7 @@ fn bookkeeping_is_freed_when_no_reply_comes() {
 fn a_reused_base_never_names_old_children() {
     // 0x50 was opened before we watched, and Run is opened below it. Then 0x50
     // closes: Run can only be named by asking about Run itself.
-    let mut t = T::new();
+    let mut t = T::seeding_on_miss();
     running(&mut t, 100, 10, EXPLORER);
     t.ev(5, 100, Some(key(10)), RawEvent::RegOpenKey(reg_open(0x60, 0x50, "Run", 0)));
     t.ev(6, 100, Some(key(10)), key_close(0x50));
@@ -1464,7 +1469,7 @@ fn a_reused_base_never_names_old_children() {
 
 #[test]
 fn an_address_absent_from_the_table_is_answered_at_once() {
-    let mut t = T::new();
+    let mut t = T::seeding_on_miss();
     running(&mut t, 100, 10, EXPLORER);
     t.ev(5, 100, Some(key(10)), reg_set(0x70, "v", 4, 4)); // its key closed before the read
     t.ev(6, 100, Some(key(10)), write(0x80));
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 411 passed, 18 ignored (`atlas-agent` unit tests: 225).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): on-miss seeding off by default (D6 R2)"
```

### Task 2: A replay benchmark of the pipeline's cost per event

**Files:**
- Modify: `crates/atlas-agent/tests/replay.rs`

- [ ] **Step 1: The benchmark** (R3, R7.3). An `#[ignore]`d test replays the CI recording through one pipeline 300 times, its time squeezed ten times (about 4,500 events/s), each round's kernel objects moved (`shift`) so the maps grow as on a host, with the driver's 10 ms ticks and the fake workers' answers (the start-up seeding passes answered with an empty table). It prints the best of 10 runs in µs per event. It is the yardstick for Tasks 3–8: a change is kept only if the number falls, compared within one sitting (the machine's load moves it; F7).

```diff
--- a/crates/atlas-agent/tests/replay.rs
+++ b/crates/atlas-agent/tests/replay.rs
@@ -12,7 +12,7 @@ use std::collections::HashMap;
 use std::path::PathBuf;
 use std::sync::Arc;
 use std::sync::mpsc::{Receiver, sync_channel};
-use std::time::Duration;
+use std::time::{Duration, Instant};
 
 use atlas_agent::config::{Config, Ticks};
 use atlas_agent::counters::IntakeCounters;
@@ -22,7 +22,7 @@ use atlas_agent::input::{Header, Incoming, Session};
 use atlas_agent::intake::{Intake, Queues};
 use atlas_agent::pipeline::{Pipeline, Setup};
 use atlas_agent::process::Identity;
-use atlas_agent::services::{LiveProcess, Reply, Request, ValueData};
+use atlas_agent::services::{LiveProcess, Reply, Request, Snapshot, ValueData};
 use atlas_agent::time::Anchor;
 use atlas_etw::Provider;
 use atlas_etw::parse::{EventMeta, PointerSize, RawEvent, parse};
@@ -402,3 +402,101 @@ fn the_replay_is_deterministic() {
     let b: Vec<Vec<u8>> = run().0.into_iter().map(encode_event).collect();
     assert_eq!(a, b);
 }
+
+/// The pipeline thread's cost per event (plan 1b-4a-p2, R3), the yardstick for
+/// its fixes: the recording, its time squeezed 10 times (about 4,500 events/s,
+/// normal use's order), replayed `ROUNDS` times through one pipeline with the
+/// kernel objects moved each round (so its maps grow as on a host), the
+/// driver's 10 ms ticks, and the fake workers' answers (the start-up seeding
+/// passes answered with an empty table). The best of `RUNS`.
+/// Run it in release:
+/// `cargo test --release -p atlas-agent --test replay -- --ignored --nocapture bench`
+#[test]
+#[ignore = "a benchmark: run it in release, with --nocapture"]
+fn bench_pipeline_per_event() {
+    const ROUNDS: i64 = 300;
+    const SQUEEZE: i64 = 10;
+    const RUNS: usize = 10;
+    let (mut best, mut events, mut emitted) = (f64::MAX, 0, 0);
+    for _ in 0..RUNS {
+        let Replay { mut p, intake, krx, urx, first, last, .. } = replay();
+        drop(intake);
+        let mut one: Vec<Incoming> = krx.try_iter().chain(urx.try_iter()).collect();
+        one.sort_by_key(|i| i.header.ts);
+        let span = (last - first) / SQUEEZE + FREQ / 100;
+        let mut all = Vec::with_capacity(one.len() * ROUNDS as usize);
+        for r in 0..ROUNDS {
+            for inc in &one {
+                let mut inc = inc.clone();
+                inc.header.ts = first + (inc.header.ts - first) / SQUEEZE + r * span;
+                shift(&mut inc.event, r as u64 * 0x10_0000);
+                all.push(inc);
+            }
+        }
+        events = all.len();
+        let step = FREQ / 100;
+        let mut now = first;
+        let mut out = 0;
+        let mut queue = all.into_iter().peekable();
+        let t0 = Instant::now();
+        loop {
+            while let Some(inc) = queue.next_if(|i| i.header.ts <= now) {
+                p.push(inc);
+            }
+            out += p.tick(now).len();
+            for r in p.take_requests() {
+                // The seeder answers the start-up passes with an empty table, as
+                // on a host whose earlier handles all closed.
+                let reply = match r {
+                    Request::Seed { kind, addresses } => Some(Reply::Snapshot(Snapshot {
+                        kind,
+                        taken: now,
+                        asked: addresses,
+                        named: vec![],
+                        unnamable: vec![],
+                    })),
+                    r => answer(&r),
+                };
+                if let Some(reply) = reply {
+                    p.reply(reply);
+                }
+            }
+            if queue.peek().is_none() {
+                break;
+            }
+            now += step;
+        }
+        out += p.stop().len();
+        best = best.min(t0.elapsed().as_secs_f64() * 1e6 / events as f64);
+        emitted = out;
+    }
+    println!("pipeline: {best:.3} us per event, best of {RUNS} ({events} events in, {emitted} out)");
+}
+
+/// Moves every kernel object address (file objects, IRPs, key objects) by `k`,
+/// so a replayed round names other objects than the last; a base of 0 (an
+/// absolute registry path) stays 0.
+fn shift(e: &mut RawEvent, k: u64) {
+    match e {
+        RawEvent::FileCreate(f) | RawEvent::FileCreateNew(f) => (f.irp, f.file_object) = (f.irp + k, f.file_object + k),
+        RawEvent::FileCleanup(f) | RawEvent::FileClose(f) => (f.irp, f.file_object) = (f.irp + k, f.file_object + k),
+        RawEvent::FileWrite(f) => (f.irp, f.file_object) = (f.irp + k, f.file_object + k),
+        RawEvent::FileSetInfo(f) | RawEvent::FileSetDelete(f) => {
+            (f.irp, f.file_object) = (f.irp + k, f.file_object + k)
+        }
+        RawEvent::FileOpEnd(f) => f.irp += k,
+        RawEvent::FileDeletePath(f) | RawEvent::FileRenamePath(f) => {
+            (f.irp, f.file_object) = (f.irp + k, f.file_object + k)
+        }
+        RawEvent::RegCreateKey(r) | RawEvent::RegOpenKey(r) => {
+            r.key_object += k;
+            if r.base_object != 0 {
+                r.base_object += k;
+            }
+        }
+        RawEvent::RegDeleteKey(r) | RawEvent::RegCloseKey(r) => r.key_object += k,
+        RawEvent::RegSetValue(r) => r.key_object += k,
+        RawEvent::RegDeleteValue(r) => r.key_object += k,
+        _ => {}
+    }
+}
```

- [ ] **Step 2: Run it**

```powershell
cargo test --release -p atlas-agent --test replay -- --ignored --nocapture bench
```
Expected (the host, on AC; the figure before Task 3): `pipeline: 0.573 us per event, best of 10 (… events in, … out)`, within about 4% from run to run.

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --test replay
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass, the benchmark ignored. Workspace: 411 passed, 19 ignored (`atlas-agent` unit tests: 225).
```powershell
git add crates/atlas-agent
git commit -m "test(agent): a replay benchmark of the pipeline's cost per event (D6 R3)"
```

### Task 3: With on-miss seeding off, the address history ends with the start-up passes

**Files:**
- Modify: `crates/atlas-agent/src/pipeline/mod.rs`, `crates/atlas-agent/src/pipeline/seed.rs`, `crates/atlas-agent/src/pipeline/tests.rs`

- [ ] **Step 1: The test** (R3, §7.4). With on-miss seeding off, a Create/Open/Close recorded while a start-up pass is outstanding is in the history; once both passes are applied, nothing more is recorded. With it on, the history is kept as before.

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -1570,6 +1570,32 @@ fn without_on_miss_seeding_only_the_start_up_pass_is_waited_for() {
     assert_eq!((t.p.counters().unknown_file_object, t.p.completion.pending_len()), (1, 0));
 }
 
+#[test]
+fn without_on_miss_seeding_the_address_history_ends_with_the_start_up_passes() {
+    // Plan 1b-4a-p2, R3: with on-miss seeding off no snapshot comes after the
+    // start-up passes, so the reuse rule's address history goes with them.
+    let history = |t: &T| t.p.bookkeeping().into_iter().find(|(n, _)| *n == "address history").unwrap().1;
+    for on_miss in [false, true] {
+        let mut t = T::with(Config { seed_on_miss: on_miss, ..Config::default() }, FakeLookups::default());
+        running(&mut t, 100, 10, EXPLORER);
+        t.ev(5, 100, Some(key(10)), create(0x1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+        t.at(1_000);
+        assert_eq!(history(&t), 1, "kept while a start-up pass is to come (on_miss {on_miss})");
+        for kind in [HandleKind::Key, HandleKind::File] {
+            t.reply(Reply::Snapshot(Snapshot {
+                kind,
+                taken: ms(1_000),
+                asked: vec![],
+                named: vec![],
+                unnamable: vec![],
+            }));
+        }
+        t.ev(1_100, 100, Some(key(10)), create(0x2, 0xB, r"\Device\HarddiskVolume3\b.txt", 0));
+        t.at(2_000);
+        assert_eq!(history(&t), if on_miss { 2 } else { 0 }, "on_miss {on_miss}");
+    }
+}
+
 #[test]
 fn a_clean_stop_drops_what_a_deadline_would() {
     let mut t = T::new();
```

- [ ] **Step 2: The rule.** `Seeding` knows whether on-miss seeding is on; `changed` records an address's change only while a snapshot can still come (`history_needed`: on-miss on, a start-up pass outstanding, or one queued). The negative cache is still cleared on every change.

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -103,7 +103,7 @@ impl<L: Lookups> Pipeline<L> {
             keys: KeyMap::new(config.key_map_cap),
             reg: Registry::default(),
             net: Network::new(config.flow_cap, t(config.udp_idle)),
-            seeding: Seeding::new(),
+            seeding: Seeding::new(config.seed_on_miss),
             expands: Expands::default(),
             self_uids: self_keys.iter().map(|k| identity.uid(*k)).collect(),
             self_keys: self_keys.into_iter().collect(),
```

```diff
--- a/crates/atlas-agent/src/pipeline/seed.rs
+++ b/crates/atlas-agent/src/pipeline/seed.rs
@@ -39,10 +39,14 @@ pub struct Seeding {
     changes: Recent<(HandleKind, u64)>,
     /// Kinds whose start-up pass has not been applied yet.
     startup: HashSet<HandleKind>,
+    /// On-miss seeding (§7.4). Off, no snapshot comes after the start-up
+    /// passes, so the address history is kept only until they are applied
+    /// (plan 1b-4a-p2, R3).
+    on_miss: bool,
 }
 
 impl Seeding {
-    pub(super) fn new() -> Self {
+    pub(super) fn new(on_miss: bool) -> Self {
         Seeding {
             asks: HashMap::new(),
             last_request: HashMap::new(),
@@ -52,6 +56,7 @@ impl Seeding {
             unnamable_seq: 0,
             changes: Recent::new(CHANGES_CAP),
             startup: HashSet::new(),
+            on_miss,
         }
     }
 
@@ -120,10 +125,17 @@ impl Seeding {
 
     /// A Create/Open/Close for this address: it may now belong to someone else.
     pub(super) fn changed(&mut self, kind: HandleKind, address: u64, ts: i64) {
-        self.changes.insert((kind, address), ts);
+        if self.history_needed() {
+            self.changes.insert((kind, address), ts);
+        }
         self.unnamable.remove(&(kind, address));
     }
 
+    /// Whether a snapshot can still come, so its reuse rule needs the history.
+    fn history_needed(&self) -> bool {
+        self.on_miss || !self.startup.is_empty() || !self.queued.is_empty()
+    }
+
     pub(super) fn last_change(&self, kind: HandleKind, address: u64) -> Option<i64> {
         self.changes.get(&(kind, address))
     }
@@ -210,6 +222,9 @@ impl<L: Lookups> Pipeline<L> {
         }
         if s.asked.is_empty() {
             self.seeding.startup.remove(&kind);
+            if !self.seeding.history_needed() {
+                self.seeding.changes = Recent::new(CHANGES_CAP);
+            }
         }
         self.seeding.latest.insert(kind, in_table);
     }
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 412 passed, 19 ignored (`atlas-agent` unit tests: 226).
```powershell
git add crates/atlas-agent
git commit -m "perf(agent): with on-miss seeding off, the address history ends with the start-up passes (D6 R3)"
```

### Task 4: foldhash for the maps keyed by kernel addresses, PIDs and ids

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `crates/atlas-agent/Cargo.toml`, `crates/atlas-agent/src/lib.rs`
- Create: `crates/atlas-agent/src/fast.rs`
- Modify: `crates/atlas-agent/src/completion.rs`, `intake.rs`, `keymap.rs`, `process.rs`, `recent.rs`, `pipeline/expand.rs`, `pipeline/file.rs`, `pipeline/mod.rs`, `pipeline/proc.rs`, `pipeline/reg.rs`, `pipeline/seed.rs`

- [ ] **Step 1: `FastMap` and `FastSet`** (R3, F2: SipHash was 4–6% of the pipeline under load). `fast` defines them over foldhash's fixed-seed hasher. Only maps keyed by values the kernel or the agent chooses use them: object and `Irp` addresses, PIDs, start keys, pending ids. Maps whose keys an outsider chooses (paths, network endpoints) keep std's SipHash, so a crafted key set cannot degrade them; `Recent` takes its hasher (`new` is foldhash, `new_in` any other), so the set of watchlist Opens, keyed by path, keeps SipHash (review 5).

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -16,6 +16,7 @@ atlas-schema = { path = "crates/atlas-schema" }
 blake3 = "1.8"
 clap = { version = "4.6", features = ["derive"] }
 crc32c = "0.6"
+foldhash = "0.1.5"
 globset = "0.4"
 criterion = "0.8"
 prost = "0.14"
```

```diff
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -98,6 +98,7 @@ dependencies = [
  "atlas-schema",
  "blake3",
  "clap",
+ "foldhash",
  "globset",
  "proptest",
  "prost",
```

```diff
--- a/crates/atlas-agent/Cargo.toml
+++ b/crates/atlas-agent/Cargo.toml
@@ -14,6 +14,7 @@ atlas-proto.workspace = true
 atlas-schema.workspace = true
 blake3.workspace = true
 clap.workspace = true
+foldhash.workspace = true
 globset.workspace = true
 prost-reflect.workspace = true
 serde.workspace = true
```

```diff
--- /dev/null
+++ b/crates/atlas-agent/src/fast.rs
@@ -0,0 +1,10 @@
+//! Hash maps for keys no outsider chooses (plan 1b-4a-p2, R3): kernel object
+//! addresses, IRPs, PIDs, start keys and the agent's own ids. They hash with
+//! foldhash, seeded per process, which costs a fraction of std's SipHash per
+//! lookup. Keys an outsider can choose (paths, names, network endpoints) keep
+//! std's SipHash, which resists hash flooding.
+
+use std::collections::{HashMap, HashSet};
+
+pub type FastMap<K, V> = HashMap<K, V, foldhash::fast::RandomState>;
+pub type FastSet<K> = HashSet<K, foldhash::fast::RandomState>;
```

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -8,6 +8,7 @@ pub mod driver;
 pub mod dump;
 pub mod evict;
 pub mod fakes;
+pub mod fast;
 pub mod health;
 pub mod input;
 pub mod intake;
```

- [ ] **Step 2: The maps.** Each such map changes type; nothing else changes.

```diff
--- a/crates/atlas-agent/src/completion.rs
+++ b/crates/atlas-agent/src/completion.rs
@@ -5,7 +5,8 @@
 //! Deadlines are QPC times. A reason without one (the failure-confirm window,
 //! which is stream time) is resolved or cancelled by the pipeline itself.
 
-use std::collections::{HashMap, VecDeque};
+use crate::fast::FastMap;
+use std::collections::VecDeque;
 
 /// Why an event is incomplete (§3.2).
 #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
@@ -56,7 +57,7 @@ struct Entry<E> {
 
 pub struct Completion<E> {
     queue: VecDeque<Slot<E>>,
-    pending: HashMap<PendingId, Entry<E>>,
+    pending: FastMap<PendingId, Entry<E>>,
     next: PendingId,
     cap: usize,
     overflow: u64,
@@ -72,7 +73,7 @@ impl<E> Completion<E> {
     pub fn new(pending_cap: usize) -> Self {
         Completion {
             queue: VecDeque::new(),
-            pending: HashMap::new(),
+            pending: FastMap::default(),
             next: 0,
             cap: pending_cap,
             overflow: 0,
```

```diff
--- a/crates/atlas-agent/src/intake.rs
+++ b/crates/atlas-agent/src/intake.rs
@@ -9,7 +9,7 @@
 //! - DNS-Client events pass a per-PID token bucket, then go to the user-mode
 //!   queue; everything else goes to the kernel queue. Full queues drop and count.
 
-use std::collections::{HashMap, HashSet, VecDeque};
+use std::collections::VecDeque;
 use std::sync::Arc;
 use std::sync::mpsc::{SyncSender, TrySendError};
 
@@ -18,6 +18,7 @@ use atlas_etw::parse::{FileOpEnd, ParseError, RawEvent};
 use crate::cleanup;
 use crate::config::{Config, Ticks};
 use crate::counters::IntakeCounters;
+use crate::fast::{FastMap, FastSet};
 use crate::input::{Header, Incoming, Session};
 use crate::recent::Recent;
 use crate::services::{EarlyKey, ValueRead};
@@ -41,7 +42,7 @@ pub struct Intake {
     dns: Buckets,
     early: Option<EarlyKeys>,
     fast_read: Option<FastRead>,
-    self_keys: HashSet<u64>,
+    self_keys: FastSet<u64>,
     /// `None` while `file.op_end` is off: no OperationEnd comes, so no
     /// Cleanup is paired or counted as unpaired (plan 1b-4a).
     cleanups: Option<Cleanups>,
@@ -186,17 +187,17 @@ impl Intake {
 /// oldest.
 pub(crate) struct Cleanups {
     /// Cleanup Irp → (FileObject, FileKey, QPC).
-    irps: HashMap<u64, (u64, u64, i64)>,
+    irps: FastMap<u64, (u64, u64, i64)>,
     /// A successful OperationEnd that reports a removed name and found no
     /// Cleanup: Irp → its header and outcome.
-    early: HashMap<u64, (Header, u64)>,
+    early: FastMap<u64, (Header, u64)>,
     /// FileKeys with a delete requested (`DeletePath`, `SetDelete` set, or a
     /// delete-on-close handle's Cleanup) and not cleared or reported since.
     requested: Recent<u64>,
     /// A request's Irp → its FileKey, until the request's OperationEnd: a
     /// failed one takes the request back, a successful one is passed on so the
     /// pipeline knows the request stood (R-M1).
-    request_irps: HashMap<u64, u64>,
+    request_irps: FastMap<u64, u64>,
     /// Handles opened delete-on-close and not closed yet.
     on_close: Recent<u64>,
     /// An outcome comes within this many ticks of its Cleanup (1 s). The rules
@@ -219,10 +220,10 @@ const DELETE_REQUESTS_CAP: usize = 4096;
 impl Cleanups {
     fn new(ticks: Ticks, counters: Arc<IntakeCounters>) -> Self {
         Cleanups {
-            irps: HashMap::new(),
-            early: HashMap::new(),
+            irps: FastMap::default(),
+            early: FastMap::default(),
             requested: Recent::new(DELETE_REQUESTS_CAP),
-            request_irps: HashMap::new(),
+            request_irps: FastMap::default(),
             on_close: Recent::new(DELETE_REQUESTS_CAP),
             window: ticks.frequency.max(1),
             counters,
@@ -337,7 +338,7 @@ impl Cleanups {
 /// A relative open whose base is unknown is simply not cached; a miss costs
 /// only a fall back to the ordered path. Bounded, evicting the oldest.
 pub struct EarlyKeys {
-    names: HashMap<u64, (String, u64)>,
+    names: FastMap<u64, (String, u64)>,
     order: VecDeque<(u64, u64)>,
     generation: u64,
     cap: usize,
@@ -345,7 +346,7 @@ pub struct EarlyKeys {
 
 impl EarlyKeys {
     pub fn new(cap: usize) -> Self {
-        EarlyKeys { names: HashMap::new(), order: VecDeque::new(), generation: 0, cap }
+        EarlyKeys { names: FastMap::default(), order: VecDeque::new(), generation: 0, cap }
     }
 
     /// Returns how many entries were evicted to make room.
@@ -408,12 +409,12 @@ struct Buckets {
     /// Tokens are kept in millitokens.
     rate: u64,
     frequency: i64,
-    by_pid: HashMap<u32, (u64, i64)>,
+    by_pid: FastMap<u32, (u64, i64)>,
 }
 
 impl Buckets {
     fn new(rate: u32, frequency: i64) -> Self {
-        Buckets { rate: u64::from(rate) * 1000, frequency, by_pid: HashMap::new() }
+        Buckets { rate: u64::from(rate) * 1000, frequency, by_pid: FastMap::default() }
     }
 
     fn take(&mut self, pid: u32, ts: i64) -> bool {
```

```diff
--- a/crates/atlas-agent/src/keymap.rs
+++ b/crates/atlas-agent/src/keymap.rs
@@ -13,7 +13,7 @@
 //! The same holds when an unknown base closes, and when the seeder names a
 //! tombstone's address after the tombstone closed (plan 1b-3a review B3).
 
-use std::collections::{HashMap, HashSet};
+use crate::fast::{FastMap, FastSet};
 
 /// What the map knows about a key handle.
 #[derive(Debug, Clone, PartialEq, Eq)]
@@ -51,9 +51,9 @@ pub enum Resolved {
 }
 
 pub struct KeyMap {
-    map: HashMap<u64, Entry>,
+    map: FastMap<u64, Entry>,
     /// base → children waiting for it.
-    waiting: HashMap<u64, HashSet<u64>>,
+    waiting: FastMap<u64, FastSet<u64>>,
     cap: usize,
     clock: u64,
     evictions: u64,
@@ -63,7 +63,7 @@ const MAX_DEPTH: usize = 64;
 
 impl KeyMap {
     pub fn new(cap: usize) -> Self {
-        KeyMap { map: HashMap::new(), waiting: HashMap::new(), cap, clock: 0, evictions: 0 }
+        KeyMap { map: FastMap::default(), waiting: FastMap::default(), cap, clock: 0, evictions: 0 }
     }
 
     /// A successful CreateKey or OpenKey (§7.4).
```

```diff
--- a/crates/atlas-agent/src/process.rs
+++ b/crates/atlas-agent/src/process.rs
@@ -4,8 +4,7 @@
 //! entries, so "the entry live at the event's timestamp" (§5.3) survives PID
 //! reuse. An entry stays `retention` after its Terminate, then is removed.
 
-use std::collections::HashMap;
-
+use crate::fast::FastMap;
 use atlas_schema::{BootId, DeviceUid, File, Integrity, ProcessRef, ProcessUid, User, process_uid};
 
 /// The start key of a new process (§5.2, S1): `(BootId << 48) | sequence`.
@@ -111,8 +110,8 @@ struct Entry {
 }
 
 pub struct ProcessCache {
-    by_key: HashMap<u64, Entry>,
-    by_pid: HashMap<u32, Vec<u64>>,
+    by_key: FastMap<u64, Entry>,
+    by_pid: FastMap<u32, Vec<u64>>,
     retention: i64,
     cap: usize,
     evictions: u64,
@@ -122,8 +121,8 @@ pub struct ProcessCache {
 impl ProcessCache {
     pub fn new(retention_ticks: i64, cap: usize) -> Self {
         ProcessCache {
-            by_key: HashMap::new(),
-            by_pid: HashMap::new(),
+            by_key: FastMap::default(),
+            by_pid: FastMap::default(),
             retention: retention_ticks,
             cap,
             evictions: 0,
```

```diff
--- a/crates/atlas-agent/src/recent.rs
+++ b/crates/atlas-agent/src/recent.rs
@@ -3,11 +3,14 @@
 //! sets every tick, and a scan of a large map per event or per tick is the
 //! trap `evict` describes.
 
+use crate::fast::FastMap;
 use std::collections::{HashMap, VecDeque};
-use std::hash::Hash;
+use std::hash::{BuildHasher, Hash};
 
-pub struct Recent<K> {
-    map: HashMap<K, i64>,
+/// `S` is the map's hasher: foldhash by default ([`crate::fast`]); a set keyed
+/// by something an outsider chooses (a path) takes std's.
+pub struct Recent<K, S = foldhash::fast::RandomState> {
+    map: HashMap<K, i64, S>,
     /// Insertion order, with the time inserted. An entry whose time no longer
     /// matches the map (the key was seen again, or removed) is stale and skipped.
     order: VecDeque<(i64, K)>,
@@ -18,7 +21,14 @@ pub struct Recent<K> {
 impl<K: Hash + Eq + Clone> Recent<K> {
     /// At most `cap` keys (and queued times) are kept; past it the oldest go.
     pub fn new(cap: usize) -> Self {
-        Recent { map: HashMap::new(), order: VecDeque::new(), cap: cap.max(1), evictions: 0 }
+        Recent::new_in(cap)
+    }
+}
+
+impl<K: Hash + Eq + Clone, S: BuildHasher + Default> Recent<K, S> {
+    /// As [`Recent::new`], with the hasher `S`.
+    pub fn new_in(cap: usize) -> Self {
+        Recent { map: HashMap::default(), order: VecDeque::new(), cap: cap.max(1), evictions: 0 }
     }
 
     /// Records `key` at `ts`; the latest time wins.
@@ -78,14 +88,14 @@ impl<K: Hash + Eq + Clone> Recent<K> {
 /// Like [`Recent`], with a value per key: forgotten oldest first, by time or
 /// past the cap, with no scans.
 pub struct RecentMap<K, V> {
-    map: HashMap<K, (i64, V)>,
+    map: FastMap<K, (i64, V)>,
     order: VecDeque<(i64, K)>,
     cap: usize,
 }
 
 impl<K: Hash + Eq + Clone, V> RecentMap<K, V> {
     pub fn new(cap: usize) -> Self {
-        RecentMap { map: HashMap::new(), order: VecDeque::new(), cap: cap.max(1) }
+        RecentMap { map: FastMap::default(), order: VecDeque::new(), cap: cap.max(1) }
     }
 
     /// Records `key` at `ts`; a second insert replaces the value and the time.
```

```diff
--- a/crates/atlas-agent/src/pipeline/expand.rs
+++ b/crates/atlas-agent/src/pipeline/expand.rs
@@ -7,8 +7,6 @@
 //! each is a slot of the same pending event, and the event waits until every
 //! slot has answered or the deadline passes.
 
-use std::collections::HashMap;
-
 use atlas_schema::classes::file::FileAction;
 use atlas_schema::classes::module::ModuleAction;
 use atlas_schema::classes::process::ProcessActivity;
@@ -16,6 +14,7 @@ use atlas_schema::{Event, EventKind, File};
 
 use super::{Pipeline, alive};
 use crate::completion::{PendingId, Reason, Wait};
+use crate::fast::FastMap;
 use crate::paths::has_short_name;
 use crate::process::file_name;
 use crate::services::{Lookups, Request};
@@ -51,9 +50,9 @@ impl Expansion {
 
 #[derive(Default)]
 pub struct Expands {
-    by_slot: HashMap<(PendingId, u8), Expansion>,
+    by_slot: FastMap<(PendingId, u8), Expansion>,
     /// Per event: slots still unanswered, and how many it asked for.
-    outstanding: HashMap<PendingId, (u8, u8)>,
+    outstanding: FastMap<PendingId, (u8, u8)>,
 }
 
 impl Expands {
```

```diff
--- a/crates/atlas-agent/src/pipeline/file.rs
+++ b/crates/atlas-agent/src/pipeline/file.rs
@@ -9,7 +9,7 @@
 //! reports no outcome (SMB), a Cleanup of a file with a delete outstanding is
 //! taken as the delete (`file_delete_outcome_unknown`).
 
-use std::collections::{HashMap, VecDeque};
+use std::collections::VecDeque;
 
 use atlas_etw::parse::{FileCreate, FileHandle, FileOpEnd, FilePath, FileSetInfo};
 use atlas_schema::classes::file::{FileAction, FileSystemActivity};
@@ -20,6 +20,7 @@ use super::expand::{Expansion, Slot};
 use crate::cleanup;
 use crate::completion::{PendingId, Reason, Wait};
 use crate::counters::Class;
+use crate::fast::FastMap;
 use crate::input::Header;
 use crate::paths::has_short_name;
 use crate::process::{empty_file, file_name};
@@ -85,7 +86,7 @@ enum Held {
 }
 
 pub struct Files {
-    pub(crate) map: HashMap<u64, FileEntry>,
+    pub(crate) map: FastMap<u64, FileEntry>,
     cap: usize,
     clock: u64,
     evictions: u64,
@@ -96,15 +97,15 @@ pub struct Files {
     /// The position of `held`'s front, counted since the start.
     held_base: u64,
     /// Irp → position of the latest held operation with it (Irps are recycled).
-    held_by_irp: HashMap<u64, u64>,
+    held_by_irp: FastMap<u64, u64>,
     /// Failed OperationEnds no held operation claimed yet (a late operation): Irp → QPC.
     failed: Recent<u64>,
     /// Recently confirmed operations, to recognise a failure that comes too late.
     confirmed: Recent<u64>,
     /// Watchlist Opens → when last emitted, for the 60 s coalescing.
-    opened: Recent<OpenKey>,
+    opened: Recent<OpenKey, std::hash::RandomState>,
     /// Pending id → the handle it waits on, to forget it when it leaves.
-    waiter_fo: HashMap<PendingId, u64>,
+    waiter_fo: FastMap<PendingId, u64>,
     /// Cleanups whose outcome may follow: Irp → (FileObject, FileKey). The next
     /// operation on the Irp ends the wait (Irps are per thread and reused).
     cleanups: RecentMap<u64, (u64, u64)>,
@@ -134,18 +135,18 @@ enum Requester {
 impl Files {
     pub fn new(cap: usize, window_ticks: i64) -> Self {
         Files {
-            map: HashMap::new(),
+            map: FastMap::default(),
             cap,
             clock: 0,
             evictions: 0,
             window: window_ticks,
             held: VecDeque::new(),
             held_base: 0,
-            held_by_irp: HashMap::new(),
+            held_by_irp: FastMap::default(),
             failed: Recent::new(FAILED_CAP),
             confirmed: Recent::new(CONFIRMED_CAP),
-            opened: Recent::new(OPENED_CAP),
-            waiter_fo: HashMap::new(),
+            opened: Recent::new_in(OPENED_CAP), // keyed by path: SipHash (review 5)
+            waiter_fo: FastMap::default(),
             cleanups: RecentMap::new(CLEANUPS_CAP),
             requests: RecentMap::new(REQUESTS_CAP),
             early_outcomes: RecentMap::new(REQUESTS_CAP),
```

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -17,13 +17,12 @@ mod proc;
 mod reg;
 mod seed;
 
-use std::collections::{HashMap, HashSet};
-
 use atlas_schema::{Device, Event, EventId, EventKind, EventMeta, ProcessRef, ProcessUid, Sensor};
 
 use crate::completion::{Completion, Expired, PendingId, Reason, Wait};
 use crate::config::{Config, Ticks};
 use crate::counters::{Class, Counters};
+use crate::fast::{FastMap, FastSet};
 use crate::input::{Header, Incoming};
 use crate::keymap::KeyMap;
 use crate::ordering::{Ordering, Pushed};
@@ -63,8 +62,8 @@ pub struct Pipeline<L> {
     pub(crate) clock: Clock,
     pub(crate) id: Identity,
     pub(crate) ccs: u32,
-    pub(crate) self_keys: HashSet<u64>,
-    pub(crate) self_uids: HashSet<ProcessUid>,
+    pub(crate) self_keys: FastSet<u64>,
+    pub(crate) self_uids: FastSet<ProcessUid>,
     pub(crate) started: i64,
     pub(crate) lookups: L,
     pub(crate) ids: IdGen,
@@ -466,12 +465,12 @@ pub(crate) fn alive<E>(c: &Completion<E>, id: PendingId, reason: Reason) -> bool
 
 /// Request bookkeeping shared by the domain modules.
 pub(crate) struct Pending<T> {
-    pub(crate) by_id: HashMap<PendingId, T>,
+    pub(crate) by_id: FastMap<PendingId, T>,
 }
 
 impl<T> Default for Pending<T> {
     fn default() -> Self {
-        Pending { by_id: HashMap::new() }
+        Pending { by_id: FastMap::default() }
     }
 }
 
```

```diff
--- a/crates/atlas-agent/src/pipeline/proc.rs
+++ b/crates/atlas-agent/src/pipeline/proc.rs
@@ -2,8 +2,6 @@
 //! §5.2), Terminate, Module Load, the rundown that seeds the cache (§6.2), and
 //! enrichment results (§6.3).
 
-use std::collections::HashMap;
-
 use atlas_etw::parse::{ClassicKind, ClassicProcess, ImageLoad, ProcessStart, ProcessStop};
 use atlas_schema::classes::module::{ModuleAction, ModuleActivity};
 use atlas_schema::classes::process::ProcessActivity;
@@ -14,6 +12,7 @@ use super::expand::{Expansion, Slot};
 use super::{Pending, Pipeline, alive};
 use crate::completion::{PendingId, Reason};
 use crate::counters::{Class, Counters};
+use crate::fast::FastMap;
 use crate::input::Header;
 use crate::paths::{fit, has_short_name};
 use crate::process::{Identity, ProcInfo, ProcessCache, file_name, integrity, start_key};
@@ -24,9 +23,9 @@ use crate::time::filetime_to_unix_ns;
 #[derive(Default)]
 pub struct Launches {
     /// pid → (QPC, the pending Launch, start key): waiting for the classic half.
-    kernel: HashMap<u32, (i64, PendingId, u64)>,
+    kernel: FastMap<u32, (i64, PendingId, u64)>,
     /// pid → the classic half and its header: waiting for the Kernel-Process half.
-    classic: HashMap<u32, (i64, ClassicProcess, Header)>,
+    classic: FastMap<u32, (i64, ClassicProcess, Header)>,
     enrich: Pending<EnrichTarget>,
 }
 
```

```diff
--- a/crates/atlas-agent/src/pipeline/reg.rs
+++ b/crates/atlas-agent/src/pipeline/reg.rs
@@ -2,8 +2,6 @@
 //! the key map, the seeder for unknown handles, the `path_unresolved` floor,
 //! and value reads after the event.
 
-use std::collections::HashMap;
-
 use atlas_etw::parse::{RegDeleteValue, RegKey, RegOpen, RegSetValue, WStr};
 use atlas_schema::classes::registry::{
     RegType, RegistryKeyAction, RegistryKeyActivity, RegistryValueAction, RegistryValueActivity,
@@ -14,6 +12,7 @@ use atlas_schema::{Event, EventKind, ProcessRef};
 use super::{Pending, Pipeline, alive};
 use crate::completion::{PendingId, Reason};
 use crate::counters::Class;
+use crate::fast::FastMap;
 use crate::input::Header;
 use crate::keymap::Resolved;
 use crate::paths;
@@ -41,12 +40,12 @@ pub(crate) struct KeyWaiter {
 #[derive(Default)]
 pub struct Registry {
     /// Fast-path read results (§7.5), until their event is processed.
-    early: HashMap<EarlyKey, (ValueRead, Option<ValueData>)>,
+    early: FastMap<EarlyKey, (ValueRead, Option<ValueData>)>,
     reads: Pending<Expected>,
     /// Unnamed root address → events waiting for it.
-    pub(crate) waiting: HashMap<u64, Vec<KeyWaiter>>,
+    pub(crate) waiting: FastMap<u64, Vec<KeyWaiter>>,
     /// Pending id → the root it waits on, to forget it when it leaves.
-    waiter_root: HashMap<PendingId, u64>,
+    waiter_root: FastMap<PendingId, u64>,
 }
 
 impl Registry {
```

```diff
--- a/crates/atlas-agent/src/pipeline/seed.rs
+++ b/crates/atlas-agent/src/pipeline/seed.rs
@@ -10,14 +10,13 @@
 //! - addresses that cannot be named go into a negative cache until the address
 //!   is reused or its owner exits; events on them do not wait.
 
-use std::collections::{HashMap, HashSet};
-
 use atlas_schema::{EventKind, File};
 
 use super::Pipeline;
 use super::file::{Fill, fill_file_event, forgets_cached};
 use crate::completion::Reason;
 use crate::config::Ticks;
+use crate::fast::{FastMap, FastSet};
 use crate::process::file_name;
 use crate::recent::Recent;
 use crate::services::{HandleKind, Lookups, Request, Snapshot};
@@ -28,17 +27,17 @@ const CHANGES_CAP: usize = 1 << 19;
 const UNNAMABLE_CAP: usize = 1 << 16;
 
 pub struct Seeding {
-    asks: HashMap<HandleKind, HashSet<u64>>,
-    last_request: HashMap<HandleKind, i64>,
-    latest: HashMap<HandleKind, HashSet<u64>>,
+    asks: FastMap<HandleKind, FastSet<u64>>,
+    last_request: FastMap<HandleKind, i64>,
+    latest: FastMap<HandleKind, FastSet<u64>>,
     queued: Vec<Snapshot>,
     /// (kind, address) → (owner PID, insertion order). Owner 0: the address
     /// was not in the table at all.
-    unnamable: HashMap<(HandleKind, u64), (u32, u64)>,
+    unnamable: FastMap<(HandleKind, u64), (u32, u64)>,
     unnamable_seq: u64,
     changes: Recent<(HandleKind, u64)>,
     /// Kinds whose start-up pass has not been applied yet.
-    startup: HashSet<HandleKind>,
+    startup: FastSet<HandleKind>,
     /// On-miss seeding (§7.4). Off, no snapshot comes after the start-up
     /// passes, so the address history is kept only until they are applied
     /// (plan 1b-4a-p2, R3).
@@ -48,14 +47,14 @@ pub struct Seeding {
 impl Seeding {
     pub(super) fn new(on_miss: bool) -> Self {
         Seeding {
-            asks: HashMap::new(),
-            last_request: HashMap::new(),
-            latest: HashMap::new(),
+            asks: FastMap::default(),
+            last_request: FastMap::default(),
+            latest: FastMap::default(),
             queued: Vec::new(),
-            unnamable: HashMap::new(),
+            unnamable: FastMap::default(),
             unnamable_seq: 0,
             changes: Recent::new(CHANGES_CAP),
-            startup: HashSet::new(),
+            startup: FastSet::default(),
             on_miss,
         }
     }
@@ -171,10 +170,10 @@ impl<L: Lookups> Pipeline<L> {
     fn apply_snapshot(&mut self, s: Snapshot) {
         let kind = s.kind;
         let taken = s.taken;
-        let in_table: HashSet<u64> = s.named.iter().map(|n| n.address).chain(s.unnamable.iter().map(|u| u.0)).collect();
+        let in_table: FastSet<u64> = s.named.iter().map(|n| n.address).chain(s.unnamable.iter().map(|u| u.0)).collect();
         // Covered but not in the table: closed before the read. Nothing can name
         // the address until it is used again (the negative cache, owner 0).
-        let absent: HashSet<u64> = if s.asked.is_empty() {
+        let absent: FastSet<u64> = if s.asked.is_empty() {
             match kind {
                 HandleKind::Key => self.reg.waiting.keys().copied().filter(|r| !in_table.contains(r)).collect(),
                 HandleKind::File => self
```

- [ ] **Step 3: Measure, check and commit.** The benchmark (Task 2's command): 0.573 → 0.500 µs per event on the host.

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass; `Cargo.lock` gains one line, `atlas-agent`'s dependency on `foldhash` (already in it, through `hashbrown`). Workspace: 412 passed, 19 ignored (`atlas-agent` unit tests: 226).
```powershell
git add Cargo.toml Cargo.lock crates/atlas-agent
git commit -m "perf(agent): foldhash for the maps keyed by kernel addresses, PIDs and ids (D6 R3)"
```

### Task 5: The ordering stage's heap holds keys; events wait in reused slots

**Files:**
- Modify: `crates/atlas-agent/src/ordering.rs`, `crates/atlas-agent/src/pipeline/mod.rs`

- [ ] **Step 1: Keys and slots** (R3, F2: the ordering stage was 16% of the pipeline under load, most of it moving whole events while sifting). The heap holds `(timestamp, arrival, slot)`, 24 bytes; each event waits in a slot of a vector, and a released slot goes on a free list for the next push. Equal timestamps keep their arrival order, late arrivals still pass straight through. A new test checks a freed slot is reused; the existing ordering tests check the order.

```diff
--- a/crates/atlas-agent/src/ordering.rs
+++ b/crates/atlas-agent/src/ordering.rs
@@ -2,38 +2,25 @@
 //! `now − event time ≥ hold`, then releases events in timestamp order. An event
 //! older than the last one released is late: it passes straight through, out of
 //! order, and is counted. Nothing is ever dropped here.
+//!
+//! The heap holds keys only (timestamp, arrival order, slot), so sifting moves
+//! 24 bytes rather than a whole event; events wait in reusable slots and leave
+//! one at a time (plan 1b-4a-p2, R3).
 
 use std::cmp::Reverse;
 use std::collections::BinaryHeap;
 
 use crate::input::Incoming;
 
-struct Held {
-    ts: i64,
-    /// Arrival order, so equal timestamps keep their order.
-    seq: u64,
-    inc: Incoming,
-}
-
-impl PartialEq for Held {
-    fn eq(&self, o: &Self) -> bool {
-        (self.ts, self.seq) == (o.ts, o.seq)
-    }
-}
-impl Eq for Held {}
-impl PartialOrd for Held {
-    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
-        Some(self.cmp(o))
-    }
-}
-impl Ord for Held {
-    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
-        (self.ts, self.seq).cmp(&(o.ts, o.seq))
-    }
-}
+/// A held event's key: its timestamp, its arrival order (so equal timestamps
+/// keep their order) and the slot it waits in.
+type Key = (i64, u64, u32);
 
 pub struct Ordering {
-    heap: BinaryHeap<Reverse<Held>>,
+    heap: BinaryHeap<Reverse<Key>>,
+    /// The held events, by slot. A freed slot is reused by the next push.
+    slots: Vec<Option<Incoming>>,
+    free: Vec<u32>,
     seq: u64,
     /// The hold, in QPC ticks.
     hold: i64,
@@ -51,7 +38,14 @@ pub enum Pushed {
 
 impl Ordering {
     pub fn new(hold_ticks: i64) -> Self {
-        Ordering { heap: BinaryHeap::new(), seq: 0, hold: hold_ticks, last_released: i64::MIN }
+        Ordering {
+            heap: BinaryHeap::new(),
+            slots: Vec::new(),
+            free: Vec::new(),
+            seq: 0,
+            hold: hold_ticks,
+            last_released: i64::MIN,
+        }
     }
 
     pub fn push(&mut self, inc: Incoming) -> Pushed {
@@ -59,28 +53,49 @@ impl Ordering {
             return Pushed::Late(inc);
         }
         self.seq += 1;
-        self.heap.push(Reverse(Held { ts: inc.header.ts, seq: self.seq, inc }));
+        let ts = inc.header.ts;
+        let slot = match self.free.pop() {
+            Some(s) => {
+                self.slots[s as usize] = Some(inc);
+                s
+            }
+            None => {
+                self.slots.push(Some(inc));
+                (self.slots.len() - 1) as u32
+            }
+        };
+        self.heap.push(Reverse((ts, self.seq, slot)));
         Pushed::Held
     }
 
+    /// The next held event whose hold has passed at `now`, in timestamp order.
+    pub fn next_due(&mut self, now: i64) -> Option<Incoming> {
+        self.next_until(now.saturating_sub(self.hold))
+    }
+
+    /// The next held event of all, in order (a clean stop, §11.4).
+    pub fn next_held(&mut self) -> Option<Incoming> {
+        self.next_until(i64::MAX)
+    }
+
+    fn next_until(&mut self, limit: i64) -> Option<Incoming> {
+        let &Reverse((ts, _, slot)) = self.heap.peek().filter(|Reverse(k)| k.0 <= limit)?;
+        self.heap.pop();
+        self.last_released = self.last_released.max(ts);
+        self.free.push(slot);
+        Some(self.slots[slot as usize].take().expect("a held key has its event"))
+    }
+
     /// Every held event whose hold has passed at `now`, in timestamp order.
+    #[cfg(test)]
     pub fn release(&mut self, now: i64) -> Vec<Incoming> {
-        self.release_until(now.saturating_sub(self.hold))
+        std::iter::from_fn(|| self.next_due(now)).collect()
     }
 
-    /// Everything held, in order (a clean stop, §11.4).
+    /// Everything held, in order.
+    #[cfg(test)]
     pub fn drain(&mut self) -> Vec<Incoming> {
-        self.release_until(i64::MAX)
-    }
-
-    fn release_until(&mut self, limit: i64) -> Vec<Incoming> {
-        let mut out = Vec::new();
-        while self.heap.peek().is_some_and(|Reverse(h)| h.ts <= limit) {
-            let Reverse(h) = self.heap.pop().expect("peeked");
-            self.last_released = self.last_released.max(h.ts);
-            out.push(h.inc);
-        }
-        out
+        std::iter::from_fn(|| self.next_held()).collect()
     }
 
     /// Stream time (§3.2): `max(last released, now − hold)`. It advances while
@@ -139,6 +154,19 @@ mod tests {
         assert_eq!(o.push(inc(50, 3)), Pushed::Held); // equal is not late
     }
 
+    #[test]
+    fn freed_slots_are_reused() {
+        // Plan 1b-4a-p2, R3: the slots grow to the most events held at once.
+        let mut o = Ordering::new(100);
+        for round in 0..3 {
+            for i in 0..4 {
+                o.push(inc(round * 1000 + i, i as u64));
+            }
+            assert_eq!(o.release(round * 1000 + 200).len(), 4);
+        }
+        assert_eq!((o.slots.len(), o.free.len()), (4, 4));
+    }
+
     #[test]
     fn equal_timestamps_keep_arrival_order() {
         let mut o = Ordering::new(0);
```

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -157,7 +157,7 @@ impl<L: Lookups> Pipeline<L> {
     /// waits on stream time, and returns the events ready to emit, in order.
     pub fn tick(&mut self, now: i64) -> Vec<Event> {
         self.now = now;
-        for inc in self.ordering.release(now) {
+        while let Some(inc) = self.ordering.next_due(now) {
             // Stream time reaches each event before it is processed, so a
             // window that closed before it is applied first.
             self.step(inc.header.ts);
@@ -170,7 +170,7 @@ impl<L: Lookups> Pipeline<L> {
     /// A clean stop (§11.4): everything held is processed and everything
     /// pending goes out as is.
     pub fn stop(&mut self) -> Vec<Event> {
-        for inc in self.ordering.drain() {
+        while let Some(inc) = self.ordering.next_held() {
             self.step(inc.header.ts);
             self.process(inc);
         }
```

- [ ] **Step 2: Measure, check and commit.** The benchmark: 0.500 → 0.448 µs per event.

```powershell
cargo test -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 413 passed, 19 ignored (`atlas-agent` unit tests: 227).
```powershell
git add crates/atlas-agent
git commit -m "perf(agent): the ordering stage's heap holds keys, events wait in reused slots (D6 R3)"
```

### Task 6: A literal-run pre-check before the watchlist's GlobSet

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`, `crates/atlas-agent/Cargo.toml`, `crates/atlas-agent/src/watchlist.rs`

- [ ] **Step 1: The pre-check** (R3, F2: the watchlist's globs were 7% of the pipeline under load). A path can match a pattern only if it contains the pattern's last literal run (`\Login Data`, `\.ssh\`, `.kdbx`), so one Aho-Corasick scan for those runs, ASCII case folded, rules out almost every path before the `GlobSet`. A pattern with no literal run turns the pre-check off. Runs are taken with `/` read as `\` (globset treats both as separators; paths have `\`), and a class is read as globset reads it, a `]` right after `[`, `[!` or `[^` being a member; in doubt a character counts as wild, which only shortens a run (review 1). The tests pin that the pre-check never hides a match, of the default list and of a user's patterns written with `/` or a `]`-first class (the result equals the `GlobSet` alone, case and stream variants included), that the `GlobSet` also folds ASCII case only, and which run each pattern gives.

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,6 +9,7 @@ license = "AGPL-3.0-only"
 publish = false
 
 [workspace.dependencies]
+aho-corasick = "1.1.5"
 atlas-buffer = { path = "crates/atlas-buffer" }
 atlas-etw = { path = "crates/atlas-etw" }
 atlas-proto = { path = "crates/atlas-proto" }
```

```diff
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -92,6 +92,7 @@ checksum = "d3fb67a6e08acf24fdeccbac2cb6ac4305825bd1f117462e0e6f2f193345ad56"
 name = "atlas-agent"
 version = "0.1.0"
 dependencies = [
+ "aho-corasick",
  "atlas-buffer",
  "atlas-etw",
  "atlas-proto",
```

```diff
--- a/crates/atlas-agent/Cargo.toml
+++ b/crates/atlas-agent/Cargo.toml
@@ -8,6 +8,7 @@ license.workspace = true
 publish.workspace = true
 
 [dependencies]
+aho-corasick.workspace = true
 atlas-buffer.workspace = true
 atlas-etw.workspace = true
 atlas-proto.workspace = true
```

```diff
--- a/crates/atlas-agent/src/watchlist.rs
+++ b/crates/atlas-agent/src/watchlist.rs
@@ -5,7 +5,14 @@
 //! `GlobSet`. They are matched against the path with its volume and any
 //! alternate data stream removed, so they match shadow copies too. `*` stays
 //! within one component; `**` crosses components.
+//!
+//! A pre-check runs first (plan 1b-4a-p2, R3): a path can match a pattern only
+//! if it contains the pattern's last literal run (`\Login Data`, `\.ssh\`,
+//! `.kdbx`), so one scan for those runs rules out almost every path before the
+//! `GlobSet`. Both fold ASCII case only (a test pins the `GlobSet`'s side). A
+//! pattern with no literal run turns the pre-check off.
 
+use aho_corasick::AhoCorasick;
 use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
 
 use crate::paths::{strip_stream, volume_relative};
@@ -39,6 +46,8 @@ pub const DEFAULT: &[&str] = &[
 
 pub struct Watchlist {
     set: GlobSet,
+    /// Every pattern's last literal run; `None` turns the pre-check off.
+    must_contain: Option<AhoCorasick>,
 }
 
 /// A pattern that does not compile.
@@ -56,6 +65,14 @@ impl Watchlist {
             Some(r) => r.to_vec(),
             None => DEFAULT.iter().map(|s| s.to_string()).collect(),
         };
+        // globset reads `/` as `\` (both become `/` before matching); paths keep `\`.
+        let runs: Vec<String> =
+            base.iter().chain(extend).map(|p| last_literal_run(&p.replace('/', "\\")).to_string()).collect();
+        let must_contain = if runs.iter().all(|r| !r.is_empty()) {
+            AhoCorasick::builder().ascii_case_insensitive(true).build(&runs).ok()
+        } else {
+            None
+        };
         for p in base.iter().chain(extend) {
             let glob = GlobBuilder::new(&slashes(p))
                 .case_insensitive(true)
@@ -66,16 +83,71 @@ impl Watchlist {
             b.add(glob);
         }
         let set = b.build().map_err(|e| BadPattern { pattern: String::new(), error: e.to_string() })?;
-        Ok(Watchlist { set })
+        Ok(Watchlist { set, must_contain })
     }
 
     /// Whether an NT path (`\Device\HarddiskVolume3\…`) is on the list.
     pub fn matches(&self, nt_path: &str) -> bool {
         let p = strip_stream(volume_relative(nt_path));
-        !p.is_empty() && self.set.is_match(slashes(p))
+        if p.is_empty() {
+            return false;
+        }
+        if let Some(runs) = &self.must_contain
+            && !runs.is_match(p)
+        {
+            return false;
+        }
+        self.set.is_match(slashes(p))
     }
 }
 
+/// The last run of literal characters in a pattern: every path it matches
+/// contains that run. `*`, `?`, and whatever is inside `[…]` or `{…}` (a class
+/// or alternatives, none of them required) end a run. A class is read as
+/// globset reads it: a `]` right after `[`, `[!` or `[^` is a member. When in
+/// doubt a character counts as wild, which only shortens the run (review 1).
+fn last_literal_run(pattern: &str) -> &str {
+    let (mut best, mut start, mut braces) = ("", None::<usize>, 0u32);
+    // Inside `[…]`: where its members start.
+    let mut class: Option<usize> = None;
+    for (i, c) in pattern.char_indices() {
+        let wild = match (class, c) {
+            (Some(m), '!' | '^') if i == m => {
+                class = Some(i + 1);
+                true
+            }
+            (Some(m), ']') if i > m => {
+                class = None;
+                true
+            }
+            (Some(_), _) => true,
+            (None, '[') => {
+                class = Some(i + 1);
+                true
+            }
+            (None, '{') => {
+                braces += 1;
+                true
+            }
+            (None, '}') => {
+                braces = braces.saturating_sub(1);
+                true
+            }
+            (None, ']' | '*' | '?') => true,
+            _ => braces > 0,
+        };
+        match (wild, start) {
+            (true, Some(s)) => {
+                best = &pattern[s..i];
+                start = None;
+            }
+            (false, None) => start = Some(i),
+            _ => {}
+        }
+    }
+    start.map_or(best, |s| &pattern[s..])
+}
+
 /// globset separates on `/`; Windows paths use `\`.
 fn slashes(s: &str) -> String {
     s.replace('\\', "/")
@@ -122,6 +194,97 @@ mod tests {
         }
     }
 
+    /// What the `GlobSet` alone says, without the pre-check.
+    fn glob_only(w: &Watchlist, nt_path: &str) -> bool {
+        let p = strip_stream(volume_relative(nt_path));
+        !p.is_empty() && w.set.is_match(slashes(p))
+    }
+
+    #[test]
+    fn the_last_literal_runs() {
+        assert_eq!(last_literal_run(r"\Users\*\AppData\Local\**\User Data\*\Login Data"), r"\Login Data");
+        assert_eq!(last_literal_run(r"\Users\*\.ssh\*"), r"\.ssh\");
+        assert_eq!(last_literal_run(r"**\*.kdbx"), ".kdbx");
+        assert_eq!(last_literal_run(r"\Windows\NTDS\ntds.dit"), r"\Windows\NTDS\ntds.dit");
+        assert_eq!(last_literal_run(r"\a\{x,yz}\b"), r"\b");
+        assert_eq!(last_literal_run(r"\a\[xy]"), r"\a\");
+        assert_eq!(last_literal_run(r"\a\{x,yz}"), r"\a\");
+        assert_eq!(last_literal_run("**"), "");
+        // A `]` first in a class is a member (review 1).
+        assert_eq!(last_literal_run(r"\x[]a]"), r"\x");
+        assert_eq!(last_literal_run(r"\x[!]a]b"), "b");
+        assert_eq!(last_literal_run(r"\x[^]]\y"), r"\y");
+    }
+
+    #[test]
+    fn the_pre_check_never_hides_a_match_of_a_users_pattern() {
+        // Review 1: `/` as a separator, and a class that starts with `]`.
+        let user = ["/Users/*/secret.txt", "**/*.KEY", r"\x[]a]", r"\q[!]a]z"].map(String::from);
+        let w = Watchlist::new(Some(&user), &[]).unwrap();
+        assert!(w.must_contain.is_some());
+        for p in [
+            r"\Device\HarddiskVolume3\Users\jake\secret.txt",
+            r"\Device\HarddiskVolume3\a\b.key",
+            r"\Device\HarddiskVolume3\x]",
+            r"\Device\HarddiskVolume3\xa",
+            r"\Device\HarddiskVolume3\qbz",
+        ] {
+            assert!(glob_only(&w, p), "the GlobSet itself matches {p}");
+            assert!(w.matches(p), "{p}");
+        }
+        for p in [r"\Device\HarddiskVolume3\Users\jake\other.txt", r"\Device\HarddiskVolume3\q]z"] {
+            assert_eq!(w.matches(p), glob_only(&w, p), "{p}");
+        }
+    }
+
+    #[test]
+    fn the_pre_check_never_hides_a_match() {
+        let w = wl();
+        assert!(w.must_contain.is_some(), "the default list has a literal run in every pattern");
+        for p in [
+            r"\Device\HarddiskVolume3\Windows\System32\config\SAM",
+            r"\Device\HarddiskVolume3\windows\system32\CONFIG\sam",
+            r"\Device\HarddiskVolume3\Windows\System32\config\RegBack\SAM",
+            r"\Device\HarddiskVolume3\Windows\System32\config\SAM.sav",
+            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Default\Login Data",
+            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\a\b\User Data\Default\LOGIN DATA",
+            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Default\Login Data-journal",
+            r"\Device\HarddiskVolume3\Users\jake\.SSH\id_rsa",
+            r"\Device\HarddiskVolume3\Users\jake\.ssh",
+            r"\Device\HarddiskVolume3\Users\jake\.azure\x\y\z",
+            r"\Device\HarddiskVolume3\Users\jake\Documents\a.kdbx",
+            r"\Device\HarddiskVolume3\a.KdBx",
+            r"\Device\HarddiskVolume3\Windows\System32\kernel32.dll",
+            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Temp\x.tmp",
+            r"\Device\HarddiskVolume3\Windows\System32\config\SAM::$DATA",
+        ] {
+            assert_eq!(w.matches(p), glob_only(&w, p), "{p}");
+        }
+    }
+
+    #[test]
+    fn the_glob_set_folds_ascii_case_only_as_the_pre_check_does() {
+        // If a globset upgrade folds more (the Kelvin sign to `k`, `Ä` to `ä`),
+        // the ASCII-only scan would hide matches: this fails first.
+        let w = Watchlist::new(Some(&["**\\*.\u{e4}b".to_string(), r"**\*.kdbx".to_string()]), &[]).unwrap();
+        assert!(glob_only(&w, r"\Device\HarddiskVolume3\x.KDBX"));
+        assert!(!glob_only(&w, "\\Device\\HarddiskVolume3\\x.\u{212A}dbx"));
+        assert!(!glob_only(&w, "\\Device\\HarddiskVolume3\\x.\u{c4}B"));
+        for p in [
+            "\\Device\\HarddiskVolume3\\x.\u{e4}B",
+            "\\Device\\HarddiskVolume3\\Users\\j\u{e9}r\u{f4}me\\Documents\\a.KDBX",
+        ] {
+            assert!(w.matches(p) && glob_only(&w, p), "{p}");
+        }
+    }
+
+    #[test]
+    fn a_pattern_without_a_literal_run_turns_the_pre_check_off() {
+        let w = Watchlist::new(None, &["**".to_string()]).unwrap();
+        assert!(w.must_contain.is_none());
+        assert!(w.matches(r"\Device\HarddiskVolume3\anything\at\all"));
+    }
+
     #[test]
     fn replace_and_extend() {
         let w = Watchlist::new(Some(&[r"\secret\*".to_string()]), &[r"**\*.pem".to_string()]).unwrap();
```

- [ ] **Step 2: Measure, check and commit.** A typical non-matching path: 220 → 95 ns.

```powershell
cargo test -p atlas-agent --lib
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass; `Cargo.lock` gains one line, `atlas-agent`'s dependency on `aho-corasick` (already in it, through `globset`). Workspace: 418 passed, 19 ignored (`atlas-agent` unit tests: 232).
```powershell
git add Cargo.toml Cargo.lock crates/atlas-agent
git commit -m "perf(agent): a literal-run pre-check before the watchlist's GlobSet (D6 R3)"
```

### Task 7: WStr::to_string_lossy narrows an all-ASCII name in one allocation

**Files:**
- Modify: `crates/atlas-etw/src/parse/mod.rs`, `crates/atlas-etw/src/parse/tests.rs`

- [ ] **Step 1: The tests.** Two property tests: on any UTF-16 units (ASCII, other BMP characters, pairs, lone surrogates), and on Latin-1 alone (the fast path's edge), `to_string_lossy` equals `String::from_utf16_lossy`; and fixed examples.

```diff
--- a/crates/atlas-etw/src/parse/tests.rs
+++ b/crates/atlas-etw/src/parse/tests.rs
@@ -86,6 +86,36 @@ fn payload_for(fields: &[layout::Field], p: PointerSize) -> Vec<u8> {
     b.0
 }
 
+proptest! {
+    /// The ASCII fast path and the pre-sized decode give std's answer for any
+    /// units: ASCII, other BMP characters, pairs, lone surrogates (plan 1b-4a-p2).
+    #[test]
+    fn to_string_lossy_is_from_utf16_lossy(
+        units in prop::collection::vec(
+            prop_oneof![0u16..0x80, 0x80u16..0xD800, 0xD800u16..0xE000, 0xE000u16..=0xFFFF],
+            0..64,
+        )
+    ) {
+        prop_assert_eq!(WStr::from_units(&units).to_string_lossy(), String::from_utf16_lossy(&units));
+    }
+
+    /// ASCII and Latin-1 only: the edge of the fast path.
+    #[test]
+    fn to_string_lossy_of_latin_1_is_from_utf16_lossy(units in prop::collection::vec(0u16..0x100, 0..64)) {
+        prop_assert_eq!(WStr::from_units(&units).to_string_lossy(), String::from_utf16_lossy(&units));
+    }
+}
+
+#[test]
+fn to_string_lossy_examples() {
+    let s = |u: &[u16]| WStr::from_units(u).to_string_lossy();
+    assert_eq!(s(&[0x63, 0x3A, 0x5C]), r"c:\");
+    assert_eq!(s(&[0x63, 0xE9]), "c\u{e9}");
+    assert_eq!(s(&[0x41, 0xD800, 0x42]), "A\u{fffd}B");
+    assert_eq!(s(&[0xD83D, 0xDE00]), "\u{1F600}");
+    assert_eq!(s(&[]), "");
+}
+
 #[test]
 fn every_layout_parses_and_every_strict_prefix_fails() {
     for l in LAYOUTS {
```

- [ ] **Step 2: The fast path** (R3). If every unit is below 0x80, the name is narrowed into one pre-sized `String`; otherwise the old decode.

```diff
--- a/crates/atlas-etw/src/parse/mod.rs
+++ b/crates/atlas-etw/src/parse/mod.rs
@@ -94,8 +94,17 @@ impl WStr {
         self.0.is_empty()
     }
 
+    /// As `String::from_utf16_lossy`, faster for the usual all-ASCII name
+    /// (plan 1b-4a-p2, R3): narrowed in one allocation, not decoded a char at
+    /// a time into a growing string.
     pub fn to_string_lossy(&self) -> String {
-        String::from_utf16_lossy(&self.0)
+        if self.0.iter().all(|&u| u < 0x80) {
+            let bytes: Vec<u8> = self.0.iter().map(|&u| u as u8).collect();
+            return String::from_utf8(bytes).expect("ASCII is UTF-8");
+        }
+        let mut s = String::with_capacity(self.0.len());
+        s.extend(char::decode_utf16(self.0.iter().copied()).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)));
+        s
     }
 }
 
```

- [ ] **Step 3: Measure, check and commit.** The benchmark: 0.437 → 0.385 µs per event.

```powershell
cargo test -p atlas-etw
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p atlas-agent -p atlas-etw --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```
Expected: all pass. Workspace: 421 passed, 19 ignored (`atlas-agent` unit tests: 232).
```powershell
git add crates/atlas-etw
git commit -m "perf(etw): WStr::to_string_lossy narrows an all-ASCII name in one allocation (D6 R3)"
```

### Task 8: After a pass that found no event, the loop waits 40 ms

**Files:**
- Modify: `crates/atlas-agent/src/driver.rs`, `crates/atlas-agent/tests/replay.rs`

- [ ] **Step 1: The cadences** (R3, F2: on a quiet machine ETW delivers once a flush period, 250 ms, so most 10 ms passes found nothing, and each sleep cost 18 µs). `DriverConfig` gains `idle_cadence` (40 ms); `drain` returns how many events it pushed, and `next_wait` gives `cadence` after a pass that pushed some, `idle_cadence` after one that pushed none. `tick` still comes at least every 50 ms. The tests: `next_wait` itself, `drain`'s count, and an idle loop's pass count over 400 ms (about 10 at 40 ms; 25–40 at 10 ms). A service reply that comes after an empty pass is applied up to 40 ms later; its deadline is unchanged, since replies are fed in before `tick` in the same pass.

```diff
--- a/crates/atlas-agent/src/driver.rs
+++ b/crates/atlas-agent/src/driver.rs
@@ -1,6 +1,8 @@
 //! The pipeline thread's loop (sensor spec §3.2; plan 1b-3c, decision D1).
 //!
-//! Every `cadence` (10 ms) one pass:
+//! One pass every `cadence` (10 ms), or `idle_cadence` (40 ms) after a pass that
+//! found no event (plan 1b-4a-p2, R3: on a quiet machine ETW delivers once a
+//! flush period, 250 ms, so most 10 ms passes found nothing):
 //! 1. drains the kernel and user-mode queues into [`Pipeline::push`], at most
 //!    a queue's capacity each, so a flood cannot starve `tick` (review R-m8);
 //! 2. feeds the services' replies into [`Pipeline::reply`], and runs any
@@ -41,6 +43,8 @@ pub trait Lanes {
 pub struct DriverConfig {
     /// One pass every this long; `tick` must come at least every 50 ms.
     pub cadence: Duration,
+    /// After a pass that found no event, the next comes this long after it.
+    pub idle_cadence: Duration,
     /// The anchor pair is re-taken this often (§3.3).
     pub anchor_every: Duration,
     /// Queue capacities (§3.2): kernel providers and Session B, and the
@@ -53,6 +57,7 @@ impl Default for DriverConfig {
     fn default() -> Self {
         DriverConfig {
             cadence: Duration::from_millis(10),
+            idle_cadence: Duration::from_millis(40),
             anchor_every: Duration::from_secs(60),
             kernel_queue_cap: 65_536,
             user_queue_cap: 8_192,
@@ -107,7 +112,7 @@ impl<L: Lookups, S: Lanes> Driver<L, S> {
         let mut next_anchor = Instant::now() + self.cfg.anchor_every;
         loop {
             let started = Instant::now();
-            let open = self.pass(&mut sink);
+            let (open, pushed) = self.pass(&mut sink);
             if !open {
                 break;
             }
@@ -115,7 +120,7 @@ impl<L: Lookups, S: Lanes> Driver<L, S> {
                 self.pipeline.set_anchor((self.anchor)());
                 next_anchor = started + self.cfg.anchor_every;
             }
-            std::thread::sleep(self.cfg.cadence.saturating_sub(started.elapsed()));
+            std::thread::sleep(next_wait(&self.cfg, pushed).saturating_sub(started.elapsed()));
         }
         for r in self.lanes.replies() {
             self.pipeline.reply(r);
@@ -126,10 +131,10 @@ impl<L: Lookups, S: Lanes> Driver<L, S> {
         self.pipeline.counters()
     }
 
-    /// One pass. Returns whether a queue is still open.
-    fn pass(&mut self, sink: &mut impl FnMut(Event)) -> bool {
-        let kernel = drain(&self.kernel, &mut self.pipeline, self.cfg.kernel_queue_cap);
-        let user = drain(&self.user, &mut self.pipeline, self.cfg.user_queue_cap);
+    /// One pass. Returns whether a queue is still open, and the events it pushed.
+    fn pass(&mut self, sink: &mut impl FnMut(Event)) -> (bool, usize) {
+        let (kernel, k) = drain(&self.kernel, &mut self.pipeline, self.cfg.kernel_queue_cap);
+        let (user, u) = drain(&self.user, &mut self.pipeline, self.cfg.user_queue_cap);
         for r in self.lanes.replies() {
             self.pipeline.reply(r);
         }
@@ -143,25 +148,36 @@ impl<L: Lookups, S: Lanes> Driver<L, S> {
         for e in out {
             sink(e);
         }
-        kernel || user
+        (kernel || user, k + u)
     }
 }
 
+/// How long after a pass's start the next one comes: `cadence` when the pass
+/// pushed events, `idle_cadence` when it found none.
+fn next_wait(cfg: &DriverConfig, pushed: usize) -> Duration {
+    if pushed > 0 { cfg.cadence } else { cfg.idle_cadence }
+}
+
 /// Pushes what is queued, at most `max` events. Returns whether the queue is
-/// still open.
-fn drain<L: Lookups>(rx: &Receiver<Incoming>, p: &mut Pipeline<L>, max: usize) -> bool {
+/// still open, and how many it pushed.
+fn drain<L: Lookups>(rx: &Receiver<Incoming>, p: &mut Pipeline<L>, max: usize) -> (bool, usize) {
+    let mut n = 0;
     for _ in 0..max.max(1) {
         match rx.try_recv() {
-            Ok(inc) => p.push(inc),
-            Err(TryRecvError::Empty) => return true,
-            Err(TryRecvError::Disconnected) => return false,
+            Ok(inc) => {
+                p.push(inc);
+                n += 1;
+            }
+            Err(TryRecvError::Empty) => return (true, n),
+            Err(TryRecvError::Disconnected) => return (false, n),
         }
     }
-    true
+    (true, n)
 }
 
 #[cfg(test)]
 mod tests {
+    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
     use std::sync::mpsc::{SyncSender, channel, sync_channel};
     use std::sync::{Arc, Mutex};
 
@@ -192,6 +208,16 @@ mod tests {
 
     fn start(cfg: DriverConfig, anchor: AnchorSource) -> Running {
         let t0 = Instant::now();
+        let p = pipeline();
+        let (ktx, krx) = sync_channel(cfg.kernel_queue_cap);
+        let (utx, urx) = sync_channel(cfg.user_queue_cap);
+        let (etx, events) = channel();
+        let driver = Driver::new(p, FakeLanes::new(|_| None), krx, urx, Box::new(move || qpc(t0)), anchor, cfg);
+        let thread = std::thread::spawn(move || driver.run(|e| etx.send(e).unwrap()));
+        Running { kernel: Some(ktx), user: Some(utx), events, thread, t0 }
+    }
+
+    fn pipeline() -> Pipeline<FakeLookups> {
         let setup = Setup {
             config: Config::default(),
             ticks: Ticks::new(FREQ),
@@ -205,13 +231,54 @@ mod tests {
             self_keys: vec![],
             started: 0,
         };
-        let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
-        let (ktx, krx) = sync_channel(cfg.kernel_queue_cap);
-        let (utx, urx) = sync_channel(cfg.user_queue_cap);
-        let (etx, events) = channel();
-        let driver = Driver::new(p, FakeLanes::new(|_| None), krx, urx, Box::new(move || qpc(t0)), anchor, cfg);
-        let thread = std::thread::spawn(move || driver.run(|e| etx.send(e).unwrap()));
-        Running { kernel: Some(ktx), user: Some(utx), events, thread, t0 }
+        Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap()
+    }
+
+    #[test]
+    fn the_next_pass_waits_longer_after_finding_nothing() {
+        let cfg = DriverConfig::default();
+        assert_eq!((cfg.cadence, cfg.idle_cadence), (Duration::from_millis(10), Duration::from_millis(40)));
+        assert_eq!(next_wait(&cfg, 0), cfg.idle_cadence);
+        assert_eq!(next_wait(&cfg, 1), cfg.cadence);
+    }
+
+    #[test]
+    fn drain_counts_what_it_pushed() {
+        let mut p = pipeline();
+        let (tx, rx) = sync_channel(8);
+        for i in 0..3 {
+            tx.send(created(i, r"\Device\HarddiskVolume3\a.txt")).unwrap();
+        }
+        assert_eq!(drain(&rx, &mut p, 2), (true, 2));
+        assert_eq!(drain(&rx, &mut p, 8), (true, 1));
+        assert_eq!(drain(&rx, &mut p, 8), (true, 0));
+        drop(tx);
+        assert_eq!(drain(&rx, &mut p, 8), (false, 0));
+    }
+
+    #[test]
+    fn an_idle_loop_passes_at_the_idle_cadence() {
+        // Plan 1b-4a-p2, R3: with nothing queued, about one pass per 40 ms, not
+        // per 10 ms. Only an upper bound: a loaded runner makes fewer passes.
+        let passes = Arc::new(AtomicUsize::new(0));
+        let n = passes.clone();
+        let t0 = Instant::now();
+        let clock = Box::new(move || {
+            n.fetch_add(1, AtomicOrdering::Relaxed);
+            qpc(t0)
+        });
+        let (ktx, krx) = sync_channel(1);
+        let (utx, urx) = sync_channel(1);
+        let driver =
+            Driver::new(pipeline(), FakeLanes::new(|_| None), krx, urx, clock, no_anchor(), DriverConfig::default());
+        let thread = std::thread::spawn(move || driver.run(|_| {}));
+        std::thread::sleep(Duration::from_millis(400));
+        let seen = passes.load(AtomicOrdering::Relaxed);
+        drop((ktx, utx));
+        thread.join().unwrap();
+        // 400 ms: about 10 passes at 40 ms; 25 to 40 at 10 ms (std's sleep
+        // uses a high-resolution timer, so the 15.6 ms tick does not round it).
+        assert!(seen <= 18, "{seen} passes in 400 ms");
     }
 
     /// A file created by the Idle process (a built-in actor), at QPC `ts`.
@@ -328,7 +395,7 @@ mod tests {
         let (ktx, krx) = sync_channel(4);
         let (utx, urx) = sync_channel(4);
         drop((ktx, utx));
-        let cfg = DriverConfig { cadence: Duration::ZERO, ..DriverConfig::default() };
+        let cfg = DriverConfig { cadence: Duration::ZERO, idle_cadence: Duration::ZERO, ..DriverConfig::default() };
         Driver::new(p, Counting(s), krx, urx, Box::new(|| 0), no_anchor(), cfg).run(|_| {});
         // The start-up seeding pass (`seed_on_start`) is asked for on the first pass.
         assert_eq!(*submitted.lock().unwrap(), 2, "a Seed per handle kind");
```

```diff
--- a/crates/atlas-agent/tests/replay.rs
+++ b/crates/atlas-agent/tests/replay.rs
@@ -163,7 +163,7 @@ fn run_driven() -> Vec<Event> {
         now
     });
     let anchor = Box::new(|| panic!("no anchor refresh within a replay"));
-    let cfg = DriverConfig { cadence: Duration::ZERO, ..DriverConfig::default() };
+    let cfg = DriverConfig { cadence: Duration::ZERO, idle_cadence: Duration::ZERO, ..DriverConfig::default() };
     let driver = Driver::new(p, FakeLanes::new(answer), krx, urx, clock, anchor, cfg);
     let mut out = Vec::new();
     driver.run(|e| out.push(e));
```

- [ ] **Step 2: Check and commit.** An idle loop wakes about 25 times a second instead of 100.

```powershell
cargo test -p atlas-agent
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 424 passed, 19 ignored (`atlas-agent` unit tests: 235).
```powershell
git add crates/atlas-agent
git commit -m "perf(agent): after a pass that found no event, the loop waits 40 ms (D6 R3)"
```

### Task 9: Cache invalidations wake no thread

**Files:**
- Modify: `crates/atlas-agent/src/win/services.rs`

- [ ] **Step 1: Invalidations that wake no thread** (R3a, F3). `Services::submit` handles `InvalidateHash` without a lane:
  - **the hash cache's:** the path goes on `Forgets`, a channel the hash workers drain and apply before their next lookup (review 4: the pipeline thread runs at normal priority and the workers below it, and Windows' user-mode locks have no priority inheritance, so the pipeline thread takes no lock a worker could be preempted holding). The cache's key holds the USN, so these only free memory: past the lane's capacity waiting, the newest is dropped;
  - **the 8.3 cache's:** the path joins `Invalidations`, a shared list the expander (normal priority) takes and applies before its next expansion. A list at the lane's capacity is cleared and marked lost, and the expander then clears its whole cache, as a dropped invalidation did; once lost, it keeps nothing more until then (review 3). Invalidations are thus applied before Expand jobs queued ahead of them: earlier, never later, which only makes the cache more conservative;
  - the pipeline thread's lock calls tolerate a lock poisoned by a panicked worker (review 9).

  The tests: a hash invalidation reaches the cache before the next lookup, not at the call; the expander applies a waiting invalidation before the next job's lookup; the invalidation that overflows the list clears the expansion cache (review 2: the one that matters overflows, so dropping the newest would fail it); past the cap neither list keeps anything (replacing the old lost-invalidation test).

```diff
--- a/crates/atlas-agent/src/win/services.rs
+++ b/crates/atlas-agent/src/win/services.rs
@@ -1,25 +1,32 @@
 //! The services' threads and the routing of [`Request`]s to them (sensor spec
 //! §3.2 [4] and [8]; plan 1b-3b, D5).
 //!
-//! - **Hash workers** (2, below normal): `Enrich`, and the hash cache's
-//!   invalidations, so the pipeline thread never takes the cache's lock.
+//! - **Hash workers** (2, below normal): `Enrich`.
 //! - **Reader lane** (1): value reads, on the ordered path and the fast path.
 //!   It does no file I/O, so a slow directory never delays a value read (R-M2).
-//! - **Expander lane** (1): 8.3 expansion, and the expansion cache's
-//!   invalidations in arrival order (D4).
+//! - **Expander lane** (1): 8.3 expansion (D4).
 //! - **Seeder** (1, below normal): `Seed`, when `SeDebugPrivilege` is available.
 //!
 //! Lanes are bounded and never block the caller. A full lane drops the request
 //! and counts it: a dropped `Enrich`, `ReadValue`, `Expand` or `Seed` costs its
-//! event's deadline (clarification 25). A dropped invalidation of the
-//! expansion cache sets a flag, and the expander clears its whole cache before
-//! its next job (R-M3). The hash cache needs no such care: its key holds the USN.
-//! Replies come back on one channel, which the driver (plan 1b-3c) feeds to
-//! `Pipeline::reply`.
-
-use std::sync::atomic::{AtomicBool, Ordering};
+//! event's deadline (clarification 25).
+//!
+//! **Invalidations wake no thread** (plan 1b-4a-p2, R3a): handing a job to a
+//! parked worker costs about 22 µs of CPU, and every file Update, Rename and
+//! Delete invalidates both caches. The hash cache's invalidations go on a
+//! channel the workers drain before their next lookup: the caller (the
+//! pipeline thread) sends without taking a lock a below-normal worker could
+//! hold (review 4). Its key holds the USN, so they only free memory, and past
+//! the lane's capacity the newest is dropped. The expansion cache's wait in a
+//! list the expander applies before its next expansion, so none is applied
+//! late; if the list fills first, the expander clears its whole cache instead
+//! (R-M3), and the list keeps nothing more until then (review 3). Neither is a
+//! counted drop. Replies come back on one channel,
+//! which the driver (plan 1b-3c) feeds to `Pipeline::reply`.
+
+use std::sync::atomic::{AtomicUsize, Ordering};
 use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
-use std::sync::{Arc, Mutex};
+use std::sync::{Arc, Mutex, PoisonError};
 use std::time::Instant;
 
 use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
@@ -34,27 +41,60 @@ use crate::counters::ServiceCounters;
 use crate::intake::FastRead;
 use crate::services::{EarlyKey, HandleKind, Reply, Request, ValueRead};
 
-enum HashJob {
-    Enrich(PendingId, String),
-    Invalidate(String),
-}
+/// An `Enrich`: the event waiting for it, and the file's NT path.
+type HashJob = (PendingId, String);
 
 enum ReaderJob {
     Read(PendingId, ValueRead),
     Early(EarlyKey, ValueRead),
 }
 
-enum ExpandJob {
-    Expand(PendingId, u8, String),
-    Invalidate(String),
+/// An `Expand`: the event, its path slot, and the NT path.
+type ExpandJob = (PendingId, u8, String);
+
+/// The hash cache's invalidations, waiting for a worker's next lookup.
+struct Forgets {
+    /// Shared by the workers only.
+    rx: Mutex<Receiver<String>>,
+    /// Sent and not yet applied.
+    waiting: AtomicUsize,
+}
+
+impl Forgets {
+    /// Applies every waiting invalidation to `cache`.
+    fn apply(&self, cache: &Mutex<HashCache>) {
+        let paths: Vec<String> = self.rx.lock().expect("hash invalidations").try_iter().collect();
+        if paths.is_empty() {
+            return;
+        }
+        self.waiting.fetch_sub(paths.len(), Ordering::Relaxed);
+        let mut c = cache.lock().expect("hash cache");
+        for p in &paths {
+            c.invalidate(p);
+        }
+    }
+}
+
+/// The expansion cache's invalidations, waiting for the expander's next job.
+#[derive(Default)]
+struct Invalidations {
+    paths: Vec<String>,
+    /// Too many waited: the expander clears its whole cache instead.
+    lost: bool,
 }
 
 pub struct Services {
     hash: SyncSender<HashJob>,
     reader: SyncSender<ReaderJob>,
     expander: SyncSender<ExpandJob>,
-    /// Set when an expansion-cache invalidation was dropped (R-M3).
-    invalidation_lost: Arc<AtomicBool>,
+    /// The workers' cache, for the tests to look into.
+    #[cfg(test)]
+    cache: Arc<Mutex<HashCache>>,
+    forget: Sender<String>,
+    forgets: Arc<Forgets>,
+    invalidations: Arc<Mutex<Invalidations>>,
+    /// The most invalidations that wait for either cache (the lane's capacity).
+    invalidations_cap: usize,
     seeder: Option<SyncSender<(HandleKind, Vec<u64>)>>,
     replies: Receiver<Reply>,
     counters: Arc<ServiceCounters>,
@@ -87,12 +127,15 @@ impl Services {
         let cache = Arc::new(Mutex::new(HashCache::new(cfg.hash_cache_cap)));
         let (hash, hash_rx) = sync_channel::<HashJob>(cfg.lane_cap);
         let hash_rx = Arc::new(Mutex::new(hash_rx));
+        let (forget, forget_rx) = channel();
+        let forgets = Arc::new(Forgets { rx: Mutex::new(forget_rx), waiting: AtomicUsize::new(0) });
         for i in 0..cfg.hash_workers.max(1) {
             let (rx, tx, cache, counters, cap) =
                 (hash_rx.clone(), reply_tx.clone(), cache.clone(), counters.clone(), cfg.hash_size_cap);
+            let forgets = forgets.clone();
             std::thread::Builder::new()
                 .name(format!("atlas-hash-{i}"))
-                .spawn(move || hash_worker(&rx, &tx, &cache, &counters, cap))
+                .spawn(move || hash_worker(&rx, &tx, &cache, &forgets, &counters, cap))
                 .expect("spawn a hash worker");
         }
 
@@ -104,12 +147,12 @@ impl Services {
             .spawn(move || reader_lane(reader_rx, &tx, backup))
             .expect("spawn the reader lane");
 
-        let invalidation_lost = Arc::new(AtomicBool::new(false));
+        let invalidations = Arc::new(Mutex::new(Invalidations::default()));
         let (expander, expand_rx) = sync_channel::<ExpandJob>(cfg.lane_cap);
-        let (tx, lost) = (reply_tx.clone(), invalidation_lost.clone());
+        let (tx, pending) = (reply_tx.clone(), invalidations.clone());
         std::thread::Builder::new()
             .name("atlas-expander".into())
-            .spawn(move || expander_lane(expand_rx, &tx, Expander::new(dir), &lost))
+            .spawn(move || expander_lane(expand_rx, &tx, Expander::new(dir), &pending))
             .expect("spawn the expander lane");
 
         let seeder = Seeder::new(cfg, counters.clone()).map(|s| {
@@ -122,7 +165,20 @@ impl Services {
             seed_tx
         });
 
-        Services { hash, reader, expander, invalidation_lost, seeder, replies, counters }
+        Services {
+            hash,
+            reader,
+            expander,
+            #[cfg(test)]
+            cache,
+            forget,
+            forgets,
+            invalidations,
+            invalidations_cap: cfg.lane_cap,
+            seeder,
+            replies,
+            counters,
+        }
     }
 
     /// Whether the seeder runs. If not, the driver sets `Config::seed_on_start`
@@ -140,21 +196,31 @@ impl Services {
         self.replies.try_iter()
     }
 
-    /// Hands a request to its lane. Never blocks, and takes no lock.
+    /// Hands a request to its lane. Never blocks; an invalidation takes one
+    /// short lock, the expander's list's, and wakes no thread (R3a).
     pub fn submit(&self, r: Request) {
         match r {
-            Request::Enrich { id, nt_path, .. } => self.count(self.hash.try_send(HashJob::Enrich(id, nt_path))),
+            Request::Enrich { id, nt_path, .. } => self.count(self.hash.try_send((id, nt_path))),
             Request::InvalidateHash { nt_path } => {
-                self.count(self.hash.try_send(HashJob::Invalidate(nt_path.clone())));
-                if self.expander.try_send(ExpandJob::Invalidate(nt_path)).is_err() {
-                    self.invalidation_lost.store(true, Ordering::Release);
-                    self.counters.service_queue_drops.fetch_add(1, Ordering::Relaxed);
+                if self.forgets.waiting.fetch_add(1, Ordering::Relaxed) < self.invalidations_cap {
+                    let _ = self.forget.send(nt_path.clone());
+                } else {
+                    self.forgets.waiting.fetch_sub(1, Ordering::Relaxed);
+                }
+                // A worker or the expander that panicked holding a lock must
+                // not take the pipeline thread with it (review 9).
+                let mut q = self.invalidations.lock().unwrap_or_else(PoisonError::into_inner);
+                if q.lost {
+                    // The whole cache is cleared at the next job anyway.
+                } else if q.paths.len() >= self.invalidations_cap {
+                    q.paths.clear();
+                    q.lost = true;
+                } else {
+                    q.paths.push(nt_path);
                 }
             }
             Request::ReadValue { id, read } => self.count(self.reader.try_send(ReaderJob::Read(id, read))),
-            Request::Expand { id, slot, nt_path } => {
-                self.count(self.expander.try_send(ExpandJob::Expand(id, slot, nt_path)))
-            }
+            Request::Expand { id, slot, nt_path } => self.count(self.expander.try_send((id, slot, nt_path))),
             Request::Seed { kind, addresses } => {
                 if let Some(s) = &self.seeder {
                     self.count(s.try_send((kind, addresses)));
@@ -185,6 +251,7 @@ fn hash_worker(
     jobs: &Mutex<Receiver<HashJob>>,
     replies: &Sender<Reply>,
     cache: &Mutex<HashCache>,
+    forgets: &Forgets,
     counters: &ServiceCounters,
     size_cap: u64,
 ) {
@@ -194,17 +261,11 @@ fn hash_worker(
     }
     let enricher = Enricher::new(size_cap);
     loop {
-        let job = match jobs.lock().expect("hash lane").recv() {
+        let (id, nt_path) = match jobs.lock().expect("hash lane").recv() {
             Ok(j) => j,
             Err(_) => return,
         };
-        let (id, nt_path) = match job {
-            HashJob::Enrich(id, p) => (id, p),
-            HashJob::Invalidate(p) => {
-                cache.lock().expect("hash cache").invalidate(&p);
-                continue;
-            }
-        };
+        forgets.apply(cache);
         let r = enricher.enrich(&nt_path, cache);
         counters.hash_cache_evictions.store(cache.lock().expect("hash cache").evictions, Ordering::Relaxed);
         let reply = Reply::Enriched { id, hashes: r.hashes, signature: r.signature, error: r.error };
@@ -233,20 +294,19 @@ fn expander_lane<D: Dir>(
     jobs: Receiver<ExpandJob>,
     replies: &Sender<Reply>,
     mut expander: Expander<D>,
-    lost: &AtomicBool,
+    pending: &Mutex<Invalidations>,
 ) {
-    for job in jobs {
-        if lost.swap(false, Ordering::Acquire) {
+    for (id, slot, nt) in jobs {
+        let waiting = std::mem::take(&mut *pending.lock().expect("invalidations"));
+        if waiting.lost {
             expander.clear();
         }
-        match job {
-            ExpandJob::Expand(id, slot, nt) => {
-                let reply = Reply::Expanded { id, slot, long_path: expander.expand(&nt, Instant::now()) };
-                if replies.send(reply).is_err() {
-                    return;
-                }
-            }
-            ExpandJob::Invalidate(nt) => expander.invalidate(&nt),
+        for p in &waiting.paths {
+            expander.invalidate(p);
+        }
+        let reply = Reply::Expanded { id, slot, long_path: expander.expand(&nt, Instant::now()) };
+        if replies.send(reply).is_err() {
+            return;
         }
     }
 }
@@ -398,27 +458,82 @@ mod tests {
         assert!(expanded(&wait(&s, 1), 1).is_some(), "the expansion still completes");
     }
 
-    /// R-M3: when the expander lane is full, a dropped invalidation clears the
-    /// expansion cache before the next job, so a reused short name is not
-    /// answered from the cache.
-    #[test]
-    fn a_lost_invalidation_clears_the_expansion_cache() {
+    /// The expander's cache answers SECRET~1 with SecretStuffAAA, then its
+    /// lane blocks on an expansion of OTHER~1 and a second SECRET~1 waits.
+    fn blocked_after_caching_secret(lane_cap: usize) -> (GateDir, Services) {
         let dir = GateDir::default();
         dir.set("SECRET~1", "SecretStuffAAA");
-        let cfg = ServiceConfig { lane_cap: 1, ..ServiceConfig::default() };
+        let cfg = ServiceConfig { lane_cap, ..ServiceConfig::default() };
         let s = Services::start_with(&cfg, dir.clone());
         s.submit(Request::Expand { id: 1, slot: 0, nt_path: format!(r"{V}\SECRET~1") });
         assert_eq!(expanded(&wait(&s, 1), 1), Some(Some(format!(r"{V}\SecretStuffAAA"))));
-
         dir.close();
         s.submit(Request::Expand { id: 2, slot: 0, nt_path: format!(r"{V}\OTHER~1") }); // the lane blocks on it
         std::thread::sleep(Duration::from_millis(100));
-        s.submit(Request::Expand { id: 3, slot: 0, nt_path: format!(r"{V}\SECRET~1") }); // fills the queue
-        s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\SecretStuffAAA") }); // dropped
-        assert!(s.counters().service_queue_drops.load(Ordering::Relaxed) >= 1);
+        s.submit(Request::Expand { id: 3, slot: 0, nt_path: format!(r"{V}\SECRET~1") });
+        (dir, s)
+    }
+
+    /// R3a: an invalidation reaches the expander without a job of its own,
+    /// before the next expansion, even while its lane is full.
+    #[test]
+    fn an_invalidation_waits_for_the_expanders_next_job() {
+        let (dir, s) = blocked_after_caching_secret(1);
+        // The lane (one job) is full; the list holds one path: it fits there.
+        s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\SecretStuffAAA") });
         dir.set("SECRET~1", "SecretStuffBBB"); // the short name now belongs to another directory
         dir.open();
         let got = wait(&s, 2);
         assert_eq!(expanded(&got, 3), Some(Some(format!(r"{V}\SecretStuffBBB"))), "{got:?}");
+        assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0, "nothing was dropped");
+    }
+
+    /// R-M3, R3a: when more invalidations wait than the list holds, the
+    /// expander clears its whole cache before its next job, so a reused short
+    /// name is not answered from the cache.
+    #[test]
+    fn too_many_waiting_invalidations_clear_the_expansion_cache() {
+        let (dir, s) = blocked_after_caching_secret(2);
+        for i in 0..2 {
+            s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\Unrelated{i}") });
+        }
+        // The one that matters overflows the list (review 2).
+        s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\SecretStuffAAA") });
+        dir.set("SECRET~1", "SecretStuffBBB");
+        dir.open();
+        let got = wait(&s, 2);
+        assert_eq!(expanded(&got, 3), Some(Some(format!(r"{V}\SecretStuffBBB"))), "{got:?}");
+        assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0);
+    }
+
+    /// R3a, review 4: the caller only sends; a worker applies the invalidation
+    /// before its next lookup.
+    #[test]
+    fn an_invalidation_reaches_the_hash_cache_before_the_next_lookup() {
+        let s = Services::start(&ServiceConfig::default());
+        let system32 = format!(r"{}\System32", std::env::var("SystemRoot").unwrap());
+        let (notepad, cmd) = (nt(&format!(r"{system32}\notepad.exe")), nt(&format!(r"{system32}\cmd.exe")));
+        s.submit(Request::Enrich { id: 1, target: EnrichTarget::LaunchImage, nt_path: notepad.clone() });
+        assert_eq!(wait(&s, 1).len(), 1);
+        assert_eq!(s.cache.lock().unwrap().len(), 1, "notepad.exe is cached");
+        s.submit(Request::InvalidateHash { nt_path: notepad.to_uppercase() });
+        assert_eq!(s.cache.lock().unwrap().len(), 1, "the caller touched no cache");
+        s.submit(Request::Enrich { id: 2, target: EnrichTarget::LaunchImage, nt_path: cmd });
+        assert_eq!(wait(&s, 1).len(), 1);
+        assert_eq!(s.cache.lock().unwrap().len(), 1, "notepad.exe gone before cmd.exe's lookup, cmd.exe cached");
+    }
+
+    /// Review 3: once the expander's list has overflowed it keeps nothing more,
+    /// and the hash cache's invalidations stop at the lane's capacity.
+    #[test]
+    fn invalidations_past_the_cap_are_not_kept() {
+        let cfg = ServiceConfig { lane_cap: 2, ..ServiceConfig::default() };
+        let s = Services::start(&cfg);
+        for i in 0..5 {
+            s.submit(Request::InvalidateHash { nt_path: format!(r"{V}\x{i}") });
+        }
+        let q = s.invalidations.lock().unwrap();
+        assert!(q.lost && q.paths.is_empty(), "lost {}, {} kept", q.lost, q.paths.len());
+        assert_eq!(s.forgets.waiting.load(Ordering::Relaxed), 2, "no hash job ran to apply them");
     }
 }
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 427 passed, 19 ignored (`atlas-agent` unit tests: 238).
```powershell
git add crates/atlas-agent
git commit -m "perf(agent): cache invalidations wake no thread (D6 R3a)"
```

### Task 10: Launch images are hashed ahead of module loads

**Files:**
- Modify: `crates/atlas-agent/src/win/services.rs`

- [ ] **Step 1: The lane** (R4, plan 1b-4a-p1's F9). `HashLane` replaces the hash workers' `sync_channel`: two queues (Launch images, module loads), each bounded at the lane's capacity, behind a mutex and a condvar; `pop` takes a Launch image first; `close` (on `Services`' drop) lets the workers drain what is queued, then end. `submit` routes an `Enrich` by its `EnrichTarget`; `push`, on the pipeline thread, tolerates a poisoned lock (review 9). Each worker applies the waiting hash invalidations (Task 9) before its job. The tests: the order, the routing, each queue's own bound, and a parked worker woken by `close`.

```diff
--- a/crates/atlas-agent/src/win/services.rs
+++ b/crates/atlas-agent/src/win/services.rs
@@ -1,7 +1,9 @@
 //! The services' threads and the routing of [`Request`]s to them (sensor spec
 //! §3.2 [4] and [8]; plan 1b-3b, D5).
 //!
-//! - **Hash workers** (2, below normal): `Enrich`.
+//! - **Hash workers** (2, below normal): `Enrich`, Launch images first (plan
+//!   1b-4a-p2, R4): while the cache is cold a Launch would otherwise wait
+//!   behind dozens of module loads and miss its 1 s deadline (F9).
 //! - **Reader lane** (1): value reads, on the ordered path and the fast path.
 //!   It does no file I/O, so a slow directory never delays a value read (R-M2).
 //! - **Expander lane** (1): 8.3 expansion (D4).
@@ -21,12 +23,13 @@
 //! list the expander applies before its next expansion, so none is applied
 //! late; if the list fills first, the expander clears its whole cache instead
 //! (R-M3), and the list keeps nothing more until then (review 3). Neither is a
-//! counted drop. Replies come back on one channel,
-//! which the driver (plan 1b-3c) feeds to `Pipeline::reply`.
+//! counted drop. Replies come back on one channel, which the driver (plan
+//! 1b-3c) feeds to `Pipeline::reply`.
 
+use std::collections::VecDeque;
 use std::sync::atomic::{AtomicUsize, Ordering};
 use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
-use std::sync::{Arc, Mutex, PoisonError};
+use std::sync::{Arc, Condvar, Mutex, PoisonError};
 use std::time::Instant;
 
 use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
@@ -39,11 +42,67 @@ use crate::completion::PendingId;
 use crate::config::ServiceConfig;
 use crate::counters::ServiceCounters;
 use crate::intake::FastRead;
-use crate::services::{EarlyKey, HandleKind, Reply, Request, ValueRead};
+use crate::services::{EarlyKey, EnrichTarget, HandleKind, Reply, Request, ValueRead};
 
 /// An `Enrich`: the event waiting for it, and the file's NT path.
 type HashJob = (PendingId, String);
 
+/// The hash workers' lane: two bounded queues, Launch images taken first.
+struct HashLane {
+    queues: Mutex<HashQueues>,
+    ready: Condvar,
+    /// The most jobs each queue holds.
+    cap: usize,
+}
+
+#[derive(Default)]
+struct HashQueues {
+    launches: VecDeque<HashJob>,
+    modules: VecDeque<HashJob>,
+    closed: bool,
+}
+
+impl HashLane {
+    fn new(cap: usize) -> Self {
+        HashLane { queues: Mutex::new(HashQueues::default()), ready: Condvar::new(), cap: cap.max(1) }
+    }
+
+    /// Queues a job; false if its queue is full.
+    fn push(&self, launch: bool, job: HashJob) -> bool {
+        // The caller is the pipeline thread: a worker's panic must not end it (review 9).
+        let mut q = self.queues.lock().unwrap_or_else(PoisonError::into_inner);
+        let queue = if launch { &mut q.launches } else { &mut q.modules };
+        if queue.len() >= self.cap {
+            return false;
+        }
+        queue.push_back(job);
+        drop(q);
+        self.ready.notify_one();
+        true
+    }
+
+    /// The next job, a Launch image's first; `None` once closed and empty.
+    fn pop(&self) -> Option<HashJob> {
+        let mut q = self.queues.lock().expect("hash lane");
+        loop {
+            if let Some(job) = q.launches.pop_front().or_else(|| q.modules.pop_front()) {
+                return Some(job);
+            }
+            if q.closed {
+                return None;
+            }
+            q = self.ready.wait(q).expect("hash lane");
+        }
+    }
+
+    /// The workers finish what is queued, then end.
+    fn close(&self) {
+        // Also on a poisoned lock: this runs on drop, perhaps while unwinding.
+        self.queues.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
+        self.ready.notify_all();
+    }
+}
+
 enum ReaderJob {
     Read(PendingId, ValueRead),
     Early(EarlyKey, ValueRead),
@@ -84,7 +143,7 @@ struct Invalidations {
 }
 
 pub struct Services {
-    hash: SyncSender<HashJob>,
+    hash: Arc<HashLane>,
     reader: SyncSender<ReaderJob>,
     expander: SyncSender<ExpandJob>,
     /// The workers' cache, for the tests to look into.
@@ -125,17 +184,16 @@ impl Services {
         let (reply_tx, replies) = channel();
 
         let cache = Arc::new(Mutex::new(HashCache::new(cfg.hash_cache_cap)));
-        let (hash, hash_rx) = sync_channel::<HashJob>(cfg.lane_cap);
-        let hash_rx = Arc::new(Mutex::new(hash_rx));
+        let hash = Arc::new(HashLane::new(cfg.lane_cap));
         let (forget, forget_rx) = channel();
         let forgets = Arc::new(Forgets { rx: Mutex::new(forget_rx), waiting: AtomicUsize::new(0) });
         for i in 0..cfg.hash_workers.max(1) {
-            let (rx, tx, cache, counters, cap) =
-                (hash_rx.clone(), reply_tx.clone(), cache.clone(), counters.clone(), cfg.hash_size_cap);
+            let (lane, tx, cache, counters, cap) =
+                (hash.clone(), reply_tx.clone(), cache.clone(), counters.clone(), cfg.hash_size_cap);
             let forgets = forgets.clone();
             std::thread::Builder::new()
                 .name(format!("atlas-hash-{i}"))
-                .spawn(move || hash_worker(&rx, &tx, &cache, &forgets, &counters, cap))
+                .spawn(move || hash_worker(&lane, &tx, &cache, &forgets, &counters, cap))
                 .expect("spawn a hash worker");
         }
 
@@ -200,7 +258,11 @@ impl Services {
     /// short lock, the expander's list's, and wakes no thread (R3a).
     pub fn submit(&self, r: Request) {
         match r {
-            Request::Enrich { id, nt_path, .. } => self.count(self.hash.try_send((id, nt_path))),
+            Request::Enrich { id, target, nt_path } => {
+                if !self.hash.push(target == EnrichTarget::LaunchImage, (id, nt_path)) {
+                    self.counters.service_queue_drops.fetch_add(1, Ordering::Relaxed);
+                }
+            }
             Request::InvalidateHash { nt_path } => {
                 if self.forgets.waiting.fetch_add(1, Ordering::Relaxed) < self.invalidations_cap {
                     let _ = self.forget.send(nt_path.clone());
@@ -247,8 +309,16 @@ impl Services {
     }
 }
 
+impl Drop for Services {
+    /// The hash workers end once their lane is empty (the other lanes end when
+    /// their senders go).
+    fn drop(&mut self) {
+        self.hash.close();
+    }
+}
+
 fn hash_worker(
-    jobs: &Mutex<Receiver<HashJob>>,
+    jobs: &HashLane,
     replies: &Sender<Reply>,
     cache: &Mutex<HashCache>,
     forgets: &Forgets,
@@ -260,11 +330,7 @@ fn hash_worker(
         let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
     }
     let enricher = Enricher::new(size_cap);
-    loop {
-        let (id, nt_path) = match jobs.lock().expect("hash lane").recv() {
-            Ok(j) => j,
-            Err(_) => return,
-        };
+    while let Some((id, nt_path)) = jobs.pop() {
         forgets.apply(cache);
         let r = enricher.enrich(&nt_path, cache);
         counters.hash_cache_evictions.store(cache.lock().expect("hash cache").evictions, Ordering::Relaxed);
@@ -376,6 +442,54 @@ mod tests {
         assert_eq!(s.counters().service_queue_drops.load(Ordering::Relaxed), 0);
     }
 
+    #[test]
+    fn launch_images_are_taken_before_module_loads() {
+        // R4: a Launch queued behind module loads is hashed next.
+        let lane = HashLane::new(8);
+        for id in 1..=3 {
+            assert!(lane.push(false, (id, format!("m{id}"))));
+        }
+        assert!(lane.push(true, (9, "launch".into())));
+        let order: Vec<PendingId> = (0..4).map(|_| lane.pop().unwrap().0).collect();
+        assert_eq!(order, [9, 1, 2, 3]);
+    }
+
+    #[test]
+    fn each_hash_queue_is_bounded_and_a_closed_lane_drains_then_ends() {
+        let lane = HashLane::new(2);
+        assert!(lane.push(false, (1, "a".into())) && lane.push(false, (2, "b".into())));
+        assert!(!lane.push(false, (3, "c".into())), "the module queue is full");
+        assert!(lane.push(true, (4, "d".into())), "the Launch queue has its own room");
+        lane.close();
+        let left: Vec<PendingId> = std::iter::from_fn(|| lane.pop()).map(|j| j.0).collect();
+        assert_eq!(left, [4, 1, 2]);
+    }
+
+    #[test]
+    fn closing_the_lane_wakes_a_parked_worker_once_it_is_empty() {
+        let lane = Arc::new(HashLane::new(4));
+        let (l, (tx, rx)) = (lane.clone(), channel());
+        std::thread::spawn(move || tx.send(std::iter::from_fn(|| l.pop()).count()));
+        std::thread::sleep(Duration::from_millis(50)); // parked in pop, nothing queued
+        lane.close();
+        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).expect("the worker ended"), 0);
+    }
+
+    #[test]
+    fn launch_images_go_to_the_priority_queue() {
+        let s = Services::start(&ServiceConfig::default());
+        s.hash.close();
+        std::thread::sleep(Duration::from_millis(200)); // the idle workers end
+        s.submit(Request::Enrich { id: 1, target: EnrichTarget::Module, nt_path: "m".into() });
+        s.submit(Request::Enrich { id: 2, target: EnrichTarget::LaunchImage, nt_path: "l".into() });
+        let ids = |d: &VecDeque<HashJob>| d.iter().map(|j| j.0).collect::<Vec<_>>();
+        let queued = {
+            let q = s.hash.queues.lock().unwrap();
+            (ids(&q.launches), ids(&q.modules))
+        };
+        assert_eq!(queued, (vec![2], vec![1]));
+    }
+
     #[test]
     fn full_lanes_drop_and_count() {
         let cfg = ServiceConfig { lane_cap: 1, hash_workers: 1, ..ServiceConfig::default() };
```

- [ ] **Step 2: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass. Workspace: 431 passed, 19 ignored (`atlas-agent` unit tests: 242).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): Launch images are hashed ahead of module loads (D6 R4)"
```

### Task 11: Each bounded structure's entries and capacity in the diagnostic log

**Files:**
- Modify: `crates/atlas-agent/src/completion.rs`, `counters.rs`, `health.rs`, `intake.rs`, `keymap.rs`, `ordering.rs`, `process.rs`, `recent.rs`, `pipeline/file.rs`, `pipeline/mod.rs`, `pipeline/net.rs`, `pipeline/seed.rs`, `pipeline/tests.rs`
- Modify: `crates/atlas-agent/src/win/expand.rs`, `win/hash.rs`, `win/health.rs`, `win/services.rs`, `crates/atlas-agent/tests/binary.rs`

- [ ] **Step 1: The pipeline's sizes** (R6). `Pipeline::sizes` gives each structure's entries and allocated capacity: the ordering stage, the completion stage's line and pending events, the process cache, the file and key maps, the two `Irp` tables, the watchlist opens, the seeding address history, the negative cache, the TCP and UDP tables. Each structure gains a `len`/`capacity` pair or a `sizes`. A test names every one.

```diff
--- a/crates/atlas-agent/src/completion.rs
+++ b/crates/atlas-agent/src/completion.rs
@@ -238,6 +238,11 @@ impl<E> Completion<E> {
         self.pending.len()
     }
 
+    /// (events in line, their capacity, pending entries, their capacity) (R6).
+    pub fn sizes(&self) -> (usize, usize, usize, usize) {
+        (self.queue.len(), self.queue.capacity(), self.pending.len(), self.pending.capacity())
+    }
+
     pub fn is_empty(&self) -> bool {
         self.queue.is_empty()
     }
```

```diff
--- a/crates/atlas-agent/src/keymap.rs
+++ b/crates/atlas-agent/src/keymap.rs
@@ -236,6 +236,10 @@ impl KeyMap {
         self.map.len()
     }
 
+    pub fn capacity(&self) -> usize {
+        self.map.capacity()
+    }
+
     pub fn is_empty(&self) -> bool {
         self.map.is_empty()
     }
```

```diff
--- a/crates/atlas-agent/src/ordering.rs
+++ b/crates/atlas-agent/src/ordering.rs
@@ -111,6 +111,11 @@ impl Ordering {
     pub fn is_empty(&self) -> bool {
         self.heap.is_empty()
     }
+
+    /// The slots allocated: the most events held at once (R6).
+    pub fn capacity(&self) -> usize {
+        self.slots.capacity()
+    }
 }
 
 #[cfg(test)]
```

```diff
--- a/crates/atlas-agent/src/process.rs
+++ b/crates/atlas-agent/src/process.rs
@@ -219,6 +219,10 @@ impl ProcessCache {
         self.by_key.len()
     }
 
+    pub fn capacity(&self) -> usize {
+        self.by_key.capacity()
+    }
+
     pub fn is_empty(&self) -> bool {
         self.by_key.is_empty()
     }
```

```diff
--- a/crates/atlas-agent/src/recent.rs
+++ b/crates/atlas-agent/src/recent.rs
@@ -76,6 +76,10 @@ impl<K: Hash + Eq + Clone, S: BuildHasher + Default> Recent<K, S> {
         self.map.len()
     }
 
+    pub fn capacity(&self) -> usize {
+        self.map.capacity()
+    }
+
     pub fn is_empty(&self) -> bool {
         self.map.is_empty()
     }
@@ -134,6 +138,10 @@ impl<K: Hash + Eq + Clone, V> RecentMap<K, V> {
         self.map.len()
     }
 
+    pub fn capacity(&self) -> usize {
+        self.map.capacity()
+    }
+
     pub fn is_empty(&self) -> bool {
         self.map.is_empty()
     }
```

```diff
--- a/crates/atlas-agent/src/pipeline/file.rs
+++ b/crates/atlas-agent/src/pipeline/file.rs
@@ -155,6 +155,16 @@ impl Files {
         }
     }
 
+    /// (entries, capacity) of the confirmed and failed IRPs and the watchlist
+    /// opens (R6).
+    pub(super) fn recent_sizes(&self) -> [(usize, usize); 3] {
+        [
+            (self.confirmed.len(), self.confirmed.capacity()),
+            (self.failed.len(), self.failed.capacity()),
+            (self.opened.len(), self.opened.capacity()),
+        ]
+    }
+
     #[cfg(test)]
     pub(super) fn sizes(&self) -> [(&'static str, usize); 7] {
         [
```

```diff
--- a/crates/atlas-agent/src/pipeline/net.rs
+++ b/crates/atlas-agent/src/pipeline/net.rs
@@ -86,6 +86,11 @@ impl Network {
         self.evictions
     }
 
+    /// (TCP connections, their capacity, UDP flows, their capacity) (R6).
+    pub fn sizes(&self) -> (usize, usize, usize, usize) {
+        (self.tcp.len(), self.tcp.capacity(), self.flows.len(), self.flows.capacity())
+    }
+
     fn close_flow(f: Flow, out: &mut Completion<Event>, clock: &Clock, ids: &mut IdGen, id: &Identity) {
         // A UDP Close is timestamped at the flow's last datagram (§3.3).
         let kind = activity(f.actor, f.local, f.remote, NetworkProtocol::Udp, f.direction, CLOSE);
```

```diff
--- a/crates/atlas-agent/src/pipeline/seed.rs
+++ b/crates/atlas-agent/src/pipeline/seed.rs
@@ -150,6 +150,18 @@ impl Seeding {
     pub fn negative_cache_len(&self) -> usize {
         self.unnamable.len()
     }
+
+    pub fn negative_cache_capacity(&self) -> usize {
+        self.unnamable.capacity()
+    }
+
+    pub fn history_len(&self) -> usize {
+        self.changes.len()
+    }
+
+    pub fn history_capacity(&self) -> usize {
+        self.changes.capacity()
+    }
 }
 
 impl<L: Lookups> Pipeline<L> {
```

```diff
--- a/crates/atlas-agent/src/pipeline/mod.rs
+++ b/crates/atlas-agent/src/pipeline/mod.rs
@@ -288,6 +288,31 @@ impl<L: Lookups> Pipeline<L> {
         out
     }
 
+    /// Each bounded structure's entries and allocated capacity, for the
+    /// diagnostic log (plan 1b-4a-p2, R6).
+    pub fn sizes(&self) -> crate::health::Sizes {
+        let (line, line_cap, pending, pending_cap) = self.completion.sizes();
+        let (tcp, tcp_cap, flows, flows_cap) = self.net.sizes();
+        let [confirmed, failed, opened] = self.files.recent_sizes();
+        let n = |len: usize, cap: usize| (len as u64, cap as u64);
+        let entries = [
+            ("ordering", n(self.ordering.len(), self.ordering.capacity())),
+            ("completion", n(line, line_cap)),
+            ("pending", n(pending, pending_cap)),
+            ("processes", n(self.procs.len(), self.procs.capacity())),
+            ("file_map", n(self.files.map.len(), self.files.map.capacity())),
+            ("key_map", n(self.keys.len(), self.keys.capacity())),
+            ("confirmed_irps", n(confirmed.0, confirmed.1)),
+            ("failed_irps", n(failed.0, failed.1)),
+            ("watch_opens", n(opened.0, opened.1)),
+            ("address_history", n(self.seeding.history_len(), self.seeding.history_capacity())),
+            ("negative_cache", n(self.seeding.negative_cache_len(), self.seeding.negative_cache_capacity())),
+            ("tcp", n(tcp, tcp_cap)),
+            ("udp_flows", n(flows, flows_cap)),
+        ];
+        entries.into_iter().map(|(name, (len, cap))| (name, len, cap)).collect()
+    }
+
     /// What is kept for pending events and recent history, by name (tests check
     /// it all goes once events leave and windows pass).
     #[cfg(test)]
```

```diff
--- a/crates/atlas-agent/src/pipeline/tests.rs
+++ b/crates/atlas-agent/src/pipeline/tests.rs
@@ -603,6 +603,44 @@ fn forgotten(t: &mut T) -> Vec<String> {
         .collect()
 }
 
+/// R6: every bounded structure by name, each with its entries and capacity.
+#[test]
+fn the_sizes_name_each_bounded_structure() {
+    let mut t = T::new();
+    running(&mut t, 100, 10, EXPLORER);
+    t.ev(5, 100, Some(key(10)), create(0x1, 0xA, r"\Device\HarddiskVolume3\a.txt", 0));
+    t.ev(6, 100, Some(key(10)), create(0x2, 0xB, r"\Device\HarddiskVolume3\b.txt", 0));
+    t.at(400); // still held
+    let held = t.p.sizes();
+    let names: Vec<&str> = held.iter().map(|s| s.0).collect();
+    assert_eq!(
+        names,
+        [
+            "ordering",
+            "completion",
+            "pending",
+            "processes",
+            "file_map",
+            "key_map",
+            "confirmed_irps",
+            "failed_irps",
+            "watch_opens",
+            "address_history",
+            "negative_cache",
+            "tcp",
+            "udp_flows"
+        ]
+    );
+    let get = |s: &[(&str, u64, u64)], n: &str| s.iter().find(|x| x.0 == n).map(|x| (x.1, x.2)).unwrap();
+    assert!(get(&held, "ordering").0 >= 2, "both Creates held");
+    assert!(held.iter().all(|(_, len, cap)| len <= cap), "{held:?}");
+    t.at(1_200);
+    let after = t.p.sizes();
+    assert_eq!(get(&after, "ordering").0, 0);
+    assert_eq!(get(&after, "file_map").0, 2, "both handles mapped");
+    assert!(get(&after, "processes").0 >= 1);
+}
+
 /// D6: the System process sets the basic information of the DLLs starting
 /// processes load; forgetting their hashes then re-hashed them on every launch.
 #[test]
```

- [ ] **Step 2: The other threads' gauges.** Structures owned by other threads report through atomics: the early key map (Session A's callback, updated on every change, a close included), the hash cache (set by a worker after each job, so after the invalidations it applied) and the expansion cache (set by the expander after each job; a `BTreeMap` allocates per entry, so its capacity is its entries). `Snapshot` carries the sizes of the moment; a report takes them from the current snapshot, never a difference.

```diff
--- a/crates/atlas-agent/src/counters.rs
+++ b/crates/atlas-agent/src/counters.rs
@@ -92,6 +92,9 @@ pub struct IntakeCounters {
     /// review R-M4).
     pub cleanup_unpaired: AtomicU64,
     pub cleanup_outcome_late: AtomicU64,
+    /// Gauges: the early key map's entries and capacity (R6).
+    pub early_keys: AtomicU64,
+    pub early_keys_capacity: AtomicU64,
 }
 
 impl IntakeCounters {
@@ -103,6 +106,12 @@ impl IntakeCounters {
         c.load(Ordering::Relaxed)
     }
 
+    /// The early key map's (entries, capacity), gauges (R6).
+    pub fn set_early_keys(&self, (len, cap): (usize, usize)) {
+        self.early_keys.store(len as u64, Ordering::Relaxed);
+        self.early_keys_capacity.store(cap as u64, Ordering::Relaxed);
+    }
+
     /// The values now, for a Sensor Health snapshot (plan 1b-4a).
     pub fn totals(&self) -> IntakeTotals {
         let g = Self::get;
@@ -117,6 +126,8 @@ impl IntakeCounters {
             fast_reads: g(&self.fast_reads),
             cleanup_unpaired: g(&self.cleanup_unpaired),
             cleanup_outcome_late: g(&self.cleanup_outcome_late),
+            early_keys: g(&self.early_keys),
+            early_keys_capacity: g(&self.early_keys_capacity),
         }
     }
 }
@@ -134,6 +145,9 @@ pub struct IntakeTotals {
     pub fast_reads: u64,
     pub cleanup_unpaired: u64,
     pub cleanup_outcome_late: u64,
+    /// Gauges: the early key map's entries and capacity (R6).
+    pub early_keys: u64,
+    pub early_keys_capacity: u64,
 }
 
 /// Counters kept by the Windows services (plan 1b-3b), shared with their
@@ -157,6 +171,12 @@ pub struct ServiceCounters {
     pub seeder_deferred_rereads: AtomicU64,
     /// Gauge, updated at the end of each snapshot.
     pub seeder_stuck_helpers: AtomicU64,
+    /// Gauges for the diagnostic log (R6): the hash cache's entries and
+    /// capacity after each hash job, the expansion cache's entries after each
+    /// expansion.
+    pub hash_cache_entries: AtomicU64,
+    pub hash_cache_capacity: AtomicU64,
+    pub expand_cache_entries: AtomicU64,
 }
 
 impl ServiceCounters {
@@ -172,6 +192,9 @@ impl ServiceCounters {
             seeder_table_reads: g(&self.seeder_table_reads),
             seeder_deferred_rereads: g(&self.seeder_deferred_rereads),
             seeder_stuck_helpers: g(&self.seeder_stuck_helpers),
+            hash_cache_entries: g(&self.hash_cache_entries),
+            hash_cache_capacity: g(&self.hash_cache_capacity),
+            expand_cache_entries: g(&self.expand_cache_entries),
         }
     }
 }
@@ -188,4 +211,8 @@ pub struct ServiceTotals {
     pub seeder_deferred_rereads: u64,
     /// A gauge.
     pub seeder_stuck_helpers: u64,
+    /// Gauges (R6).
+    pub hash_cache_entries: u64,
+    pub hash_cache_capacity: u64,
+    pub expand_cache_entries: u64,
 }
```

```diff
--- a/crates/atlas-agent/src/intake.rs
+++ b/crates/atlas-agent/src/intake.rs
@@ -128,6 +128,7 @@ impl Intake {
                     if evicted > 0 {
                         self.counters.early_key_map_evictions.fetch_add(evicted, std::sync::atomic::Ordering::Relaxed);
                     }
+                    self.counters.set_early_keys(m.sizes());
                 }
             }
             RawEvent::RegCloseKey(k) => {
@@ -136,6 +137,7 @@ impl Intake {
                     && !header.start_key.is_some_and(|s| self.self_keys.contains(&s))
                 {
                     m.close(k.key_object);
+                    self.counters.set_early_keys(m.sizes());
                 }
             }
             RawEvent::RegSetValue(v) if v.status == 0 => {
@@ -374,6 +376,11 @@ impl EarlyKeys {
         self.names.remove(&key);
     }
 
+    /// (entries, capacity) (R6).
+    pub fn sizes(&self) -> (usize, usize) {
+        (self.names.len(), self.names.capacity())
+    }
+
     pub fn name(&self, key: u64) -> Option<&str> {
         self.names.get(&key).map(|(n, _)| n.as_str())
     }
@@ -800,6 +807,20 @@ mod tests {
         assert_eq!(reads.lock().unwrap().len(), 1);
     }
 
+    #[test]
+    fn the_early_maps_size_is_a_gauge() {
+        // R6: entries and capacity after each open and close.
+        let (mut i, _, _, c, _) = intake(64);
+        i.on_event(header(1, 1), Ok(open(10, 0, r"\REGISTRY\MACHINE\X")));
+        i.on_event(header(1, 2), Ok(open(11, 10, "Y")));
+        assert_eq!(IntakeCounters::get(&c.early_keys), 2);
+        assert!(IntakeCounters::get(&c.early_keys_capacity) >= 2);
+        let close = RawEvent::RegCloseKey(RegKey { key_object: 10, status: 0, key_name: WStr::default() });
+        i.on_event(header(1, 3), Ok(close));
+        assert_eq!(IntakeCounters::get(&c.early_keys), 1);
+        assert_eq!(c.totals().early_keys, 1);
+    }
+
     #[test]
     fn the_early_map_evicts_the_oldest() {
         let mut m = EarlyKeys::new(2);
```

```diff
--- a/crates/atlas-agent/src/health.rs
+++ b/crates/atlas-agent/src/health.rs
@@ -19,6 +19,11 @@ use atlas_schema::{Device, Event, EventId, EventKind, EventMeta, Sensor};
 
 use crate::counters::{Class, Counters, IntakeTotals, ServiceTotals};
 
+/// Bounded structures' entries and allocated capacity, by name, for the
+/// diagnostic log (plan 1b-4a-p2, R6): gauges, not differences. Capacity is
+/// what holds memory: a map keeps its largest allocation after entries leave.
+pub type Sizes = Vec<(&'static str, u64, u64)>;
+
 /// One ETW session's loss counters (`EVENT_TRACE_PROPERTIES`): 32-bit and
 /// cumulative, so differences wrap.
 #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
@@ -53,6 +58,9 @@ pub struct Snapshot {
     /// 1b-4a-p1); `None` when not listed this time.
     pub threads: Option<ThreadList>,
     pub seeding: bool,
+    /// The pipeline's structures (R6), read with its counters; empty when not
+    /// read.
+    pub pipeline_sizes: Sizes,
 }
 
 /// One listing of the agent's threads (plan 1b-4a-p1).
@@ -113,6 +121,9 @@ pub struct Diagnostics {
     /// The time between the two listings, ns: it differs from the report's
     /// interval when the threads were last listed before the last report.
     pub threads_interval: Option<i64>,
+    /// Every bounded structure's entries and capacity at `now` (R6): the
+    /// pipeline's, then the early key map, the hash and expansion caches.
+    pub sizes: Sizes,
 }
 
 fn d(prev: u64, now: u64) -> u64 {
@@ -235,11 +246,25 @@ pub fn report(prev: &Snapshot, now: &Snapshot) -> (HealthReport, Diagnostics) {
         fast_reads: d(i0.fast_reads, i1.fast_reads),
         threads: prev.threads.as_ref().zip(now.threads.as_ref()).map(|(a, b)| d_threads(a, b)),
         threads_interval: prev.threads.as_ref().zip(now.threads.as_ref()).map(|(a, b)| b.at - a.at),
+        sizes: sizes(now),
     };
     let report = HealthReport { loss, quality, housekeeping, resources, throughput, ..HealthReport::default() };
     (report, diagnostics)
 }
 
+/// The structures' gauges in `now`: the pipeline's when it was read, then the
+/// intake's and the services'.
+fn sizes(now: &Snapshot) -> Sizes {
+    let (i, s) = (&now.intake, &now.services);
+    let mut v = now.pipeline_sizes.clone();
+    v.push(("early_key_map", i.early_keys, i.early_keys_capacity));
+    v.push(("hash_cache", s.hash_cache_entries, s.hash_cache_capacity));
+    // A BTreeMap allocates per entry, with no spare room: its capacity is its
+    // entries (review 7).
+    v.push(("expand_cache", s.expand_cache_entries, s.expand_cache_entries));
+    v
+}
+
 /// Whether any loss counter rose between the two snapshots: a report is then
 /// due at once (§9.3).
 pub fn loss_rose(prev: &Snapshot, now: &Snapshot) -> bool {
@@ -297,9 +322,32 @@ mod tests {
             working_set: Some(50 << 20),
             threads: Some(listing(1_000, &[(1, 10, "atlas-hash-0", 900), (2, 10, "atlas-pipeline", 100)])),
             seeding: true,
+            pipeline_sizes: vec![("file_map", 3, 16)],
         }
     }
 
+    /// R6: the structures' gauges come from `now` alone, the pipeline's first,
+    /// then the early key map, the hash and expansion caches.
+    #[test]
+    fn a_report_carries_every_structures_size_from_now() {
+        let mut now = busy();
+        now.intake.early_keys = 7;
+        now.intake.early_keys_capacity = 14;
+        now.services.hash_cache_entries = 5;
+        now.services.hash_cache_capacity = 28;
+        now.services.expand_cache_entries = 2;
+        let mut prev = busy();
+        prev.pipeline_sizes = vec![("file_map", 99, 99)];
+        let (_, diag) = report(&prev, &now);
+        assert_eq!(
+            diag.sizes,
+            vec![("file_map", 3, 16), ("early_key_map", 7, 14), ("hash_cache", 5, 28), ("expand_cache", 2, 2)]
+        );
+        // The pipeline not read: its part is absent, the rest still there.
+        now.pipeline_sizes = vec![];
+        assert_eq!(report(&prev, &now).1.sizes.len(), 3);
+    }
+
     /// A thread listing: (thread id, creation time, name, CPU ns).
     fn listing(at: i64, threads: &[(u32, u64, &str, u64)]) -> ThreadList {
         ThreadList { at, threads: threads.iter().map(|&(t, c, n, cpu)| ((t, c), (n.to_owned(), cpu))).collect() }
```

```diff
--- a/crates/atlas-agent/src/win/expand.rs
+++ b/crates/atlas-agent/src/win/expand.rs
@@ -135,8 +135,7 @@ impl<D: Dir> Expander<D> {
         }
     }
 
-    #[cfg(test)]
-    fn len(&self) -> usize {
+    pub(crate) fn len(&self) -> usize {
         self.cache.len()
     }
 }
```

```diff
--- a/crates/atlas-agent/src/win/hash.rs
+++ b/crates/atlas-agent/src/win/hash.rs
@@ -92,10 +92,13 @@ impl HashCache {
         }
     }
 
-    #[cfg(test)]
     pub(crate) fn len(&self) -> usize {
         self.map.len()
     }
+
+    pub(crate) fn capacity(&self) -> usize {
+        self.map.capacity()
+    }
 }
 
 /// What a signature check concluded.
```

```diff
--- a/crates/atlas-agent/src/win/services.rs
+++ b/crates/atlas-agent/src/win/services.rs
@@ -210,7 +210,10 @@ impl Services {
         let (tx, pending) = (reply_tx.clone(), invalidations.clone());
         std::thread::Builder::new()
             .name("atlas-expander".into())
-            .spawn(move || expander_lane(expand_rx, &tx, Expander::new(dir), &pending))
+            .spawn({
+                let counters = counters.clone();
+                move || expander_lane(expand_rx, &tx, Expander::new(dir), &pending, &counters)
+            })
             .expect("spawn the expander lane");
 
         let seeder = Seeder::new(cfg, counters.clone()).map(|s| {
@@ -333,7 +336,12 @@ fn hash_worker(
     while let Some((id, nt_path)) = jobs.pop() {
         forgets.apply(cache);
         let r = enricher.enrich(&nt_path, cache);
-        counters.hash_cache_evictions.store(cache.lock().expect("hash cache").evictions, Ordering::Relaxed);
+        {
+            let c = cache.lock().expect("hash cache");
+            counters.hash_cache_evictions.store(c.evictions, Ordering::Relaxed);
+            counters.hash_cache_entries.store(c.len() as u64, Ordering::Relaxed);
+            counters.hash_cache_capacity.store(c.capacity() as u64, Ordering::Relaxed);
+        }
         let reply = Reply::Enriched { id, hashes: r.hashes, signature: r.signature, error: r.error };
         if replies.send(reply).is_err() {
             return;
@@ -361,6 +369,7 @@ fn expander_lane<D: Dir>(
     replies: &Sender<Reply>,
     mut expander: Expander<D>,
     pending: &Mutex<Invalidations>,
+    counters: &ServiceCounters,
 ) {
     for (id, slot, nt) in jobs {
         let waiting = std::mem::take(&mut *pending.lock().expect("invalidations"));
@@ -371,6 +380,7 @@ fn expander_lane<D: Dir>(
             expander.invalidate(p);
         }
         let reply = Reply::Expanded { id, slot, long_path: expander.expand(&nt, Instant::now()) };
+        counters.expand_cache_entries.store(expander.len() as u64, Ordering::Relaxed);
         if replies.send(reply).is_err() {
             return;
         }
@@ -581,6 +591,7 @@ mod tests {
         let s = Services::start_with(&cfg, dir.clone());
         s.submit(Request::Expand { id: 1, slot: 0, nt_path: format!(r"{V}\SECRET~1") });
         assert_eq!(expanded(&wait(&s, 1), 1), Some(Some(format!(r"{V}\SecretStuffAAA"))));
+        assert_eq!(s.counters.expand_cache_entries.load(Ordering::Relaxed), 1, "R6: one short name cached");
         dir.close();
         s.submit(Request::Expand { id: 2, slot: 0, nt_path: format!(r"{V}\OTHER~1") }); // the lane blocks on it
         std::thread::sleep(Duration::from_millis(100));
@@ -630,11 +641,15 @@ mod tests {
         s.submit(Request::Enrich { id: 1, target: EnrichTarget::LaunchImage, nt_path: notepad.clone() });
         assert_eq!(wait(&s, 1).len(), 1);
         assert_eq!(s.cache.lock().unwrap().len(), 1, "notepad.exe is cached");
+        let gauge = |g: &std::sync::atomic::AtomicU64| g.load(Ordering::Relaxed);
+        assert_eq!(gauge(&s.counters.hash_cache_entries), 1, "R6: the gauge is set before the reply");
+        assert!(gauge(&s.counters.hash_cache_capacity) >= 1);
         s.submit(Request::InvalidateHash { nt_path: notepad.to_uppercase() });
         assert_eq!(s.cache.lock().unwrap().len(), 1, "the caller touched no cache");
         s.submit(Request::Enrich { id: 2, target: EnrichTarget::LaunchImage, nt_path: cmd });
         assert_eq!(wait(&s, 1).len(), 1);
         assert_eq!(s.cache.lock().unwrap().len(), 1, "notepad.exe gone before cmd.exe's lookup, cmd.exe cached");
+        assert_eq!(gauge(&s.counters.hash_cache_entries), 1, "R6: the gauge follows the invalidation (review 7)");
     }
 
     /// Review 3: once the expander's list has overflowed it keeps nothing more,
```

- [ ] **Step 3: The log line.** `sizes="name=entries/capacity …"` next to `threads=`; the binary test checks a report logs it, and its thread note ends before it.

```diff
--- a/crates/atlas-agent/src/win/health.rs
+++ b/crates/atlas-agent/src/win/health.rs
@@ -26,7 +26,7 @@ use super::agent::{HealthSources, Stopped};
 use super::identity;
 use super::util::Owned;
 use crate::counters::Counters;
-use crate::health::{self, Diagnostics, SessionLoss, Snapshot, ThreadList};
+use crate::health::{self, Diagnostics, SessionLoss, Sizes, Snapshot, ThreadList};
 use crate::store::Record;
 
 #[derive(Debug, Clone, Copy)]
@@ -151,6 +151,7 @@ fn log(r: &atlas_schema::classes::sensor_health::HealthReport, d: &Diagnostics,
         working_set_mb = n(r.resources.working_set.map(|b| b >> 20)),
         threads_ms = d.threads_interval.map_or(-1, |ns| ns / 1_000_000),
         threads = d.threads.as_deref().map_or_else(|| "-".to_owned(), thread_line),
+        sizes = sizes_line(&d.sizes),
         "health report"
     );
 }
@@ -161,11 +162,17 @@ fn thread_line(threads: &[(String, u64)]) -> String {
     parts.join(" ")
 }
 
-/// The pipeline's counters, read on its thread between two passes; `None` if
-/// it does not answer in time (it ended, or is stuck).
-fn pipeline_counters(src: &HealthSources, timeout: Duration) -> Option<Counters> {
+/// `name=entries/capacity` for each structure, as the log line carries them (R6).
+fn sizes_line(sizes: &[(&str, u64, u64)]) -> String {
+    let parts: Vec<String> = sizes.iter().map(|(name, len, cap)| format!("{name}={len}/{cap}")).collect();
+    parts.join(" ")
+}
+
+/// The pipeline's counters and structure sizes, read on its thread between
+/// two passes; `None` if it does not answer in time (it ended, or is stuck).
+fn pipeline_counters(src: &HealthSources, timeout: Duration) -> Option<(Counters, Sizes)> {
     let (tx, rx) = channel();
-    src.control.send(Box::new(move |p| drop(tx.send(p.counters())))).ok()?;
+    src.control.send(Box::new(move |p| drop(tx.send((p.counters(), p.sizes()))))).ok()?;
     rx.recv_timeout(timeout).ok()
 }
 
@@ -176,12 +183,14 @@ fn session_loss(info: Option<SessionInfo>) -> Option<SessionLoss> {
 /// Every source now but the threads (the caller lists them when due).
 /// `pipeline` is read by the caller: through the control channel while
 /// running, from `Stopped` at the end.
-pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<Counters>) -> Snapshot {
+pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<(Counters, Sizes)>) -> Snapshot {
     let query = |name: &str| session_loss(query_by_name(name).ok().flatten());
     let (cpu_time, working_set) = resources();
+    let (pipeline, pipeline_sizes) = pipeline.map_or((None, Sizes::new()), |(c, s)| (Some(c), s));
     Snapshot {
         threads: None,
         pipeline,
+        pipeline_sizes,
         intake: src.intake.totals(),
         services: src.services.totals(),
         callback_panics: src.counters.iter().map(|c| c.panics.load(Ordering::Relaxed)).sum(),
@@ -209,6 +218,8 @@ pub fn final_snapshot(
     Snapshot {
         threads,
         pipeline: stopped.pipeline.as_ref().ok().cloned(),
+        // The pipeline has stopped: its structures were emptied.
+        pipeline_sizes: Sizes::new(),
         intake: stopped.intake.totals(),
         services: stopped.services.totals(),
         callback_panics: stopped.callback_panics,
@@ -484,6 +495,11 @@ mod tests {
         assert_eq!(line, "atlas-hash-0=1234 atlas-pipeline=0");
     }
 
+    #[test]
+    fn the_sizes_line_is_name_equals_entries_over_capacity() {
+        assert_eq!(sizes_line(&[("file_map", 3, 16), ("hash_cache", 0, 0)]), "file_map=3/16 hash_cache=0/0");
+    }
+
     #[test]
     fn resources_are_read() {
         let (cpu, ws) = resources();
```

```diff
--- a/crates/atlas-agent/tests/binary.rs
+++ b/crates/atlas-agent/tests/binary.rs
@@ -363,8 +363,14 @@ fn binary_run_and_dump() {
     // m2). Not a check: a thread shows only if it used a clock tick (~15.6 ms)
     // since the last listing, a few seconds before; its line is kept instead.
     let last = log_text.lines().rfind(|l| l.contains("health report")).unwrap_or("");
-    let threads = last.split("threads=").nth(1).unwrap_or("").trim();
+    let threads = last.split("threads=").nth(1).and_then(|t| t.split(" sizes=").next()).unwrap_or("").trim();
     report.note("final_report_threads", json!(threads));
+    // A periodic report's line gives each bounded structure's entries and
+    // capacity (plan 1b-4a-p2, R6); the pipeline's only while it runs.
+    let sizes = log_text.lines().find(|l| l.contains("health report") && l.contains("file_map="));
+    report.check("a_report_logs_structure_sizes", sizes.is_some(), sizes.unwrap_or("").trim());
+    let sizes = sizes.and_then(|l| l.split("sizes=").nth(1)).unwrap_or("").trim();
+    report.note("a_reports_sizes", json!(sizes));
 
     // ---- the buffer: through dump, and directly ----
     let (code, out, err) = run_to_end(&["dump", "--data-dir", &data.to_string_lossy()], Duration::from_secs(60));
```

- [ ] **Step 4: Check and commit**

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass; the binary test's check runs in CI's `agent-live` job. Workspace: 435 passed, 19 ignored (`atlas-agent` unit tests: 246).
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): each bounded structure's entries and capacity in the diagnostic log (D6 R6)"
```

### Task 12: Memory follows contents: the queues, the sink and the ordering stage

**Files:**
- Create: `crates/atlas-agent/src/queue.rs`
- Modify: `crates/atlas-agent/src/lib.rs`, `driver.rs`, `intake.rs`, `store.rs`, `ordering.rs`, `health.rs`, `win/agent.rs`, `win/health.rs`, `win/run.rs`, `crates/atlas-agent/tests/replay.rs`, `crates/atlas-agent/tests/binary.rs`

- [ ] **Step 1: `queue::bounded`** (R6b, F6). std's unbounded channel, which allocates blocks of 31 messages and frees each once read, with the cap kept by a shared count: `try_send` refuses at the cap exactly as `SyncSender::try_send` did (`Full`, the message handed back), a receive makes room, `send` (for the final report) queues past the cap. A closed end is reported and leaves the count right. A `QueueGauge` reads a queue's fill and cap from any thread (review 7). The tests pin each.

```diff
--- /dev/null
+++ b/crates/atlas-agent/src/queue.rs
@@ -0,0 +1,161 @@
+//! Bounded queues whose memory follows their contents (plan 1b-4a-p2, R6b).
+//!
+//! std's bounded channel allocates every slot when it is made and stamps each
+//! one, so all of it is resident from the start: 45 MB for the sink's 65,536
+//! events of 688 bytes, 11.5 MB for the kernel queue. Its unbounded channel
+//! allocates blocks of 31 messages and frees each one once read. These queues
+//! are the unbounded channel with the cap kept by a counter: a full queue
+//! refuses a `try_send` exactly as before, and the caller counts the drop.
+
+use std::sync::Arc;
+use std::sync::atomic::{AtomicUsize, Ordering};
+use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError, TrySendError};
+use std::time::Duration;
+
+/// A queue of at most `cap` messages for [`QueueSender::try_send`].
+pub fn bounded<T>(cap: usize) -> (QueueSender<T>, QueueReceiver<T>) {
+    let (tx, rx) = mpsc::channel();
+    let len = Arc::new(AtomicUsize::new(0));
+    (QueueSender { tx, len: len.clone(), cap: cap.max(1) }, QueueReceiver { rx, len })
+}
+
+pub struct QueueSender<T> {
+    tx: mpsc::Sender<T>,
+    len: Arc<AtomicUsize>,
+    cap: usize,
+}
+
+impl<T> Clone for QueueSender<T> {
+    fn clone(&self) -> Self {
+        QueueSender { tx: self.tx.clone(), len: self.len.clone(), cap: self.cap }
+    }
+}
+
+impl<T> QueueSender<T> {
+    /// Queues `msg` unless the queue holds `cap` messages already. Never blocks.
+    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
+        if self.len.fetch_add(1, Ordering::AcqRel) >= self.cap {
+            self.len.fetch_sub(1, Ordering::AcqRel);
+            return Err(TrySendError::Full(msg));
+        }
+        self.tx.send(msg).map_err(|e| {
+            self.len.fetch_sub(1, Ordering::AcqRel);
+            TrySendError::Disconnected(e.0)
+        })
+    }
+
+    /// The queue's fill, for the diagnostic log (R6, review 7).
+    pub fn gauge(&self) -> QueueGauge {
+        QueueGauge { len: self.len.clone(), cap: self.cap }
+    }
+
+    /// Queues `msg` even past the cap: for the rare records that must not be
+    /// dropped (a Sensor Health report). Never blocks.
+    pub fn send(&self, msg: T) -> Result<(), mpsc::SendError<T>> {
+        self.len.fetch_add(1, Ordering::AcqRel);
+        self.tx.send(msg).inspect_err(|_| {
+            self.len.fetch_sub(1, Ordering::AcqRel);
+        })
+    }
+}
+
+/// A queue's messages and cap, readable from any thread.
+#[derive(Clone)]
+pub struct QueueGauge {
+    len: Arc<AtomicUsize>,
+    cap: usize,
+}
+
+impl QueueGauge {
+    /// (messages queued now, the cap).
+    pub fn read(&self) -> (u64, u64) {
+        (self.len.load(Ordering::Acquire) as u64, self.cap as u64)
+    }
+}
+
+pub struct QueueReceiver<T> {
+    rx: mpsc::Receiver<T>,
+    len: Arc<AtomicUsize>,
+}
+
+impl<T> QueueReceiver<T> {
+    pub fn try_recv(&self) -> Result<T, TryRecvError> {
+        self.rx.try_recv().inspect(|_| self.taken())
+    }
+
+    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
+        self.rx.recv_timeout(timeout).inspect(|_| self.taken())
+    }
+
+    /// What is queued now, without waiting.
+    pub fn try_iter(&self) -> impl Iterator<Item = T> + '_ {
+        std::iter::from_fn(|| self.try_recv().ok())
+    }
+
+    /// Messages queued now.
+    pub fn len(&self) -> usize {
+        self.len.load(Ordering::Acquire)
+    }
+
+    pub fn is_empty(&self) -> bool {
+        self.len() == 0
+    }
+
+    fn taken(&self) {
+        self.len.fetch_sub(1, Ordering::AcqRel);
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::*;
+
+    #[test]
+    fn the_gauge_reads_the_fill_and_the_cap() {
+        let (tx, rx) = bounded::<u8>(3);
+        let g = tx.gauge();
+        tx.try_send(1).unwrap();
+        tx.try_send(2).unwrap();
+        assert_eq!(g.read(), (2, 3));
+        rx.try_recv().unwrap();
+        assert_eq!(g.read(), (1, 3), "a receive is seen");
+    }
+
+    #[test]
+    fn the_cap_refuses_a_try_send_and_a_receive_makes_room() {
+        let (tx, rx) = bounded(2);
+        assert!(tx.try_send(1).is_ok() && tx.clone().try_send(2).is_ok());
+        assert!(matches!(tx.try_send(3), Err(TrySendError::Full(3))));
+        assert_eq!(rx.len(), 2);
+        assert_eq!(rx.try_recv(), Ok(1));
+        assert!(tx.try_send(4).is_ok());
+        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [2, 4]);
+        assert!(rx.is_empty());
+    }
+
+    #[test]
+    fn send_queues_past_the_cap() {
+        let (tx, rx) = bounded(1);
+        tx.try_send(1).unwrap();
+        tx.send(2).unwrap();
+        assert_eq!(rx.len(), 2);
+        assert!(matches!(tx.try_send(3), Err(TrySendError::Full(3))));
+        assert_eq!(rx.recv_timeout(Duration::from_millis(10)), Ok(1));
+        assert_eq!(rx.recv_timeout(Duration::from_millis(10)), Ok(2));
+        assert_eq!(rx.recv_timeout(Duration::from_millis(10)), Err(RecvTimeoutError::Timeout));
+    }
+
+    #[test]
+    fn a_closed_end_is_reported_and_leaves_the_count_right() {
+        let (tx, rx) = bounded::<u8>(1);
+        drop(rx);
+        assert!(matches!(tx.try_send(1), Err(TrySendError::Disconnected(1))));
+        assert!(tx.send(2).is_err());
+        assert_eq!(tx.len.load(Ordering::Acquire), 0);
+        let (tx, rx) = bounded::<u8>(1);
+        tx.try_send(5).unwrap();
+        drop(tx);
+        assert_eq!(rx.try_recv(), Ok(5));
+        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
+    }
+}
```

```diff
--- a/crates/atlas-agent/src/lib.rs
+++ b/crates/atlas-agent/src/lib.rs
@@ -17,6 +17,7 @@ pub mod ordering;
 pub mod paths;
 pub mod pipeline;
 pub mod process;
+pub mod queue;
 pub mod recent;
 pub mod services;
 pub mod settings;
```

- [ ] **Step 2: Its users.** The kernel and user queues (`driver`, `intake`, `win::agent`, the replay test) and the sink (`store`, `win::health`) use it, with the same caps and the same drop counters.

```diff
--- a/crates/atlas-agent/src/driver.rs
+++ b/crates/atlas-agent/src/driver.rs
@@ -21,6 +21,8 @@
 //! (`win::agent`); the tests use fakes.
 
 use std::sync::mpsc::{Receiver, TryRecvError, channel};
+
+use crate::queue::QueueReceiver;
 use std::time::{Duration, Instant};
 
 use atlas_schema::Event;
@@ -77,8 +79,8 @@ pub type Control<L> = Box<dyn FnOnce(&mut Pipeline<L>) + Send>;
 pub struct Driver<L, S> {
     pipeline: Pipeline<L>,
     lanes: S,
-    kernel: Receiver<Incoming>,
-    user: Receiver<Incoming>,
+    kernel: QueueReceiver<Incoming>,
+    user: QueueReceiver<Incoming>,
     control: Receiver<Control<L>>,
     clock: Clock,
     anchor: AnchorSource,
@@ -89,8 +91,8 @@ impl<L: Lookups, S: Lanes> Driver<L, S> {
     pub fn new(
         pipeline: Pipeline<L>,
         lanes: S,
-        kernel: Receiver<Incoming>,
-        user: Receiver<Incoming>,
+        kernel: QueueReceiver<Incoming>,
+        user: QueueReceiver<Incoming>,
         clock: Clock,
         anchor: AnchorSource,
         cfg: DriverConfig,
@@ -160,7 +162,7 @@ fn next_wait(cfg: &DriverConfig, pushed: usize) -> Duration {
 
 /// Pushes what is queued, at most `max` events. Returns whether the queue is
 /// still open, and how many it pushed.
-fn drain<L: Lookups>(rx: &Receiver<Incoming>, p: &mut Pipeline<L>, max: usize) -> (bool, usize) {
+fn drain<L: Lookups>(rx: &QueueReceiver<Incoming>, p: &mut Pipeline<L>, max: usize) -> (bool, usize) {
     let mut n = 0;
     for _ in 0..max.max(1) {
         match rx.try_recv() {
@@ -178,7 +180,9 @@ fn drain<L: Lookups>(rx: &Receiver<Incoming>, p: &mut Pipeline<L>, max: usize) -
 #[cfg(test)]
 mod tests {
     use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
-    use std::sync::mpsc::{SyncSender, channel, sync_channel};
+    use std::sync::mpsc::channel;
+
+    use crate::queue::{QueueSender, bounded};
     use std::sync::{Arc, Mutex};
 
     use atlas_etw::parse::{FileCreate, RawEvent};
@@ -195,8 +199,8 @@ mod tests {
 
     /// A driver on its own thread with a real clock; the sink sends events back.
     struct Running {
-        kernel: Option<SyncSender<Incoming>>,
-        user: Option<SyncSender<Incoming>>,
+        kernel: Option<QueueSender<Incoming>>,
+        user: Option<QueueSender<Incoming>>,
         events: std::sync::mpsc::Receiver<Event>,
         thread: std::thread::JoinHandle<Counters>,
         t0: Instant,
@@ -209,8 +213,8 @@ mod tests {
     fn start(cfg: DriverConfig, anchor: AnchorSource) -> Running {
         let t0 = Instant::now();
         let p = pipeline();
-        let (ktx, krx) = sync_channel(cfg.kernel_queue_cap);
-        let (utx, urx) = sync_channel(cfg.user_queue_cap);
+        let (ktx, krx) = bounded(cfg.kernel_queue_cap);
+        let (utx, urx) = bounded(cfg.user_queue_cap);
         let (etx, events) = channel();
         let driver = Driver::new(p, FakeLanes::new(|_| None), krx, urx, Box::new(move || qpc(t0)), anchor, cfg);
         let thread = std::thread::spawn(move || driver.run(|e| etx.send(e).unwrap()));
@@ -245,7 +249,7 @@ mod tests {
     #[test]
     fn drain_counts_what_it_pushed() {
         let mut p = pipeline();
-        let (tx, rx) = sync_channel(8);
+        let (tx, rx) = bounded(8);
         for i in 0..3 {
             tx.send(created(i, r"\Device\HarddiskVolume3\a.txt")).unwrap();
         }
@@ -267,8 +271,8 @@ mod tests {
             n.fetch_add(1, AtomicOrdering::Relaxed);
             qpc(t0)
         });
-        let (ktx, krx) = sync_channel(1);
-        let (utx, urx) = sync_channel(1);
+        let (ktx, krx) = bounded(1);
+        let (utx, urx) = bounded(1);
         let driver =
             Driver::new(pipeline(), FakeLanes::new(|_| None), krx, urx, clock, no_anchor(), DriverConfig::default());
         let thread = std::thread::spawn(move || driver.run(|_| {}));
@@ -392,8 +396,8 @@ mod tests {
             started: 0,
         };
         let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
-        let (ktx, krx) = sync_channel(4);
-        let (utx, urx) = sync_channel(4);
+        let (ktx, krx) = bounded(4);
+        let (utx, urx) = bounded(4);
         drop((ktx, utx));
         let cfg = DriverConfig { cadence: Duration::ZERO, idle_cadence: Duration::ZERO, ..DriverConfig::default() };
         Driver::new(p, Counting(s), krx, urx, Box::new(|| 0), no_anchor(), cfg).run(|_| {});
@@ -417,8 +421,8 @@ mod tests {
             started: 0,
         };
         let p = Pipeline::new(setup, FakeLookups::default(), sequential_ids()).unwrap();
-        let (ktx, krx) = sync_channel(4);
-        let (_utx, urx) = sync_channel(4);
+        let (ktx, krx) = bounded(4);
+        let (_utx, urx) = bounded(4);
         let (ctx, crx) = channel::<Control<FakeLookups>>();
         let t0 = Instant::now();
         let driver = Driver::new(
```

```diff
--- a/crates/atlas-agent/src/intake.rs
+++ b/crates/atlas-agent/src/intake.rs
@@ -11,7 +11,7 @@
 
 use std::collections::VecDeque;
 use std::sync::Arc;
-use std::sync::mpsc::{SyncSender, TrySendError};
+use std::sync::mpsc::TrySendError;
 
 use atlas_etw::parse::{FileOpEnd, ParseError, RawEvent};
 
@@ -20,6 +20,7 @@ use crate::config::{Config, Ticks};
 use crate::counters::IntakeCounters;
 use crate::fast::{FastMap, FastSet};
 use crate::input::{Header, Incoming, Session};
+use crate::queue::QueueSender;
 use crate::recent::Recent;
 use crate::services::{EarlyKey, ValueRead};
 
@@ -29,15 +30,15 @@ pub type FastRead = Box<dyn FnMut(EarlyKey, ValueRead) + Send>;
 /// The two queues into the pipeline (§3.2): bounded, never blocking.
 pub struct Queues {
     /// Kernel providers and Session B (default 65,536 entries).
-    pub kernel: SyncSender<Incoming>,
+    pub kernel: QueueSender<Incoming>,
     /// User-mode providers, DNS-Client (default 8,192), so a forger cannot
     /// push kernel events out (§4.4).
-    pub user: SyncSender<Incoming>,
+    pub user: QueueSender<Incoming>,
 }
 
 pub struct Intake {
-    kernel: SyncSender<Incoming>,
-    user: SyncSender<Incoming>,
+    kernel: QueueSender<Incoming>,
+    user: QueueSender<Incoming>,
     counters: Arc<IntakeCounters>,
     dns: Buckets,
     early: Option<EarlyKeys>,
@@ -451,11 +452,11 @@ impl Buckets {
 #[cfg(test)]
 mod tests {
     use super::*;
+    use crate::queue::{QueueReceiver, bounded};
     use atlas_etw::parse::{
         DnsQuery, FileCreate, FileHandle, FileOpEnd, FileSetInfo, RegKey, RegOpen, RegSetValue, WStr,
     };
     use std::sync::Mutex;
-    use std::sync::mpsc::{Receiver, sync_channel};
 
     fn header(pid: u32, ts: i64) -> Header {
         Header { session: Session::Sensor, pid, tid: 9, ts, start_key: Some(77) }
@@ -463,16 +464,18 @@ mod tests {
 
     type Reads = Arc<Mutex<Vec<(EarlyKey, ValueRead)>>>;
 
-    fn intake(kernel_cap: usize) -> (Intake, Receiver<Incoming>, Receiver<Incoming>, Arc<IntakeCounters>, Reads) {
+    fn intake(
+        kernel_cap: usize,
+    ) -> (Intake, QueueReceiver<Incoming>, QueueReceiver<Incoming>, Arc<IntakeCounters>, Reads) {
         intake_with(kernel_cap, Config { dns_rate_per_pid: 2, ..Config::default() })
     }
 
     fn intake_with(
         kernel_cap: usize,
         cfg: Config,
-    ) -> (Intake, Receiver<Incoming>, Receiver<Incoming>, Arc<IntakeCounters>, Reads) {
-        let (ktx, krx) = sync_channel(kernel_cap);
-        let (utx, urx) = sync_channel(4);
+    ) -> (Intake, QueueReceiver<Incoming>, QueueReceiver<Incoming>, Arc<IntakeCounters>, Reads) {
+        let (ktx, krx) = bounded(kernel_cap);
+        let (utx, urx) = bounded(4);
         let counters = Arc::new(IntakeCounters::default());
         let reads: Reads = Arc::new(Mutex::new(Vec::new()));
         let r = reads.clone();
@@ -524,7 +527,7 @@ mod tests {
         RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: info, status: 0 })
     }
 
-    fn passed_outcomes(krx: &Receiver<Incoming>) -> Vec<(u64, u64)> {
+    fn passed_outcomes(krx: &QueueReceiver<Incoming>) -> Vec<(u64, u64)> {
         krx.try_iter()
             .filter_map(|inc| match inc.event {
                 RawEvent::FileOpEnd(o) if !o.failed() => Some((o.irp, o.extra_information)),
```

```diff
--- a/crates/atlas-agent/src/store.rs
+++ b/crates/atlas-agent/src/store.rs
@@ -22,7 +22,9 @@ use std::collections::VecDeque;
 use std::io;
 use std::sync::Arc;
 use std::sync::atomic::{AtomicU64, Ordering};
-use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
+use std::sync::mpsc::RecvTimeoutError;
+
+use crate::queue::{QueueReceiver, QueueSender, bounded};
 use std::thread::JoinHandle;
 use std::time::{Duration, Instant};
 
@@ -40,7 +42,7 @@ pub enum Record {
 /// The pipeline's sink (plan 1b-3c's interface): never blocks.
 #[derive(Clone)]
 pub struct Sink {
-    tx: SyncSender<Record>,
+    tx: QueueSender<Record>,
     drops: Arc<AtomicU64>,
 }
 
@@ -54,7 +56,7 @@ impl Sink {
 
 /// The running writer thread.
 pub struct Store {
-    tx: SyncSender<Record>,
+    tx: QueueSender<Record>,
     drops: Arc<AtomicU64>,
     thread: JoinHandle<io::Result<Stats>>,
 }
@@ -107,7 +109,7 @@ impl Store {
 
     /// The writer thread over `writer`.
     pub(crate) fn spawn<W: Log + Send + 'static>(writer: W, recovery: Recovery, channel_cap: usize) -> Store {
-        let (tx, rx) = sync_channel(channel_cap);
+        let (tx, rx) = bounded(channel_cap);
         let drops = Arc::new(AtomicU64::new(0));
         let mut core = Core::new(writer, drops.clone(), recovery);
         let thread = std::thread::Builder::new()
@@ -136,9 +138,9 @@ impl Store {
         self.drops.clone()
     }
 
-    /// Where the health thread sends its reports. It may block while the
-    /// writer is stuck on the disk; the health thread can wait.
-    pub fn health(&self) -> SyncSender<Record> {
+    /// Where the health thread sends its reports, with `send`: a report is
+    /// queued even past the cap, and never waits (plan 1b-4a-p2, R6b).
+    pub fn health(&self) -> QueueSender<Record> {
         self.tx.clone()
     }
 
@@ -218,7 +220,7 @@ impl<W: Log> Core<W> {
     }
 
     /// Until every sender is gone.
-    pub(crate) fn run(&mut self, rx: &Receiver<Record>) {
+    pub(crate) fn run(&mut self, rx: &QueueReceiver<Record>) {
         loop {
             match rx.recv_timeout(TICK_EVERY) {
                 Ok(r) => {
@@ -456,7 +458,7 @@ mod tests {
 
     #[test]
     fn a_full_channel_drops_and_counts_never_blocks() {
-        let (tx, rx) = sync_channel(2);
+        let (tx, rx) = bounded(2);
         let drops = Arc::new(AtomicU64::new(0));
         let sink = Sink { tx, drops: drops.clone() };
         // On its own thread: a send that blocked would fail the test, not hang
```

```diff
--- a/crates/atlas-agent/tests/replay.rs
+++ b/crates/atlas-agent/tests/replay.rs
@@ -8,10 +8,10 @@
 //!   `ATLAS_UPDATE_SNAPSHOT=1 cargo test -p atlas-agent --test replay` and review
 //!   the diff against the assertions.
 
+use atlas_agent::queue::{QueueReceiver, bounded};
 use std::collections::HashMap;
 use std::path::PathBuf;
 use std::sync::Arc;
-use std::sync::mpsc::{Receiver, sync_channel};
 use std::time::{Duration, Instant};
 
 use atlas_agent::config::{Config, Ticks};
@@ -114,8 +114,8 @@ fn live_processes(lines: &[Line]) -> HashMap<u32, LiveProcess> {
 struct Replay {
     p: Pipeline<FakeLookups>,
     intake: Intake,
-    krx: Receiver<Incoming>,
-    urx: Receiver<Incoming>,
+    krx: QueueReceiver<Incoming>,
+    urx: QueueReceiver<Incoming>,
     counters: Arc<IntakeCounters>,
     first: i64,
     last: i64,
@@ -194,8 +194,8 @@ fn replay() -> Replay {
         started: first,
     };
     let p = Pipeline::new(setup, lookups, sequential_ids()).unwrap();
-    let (ktx, krx) = sync_channel(65_536);
-    let (utx, urx) = sync_channel(8_192);
+    let (ktx, krx) = bounded(65_536);
+    let (utx, urx) = bounded(8_192);
     let counters = Arc::new(IntakeCounters::default());
     let mut intake =
         Intake::new(Session::Sensor, &config, ticks, Queues { kernel: ktx, user: utx }, counters.clone(), None, &[]);
```

- [ ] **Step 3: The queues in `sizes=`** (review 7: F6 found the memory there). `HealthSources::queues` carries the kernel and user queues' gauges, and `run` adds the sink's; the snapshot reads them and `sizes=` ends with `kernel_queue`, `user_queue` and `sink`. The final report's are empty (the sensor has stopped). The binary test requires all three in a report's line.

```diff
--- a/crates/atlas-agent/src/health.rs
+++ b/crates/atlas-agent/src/health.rs
@@ -61,6 +61,8 @@ pub struct Snapshot {
     /// The pipeline's structures (R6), read with its counters; empty when not
     /// read.
     pub pipeline_sizes: Sizes,
+    /// The kernel, user and sink queues' fill and cap (review 7).
+    pub queues: Sizes,
 }
 
 /// One listing of the agent's threads (plan 1b-4a-p1).
@@ -262,6 +264,7 @@ fn sizes(now: &Snapshot) -> Sizes {
     // A BTreeMap allocates per entry, with no spare room: its capacity is its
     // entries (review 7).
     v.push(("expand_cache", s.expand_cache_entries, s.expand_cache_entries));
+    v.extend(now.queues.iter().copied());
     v
 }
 
@@ -323,6 +326,7 @@ mod tests {
             threads: Some(listing(1_000, &[(1, 10, "atlas-hash-0", 900), (2, 10, "atlas-pipeline", 100)])),
             seeding: true,
             pipeline_sizes: vec![("file_map", 3, 16)],
+            queues: vec![("kernel_queue", 4, 65_536)],
         }
     }
 
@@ -338,14 +342,21 @@ mod tests {
         now.services.expand_cache_entries = 2;
         let mut prev = busy();
         prev.pipeline_sizes = vec![("file_map", 99, 99)];
+        prev.queues = vec![("kernel_queue", 99, 99)];
         let (_, diag) = report(&prev, &now);
         assert_eq!(
             diag.sizes,
-            vec![("file_map", 3, 16), ("early_key_map", 7, 14), ("hash_cache", 5, 28), ("expand_cache", 2, 2)]
+            vec![
+                ("file_map", 3, 16),
+                ("early_key_map", 7, 14),
+                ("hash_cache", 5, 28),
+                ("expand_cache", 2, 2),
+                ("kernel_queue", 4, 65_536)
+            ]
         );
         // The pipeline not read: its part is absent, the rest still there.
         now.pipeline_sizes = vec![];
-        assert_eq!(report(&prev, &now).1.sizes.len(), 3);
+        assert_eq!(report(&prev, &now).1.sizes.len(), 4);
     }
 
     /// A thread listing: (thread id, creation time, name, CPU ns).
```

```diff
--- a/crates/atlas-agent/src/win/agent.rs
+++ b/crates/atlas-agent/src/win/agent.rs
@@ -10,7 +10,7 @@ use std::fmt;
 use std::path::PathBuf;
 use std::sync::Arc;
 use std::sync::atomic::AtomicU64;
-use std::sync::mpsc::{Sender, channel, sync_channel};
+use std::sync::mpsc::{Sender, channel};
 use std::thread::JoinHandle;
 use std::time::Duration;
 
@@ -29,6 +29,7 @@ use crate::health::ThreadList;
 use crate::input::{Header, Session as Which};
 use crate::intake::{Intake, Queues};
 use crate::pipeline::{Pipeline, Setup};
+use crate::queue::QueueGauge;
 use crate::watchlist::BadPattern;
 
 /// What to start.
@@ -125,6 +126,9 @@ pub struct HealthSources {
     /// a test counts its calls). It costs ~45 ms, so it runs at most once a
     /// report period, for a report (plan 1b-4a-p1, Q7, review m3).
     pub threads: Arc<dyn Fn() -> Option<ThreadList> + Send + Sync>,
+    /// The event queues' (and, added by `run`, the sink's) gauges, by name
+    /// (R6, review 7).
+    pub queues: Vec<(&'static str, QueueGauge)>,
 }
 
 /// Starts the sensor (§11.4): identity, the services, both sessions and their
@@ -151,8 +155,8 @@ pub fn start(opts: AgentOptions, sink: impl FnMut(Event) + Send + 'static) -> Re
     let pipeline =
         Pipeline::new(setup, WinLookups::new(), Box::new(|_| EventId::new_v7())).map_err(AgentError::Watchlist)?;
 
-    let (ktx, krx) = sync_channel(opts.driver.kernel_queue_cap);
-    let (utx, urx) = sync_channel(opts.driver.user_queue_cap);
+    let (ktx, krx) = crate::queue::bounded(opts.driver.kernel_queue_cap);
+    let (utx, urx) = crate::queue::bounded(opts.driver.user_queue_cap);
     let queues = || Queues { kernel: ktx.clone(), user: utx.clone() };
     let intake_counters = Arc::new(IntakeCounters::default());
     let self_keys = [started.self_key];
@@ -251,6 +255,11 @@ impl Running {
             sessions: self.names.clone(),
             seeding: self.seeding,
             threads: Arc::new(super::health::threads),
+            queues: self
+                .spare
+                .as_ref()
+                .map(|q| vec![("kernel_queue", q.kernel.gauge()), ("user_queue", q.user.gauge())])
+                .unwrap_or_default(),
         }
     }
 
```

```diff
--- a/crates/atlas-agent/src/win/health.rs
+++ b/crates/atlas-agent/src/win/health.rs
@@ -6,7 +6,7 @@
 use std::collections::BTreeMap;
 use std::sync::Arc;
 use std::sync::atomic::{AtomicU64, Ordering};
-use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel};
+use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
 use std::thread::JoinHandle;
 use std::time::{Duration, Instant};
 
@@ -63,7 +63,7 @@ pub struct HealthThread {
 /// Everything a report needs besides the sources.
 #[derive(Clone)]
 pub struct Reporting {
-    pub to: SyncSender<Record>,
+    pub to: crate::queue::QueueSender<Record>,
     pub device: Device,
     /// `loss.buffer_backlog_drops` (`Store::drops`).
     pub buffer_drops: Arc<AtomicU64>,
@@ -201,6 +201,7 @@ pub fn snapshot(src: &HealthSources, buffer_drops: &AtomicU64, pipeline: Option<
         cpu_time,
         working_set,
         seeding: src.seeding,
+        queues: src.queues.iter().map(|(name, g)| (*name, g.read().0, g.read().1)).collect(),
     }
 }
 
@@ -230,6 +231,8 @@ pub fn final_snapshot(
         cpu_time,
         working_set,
         seeding,
+        // The sensor has stopped: its queues were drained.
+        queues: Sizes::new(),
     }
 }
 
@@ -302,7 +305,7 @@ fn thread(tid: u32, me: u32) -> Option<((u32, u64), (String, u64))> {
 
 #[cfg(test)]
 mod tests {
-    use std::sync::mpsc::sync_channel;
+    use crate::queue::QueueReceiver;
 
     use atlas_schema::classes::sensor_health::SensorHealthAction;
     use atlas_schema::{BootId, DeviceUid, EventKind};
@@ -329,19 +332,18 @@ mod tests {
                 lists.fetch_add(1, Ordering::Relaxed);
                 Some(ThreadList::default())
             }),
+            queues: vec![],
         }
     }
 
-    fn reporting() -> (Reporting, Receiver<Record>) {
-        // Room for a report at every check of a test (a loss at each one):
-        // a full channel would block the health thread and hang the test.
-        let (to, rx) = sync_channel(4_096);
+    fn reporting() -> (Reporting, QueueReceiver<Record>) {
+        let (to, rx) = crate::queue::bounded(4_096);
         let device = Device { uid: DeviceUid::from_bytes([1; 16]), boot_id: BootId::from_bytes([2; 16]) };
         (Reporting { to, device, buffer_drops: Arc::new(AtomicU64::new(0)) }, rx)
     }
 
     /// The next report's time and `loss.kernel_queue_drops`.
-    fn next_report(rx: &Receiver<Record>, within: Duration) -> Option<(i64, Option<u64>)> {
+    fn next_report(rx: &QueueReceiver<Record>, within: Duration) -> Option<(i64, Option<u64>)> {
         let Record::Health(e) = rx.recv_timeout(within).ok()? else { panic!("not a report") };
         let EventKind::SensorHealth(a) = &e.kind else { panic!("not Sensor Health") };
         let SensorHealthAction::Report(r) = &a.action;
```

```diff
--- a/crates/atlas-agent/src/win/run.rs
+++ b/crates/atlas-agent/src/win/run.rs
@@ -174,7 +174,9 @@ where
         buffer_drops: store.drops(),
     };
     let first = Interval { from: Snapshot::at_start(seeding, started.anchor.unix_ns), start: started.anchor.unix_ns };
-    let health = HealthThread::start(running.health_sources(), reporting.clone(), first, opts.health);
+    let mut sources = running.health_sources();
+    sources.queues.push(("sink", store.health().gauge()));
+    let health = HealthThread::start(sources, reporting.clone(), first, opts.health);
 
     tracing::info!("running; Ctrl+C stops");
     let mut code = EXIT_OK;
```

```diff
--- a/crates/atlas-agent/tests/binary.rs
+++ b/crates/atlas-agent/tests/binary.rs
@@ -366,8 +366,12 @@ fn binary_run_and_dump() {
     let threads = last.split("threads=").nth(1).and_then(|t| t.split(" sizes=").next()).unwrap_or("").trim();
     report.note("final_report_threads", json!(threads));
     // A periodic report's line gives each bounded structure's entries and
-    // capacity (plan 1b-4a-p2, R6); the pipeline's only while it runs.
-    let sizes = log_text.lines().find(|l| l.contains("health report") && l.contains("file_map="));
+    // capacity (plan 1b-4a-p2, R6); the pipeline's only while it runs; the
+    // event queues and the sink too (review 7).
+    let sizes = log_text.lines().find(|l| {
+        l.contains("health report")
+            && ["file_map=", "kernel_queue=", "user_queue=", "sink="].iter().all(|n| l.contains(n))
+    });
     report.check("a_report_logs_structure_sizes", sizes.is_some(), sizes.unwrap_or("").trim());
     let sizes = sizes.and_then(|l| l.split("sizes=").nth(1)).unwrap_or("").trim();
     report.note("a_reports_sizes", json!(sizes));
```

- [ ] **Step 4: The ordering stage gives a burst's slots back.** After a release, if more than 8,192 slots are allocated and fewer than an eighth hold an event, the held events move to a new vector of twice their number (at least 8,192), and the keys are rebuilt with the new slot numbers, in the same order. The tests: a burst then a drain shrinks the slots and keeps the order; under the floor nothing is compacted.

```diff
--- a/crates/atlas-agent/src/ordering.rs
+++ b/crates/atlas-agent/src/ordering.rs
@@ -5,13 +5,18 @@
 //!
 //! The heap holds keys only (timestamp, arrival order, slot), so sifting moves
 //! 24 bytes rather than a whole event; events wait in reusable slots and leave
-//! one at a time (plan 1b-4a-p2, R3).
+//! one at a time (plan 1b-4a-p2, R3). After a burst the slots are compacted
+//! once fewer than an eighth hold an event, so a peak's memory is given back
+//! (R6b).
 
 use std::cmp::Reverse;
 use std::collections::BinaryHeap;
 
 use crate::input::Incoming;
 
+/// The fewest slots compaction leaves (about 1.4 MB).
+const SLOTS_FLOOR: usize = 8_192;
+
 /// A held event's key: its timestamp, its arrival order (so equal timestamps
 /// keep their order) and the slot it waits in.
 type Key = (i64, u64, u32);
@@ -83,7 +88,26 @@ impl Ordering {
         self.heap.pop();
         self.last_released = self.last_released.max(ts);
         self.free.push(slot);
-        Some(self.slots[slot as usize].take().expect("a held key has its event"))
+        let inc = self.slots[slot as usize].take().expect("a held key has its event");
+        if self.slots.capacity() > SLOTS_FLOOR && self.heap.len() * 8 < self.slots.capacity() {
+            self.compact();
+        }
+        Some(inc)
+    }
+
+    /// Moves the held events into slots sized for them (twice their number,
+    /// at least `SLOTS_FLOOR`) and rebuilds the keys; their order is kept.
+    fn compact(&mut self) {
+        let keys = std::mem::take(&mut self.heap).into_vec();
+        let mut slots = Vec::with_capacity((keys.len() * 2).max(SLOTS_FLOOR));
+        let mut heap = Vec::with_capacity(keys.len());
+        for Reverse((ts, seq, slot)) in keys {
+            heap.push(Reverse((ts, seq, slots.len() as u32)));
+            slots.push(self.slots[slot as usize].take());
+        }
+        self.heap = BinaryHeap::from(heap);
+        self.slots = slots;
+        self.free = Vec::new();
     }
 
     /// Every held event whose hold has passed at `now`, in timestamp order.
@@ -172,6 +196,32 @@ mod tests {
         assert_eq!((o.slots.len(), o.free.len()), (4, 4));
     }
 
+    #[test]
+    fn after_a_burst_the_slots_shrink_and_the_order_holds() {
+        // R6b: 40,000 held, then all but 300 released.
+        let mut o = Ordering::new(0);
+        for i in 0..40_000 {
+            o.push(inc(i, i as u64));
+        }
+        assert!(o.capacity() >= 40_000);
+        assert_eq!(o.release(39_699).len(), 39_700);
+        assert_eq!(o.len(), 300);
+        assert!(o.capacity() <= 2 * SLOTS_FLOOR, "{} slots kept", o.capacity());
+        // What stays comes out in order, and pushes after it slot in.
+        o.push(inc(50_000, 99));
+        let rest: Vec<u64> = o.drain().iter().map(tag).collect();
+        assert_eq!(rest.len(), 301);
+        assert!(rest[..300].iter().copied().eq(39_700..40_000));
+        assert_eq!(rest[300], 99);
+        // Small ones are left alone.
+        let mut o = Ordering::new(0);
+        for i in 0..100 {
+            o.push(inc(i, 0));
+        }
+        o.release(98);
+        assert_eq!(o.capacity(), 128, "under the floor: no compaction");
+    }
+
     #[test]
     fn equal_timestamps_keep_arrival_order() {
         let mut o = Ordering::new(0);
```

- [ ] **Step 5: Check and commit**

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p atlas-agent -p atlas-etw --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```
Expected: all pass; the binary test's check runs in CI's `agent-live` job. Workspace: 440 passed, 19 ignored (`atlas-agent` unit tests: 251).
```powershell
git add crates/atlas-agent
git commit -m "perf(agent): memory follows contents: the queues, the sink and the ordering stage (D6 R6b)"
```

### Task 13: The agent opts out of power throttling at start

**Files:**
- Create: `crates/atlas-agent/src/win/power.rs`
- Modify: `crates/atlas-agent/src/win/mod.rs`, `crates/atlas-agent/src/win/run.rs`, `crates/atlas-agent/tests/binary.rs`

- [ ] **Step 1: `win::power`** (R8, F7). `run_at_full_speed(process)` sets `ProcessPowerThrottling` with the execution-speed bit in the control mask and out of the state mask ("HighQoS"); `runs_at_full_speed(process)` reads it back. The test uses a child (`ping.exe`, which starts nothing of its own), so the test process is untouched: a new process has opted out of nothing, one that asked to be throttled reads as throttled, and after `run_at_full_speed` it runs at full speed.

```diff
--- /dev/null
+++ b/crates/atlas-agent/src/win/power.rs
@@ -0,0 +1,73 @@
+//! Power throttling (plan 1b-4a-p2, R8). Windows lowers a process's quality of
+//! service (EcoQoS) by its own heuristics, such as a minimized or background
+//! window; on a hybrid CPU with a quiet power plan its threads then run on a
+//! few slow cores, and under load waited to run for most of a 40 s span: hash
+//! results came back past their deadline and launches went out without a hash.
+//! The agent opts out at start, so its timeliness does not depend on them.
+
+use windows::Win32::Foundation::HANDLE;
+use windows::Win32::System::Threading::{
+    GetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
+    PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetProcessInformation,
+};
+
+const SIZE: u32 = size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32;
+
+/// Asks Windows never to throttle `process`'s execution speed (`HighQoS`).
+pub fn run_at_full_speed(process: HANDLE) -> windows::core::Result<()> {
+    let state = PROCESS_POWER_THROTTLING_STATE {
+        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
+        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
+        StateMask: 0,
+    };
+    // SAFETY: `state` is a valid PROCESS_POWER_THROTTLING_STATE of `SIZE` bytes
+    // that outlives the call.
+    unsafe { SetProcessInformation(process, ProcessPowerThrottling, (&raw const state).cast(), SIZE) }
+}
+
+/// Whether `process` opted out of execution-speed throttling.
+pub fn runs_at_full_speed(process: HANDLE) -> windows::core::Result<bool> {
+    let mut state =
+        PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
+    // SAFETY: `state` is a writable PROCESS_POWER_THROTTLING_STATE of `SIZE` bytes.
+    unsafe { GetProcessInformation(process, ProcessPowerThrottling, (&raw mut state).cast(), SIZE)? };
+    let speed = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
+    Ok(state.ControlMask & speed != 0 && state.StateMask & speed == 0)
+}
+
+#[cfg(test)]
+mod tests {
+    use std::os::windows::io::AsRawHandle;
+    use std::process::{Command, Stdio};
+
+    use super::*;
+
+    #[test]
+    fn a_process_runs_at_full_speed_once_it_opts_out() {
+        // A child, so the test process's own setting is left alone; ping
+        // starts no process of its own, so killing it leaves nothing behind.
+        let mut child = Command::new("ping.exe")
+            .args(["-n", "30", "127.0.0.1"])
+            .stdout(Stdio::null())
+            .spawn()
+            .expect("spawn a child");
+        let h = HANDLE(child.as_raw_handle());
+        let before = runs_at_full_speed(h);
+        let eco = PROCESS_POWER_THROTTLING_STATE {
+            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
+            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
+            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
+        };
+        // SAFETY: as in `run_at_full_speed`.
+        let asked = unsafe { SetProcessInformation(h, ProcessPowerThrottling, (&raw const eco).cast(), SIZE) };
+        let throttled = runs_at_full_speed(h);
+        let set = run_at_full_speed(h);
+        let after = runs_at_full_speed(h);
+        let _ = child.kill();
+        let _ = child.wait();
+        assert_eq!(before.ok(), Some(false), "a new process has opted out of nothing");
+        assert!(asked.is_ok() && set.is_ok(), "{asked:?} {set:?}");
+        assert_eq!(throttled.ok(), Some(false), "a process that asked to be throttled");
+        assert_eq!(after.ok(), Some(true));
+    }
+}
```

```diff
--- a/crates/atlas-agent/src/win/mod.rs
+++ b/crates/atlas-agent/src/win/mod.rs
@@ -22,6 +22,7 @@ pub mod health;
 pub mod identity;
 pub mod instance;
 pub mod lookups;
+pub mod power;
 pub mod privilege;
 pub mod run;
 mod seeder;
```

- [ ] **Step 2: At start.** `run_with_sink` calls it for its own process just before the sensor starts (a failure is a warning in the log); the binary test reads the agent process's state once it is up.

```diff
--- a/crates/atlas-agent/src/win/run.rs
+++ b/crates/atlas-agent/src/win/run.rs
@@ -3,8 +3,9 @@
 //!
 //! Start: an elevated token → the single-instance lock → the data directory
 //! (created or verified, §11.1) → `agent.toml` → the diagnostic log → the
-//! buffer → the sensor ([`agent::start`]) with the buffer writer as its sink →
-//! the health thread.
+//! buffer → power throttling turned off ([`power`], plan 1b-4a-p2, R8) → the
+//! sensor ([`agent::start`]) with the buffer writer as its sink → the health
+//! thread.
 //!
 //! Clean stop: the health thread → the sensor ([`agent::Running::stop`]) →
 //! the final Sensor Health report → the buffer, flushed.
@@ -31,6 +32,7 @@ use super::agent::{self, AgentOptions};
 use super::datadir;
 use super::health::{self, HealthConfig, HealthThread, Interval, Reporting};
 use super::instance;
+use super::power;
 use super::util::Owned;
 use crate::config::Config;
 use crate::driver::DriverConfig;
@@ -152,6 +154,10 @@ where
     // Before the sessions start: Ctrl+C during start-up must not end the
     // process with them running. A stop asked for meanwhile waits in `stop`.
     let stop = install_ctrl_handler();
+    // SAFETY: GetCurrentProcess returns a pseudo-handle; nothing to close.
+    if let Err(e) = power::run_at_full_speed(unsafe { GetCurrentProcess() }) {
+        tracing::warn!("power throttling could not be turned off: {e}");
+    }
     tracing::info!("starting the sensor");
     let running = match agent::start(agent_opts, sink) {
         Ok(r) => r,
```

```diff
--- a/crates/atlas-agent/tests/binary.rs
+++ b/crates/atlas-agent/tests/binary.rs
@@ -5,7 +5,8 @@
 //! `cargo test -p atlas-agent --test binary -- --ignored --nocapture`
 //!
 //! 1. `atlas-agent run --data-dir C:\ProgramData\Atlas-Test-binary-<pid>`
-//!    starts in its own process group, which creates the data directory;
+//!    starts in its own process group, which creates the data directory, and
+//!    turns power throttling off for itself;
 //! 2. a second `run` is refused by the single-instance lock;
 //! 3. an actor (this test binary again, `ATLAS_BINARY_ACTOR=1`) creates,
 //!    renames and deletes a file, creates a registry key and value and deletes
@@ -25,13 +26,14 @@
 use std::collections::HashSet;
 use std::io::{BufRead, BufReader, Read, Write};
 use std::net::{TcpListener, TcpStream, ToSocketAddrs};
+use std::os::windows::io::AsRawHandle;
 use std::os::windows::process::CommandExt;
 use std::path::{Path, PathBuf};
 use std::process::{Child, Command, Stdio};
 use std::sync::{Arc, Mutex};
 use std::time::{Duration, Instant};
 
-use atlas_agent::win::datadir;
+use atlas_agent::win::{datadir, power};
 use atlas_buffer::{Cursor, Reader};
 use atlas_schema::classes::file::FileAction;
 use atlas_schema::classes::network::NetworkAction;
@@ -40,6 +42,7 @@ use atlas_schema::classes::registry::{RegistryKeyAction, RegistryValueAction};
 use atlas_schema::classes::sensor_health::{HealthReport, SensorHealthAction};
 use atlas_schema::{Event, EventKind, decode_event};
 use serde_json::{Value, json};
+use windows::Win32::Foundation::HANDLE;
 use windows::Win32::System::Console::{AllocConsole, CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetConsoleWindow};
 use windows::Win32::System::Registry::{
     HKEY, HKEY_CURRENT_USER, KEY_ALL_ACCESS, REG_OPTION_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
@@ -280,6 +283,9 @@ fn binary_run_and_dump() {
     let up_ns = unix_now();
     report.note("start_ms", json!(started.elapsed().as_millis() as u64));
     assert!(up, "the agent did not start: {:#?}", lines.lock().unwrap());
+    // Plan 1b-4a-p2, R8: the agent opted out of power throttling at start.
+    let full_speed = cleanup.agent.as_ref().map(|a| power::runs_at_full_speed(HANDLE(a.as_raw_handle())));
+    report.check("the_agent_runs_at_full_speed", matches!(full_speed, Some(Ok(true))), format!("{full_speed:?}"));
 
     // ---- a second instance ----
     let second = base.join("Second");
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: all pass; `the_agent_runs_at_full_speed` runs in CI's `agent-live` job. Workspace: 441 passed, 19 ignored (`atlas-agent` unit tests: 252).
```powershell
git add crates/atlas-agent
git commit -m "perf(agent): the agent opts out of power throttling at start (D6 R8)"
```

### Task 14: The host A/B run (the user)

The re-measure (R7.5): the old build (plan 1b-4a-p1's final code) and this plan's, back to back under the same conditions, with the trace script of plan 1b-4a-p1. It needs elevation, so the user runs it. The tools are git-ignored under `spikes\`.

- [ ] **Step 1: Build the tools and the agents** (Claude, unelevated). `d6-target-p1` is plan 1b-4a-p1's build (`main` before this plan), kept from that plan; `d6-target-p2` is this plan's:

```powershell
cargo build --release --manifest-path spikes/d6load/Cargo.toml
dotnet build -c Release spikes/d6wait
$env:CARGO_PROFILE_RELEASE_DEBUG = 'line-tables-only'; $env:CARGO_TARGET_DIR = 'spikes/d6-target-p2'
cargo build --release -p atlas-agent
Remove-Item Env:CARGO_PROFILE_RELEASE_DEBUG, Env:CARGO_TARGET_DIR
```

`d6load` (plan 1b-4a-p1's, plus `qos`, which gives a process HighQoS from outside, so both builds run alike whatever Windows decides, F7):

```rust
//! D6 profiling helper (spikes only).
//!
//!   d6load load <dir> <seconds>          a fixed, paced mix of file, registry,
//!                                        launch and DNS activity; prints JSON counts
//!   d6load threads <pid> <csv> <label>   appends each thread's name and CPU times
//!   d6load stop <pid>                    sends Ctrl+C to the console <pid> runs in
//!   d6load cleanup <dir>                 removes the load's files and registry key
//!   d6load qos <pid>                     turns power throttling off for <pid> (HighQoS,
//!                                        as the agent does itself since D6 R8)

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
        Some("qos") if args.len() == 3 => qos(args[2].parse().expect("pid")),
        Some("probe") if args.len() >= 3 => {
            for a in &args[2..] {
                probe(a);
            }
            0
        }
        _ => {
            eprintln!("usage: d6load load <dir> <seconds> | threads <pid> <csv> <label> | stop <pid> | cleanup <dir> | qos <pid>");
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

// ---------------------------------------------------------------- qos

/// HighQoS for `pid`: execution-speed throttling controlled and off. Prints the
/// state before and after as `control/state` masks.
fn qos(pid: u32) -> i32 {
    use windows::Win32::System::Threading::{
        GetProcessInformation, OpenProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SET_INFORMATION, ProcessPowerThrottling, SetProcessInformation,
    };
    const SIZE: u32 = size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32;
    let h = match unsafe { OpenProcess(PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(h) => h,
        Err(e) => {
            eprintln!("OpenProcess({pid}): {e}");
            return 1;
        }
    };
    let get = |h: HANDLE| {
        let mut s = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ..Default::default() };
        match unsafe { GetProcessInformation(h, ProcessPowerThrottling, (&raw mut s).cast(), SIZE) } {
            Ok(()) => format!("{}/{}", s.ControlMask, s.StateMask),
            Err(e) => format!("? ({e})"),
        }
    };
    let before = get(h);
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    let r = unsafe { SetProcessInformation(h, ProcessPowerThrottling, (&raw const state).cast(), SIZE) };
    let after = get(h);
    unsafe {
        let _ = CloseHandle(h);
    }
    println!("qos {pid}: before {before}, after {after}");
    if let Err(e) = r {
        eprintln!("SetProcessInformation: {e}");
        return 1;
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

`d6wait` (each agent thread's CPU per second, its waits, its ready time and its quality of service, and the machine's CPU by process, from a trace's context switches):

```
<Project Sdk="Microsoft.NET.Sdk">

  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net9.0</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="Microsoft.Windows.EventTracing.Processing.All" Version="1.12.10" />
  </ItemGroup>

</Project>
```

```
// D6: what one process's threads waited on in a window of a WPR trace.
//   d6wait <trace.etl> <pid> <threads.csv> <from> <to> [minWaitMs=100] [threadPrefix=atlas-]
// For each named thread: CPU per second, and every wait longer than minWaitMs
// that ended in the window: when, how long, the stack it blocked in, who woke it.
using System.Globalization;
using Microsoft.Windows.EventTracing;
using Microsoft.Windows.EventTracing.Cpu;
using Microsoft.Windows.EventTracing.Symbols;

string etl = args[0];
int pid = int.Parse(args[1]);
var from = DateTimeOffset.Parse(args[3], CultureInfo.InvariantCulture);
var to = DateTimeOffset.Parse(args[4], CultureInfo.InvariantCulture);
double minWait = args.Length > 5 ? double.Parse(args[5], CultureInfo.InvariantCulture) : 100;
string prefix = args.Length > 6 ? args[6] : "atlas-";

var tname = new Dictionary<int, string>();
foreach (var line in File.ReadLines(args[2]).Skip(1))
{
    var c = line.Split(',');
    tname[int.Parse(c[2])] = c[3];
}

var settings = new TraceProcessorSettings { AllowLostEvents = true, AllowTimeInversion = true };
using var trace = TraceProcessor.Create(etl, settings);
var pendingSched = trace.UseCpuSchedulingData();
var pendingSymbols = trace.UseSymbols();
Console.Error.WriteLine("processing...");
trace.Process();
Console.Error.WriteLine("loading symbols...");
var symPath = Environment.GetEnvironmentVariable("_NT_SYMBOL_PATH") ?? "";
var symCache = Path.Combine(Path.GetDirectoryName(Path.GetFullPath(etl))!, "symcache");
await pendingSymbols.Result.LoadSymbolsAsync(new SymCachePath(symCache), new SymbolPath(symPath.Split(';')), null, new[] { "atlas-agent.exe" }, Array.Empty<string>());

static string Fn(StackFrame f) => (f.Image?.FileName ?? "?") + "!" + (f.Symbol?.FunctionName ?? "?");

// The interesting frames of a wait stack: the kernel's wait function nearest the
// leaf, then the first frames of the agent's own code.
static string Where(IStackSnapshot? st)
{
    if (st?.Frames is not { } fr || fr.Count == 0) return "(no stack)";
    var names = fr.Select(Fn).ToList();
    var agent = names.Where(n => n.StartsWith("atlas-agent.exe!")).Take(4).Select(n => n["atlas-agent.exe!".Length..]);
    var sys = names.Where(n => !n.StartsWith("atlas-agent.exe!") && !n.StartsWith("ntoskrnl.exe!Ke") && !n.StartsWith("ntoskrnl.exe!Ki") && !n.StartsWith("ntoskrnl.exe!Etw") && !n.StartsWith("ntoskrnl.exe!SwapContext")).Take(5);
    return string.Join(" < ", sys) + "\n        agent: " + string.Join(" < ", agent);
}

var qos = new SortedDictionary<string, double>();
var totals = new SortedDictionary<string, (double run, double ready, double wait, SortedDictionary<int, double> cpus)>();
var perSec = new SortedDictionary<string, SortedDictionary<long, double>>();
var waits = new List<(DateTimeOffset end, string th, double ms, double ready, double ran, string where, string by)>();
foreach (var a in pendingSched.Result.ThreadActivity)
{
    if (a.Process?.Id != pid) continue;
    int tid = a.Thread?.Id ?? -1;
    string th = tname.GetValueOrDefault(tid, "tid" + tid);
    if (!th.StartsWith(prefix)) continue;
    var start = a.StartTime.DateTimeOffset;
    var stop = a.StopTime.DateTimeOffset;
    if (stop < from || start > to) continue;
    // CPU per second of the window.
    for (var s = start > from ? start : from; s < (stop < to ? stop : to);)
    {
        long sec = (long)Math.Floor((s - from).TotalSeconds);
        var next = from.AddSeconds(sec + 1);
        var end = next < stop ? next : stop;
        if (!perSec.TryGetValue(th + " " + tid, out var m)) perSec[th + " " + tid] = m = new();
        m[sec] = m.GetValueOrDefault(sec) + (end - s).TotalMilliseconds;
        s = end;
    }
    double w = (double)(a.WaitingDuration?.TotalMilliseconds ?? 0);
    double ready = (double)(a.ReadyDuration?.TotalMilliseconds ?? 0);
    if (start >= from && start <= to)
    {
        var key = th + " " + tid;
        var t = totals.GetValueOrDefault(key, (0, 0, 0, new SortedDictionary<int, double>()));
        int cpu = a.Processor;
        var q = a.SwitchIn?.ThreadQualityOfService.ToString() ?? "?";
        qos[th + " " + q] = qos.GetValueOrDefault(th + " " + q) + (stop - start).TotalMilliseconds;
        t.cpus[cpu] = t.cpus.GetValueOrDefault(cpu) + (stop - start).TotalMilliseconds;
        totals[key] = (t.run + (stop - start).TotalMilliseconds, t.ready + ready, t.wait + w, t.cpus);
    }
    if (w + ready >= minWait && start >= from && start <= to)
    {
        string by = (a.ReadyingProcess?.ImageName ?? "?") + " tid " + (a.ReadyingThread?.Id.ToString() ?? "?");
        if (a.ReadyingThread is { } rt && a.ReadyingProcess?.Id == pid) by = tname.GetValueOrDefault(rt.Id, "tid" + rt.Id);
        waits.Add((start, th + " " + tid, w, ready, (stop - start).TotalMilliseconds, Where(a.SwitchIn?.Stack), by));
    }
}

// The whole machine in the window: CPU by process, and how busy each processor was.
var byProc = new Dictionary<string, double>();
var byCpu = new SortedDictionary<int, double>();
foreach (var sl in pendingSched.Result.CpuTimeSlices)
{
    var s0 = sl.StartTime.DateTimeOffset;
    var s1 = sl.StopTime.DateTimeOffset;
    if (s1 <= from || s0 >= to) continue;
    double ms = ((s1 < to ? s1 : to) - (s0 > from ? s0 : from)).TotalMilliseconds;
    string name = sl.Process?.ImageName ?? "?";
    if (name == "Idle") continue;
    byProc[name] = byProc.GetValueOrDefault(name) + ms;
    byCpu[sl.Processor] = byCpu.GetValueOrDefault(sl.Processor) + ms;
}
double windowMs = (to - from).TotalMilliseconds;
Console.WriteLine($"machine: busy % by processor: " + string.Join(" ", byCpu.Select(c => $"{c.Key}:{100 * c.Value / windowMs:F0}")));
Console.WriteLine("machine: CPU by process (% of one core): " + string.Join(", ", byProc.OrderByDescending(x => x.Value).Take(15).Select(x => $"{x.Key} {100 * x.Value / windowMs:F1}")));
Console.WriteLine("qos: ran ms by thread and QoS at switch-in: " + string.Join(", ", qos.Select(x => $"{x.Key} {x.Value:F0}")));
Console.WriteLine($"window {from:HH:mm:ss}..{to:HH:mm:ss}; CPU ms per second of the window:");
foreach (var (th, m) in perSec)
    Console.WriteLine($"  {th,-24} " + string.Join(" ", Enumerable.Range(0, (int)(to - from).TotalSeconds + 1).Select(i => ((int)m.GetValueOrDefault(i)).ToString().PadLeft(4))));
Console.WriteLine("\nper thread, activities starting in the window: ran / ready / blocked ms; ran ms by processor");
foreach (var (k, t) in totals)
    Console.WriteLine($"  {k,-24} ran {t.run,7:F0}  ready {t.ready,7:F0}  blocked {t.wait,7:F0}   cpus " + string.Join(" ", t.cpus.Select(c => $"{c.Key}:{c.Value:F0}")));
Console.WriteLine($"\nwaits (blocked + ready) of {minWait} ms or more that started running in the window:");
foreach (var w in waits.OrderBy(x => x.end))
    Console.WriteLine($"  {w.end:HH:mm:ss.fff} {w.th,-22} waited {w.ms,8:F0} ms, then ready {w.ready,8:F0} ms, ran {w.ran,6:F1} ms; woken by {w.by}\n        {w.where}");
```

- [ ] **Step 2: The runs** (the user, in an elevated window; about 20 minutes: each run's first 3½ minutes and its last one without using the machine, five minutes of normal use between, similar in both runs; never click inside the console, which pauses it):

```powershell
& C:\Users\jakef\Desktop\atlas-edr\spikes\run-d6-ab.ps1
```

It refuses each run unless the laptop is on AC power with the ASUS Silent plan (F10's conditions, F7). It changes on the host what plan 1b-4a-p1's script does, twice: the two Atlas sessions and a WPR trace while it runs; `C:\ProgramData\Atlas-D6`, `%TEMP%\atlas-d6-load` and `HKCU\Software\AtlasLoad`, all removed at the end; about 600 lookups of made-up `*.example.com` names; and the agent process's power-throttling state. Each run keeps a 4–9 GB trace in `spikes\results\d6-trace\`; the script refuses to start below 16 GB free.

```powershell
# D6 final re-measure: the trace script on the p1 build, then on the p2 build,
# back to back under the same conditions (AC power, the ASUS "Silent" plan,
# as F10 was measured on 2026-10-15). Elevated shell:
#   & C:\Users\jakef\Desktop\atlas-edr\spikes\run-d6-ab.ps1
# Each run is about 9 minutes (see run-d6-trace.ps1 for its phases and what it
# changes on the host); each refuses to start unless the laptop is on AC with
# the Silent plan active. Do not click inside this console while it runs.
$ErrorActionPreference = 'Stop'
$silent = '64a64f24-65b9-4b56-befd-5ec1eaced9b3'

function Check-Power([string]$when) {
    $ac = (Get-CimInstance Win32_Battery).BatteryStatus -eq 2
    $scheme = (powercfg /getactivescheme) -join ' '
    $line = '{0} {1}: AC={2}; {3}' -f (Get-Date -Format 'HH:mm:ss'), $when, $ac, $scheme
    Write-Host $line -ForegroundColor Cyan
    Add-Content -Path (Join-Path $PSScriptRoot 'results\d6-trace\ab-power.txt') -Value $line
    if (-not $ac) { throw "not on AC power ($when): plug the charger in and run this line again" }
    if ($scheme -notmatch $silent) { throw "the Silent plan is not active ($when): switch to Silent in Armoury Crate and run this line again" }
}

foreach ($build in @('d6-target-p1', 'd6-target-p2')) {
    Check-Power "before $build"
    & (Join-Path $PSScriptRoot 'run-d6-trace.ps1') -build $build
    Check-Power "after $build"
}
Write-Host 'Both runs done.' -ForegroundColor Green
```

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
    # D6 R8: HighQoS for every build, so Windows' window-based EcoQoS cannot
    # starve one run and not another (2026-10-27). A build with R8 shows 1/0
    # before already.
    $q = & $load qos $agent.Id
    if ($LASTEXITCODE -ne 0) { throw "d6load qos failed: $q" }
    Say $q
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

- [ ] **Step 3: The comparison** (Claude): per phase, events, CPU per thread and the working set (plan 1b-4a-p1's `compare.py`); the Sensor Health sums and Launches without a hash (`health.py`); module loads without a hash per 5 s (`misses.py`); emitted events by kind and actor (`mix.py`); SetAttributes by actor and path (`setattr.py`); and each run's quality of service and ready time from its trace (`d6wait`, with `_NT_SYMBOL_PATH` set to the build's PDB folder and Microsoft's symbol server):

```powershell
python -I spikes/p1-plan-src/compare.py spikes/results/d6-trace/<old> spikes/results/d6-trace/<new>
python -I spikes/p2-plan-src/health.py spikes/results/d6-trace/<old> spikes/results/d6-trace/<new>
spikes/d6wait/bin/Release/net9.0/d6wait.exe <trace.etl> <agent pid> <threads.csv> <from> <to> 100000 atlas-
```

```
"""Sums of the Sensor Health reports' counters, and launches without a hash, per run.
Usage: python -I health.py RUN_DIR [RUN_DIR ...]"""
import collections
import json
import os
import sys

for run in sys.argv[1:]:
    sums = collections.Counter()
    reports = 0
    launches = collections.Counter()
    ws = []
    for line in open(os.path.join(run, "dump.jsonl"), encoding="utf-8"):
        d = json.loads(line)
        if "sensorHealth" in d:
            reports += 1
            r = d["sensorHealth"]["report"]
            for group in ("loss", "quality", "housekeeping"):
                for k, v in r.get(group, {}).items():
                    if isinstance(v, str) and v.isdigit():
                        sums[f"{group}.{k}"] += int(v)
            ws.append(int(r["resources"]["workingSet"]) // 2**20)
        elif "process" in d and "launch" in d["process"]:
            f = d["process"]["launch"]["process"].get("file", {})
            launches["with hash" if f.get("hashes") else "WITHOUT hash"] += 1
    print(f"##### {os.path.basename(run)}: {reports} reports; working set MB by report {ws}")
    print("   launches:", dict(launches))
    keep = [k for k in sums if k.startswith("loss.") or k in (
        "quality.enrichmentMisses", "quality.enrichmentErrors", "quality.unknownFileObject",
        "quality.registryUnresolved", "housekeeping.hashCacheEvictions", "housekeeping.serviceQueueDrops",
        "housekeeping.pendingOverflow", "housekeeping.seederHandlesNamed", "housekeeping.seederTableReads")]
    print("   " + "  ".join(f"{k.split('.')[1]}={sums[k]}" for k in keep if sums[k] or k.startswith("quality.enrich")))
```

```
"""Module loads and launches without a hash, per 5 s of the run, and the files.
Usage: python -I misses.py RUN_DIR"""
import collections
import contextlib
import io
import json
import os
import sys

sys.path.insert(0, r"C:\Users\jakef\Desktop\atlas-edr\spikes\p1-plan-src")
with contextlib.redirect_stdout(io.StringIO()):
    import compare  # noqa: E402

run = sys.argv[1]
ph = compare.phases(run)
t0 = ph[0][1]
loads = collections.Counter()
bare = collections.Counter()
bare_launch = collections.Counter()
files = collections.Counter()
seen = set()
first_bare = collections.Counter()
for line in open(os.path.join(run, "dump.jsonl"), encoding="utf-8"):
    d = json.loads(line)
    t = int(d["time"]) / 1e9
    b = int((t - t0) // 5) * 5
    if "module" in d and "load" in d["module"]:
        f = d["module"]["load"]["file"]
        loads[b] += 1
        p = f.get("path", "?").lower()
        if not f.get("hashes"):
            bare[b] += 1
            files[os.path.basename(p)] += 1
            if p not in seen:
                first_bare[b] += 1
        seen.add(p)
    elif "process" in d and "launch" in d["process"]:
        if not d["process"]["launch"]["process"].get("file", {}).get("hashes"):
            bare_launch[b] += 1
print(f"phases from quiet start: synth at {ph[1][1] - t0:.0f} s, normal at {ph[2][1] - t0:.0f} s")
for b in sorted(set(bare) | set(bare_launch)):
    print(f"  t={b:4d}s  module loads {loads[b]:5d}  without hash {bare[b]:5d} (first sight {first_bare[b]:4d})  launches without hash {bare_launch[b]}")
print("files:", files.most_common(12))
```

```
"""Emitted events per phase by class/action and by actor. Usage: python -I mix.py RUN_DIR"""
import collections
import contextlib
import io
import json
import os
import sys

sys.path.insert(0, r"C:\Users\jakef\Desktop\atlas-edr\spikes\p1-plan-src")
with contextlib.redirect_stdout(io.StringIO()):
    import compare  # noqa: E402

run = sys.argv[1]
ph = compare.phases(run)
kinds = {p[0]: collections.Counter() for p in ph}
actors = {p[0]: collections.Counter() for p in ph}
for line in open(os.path.join(run, "dump.jsonl"), encoding="utf-8"):
    d = json.loads(line)
    t = int(d["time"]) / 1e9
    name = next((n for n, a, b in ph if a <= t < b), None)
    if not name:
        continue
    cls = next(k for k in d if k not in ("eventId", "time", "sensor", "device"))
    body = d[cls]
    act = next((k for k in body if k not in ("actor",) and isinstance(body[k], dict) and k not in ("file", "process", "key", "value", "connection")), "?")
    kinds[name][f"{cls}.{act}"] += 1
    a = body.get("actor", {})
    actors[name][f'{a.get("pid")} {a.get("file", {}).get("name", "?")}'] += 1
for n, a, b in ph:
    secs = b - a
    print(f"== {n} ({secs:.0f} s): {sum(kinds[n].values()) / secs:.0f} emitted/s")
    print("   kinds:  " + ", ".join(f"{k} {v / secs:.0f}/s" for k, v in kinds[n].most_common(6)))
    print("   actors: " + ", ".join(f"{k} {v / secs:.0f}/s" for k, v in actors[n].most_common(5)))
```

```
"""SetAttributes per phase: by actor and by path. Usage: python -I setattr.py RUN_DIR"""
import collections
import json
import os
import sys

sys.path.insert(0, r"C:\Users\jakef\Desktop\atlas-edr\spikes\p1-plan-src")
import compare  # noqa: E402

run = sys.argv[1]
ph = compare.phases(run)
actors = {p[0]: collections.Counter() for p in ph}
paths = {p[0]: collections.Counter() for p in ph}
per_sec = {p[0]: collections.Counter() for p in ph}
for line in open(os.path.join(run, "dump.jsonl"), encoding="utf-8"):
    d = json.loads(line)
    if "file" not in d or "setAttributes" not in d["file"]:
        continue
    t = int(d["time"]) / 1e9
    name = next((n for n, a, b in ph if a <= t < b), None)
    if not name:
        continue
    a = d["file"].get("actor", {})
    actors[name][f'{a.get("pid")} {a.get("file", {}).get("name", "?")}'] += 1
    paths[name][d["file"]["file"].get("path", "").lower()] += 1
    per_sec[name][int(t - next(a for n, a, b in ph if n == name))] += 1
for n, _, _ in ph:
    total = sum(actors[n].values())
    print(f"== {n}: {total} SetAttributes, peak {max(per_sec[n].values(), default=0)}/s")
    for k, v in actors[n].most_common(6):
        print(f"   actor {v:6d}  {k}")
    for k, v in paths[n].most_common(8):
        print(f"   path  {v:6d}  {k}")
    busy = sorted(per_sec[n].items())
    print("   seconds with >100:", [s for s, c in busy if c > 100][:40])
```

Expected: both runs at high QoS throughout (the new build shows `1/0` before `d6load qos`); under the synthetic load no Launch without a hash and enrichment misses near 0; the seeder 0 and the pipeline lower in the new build; its working set under 80 MB. The figures go to §13 and §15.3 (Task 15). On the verify-first code: F8.

### Task 15: Documentation

- [ ] **Step 1: The spec.** In `docs/specs/2026-10-01-etw-sensor-design.md`:
  - the status line (its date is the build's merge day: change `2026-10-27` if it differs) and table E13;
  - §3.2: the diagram, the service lanes (R3a, R4), the benchmark (R3), the driver's cadences (R3), the sink and the queues (R6b), the ordering stage's memory (R6b);
  - §6.3: Launch images first, the deadline kept, invalidations by the pipeline thread;
  - §7.2: the 8.3 cache's invalidation list;
  - §7.4: on-miss seeding off, and the address history;
  - §9.3: the `sizes=` line;
  - §11.2: the seeding default; §11.4: no power throttling;
  - §13: the re-measured figures (Task 14's replace F8's if they differ);
  - §15.3: a "Plan 1b-4a-p2" addendum with F1–F10;
  - §17: "Plan 1b-4a-p2 clarifications" (and, after the review, "review fixes").

  `docs/architecture-overview.md` (roadmap row 1, "plan 1b-4a-p2 done", and a decision-log row for the build) is a second commit, `docs(1): plan 1b-4a-p2 built: roadmap and decision log`, made once Task 14's figures are in.

```diff
--- a/docs/specs/2026-10-01-etw-sensor-design.md
+++ b/docs/specs/2026-10-01-etw-sensor-design.md
@@ -1,6 +1,6 @@
 # Sub-project 1 — ETW Sensor Design (Agent Core)
 
-**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15); the rest of the profiling step is next, then plan 1b-4b (blinding detection), then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
+**Status:** Approved (2026-10-02), revision 3. Revision 2 was approved on 2026-10-01; revision 3 folds the spike results (§15.3) and the decisions made during the spikes into the design sections (§17). Revision 1 had an independent review; its findings are folded in. Implementation: **plan 1a** (spikes S1–S10, §15.2) is done; **plan 1b** (the build) comes in six parts, each reviewed and approved before it runs (decision log, 2026-10-02; plan 1b-3 was split in two on 2026-10-05, and a plan 1b-3c added on 2026-10-06); plan 1b-1 (schema additions + `atlas-buffer`) is done (2026-10-04); plan 1b-2 (`atlas-etw`) is done (2026-10-05); plan 1b-3a (the `atlas-agent` pipeline core) is done (2026-10-05); plan 1b-3b (the Windows services behind it) is done (2026-10-06); plan 1b-3c (the driver, the agent-level live test, and deletes from the Cleanup outcome) is done (2026-10-06); plan 1b-4 is split in three (decision log, 2026-10-12): plan 1b-4a (a runnable agent: `run`, `dump`, `agent.toml`, the buffer as the sink, Sensor Health reports) is done (2026-10-13); the profiling step that comes before 1b-4b (decision log, 2026-10-12, D6) produced plan 1b-4a-p1 (the hash cache no longer forgets a file on a SetAttributes; the events each session delivers and each thread's CPU are measured), done (2026-10-15), and plan 1b-4a-p2 (on-miss seeding off; the pipeline's measured fixes; cache invalidations that wake no thread; Launch images hashed first; each bounded structure's size in the log; memory that follows contents; no power throttling), done (2026-10-27); the hour run that ends the profiling step is next, then plan 1b-4b (blinding detection), then 1b-4c (the service and acceptance). Brainstorm handoff: [etw-sensor-brainstorm-notes](2026-10-01-etw-sensor-brainstorm-notes.md).
 **Depends on:** 0a (event schema: domain types, `process_uid`, validating `TryFrom`), 0b (CI, nightly fuzz workflow).
 **Depended on by:** sub-project 2 (adds enrollment + gRPC on top of the buffer's read API), sub-project 3 (fills the agent-side detection hook), sub-project 6 (the driver becomes a second sensor feeding the same pipeline).
 
@@ -44,7 +44,7 @@ Build the **agent core**: a Rust user-mode agent that collects Windows telemetry
 | E9 | Sensor health monitoring in this sub-project: watchdog, canary, health records in the buffer | Blinding is a standard intrusion step; silent loss makes an EDR untrustworthy. |
 | E11 | Never drop an event for lack of context: if the actor's uid is computable, emit with what is known (empty path/name) and count it; drop and count only when no uid can be computed. Carve-out (rev 3): a file write whose handle never resolves to a path is counted, not emitted (§7.1) | Incomplete telemetry beats missing telemetry; dropping creates a blind spot an attacker can aim at (very short-lived processes, activity right after a blinding). |
 | E12 | Registry value data is read by the agent after the SetValueKey event and flagged as read after (§7.5). A fast path in Session A's callback reads within about 0.1–0.3 s; otherwise the ordered path reads about 1 s after the write | Kernel-Registry never fills `CapturedData` (S4), and value data is what registry detections key on. Racy by design; type and length checks catch most stale reads; the driver (sub-project 6) later supplies exact data. Re-decided after the rev 3 review showed the ordered path alone reads about 1 s late. Rejected: ordered path only; no data. |
-| E13 | The `FileObject` and `KeyObject` maps are seeded from the system handle table at start and on a miss, with snapshots applied in stream-time order and a negative cache for handles that cannot be named (§7.4); unresolvable registry paths are emitted with a `path_unresolved` flag | Neither provider has a rundown, and their names are relative to handles (S6, S7). The handle table's object addresses equal the ETW object fields (verified). Rejected: keeping it a documented gap; dropping (contradicts E11). |
+| E13 | The `FileObject` and `KeyObject` maps are seeded from the system handle table at start and on a miss (on a miss: off by default since plan 1b-4a-p2), with snapshots applied in stream-time order and a negative cache for handles that cannot be named (§7.4); unresolvable registry paths are emitted with a `path_unresolved` flag | Neither provider has a rundown, and their names are relative to handles (S6, S7). The handle table's object addresses equal the ETW object fields (verified). Rejected: keeping it a documented gap; dropping (contradicts E11). |
 | E14 | A process-launch canary joins the registry and file canaries (§9.2) | It costs 0.082% of one core with Defender on (S8), within the 0.1% threshold. |
 
 ## 3. Architecture
@@ -81,11 +81,11 @@ ETW (Session A: manifest providers; Session B: system logger, process events)
       then, at emission: self-filter on the resolved actor                  │
       → domain Events (some pending: see "Pending events")                  │
    │              ▲ seeder replies         [4] workers (2, below-normal) ◄──┘
-   │              │                            SHA-256 + Authenticode (§6.3)
+   │              │                            SHA-256 + Authenticode (§6.3), Launch images first
    │              │                            + 1 reader lane: registry value reads (§7.5)
    │              │                            + 1 expander lane: 8.3 name expansion (§7.2)
    │         [8] seeder (below-normal): names key and file handles from the
-   │             system handle table at start and on a miss (§7.4)
+   │             system handle table at start, and on a miss if enabled (§7.4)
 [5] completion stage: emits in order; a pending event holds the line until
       completed, cancelled, or past its deadline, then goes as-is
    │
@@ -114,18 +114,23 @@ ETW (Session A: manifest providers; Session B: system logger, process events)
 
 **Seeding.** Handles opened before the sessions started are never named by ETW (§7.4). The seeder [8] names them from the system handle table. At start the sessions start first and the seeder runs after, so no handle falls between the snapshot and the first event. Each snapshot is stamped with its QPC time, and [3] applies it in stream-time order (§7.4).
 
-**Service lanes** (plan 1b-3b). The hash workers, the reader lane, the expander lane and the seeder each take at most 8,192 waiting requests. A full lane drops the request and counts `service_queue_drops`: a dropped work request costs its event's deadline, and a dropped invalidation clears the expansion cache. The reader lane does no file I/O, so a stalled directory never delays a value read. The pipeline thread takes no lock to hand work out.
+**Service lanes** (plan 1b-3b). The hash workers, the reader lane, the expander lane and the seeder each take at most 8,192 waiting requests; the hash workers have two queues of 8,192, Launch images taken before module loads (plan 1b-4a-p2, R4). A full lane drops the request and counts `service_queue_drops`: a dropped work request costs its event's deadline. The reader lane does no file I/O, so a stalled directory never delays a value read.
+- **Invalidations wake no thread** (plan 1b-4a-p2, R3a). Waking a parked worker costs about 22 µs of CPU, and under load invalidations are the most frequent request. The hash cache's go on a channel the hash workers drain before their next lookup, so the pipeline thread takes no lock a below-normal worker could hold (review finding 4); the cache's key holds the USN, so they only free memory, and past 8,192 waiting the newest is dropped. The 8.3 cache's invalidations wait in a shared list (at most 8,192) that the expander applies before its next expansion; a list that overflows clears the expansion cache instead, and keeps nothing more until then.
+- So to hand work out the pipeline thread takes only short locks, shared with threads of its own priority or for a queue operation: the hash lane's and the expander's list's.
 
 **One thread for [2], [3] and [5]** (plan 1b-3a, D2). The three stages are one loop on the pipeline thread, with the clock passed in: the driver pushes queued events and worker replies, and calls `tick(now)` at least every 50 ms, which returns the events to emit in order. Measured at 0.57 µs per event in steady state, and 0.91 µs at 52,000 events/s of never-repeated addresses.
+- **Plan 1b-4a-p2 (R3)** measured the three with a replay benchmark (`bench_pipeline_per_event`, an ignored `atlas-agent` test that replays the CI recording 300 times, its kernel objects moved each round, with the driver's 10 ms ticks; the best of 10 runs): 0.573 µs per event, then 0.385 with foldhash for the maps keyed by kernel addresses, PIDs and ids (maps whose keys an outsider chooses, paths and network endpoints, keep SipHash), the ordering stage's heap holding keys only while events wait in reused slots, and an all-ASCII name narrowed to UTF-8 in one allocation. A literal-run pre-check before the watchlist's `GlobSet` (§7.2) took a typical path from 220 to 95 ns. With on-miss seeding off, the seeding address history is kept only until the start-up passes are applied (§7.4).
+- **Memory** (R6b): after a burst, the ordering stage compacts its slots once fewer than an eighth hold an event, never below 8,192 (about 1.4 MB); without this a burst's slots stayed (131,072 after the host's synthetic load).
 
-**The driver** (plan 1b-3c, D1, D4). The pipeline thread runs one pass every 10 ms: drain both queues into the pipeline, feed in the services' replies, `tick` with the current QPC, hand the new requests to the services, and pass the emitted events, in order, to the sink (the buffer writer, plan 1b-4). It re-takes the anchor every 60 s (§3.3). The loop is portable and tested with fakes, including the CI recording, which it turns into exactly the events of the synchronous replay. The loop ends when both queues are closed: it applies the replies already in and stops the pipeline (§11.4). It owns the services, so they go after the callbacks' senders and Session A's `FastRead`.
+**The driver** (plan 1b-3c, D1, D4). The pipeline thread runs one pass every 10 ms, or 40 ms after a pass that found no event (plan 1b-4a-p2, R3: on a quiet machine ETW delivers once a flush period, so most 10 ms passes found nothing; `tick` still comes at least every 50 ms): drain both queues into the pipeline, feed in the services' replies, `tick` with the current QPC, hand the new requests to the services, and pass the emitted events, in order, to the sink (the buffer writer, plan 1b-4). It re-takes the anchor every 60 s (§3.3). The loop is portable and tested with fakes, including the CI recording, which it turns into exactly the events of the synchronous replay. The loop ends when both queues are closed: it applies the replies already in and stops the pipeline (§11.4). It owns the services, so they go after the callbacks' senders and Session A's `FastRead`.
 - Stream time advances to each released event before that event is processed. Seeder snapshots, confirm windows and Launch halves that fall due by then are applied first.
 - Sweeps over whole tables (UDP idle, cache retention) run once per second of stream time.
 - A pending event's request bookkeeping is freed when the event leaves [5], whether or not its reply came; a later reply is ignored. Each request gets at most one reply.
 
-**The sink and the health thread** (plan 1b-4a, D1, D5). The sink moves each emitted event into a bounded channel (65,536 entries) to the buffer writer thread [6] and never blocks: a full channel drops the event and counts it in `loss.buffer_backlog_drops` (§10.3). [6] encodes and appends each event and calls the writer's `tick` about every 100 ms (at most about 200: a wait for the first record, then a drain as long; the writer flushes every second); `tick` may block on the disk while the channel absorbs events. Thread [7] is, for now, the health thread: every 5 s it takes a snapshot of every counter (the pipeline's through the driver's control channel, between two passes), and every 60 s, or at once when a loss counter rose, it sends [6] a Sensor Health report (§9.3). Plan 1b-4b adds the watchdog's checks to it.
+**The sink and the health thread** (plan 1b-4a, D1, D5). The sink moves each emitted event into a queue of at most 65,536 events to the buffer writer thread [6] and never blocks: a full queue drops the event and counts it in `loss.buffer_backlog_drops` (§10.3). [6] encodes and appends each event and calls the writer's `tick` about every 100 ms (at most about 200: a wait for the first record, then a drain as long; the writer flushes every second); `tick` may block on the disk while the channel absorbs events. Thread [7] is, for now, the health thread: every 5 s it takes a snapshot of every counter (the pipeline's through the driver's control channel, between two passes), and every 60 s, or at once when a loss counter rose, it sends [6] a Sensor Health report (§9.3). Plan 1b-4b adds the watchdog's checks to it.
 
-**Backpressure.** The ETW callback must stay fast: a slow consumer makes ETW drop events in the kernel. The callback parses, enqueues and keeps the small early registry map (a hash-map operation per CreateKey, OpenKey or CloseKey). Successful OperationEnds, about half of Kernel-File's volume, are discarded right there: they cost parse time but never reach [2]. The exceptions are a Cleanup's OperationEnd that reports a delete, and a delete request's (§5.1, plan 1b-3c): the callback remembers each Cleanup's and each request's `Irp` until its OperationEnd, one or two hash-map operations per Kernel-File event. Queues never block (kernel queue default 65,536 entries; user-mode queue 8,192); drops are counted per queue. User-mode providers get their own queue so a process flooding forged DNS-Client events (§4.4) cannot push kernel events out.
+**Backpressure.** The ETW callback must stay fast: a slow consumer makes ETW drop events in the kernel. The callback parses, enqueues and keeps the small early registry map (a hash-map operation per CreateKey, OpenKey or CloseKey). Successful OperationEnds, about half of Kernel-File's volume, are discarded right there: they cost parse time but never reach [2]. The exceptions are a Cleanup's OperationEnd that reports a delete, and a delete request's (§5.1, plan 1b-3c): the callback remembers each Cleanup's and each request's `Irp` until its OperationEnd, one or two hash-map operations per Kernel-File event. Queues never block (kernel queue default 65,536 entries; user-mode queue 8,192); drops are counted per queue.
+- **Memory follows contents** (plan 1b-4a-p2, R6b). std's bounded channel allocates and stamps every slot when it is made, so the sink's 65,536 events of 688 bytes (about 45 MB) and the kernel queue (about 11.5 MB) were resident from start-up. The sink and both event queues are std's unbounded channel, which allocates blocks of 31 messages and frees each once read, with the cap kept by a counter: a full queue refuses an event exactly as before, and the drop is counted the same way. User-mode providers get their own queue so a process flooding forged DNS-Client events (§4.4) cannot push kernel events out.
 
 ### 3.3 Time base
 
@@ -290,11 +295,11 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 ### 6.3 Hashes and signatures
 
 - Applies to `process.file` on Launch and `module.file` on Module Load.
-- **Cache key:** volume serial + 128-bit file ID (`FILE_ID_INFO`) + **the file's USN** (`FSCTL_READ_FILE_USN_DATA`). The USN changes on every modification and, unlike last-write time, cannot be set back by a user. On volumes without a USN journal, results are not cached. Entries for a path are also invalidated when the pipeline sees an Update, Rename or Delete on it. A SetAttributes does not invalidate (plan 1b-4a-p1, D6): attributes and timestamps change neither a file's content nor its name, and the System process sets the basic information of the DLLs that starting processes load, so invalidating there re-hashed them on every launch.
-- **Work** runs on 2 below-normal-priority worker threads, never on an ETW or pipeline thread. Files are opened with full sharing (`FILE_SHARE_READ | WRITE | DELETE`) and read sequentially.
+- **Cache key:** volume serial + 128-bit file ID (`FILE_ID_INFO`) + **the file's USN** (`FSCTL_READ_FILE_USN_DATA`). The USN changes on every modification and, unlike last-write time, cannot be set back by a user. On volumes without a USN journal, results are not cached. Entries for a path are also invalidated when the pipeline sees an Update, Rename or Delete on it; a hash worker applies them before its next lookup (§3.2, plan 1b-4a-p2). A SetAttributes does not invalidate (plan 1b-4a-p1, D6): attributes and timestamps change neither a file's content nor its name, and the System process sets the basic information of the DLLs that starting processes load, so invalidating there re-hashed them on every launch.
+- **Work** runs on 2 below-normal-priority worker threads, never on an ETW or pipeline thread. Launch images are taken before module loads (plan 1b-4a-p2, R4): while the cache is cold a Launch would otherwise wait behind dozens of module loads and could miss its deadline (plan 1b-4a-p1, F9). Files are opened with full sharing (`FILE_SHARE_READ | WRITE | DELETE`) and read sequentially.
 - **SHA-256** for files up to the size cap (default 100 MB); larger files skip hashing.
 - **Signature:** `WinVerifyTrust` with `WTD_UI_NONE`, `WTD_REVOKE_NONE`, `WTD_CACHE_ONLY_URL_RETRIEVAL` (no network calls from the sensor), and catalog lookup (`CryptCATAdmin*`) for catalog-signed OS files. `signer` = the leaf certificate's subject CN. Mapping: valid chain → `Valid`; no signature (`TRUST_E_NOSIGNATURE`) → `Unsigned`; a signature that fails verification (bad digest, untrusted root, explicit distrust, revoked per local cache) → `Invalid`; any operational error (file locked, CryptSvc unavailable, timeout) → signature **absent**, counted.
-- **Deadline:** the completion stage's (§3.2), 1 s. A late result is cached for the next event on the same file. S10 measured 38 first launches with no miss (p99 540 ms), so the deadline stays.
+- **Deadline:** the completion stage's (§3.2), 1 s. A late result is cached for the next event on the same file. S10 measured 38 first launches with no miss (p99 540 ms), so the deadline stays. Plan 1b-4a-p2 (R4) kept it at 1 s for start-up too: the completion stage emits in order, so a longer deadline would hold every later event (about 80,000 in 5 s at 16,000 events/s, near the 100,000 pending cap) exactly when the workers fall behind.
 - **Plan 1b-3b:**
   - The file is opened by `\\?\GLOBALROOT` + its NT path, for reading, with full sharing and backup semantics. One handle gives the key, the hash and the signature.
   - While some handle can write the file, the cache is neither read nor written: writes through a handle that stays open do not change the USN. An open for reading that refuses write sharing tells, without blocking anyone. A result is also cached only if the USN is the same after the reads.
@@ -345,7 +350,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 - **8.3 names.** A Create opened through a short name logs the short form (S6), for example `…\ATLASS~1\…\LONG-F~1.TXT`.
   - A path with any component matching the 8.3 pattern (`^[^.~]{1,6}~[0-9]+(\.[^.]{0,3})?$`, case-insensitive) is expanded by a worker [4]. The Open is pending meanwhile (deadline §3.2).
   - Each short component is looked up in its parent directory, opened by NT path, with `NtQueryDirectoryFile` (`FileBothDirectoryInformation`) and the short name as the filter, about 54 µs each; the answer counts only if its short or long name equals the component exactly. `GetLongPathNameW` cannot do this: it rejects `\\?\GLOBALROOT\Device\…` paths (plan 1b-3b, F1). This works for shadow copies too. Only `\Device\HarddiskVolume…` paths are expanded, on a lane of their own (the expander lane).
-  - Results are cached by (parent directory, short name). Short names are reused after a delete or rename, so the pipeline's `InvalidateHash` for a path drops the entries for that path and everything under it (sent on an Update, Rename or Delete; not on a SetAttributes, which frees no name, plan 1b-4a-p1). Short components in that path are resolved from the cache, and one it cannot resolve clears the whole cache. An invalidation dropped because the lane was full also clears it. Entries expire after 60 s (plan 1b-3b, D4).
+  - Results are cached by (parent directory, short name). Short names are reused after a delete or rename, so the pipeline's `InvalidateHash` for a path drops the entries for that path and everything under it (sent on an Update, Rename or Delete; not on a SetAttributes, which frees no name, plan 1b-4a-p1). Short components in that path are resolved from the cache, and one it cannot resolve clears the whole cache. Invalidations wait in a list the expander applies before its next expansion; one that overflows the list clears the cache too (plan 1b-4a-p2, R3a). Entries expire after 60 s (plan 1b-3b, D4).
   - The logged path is matched at once. An Open that matches neither form is dropped only after the expansion.
   - The emitted `file.path` is the expanded form when expansion succeeded, otherwise the logged one.
   - **Every emitted file path is expanded the same way** (plan 1b-3a, D4), not only a watchlist Open's: Create, Update, Delete, SetAttributes, both paths of a Rename, and the images of Launch and Module Load. An event with a short component waits for its expansion (deadline §3.2). The handle map keeps the expanded path, and a Launch's expanded image also updates the cached process; actor references built before the reply keep the short form. On the CI runner 13% of file events had a short component (`C:\Users\RUNNER~1\…`, from `TEMP`).
@@ -387,6 +392,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
 - **When.**
   - At start, after the sessions are live: keys first (about 140 ms on the host; 205–363 ms with plan 1b-3b's verification), then files (about 1.2 s; 0.5–1.0 s in plan 1b-3b).
   - Then on a miss: an unknown `KeyObject`/`BaseObject`, or a Write, truncation, rename or attribute change on an unknown `FileObject` (§7.1).
+  - **On-miss seeding is off by default** (plan 1b-4a-p2, R2; §13's first fallback). After start-up it spent its whole budget, 1% of one core, every minute, quiet ones included (two table reads per snapshot, about 70 ms each, mostly inside the kernel's handle-table walk), and named about 20 handles in 9 minutes against about 1,500 dropped file events and 180 unresolved registry events. `on_miss = true` under `[seeding]` in `agent.toml` turns it back on.
   - Only an address absent from the latest snapshot triggers a re-read. Misses are batched, the table is re-read at most once per second, and the seeder keeps to a CPU budget (default 1% of one core averaged over 60 s; beyond it, re-reads are deferred and counted).
 - **Negative cache.** Addresses that are in the snapshot but cannot be named (protected processes, failed or timed-out queries) go into a negative cache. They stay there until a CreateKey, OpenKey or CloseKey (for files: a Create or Close) for that address is seen, or the owning process exits. Events on them are emitted unresolved at once, without waiting. Without this cache, writes through protected processes' handles would force constant re-reads (S6/S7 saw 955 key and 2 174 file handles in processes the agent cannot open).
 - **Cost** (host, all processes): keys, 15 156 handles, 42 ms to read the table + 98 ms to name, 93% named; files, 10 494 handles, 3 830 disk files named in 1.18 s (3 timeouts).
@@ -400,7 +406,7 @@ Querying `ProcessTelemetryIdInformation` for the new PID at Launch is **not** a
   - **Address reuse.** Key objects are per handle, so a successful CreateKey or OpenKey at an address is a new object there. Children waiting on that address belonged to the earlier object: they become orphans, which only the seeder naming the child handle itself can name. The same holds for the children of an unknown base that closes, and of a tombstone whose address the seeder names after the tombstone closed. Otherwise a reused address would give a key the path of an unrelated one.
   - **Coverage.** A snapshot carries the addresses it was asked about; the start-up pass covers the whole table. Every covered handle is in `named` or `unnamable` (non-disk files are unnamable). A covered address in neither was not in the table: it joins the negative cache until the address is used again, and the events waiting on it are answered at once.
   - **The reuse rule is conservative:** a seeded name answers an event only if no Create, Open or Close for the address came after the earlier of the event and the snapshot.
-  - **Bounds.** The address history and the negative cache are capped, and the history expires after four start-up seeder deadlines (20 s).
+  - **Bounds.** The address history and the negative cache are capped, and the history expires after four start-up seeder deadlines (20 s). With on-miss seeding off no snapshot comes after the start-up passes, so the history is kept only until they are applied (plan 1b-4a-p2, R3).
 - **Floor (E11).** A registry event whose key stays unresolved is emitted, not dropped, with `path_unresolved = true` (§10.4) and the longest name known: the chain of relative names below the first unresolved base, or empty if nothing is known. It counts `registry_unresolved`. Unresolved file entries follow §7.1.
 
 ### 7.5 Registry value reads (E12)
@@ -503,8 +509,8 @@ Emitted every 60 s (and immediately on a non-zero change in a loss counter):
 - **Snapshots and differences:** the health thread [7] reads every cumulative counter into a snapshot: the pipeline's (through the driver's control channel, waiting at most 2 s), the intake's and the services' atomics, the callbacks' panics and delivered events (counters the agent owns and hands to each consumer it starts, so a restarted session counts on into them; plan 1b-4a-p1, review M1), both sessions' losses (`ControlTraceW` query), the sink's drops, and the process's CPU time and working set. A report holds the differences between two snapshots. The sessions' counters are 32-bit, so their differences wrap. A source that could not be read is absent from the report, and the next report covers its interval too.
 - **When:** every 60 s; at once when a loss counter rose (checked every 5 s); and a final report at the clean stop, taken after the sessions stopped, so everything counted during the run is reported. The intervals chain: each starts where the last ended, the first at the anchor taken at start. A report's time is the system clock at generation (`GetSystemTimePreciseAsFileTime`, the clock the anchor reads, §3.3); its id is a UUIDv7. If the pipeline thread died, the final report is still sent, without the pipeline's counters (they died with it).
 - **The buffer's figures:** [6] fills in the `buffer` group, `loss.buffer_backlog_drops`, `housekeeping.retention_evictions` and the gap as it appends the report, each since the report before; so the drops of the events queued ahead of a report, the final drain's included, are in it (review finding 3). The health thread's own reading of the drops only decides an early report; the first report of a run carries what recovery found at open. A report the writer's backlog cannot take is held (up to 16) and written before anything else once there is room.
-- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums, and (plan 1b-4a-p1) the CPU each agent thread used since the last listing: each thread's difference (a thread is its id and creation time), summed by thread name, most first, with the listings' own interval. Threads are listed with a snapshot of every thread in the system (about 45 ms on the host), so at most once a report period, for a report of either kind; a report without them leaves them to the next listing. The final report's listing is taken just before the sensor stops, while its threads still run.
-- **Not measured yet** (absent): `quality.buffer_invalid_records` and `buffer.corrupt_segments` (the agent never reads its own buffer back; `dump` counts both), and `housekeeping.seeder_negative_cache_size` (the seeder exposes no size).
+- **The diagnostic log** gets one line per report with its main figures, including the two parts of `quality.file_cleanup_unpaired` (another operation came first on the `Irp`; the outcome came after the window), which the schema sums, and (plan 1b-4a-p1) the CPU each agent thread used since the last listing: each thread's difference (a thread is its id and creation time), summed by thread name, most first, with the listings' own interval. Threads are listed with a snapshot of every thread in the system (about 45 ms on the host), so at most once a report period, for a report of either kind; a report without them leaves them to the next listing. The final report's listing is taken just before the sensor stops, while its threads still run. Plan 1b-4a-p2 (R6) adds each bounded structure's entries and capacity, `sizes="name=entries/capacity …"`: the ordering stage, the completion stage's line and its pending events, the process cache, the file and key maps, the two `Irp` tables, the watchlist opens, the seeding address history, the negative cache, the TCP and UDP tables, the early key map, the hash and expansion caches, and the kernel, user and sink queues.
+- **Not measured yet** (absent): `quality.buffer_invalid_records` and `buffer.corrupt_segments` (the agent never reads its own buffer back; `dump` counts both), and `housekeeping.seeder_negative_cache_size` (the negative cache's size is in the diagnostic log's `sizes=` since plan 1b-4a-p2, not in the schema).
 
 ## 10. Schema Additions (`atlas.events.v1`, additive)
 
@@ -572,7 +578,7 @@ Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9
 
 ### 11.2 `agent.toml`
 
-- Keys: buffer cap, retention/overflow policy, `transport` (`none` in this sub-project), ordering hold, completion deadline, enrichment size cap, watchlist (replace or extend the default), per-class enable, `network.udp`, UDP idle timeout, DNS per-PID rate, the failure-confirm window (§5.5), `file.op_end` (on by default; off emits Create, Delete and Rename without the window), seeding on start and on miss (two settings, both on; without `SeDebugPrivilege` both are off; with on-miss off, events on unknown handles wait only for the start-up pass), the seeder CPU budget (§7.4), `registry.value_reads` (on by default), log level.
+- Keys: buffer cap, retention/overflow policy, `transport` (`none` in this sub-project), ordering hold, completion deadline, enrichment size cap, watchlist (replace or extend the default), per-class enable, `network.udp`, UDP idle timeout, DNS per-PID rate, the failure-confirm window (§5.5), `file.op_end` (on by default; off emits Create, Delete and Rename without the window), seeding on start and on miss (two settings: on start on, on miss off since plan 1b-4a-p2; without `SeDebugPrivilege` both are off; with on-miss off, events on unknown handles wait only for the start-up pass), the seeder CPU budget (§7.4), `registry.value_reads` (on by default), log level.
 - Built-in defaults; the file only overrides them. Validated at start: an invalid file stops startup with a clear error in the Windows Event Log (Application, source `Atlas`). No hot reload in v1.
 - **Plan 1b-4a (D5):** the file's sections, keys and ranges are documented in `atlas-agent`'s `settings` module. Unknown keys and sections are refused, and every error names its key. Every key above exists except per-class enable, which is deferred: it is the last §13 fallback, and a later plan adds it if the fallbacks reach it. Until the service exists (1b-4c), an invalid file is reported on the console and in the diagnostic log.
 
@@ -595,7 +601,7 @@ Plan 1b-1 also updates the 0a documents: the 0a spec's status line and its §5.9
 - **Clean stop:** flush both sessions → wait out the hold → stop the sessions, so the consumers deliver the rest and finish → drain the ordering and completion stages → flush the buffer (plan 1b-3c: the queues close when the consumers do, which ends the pipeline loop). No in-flight event is lost on a clean stop. Pending events go out as at their deadlines, so one marked "drop at deadline" is dropped (plan 1b-3a).
 - **Diagnostic log:** `tracing` with a rolling file in `logs\`; its writes are self-filtered (§5.5).
 - **Plan 1b-4a:**
-  - **Start:** `run` checks for an elevated token first. The diagnostic log starts once the config is loaded, at its level. The buffer opens before the sessions start, so the sink is ready for the first event, and the health thread starts last. The console handler is installed just before the sessions start, so Ctrl+C during start-up waits and then stops cleanly, instead of ending the process with the sessions running (review finding 6).
+  - **Start:** `run` checks for an elevated token first. Just before the sensor starts, the agent opts out of Windows' power throttling (plan 1b-4a-p2, R8): Windows otherwise lowers a background process's quality of service by its own heuristics (a minimized window, a quiet power plan), and its threads then wait for a slow core. The diagnostic log starts once the config is loaded, at its level. The buffer opens before the sessions start, so the sink is ready for the first event, and the health thread starts last. The console handler is installed just before the sessions start, so Ctrl+C during start-up waits and then stops cleanly, instead of ending the process with the sessions running (review finding 6).
   - **Clean stop:** the health thread, then the sensor (as above), then a final Sensor Health report covering the stop, then the buffer flushed and closed. On the host it took 1.0 s (§15.3). The same stop follows when the pipeline thread or the buffer writer thread died (review findings 2 and 7): neither's panic takes `run` down with it. A console close waits for the stop and for the log file's last lines.
   - **The log** goes to the console (stderr) and to `logs\atlas-agent.<date>.log`, rolled daily with seven files kept, through a non-blocking writer.
 
@@ -652,7 +658,9 @@ Late arrivals, enrichment misses and DNS rate-limit drops are reported, not gate
 
 **Profiled** (D6, 2026-10-15; a WPR CPU trace on the host, §15.3): the hash workers took two-thirds of the agent's CPU, most of it re-checking DLLs they had already checked (about half the agent's CPU), because each SetAttributes the System process makes on a loaded DLL made the cache forget it. Plan 1b-4a-p1 removed that invalidation (§6.3). Under the same trace script, five minutes of normal use at about 14,000 events/s went from 19.0% to 7.9% of one core (13.1 to 5.6 ms of CPU per 1,000 events), and the synthetic load from 17.6% to 12.9% (11.4% on the final code, F10). Still over budget: the hash workers' first-time checks, the pipeline (2.0%, spread over the ordering stage, allocation and the key map), on-miss seeding (0.8%, all handle-table snapshots) and the working set are profiled next, and the first-time checks can leave a launch early in a run without its hash (F9, §15.3); the hour run measures the result at the end of the profiling step.
 
-**Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.
+**Plan 1b-4a-p2** (D6, 2026-10-15 to 2026-10-27): on-miss seeding off (§13's first fallback), the pipeline's measured fixes, invalidations that wake no thread, Launch images first, memory that follows contents and no power throttling (§15.3). The old and the new build, back to back on the host under the same trace script, both on AC power, the Silent power plan and Windows' high quality of service: under the synthetic load, 11.4% → 10.5% of one core (without the hash workers 5.3% → 3.4%; the seeder 1.0% → 0), with 0 and 1 enrichment misses; the working set 109–130 MB → 40–66 MB, under budget. Five minutes of normal use took 2.2% of one core, most of it first-time hashes of DLLs the agent had not seen since it started; quiet minutes 1.4%. The hash workers vary from run to run (6.1% and 6.1% for the old build, 5.3% and 7.2% for the new under the same load). The hour run judges the workday figures; anything still over budget comes back as a decision.
+
+**Fallbacks if over budget** (applied in order, each one a decision-log entry): on-miss seeding off (start-up seeding stays; applied by plan 1b-4a-p2); then `file.op_end` off (Create, Delete and Rename are emitted without failure confirmation, counted); then UDP off; then File Update and SetAttributes off (drops the `FILEIO` and `WRITE` keywords, the largest kernel-side cost; Create/Delete/Rename/Open remain); then registry value reads off (`data_unavailable` on every Value Set); then per-class disable in config as a last resort.
 
 ## 14. Definition of Done
 
@@ -785,6 +793,18 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 - **F9, launches at start-up can miss their hash:** on the CI runner the agent live test's `cmd.exe` launch went out without its hash and signature in 2 of 9 runs on the same code (0 in the 33 runs before this plan). The live test's notes now record the enrichment counters: in four more runs of the same code, 0 errors every time and 0, 74, 50 and 0 enrichment misses (results past the 1 s deadline). So the hash workers sometimes fall over a second behind early in a run, when every file is checked for the first time (10–80 ms each, two workers), and a launch then goes out without its hash (the late result is cached for the next event on that file). This plan's fix only lowers that work; why it never showed before is not known (runner load; the earlier runs recorded no miss counts).
 - **F10, the final code** (Task 3's script, the build branch): the synthetic load 11.4% of one core (hash workers 5.9%, pipeline 3.1%, seeder 1.1%), against F4's 12.9%, the health thread now 0.05% against 0.85% (F5's listing change); quiet 3.3%. The normal use was lighter than F4's (6,521 events/s and 159 launches against 14,161 and 1,199), so its 4.7% is not compared with F4's 7.9%. All ten reports carried both sessions' received counts and a thread line, the final report too; nothing was lost of any kind; the clean stop took 1.5 s.
 
+**Plan 1b-4a-p2 (2026-10-15 to 2026-10-27): the rest of the profiling step** (the host, build 26200; the CI runner, 26100; the D6 notes have the details):
+- **F1, the seeder's yield:** after start-up, on-miss seeding spent its whole budget (600 ms per 60 s) every minute, 16–18 table reads a minute, 68% of it inside the kernel's `ExpSnapShotHandleTables`, and named about 20 handles in the 9 minutes after start against about 1,500 dropped file events and 180 unresolved registry events (R2).
+- **F2, the pipeline:** in a quiet minute half its time was `tick`, 29% the 10 ms loop's sleep (a waitable timer created per sleep: 18 µs per wake, 0.17% of one core idle); under load, allocation about 25%, the ordering stage 16%, worker wakes about 10%. Live it costs about 1.9 µs per event against the benchmark's 0.58: the benchmark has no real lookups, worker wakes, sink or tracing (R3).
+- **F3, a worker wake** (an unelevated probe, 2,000 jobs 5 ms apart): handing a job to a parked worker costs about 22 µs of CPU, 6–7 µs for the sender and 15–16 µs for the worker's wake and park; at the synthetic load's 622 jobs a second, about 1.4% of one core, most of them invalidations (R3a).
+- **F4, the completion stage holds the line:** it emits strictly in order, so one event waiting on a longer deadline holds every later one (R4).
+- **F5, the kernel's own signature verdict** (the MOF image-load event's `SignatureLevel` and `SignatureType`) cannot replace `WinVerifyTrust`: it gives no signer's name.
+- **F6, the working set** (the first host run of the new build, with R6's `sizes=` line): std's bounded channels made the sink (about 45 MB) and the kernel queue (about 11.5 MB) resident from start-up, and the ordering stage's slots grew to 131,072 after the synthetic load and stayed. With R6b the working set went from 107–151 MB to 43–75 MB, and no cap needed changing.
+- **F7, power plans and quality of service:** the laptop's vendor tool switches between its Silent and Turbo plans by itself; the replay benchmark ran 1.7 times slower on battery with Silent than on AC. F10 above and the final pair below ran on AC with Silent. In the first pair, Windows ran the new build's whole process at its Low quality of service (EcoQoS) for the whole run, by its own heuristics (the old build's run mostly at High): on the Silent plan its threads then waited for one of a few slow cores, each hash worker ready but not running for 30.5 s of a 40 s span under the synthetic load, while Defender scanned each fresh binary copy; 3,471 enrichment misses and 143 Launches without a hash followed. Hence R8, and the trace script now gives every build high QoS from outside.
+- **F8, the final pair** (both builds at high QoS for the whole run): see §13. The old build's quiet and normal phases received 17,000–25,000 events/s of background activity it filtered out (11–13 emitted a second), so its normal use does not compare.
+- **F9, not chased:** in both of the new build's runs that day, the System process set the attributes of the same 28 system files (`ntdll.dll`, `kernel32.dll`, the `.nls` tables, the volume root) at every launch, about 12,000–13,600 times in the synthetic load, against 430–530 in the old build's runs. The new build always ran second, after the user's normal use, and its first host run showed 210, so the order is as likely a cause as the build; they cost about 2 µs each.
+- **F10, SetAttributes kept** (R5): Windows' SMB server (`srv2.sys`) works in the System process, so a remote client's changes on this machine's shares also show as PID 4; a filter on PID 4 would hide their timestomping.
+
 ## 16. Known Limitations
 
 - **No boot-time coverage:** events before the agent starts (early boot, or while the service is stopped) are not captured. An AutoLogger session is sub-project 8.
@@ -943,3 +963,19 @@ Plan 1a ([2026-10-02-etw-sensor-spikes-plan](../plans/2026-10-02-etw-sensor-spik
 1. each session's callback counters are the agent's and are handed to every consumer it starts, so a restarted session (plan 1b-4b) counts on into them instead of leaving the health thread reading a dead consumer's;
 2. per-thread CPU is differenced per thread, then summed by name, so a thread that ends no longer takes its whole lifetime out of its name's sum; the final report's threads are listed before the sensor stops; the threads are listed once a period for a report of either kind, with the listings' own interval;
 3. tests: every file action's invalidation, the sessions' order, the final report's count, and a report that lists the threads.
+
+**Plan 1b-4a-p2 clarifications (2026-10-27).** The rest of the profiling step (decision log, 2026-10-15 and 2026-10-27). Applied to §3.2, §6.3, §7.2, §7.4, §9.3, §11.2, §11.4, §13 and §15.3:
+1. on-miss seeding is off by default; with it off, the address history ends with the start-up passes;
+2. the pipeline loop waits 40 ms after a pass that found no event; the pipeline's measured fixes, with a replay benchmark;
+3. cache invalidations wake no thread: the hash workers apply the hash cache's from a channel before their next lookup, and the expander the 8.3 cache's from a list;
+4. Launch images are hashed before module loads; the enrichment deadline stays 1 s;
+5. the diagnostic log gives each bounded structure's entries and capacity;
+6. the sink and the event queues hold memory only for what they hold, and the ordering stage gives a burst's slots back;
+7. the agent opts out of power throttling at start.
+
+**Plan 1b-4a-p2 review fixes (2026-10-27).** The plan's independent review found no major issue and seven minor ones; each fix has a test checked by reverting it. Applied to §3.2, §6.3, §7.2 and §9.3:
+1. the watchlist's pre-check reads a pattern's `/` as `\` and a `]` first in a class as a member, so a user's pattern written with `/` is no longer hidden;
+2. the hash cache's invalidations go through a channel the workers apply, so the pipeline thread never waits on a lock a below-normal worker holds; the expander's list keeps nothing more once it has overflowed;
+3. the set of watchlist Opens, keyed by path, keeps SipHash;
+4. the `sizes=` line gives the kernel, user and sink queues too;
+5. a test that could not catch its bug (the expander list's overflow) now can.
```

- [ ] **Step 2: Commit**

```powershell
git add docs
git commit -m "docs(1): plan 1b-4a-p2 clarifications and findings"
```

### Task 16: PR and merge

- [ ] **Step 1:** Push `feat/1b-4a-p2` and open the PR. The body summarises each fix with its figure, the review's fixes, Task 14's before and after, the test counts, and ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `agent-live`, `proto`, `powershell` and `audit`. If a test fails only on the runner, fix it with a test, never by skipping.
- [ ] **Step 3:** When every check on the PR's current head is green, merge. Then update the decision log's build row if anything changed on the way, and delete the remote `scratch/p2` branch. The hour run follows (Interfaces).

## Review Log

### Independent review (2026-10-27, of the draft on `scratch/p2` at `9b6e8ed`)

An independent agent read the draft against the spec, the D6 notes, plan 1b-4a-p1's interfaces and the code. It built the code in its own target directory and ran the unelevated tests: the last two full runs all green, `atlas-agent` 249 passed and 15 ignored; clippy (Windows and Linux) and fmt clean. It probed suspected bugs in a throwaway worktree. It found no major issue, seven minor ones and some nits. Each fix was made on `scratch/p2-review` in the task that owns the code, with a test, and each test was checked by reverting its fix (the verification's reverts). CI then ran the elevated jobs on the fixed code.

**Minor:**
1. **The watchlist pre-check hid matches of a user's pattern written with `/`.** The runs came from the raw pattern (`/secret.txt`), but paths are scanned with `\`; the `GlobSet` alone, as before this plan, matched. A class starting with `]` (`[]a]`) was misread the same way. Nothing in `agent.toml`'s validation refuses `/`. **Fixed** (Task 6): runs are taken with `/` read as `\`, and a `]` right after `[`, `[!` or `[^` is a class member; in doubt a character counts as wild, which only shortens a run. A new test runs a user's patterns; two reverts.
2. **The expander list's overflow test could not catch its bug.** The invalidation that mattered was queued first, so an implementation that dropped the overflowing one still passed (the reviewer's probe: 7 of 7 tests passed with the clear removed). **Fixed** (Task 9): the one that matters is the one that overflows; one revert.
3. **The expander's list kept filling after an overflow.** Expand jobs are rare on the host, so at the synthetic load's 408 invalidations a second it would sit at 8,192 paths (1–2 MB) between expansions, all of them pointless once the cache is to be cleared. **Fixed** (Task 9): once lost, the list keeps nothing more; the hash cache's invalidations stop at the cap too. Two reverts. The plan now also states that invalidations apply before Expand jobs queued ahead of them, which only makes the cache more conservative.
4. **The pipeline thread took the hash cache's lock, which below-normal workers hold.** Windows' user-mode locks have no priority inheritance, so a worker preempted inside its short critical section (or clearing a full cache, up to 65,536 entries) would stall the pipeline thread, for an operation that only frees memory (the key holds the USN). Not reproduced; F7 showed such starvation does happen. **Fixed** (Task 9): the hash cache's invalidations go on a channel the workers apply before their next lookup. Its test now checks the invalidation reaches the cache before the next lookup, not at the call; one revert.
5. **One foldhash map was keyed by a path,** against the plan's own rule: the set of watchlist Opens (`(process, path)`). Small in practice (capped, only watchlist Opens reach it, the hasher seeded per process). **Fixed** (Task 4): `Recent` takes its hasher, and that set keeps SipHash.
6. **The plan's "one atomic add" in the ETW callback understated it:** std's list channel also allocates a block every 31 events on the sending thread, and the early key map's gauge adds two atomic stores per registry Create, Open or Close. **Fixed:** the Global Constraints say so.
7. **Gauges and coverage:** the expansion cache logged its entries as its capacity; the hash cache's gauge, set only by the workers, overstated after the pipeline thread's invalidations; and the event queues and the sink, where F6 found the memory, were missing from `sizes=`, while the plan said "every bounded structure". **Fixed:** the hash cache's gauge is now consistent (the workers apply the invalidations, then set it; Task 11's test checks it after one); the expansion cache is a `BTreeMap`, which allocates per entry, so its capacity is its entries (documented); `QueueGauge` puts `kernel_queue`, `user_queue` and `sink` in `sizes=`, and the binary test requires them (Task 12; two reverts, and the binary check); the Review Focus names what is listed and what is not.

**Nits, fixed:** the idle loop's test comment no longer cites Windows' 15.6 ms tick (std's sleep uses a high-resolution timer); the plan states that a service reply after an empty pass is applied up to 40 ms later, with no deadline change (replies are fed in before `tick`) and that `tick`'s 50 ms bound keeps a 10 ms margin; the pipeline thread's lock calls tolerate a lock poisoned by a panicked worker (review 9; no revert check, it needs a worker to panic holding one); R8's wording (HighQoS lifts the heuristics, not the power plan's frequency caps); the benchmark's steps are in this plan, not the D6 notes, and the watchlist's step (0.448 → 0.437) is stated; a Launch waits for the jobs in progress and the Launches queued ahead of it; §9.3 notes the negative cache's size is now in the log.

**A flake, not a finding:** in one of the reviewer's four full runs, made while other builds ran, `every_request_kind_is_answered` and `full_lanes_drop_and_count` (hashing waits with a 10 s timeout, unchanged by this plan) failed; they passed in five later runs.

**What held up:** `queue::bounded`'s count cannot underflow (added before a send, taken back on a failed one, subtracted only after a receive), refuses exactly at the cap, and `send` past the cap never blocks; the only imprecision is a spurious `Full` at the boundary when two producers race. `HashLane` loses no wakeup (the condition checked under the lock, in a loop; `notify_one` after the push), `close` sets its flag under a poison-tolerant lock and wakes all, workers drain both queues, then end, and `Services`' drop matches the old end-on-sender-drop. The ordering stage: keys are (timestamp, sequence, slot) with the sequence unique, so the slot never decides order; compaction rebuilds the keys after the popped slot is freed, keeps the sequence, and recurs only geometrically; `next_due` and `next_held` keep their late-event semantics. The history rule: `startup` is filled in `Pipeline::new`, before any event, so no change a start-up snapshot needs goes unrecorded; snapshots are applied before each later event; a start-up pass that never answers just keeps the history, bounded and expiring. `to_string_lossy`'s property tests; `power.rs`'s opt-out is the documented one and its SAFETY comments accurate; the default watchlist, ASCII-only folding on both sides, streams and `**`; on-miss seeding off reflected throughout.
