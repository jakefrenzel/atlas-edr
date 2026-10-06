# Sub-project 1b-3a — Agent Pipeline Core Implementation Plan

> **Status:** Approved (2026-10-05). **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking. Nothing in this plan needs elevation, the VM or a kernel driver: the pipeline is portable, and Windows is faked.

**Goal:** Build the portable core of `atlas-agent` (sensor spec §3.2, §5–§7), the code between `atlas-etw`'s parsed events and the buffer:
- the ETW callbacks' logic;
- the ordering, pipeline and completion stages on one thread;
- the process cache, Launch join and actor resolution;
- files, registry, network and DNS;
- seeding from the handle table, registry value reads, and 8.3 expansion;
- the self-filter.

Windows itself (identity, the device map, telemetry, hashing, value reads, 8.3 expansion, the seeder) sits behind two interfaces that plan 1b-3b implements. Here, fakes stand in for them. A full-pipeline replay of the CI recording checks the whole chain.

**Architecture:**
- **Intake** (`intake`, sensor spec §3.2 [1]) runs inside each ETW callback, a few hash-map operations per event:
  - it counts parse failures and discards successful OperationEnds;
  - it keeps Session A's early registry key map and sends fast-path value reads;
  - it rate-limits DNS-Client per PID;
  - it routes events into the bounded kernel and user-mode queues.
- **The pipeline thread** (`pipeline`, [2] + [3] + [5]; decision D2) is one loop with the clock passed in. The driver (plan 1b-4) calls:
  - `push(Incoming)` for each queued event;
  - `reply(Reply)` for each worker result;
  - `tick(now)` at least every ~50 ms, which returns the events to emit, in order;
  - `take_requests()` to hand work out.

  Inside the loop:
  - the ordering stage holds each event for 750 ms and releases in timestamp order;
  - stream time advances with each released event, so windows that closed before it apply first;
  - the completion stage emits in order and holds pending events until each reason is resolved or past its deadline;
  - the self-filter runs at emission.
- **State, owned only by the pipeline thread:**
  - the process cache, keyed by start key with a per-PID index of lifetimes;
  - Launch halves;
  - the FileObject map, with the failure-confirm window and watchlist Opens;
  - the KeyObject map, with relative names, pending children and tombstones;
  - the seeding state: batched questions, snapshots applied in stream order, and the negative cache;
  - TCP directions and UDP flows;
  - 8.3 expansion slots (decision D4).
- **The Windows boundary** (`services`):
  - `Lookups`, cheap calls made on the pipeline thread: the device map, a live process's details, account names;
  - `Request` / `Reply`, slow work done elsewhere: hashes and signatures, value reads, 8.3 expansion, handle-table snapshots.

  `fakes` implements them from tables, for tests and the replay.
- **Schema:** two additive Sensor Health counters, `loss.callback_panics` and `quality.reg_name_ambiguous`.

**Tech stack:** Rust 1.97 (edition 2024).
- New: `globset` 0.4.20 (MIT/Unlicense), for the watchlist.
- Existing: `atlas-etw`, `atlas-schema`, `uuid` 1.26 (v7); in tests, `proptest`, `serde_json`, `prost`, `prost-reflect`, `atlas-proto`.
- Tools: `buf` 1.73.0.

**Spec:** `docs/specs/2026-10-01-etw-sensor-design.md`, revision 3, with plan 1b-2's clarifications. Section numbers (§) refer to it.

**Verification note (2026-10-05, host, Windows 11 build 26200, unelevated):** every code block below was compiled and run in a scratch worktree of `main` (c6b1704).
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean. `atlas-agent` and `atlas-etw` are also clippy-clean for `x86_64-unknown-linux-gnu`, checked from Windows.
- `cargo test --workspace`: 276 pass, and 1 is ignored (1b-2's live test). That includes 93 `atlas-agent` unit tests and the 3 full-pipeline replay tests.
- Each fix from the independent review (Review Log) was checked by reverting it: all 13 reverts make their pinned test fail.
- `buf lint` and `buf breaking` (against `main`) pass. Regenerating the golden fixtures changed none of the 25 (line endings only).
- **The full-pipeline replay of the CI recording** (1,489 events from Windows build 26100) emits 74 events. All of them pass the schema's validating decoder unchanged. Every scenario step appears; Task 5 lists them.
- **Cost** (release), for push, ordering, state and completion:
  - steady state: 0.57 µs per event, feeding the recording through one pipeline 100 times (148,900 events);
  - high cardinality: 0.68 µs per event at 10,000 events/s and 0.91 µs at 52,000/s, for 40 s of Creates, Closes, OpenKeys and CloseKeys on addresses never repeated. This is the load that exposed review finding R-B1, which cost 24–53 µs per event before the fix.
  - That is under 5% of one core at the host's 52,000/s peaks. It confirms the premise of D2 (estimated 1–2 µs).
- **Not run yet:** the Linux build of `atlas-schema`'s tests (its `criterion` benchmark needs a Linux C compiler; CI has one), and `cargo audit` (CI runs it).

## Global Constraints

- **Branches.** This plan is reviewed on `docs/1b-3a-plan`. The build runs on `feat/1b-3a-pipeline`, created from `main` after this plan merges. Never commit to `main`. (Claude may merge a PR itself once every check on its current head is green; the user granted that on 2026-10-05.)
- **No `unsafe`, no Windows calls.** `atlas-agent`'s modules in this plan are portable. Plan 1b-3b adds the Windows implementations behind `services`.
- **The pipeline thread owns all mutable state.** The only state shared with the callbacks is `IntakeCounters` (atomics) and the queues. No locks on the event path.
- **Time is passed in.** Nothing in the pipeline reads a clock, so every rule is tested deterministically.
- **Every emitted event is valid:** it passes `atlas_schema::decode_event` unchanged. Strings are cut to the schema's limits (clarification 11), and tests check this on every scenario.
- **Additive schema only:** `buf breaking` must pass, and the existing golden fixtures must stay byte-identical.
- **Formatting and lint:** the repo's `rustfmt.toml`; clippy with `-D warnings` at the end of every task.
- **Shell:** PowerShell 7 (`cargo` commands are the same in bash).
- **Line endings:** `core.autocrlf` is on; the snapshot comparison ignores CR.
- Every commit message ends with the attribution lines the session's system reminder specifies.

## Decisions (chosen 2026-10-05: D1 A, D2 A, D3 C, D4 A)

### D1, how plan 1b-3 is split

Spec, enrichment, value reads and seeding together would make a plan of about 12,000–15,000 lines: twice 1b-2, the length the decision log already rejected.
- **(A) Chosen: two plans along the portable/Windows seam.** 1b-3a (this plan) is the pipeline core, with Windows behind interfaces and faked. 1b-3b covers the real implementations and the agent-level live test.
- (B) One plan. (C) Split by data type, process, network and DNS, then files and registry; each half would mix portable and Windows code.

### D2, threads for the ordering, pipeline and completion stages

- **(A) Chosen: one thread runs all three**, with the clock passed in.
  - Every rule is testable without sleeping. Every piece of mutable state has one owner, so pending events need no locks.
  - The estimated 1–2 µs per event was measured at 0.57–0.91 µs (verification note).
- (B) A thread per stage: parallelism nobody needs, two hand-offs per event, and timing tests that depend on scheduling.
- (C) One thread, with the option to split written into the spec.

### D3, how the full-pipeline replay checks its output

- **(C) Chosen: scenario assertions plus a golden snapshot.** The assertions state what the scenario must produce. The snapshot (`tests/snapshots/scenario.jsonl`, protobuf-JSON, one event per line, 74 events, 44 KB) catches any change nobody checked for.
- (A) Assertions only. (B) Snapshot only.

### D4, where 8.3 short names are expanded (refines §7.2)

Measured on the recordings:
- 0 of 1,447 file events on the host had an 8.3 component, because the username is short.
- 13% on the CI runner did, all under `C:\Users\RUNNER~1`: Windows writes `TEMP` in short form whenever the username is longer than 8 characters.

Attackers also use 8.3 names on purpose, to dodge path-based rules.
- **(A) Chosen: expand in every emitted file path,** Launch and Module images included.
  - An event with a `~N` component waits for the expansion, up to the 1 s deadline. A failure leaves the path as logged.
  - The handle map keeps the expanded path once per handle, and 1b-3b caches per directory.
- (B) The watchlist only (the spec as written): emitted paths would mix spellings.
- (C) A, plus a schema field with the logged short form: deferred until a detection needs it.

## Findings from the build

- **B1, cost:** 0.57 µs per event in steady state, 0.91 µs under high cardinality (verification note). A test that rebuilt the pipeline for each run measured 5.7 µs. That figure was dominated by start-up and 740 idle ticks spread over only 1,489 events, so it is not the per-event cost.
- **B2, an O(n) trap:** evicting one entry per insert from a full map scans the whole map. At the key map's cap (262,144) and OpenKey's thousands per second, that would cost billions of operations a second. Every bounded map now evicts the oldest eighth in one pass instead (`evict`, clarification 6).
- **B3, stream time must advance per event:** when one tick released a batch, applying the confirm windows only after the whole batch let a Cleanup at 900 ms run before a rename at 7 ms had been confirmed. The Update then carried the old name. Stream time now advances to each released event before it is processed (clarification 2; test `a_confirmed_rename_renames_the_handle`).
- **B4, a failure "before its event"** happens only when the operation itself arrives late (out of order): an OperationEnd is always logged after its operation. The ring of failed OperationEnds is for that case (test `failed_operations_are_dropped_by_irp_within_the_window`).
- **B5, the CI recording resolves end to end with no seeding:**
  - Every registry path resolves through the actor's own absolute opens of `HKCU`.
  - Every file actor resolves.
  - The observer, already running when the sessions started, resolves through the rundown, once its start key comes from the (faked) telemetry query.

## Deliberate clarifications of the spec (applied to the spec text in Task 6)

1. **Plan 1b-3 is split** into 1b-3a and 1b-3b (D1). The decision log's four-part split of plan 1b becomes five parts.
2. **One pipeline thread** (D2; refines §3.2):
   - The ordering, pipeline and completion stages are one loop with the time passed in.
   - Stream time advances to each released event before it is processed. Seeder snapshots, confirm windows and Launch halves due by then are applied first.
   - Sweeps over whole tables (UDP idle, cache retention, seeder questions) run at the end of each tick.
3. **The Windows boundary** (new; for 1b-3b):
   - `Lookups`: `dos_path`, `live_process`, `account_name`, cached by the implementation.
   - `Request` / `Reply`: `Enrich`, `InvalidateHash`, `ReadValue`, `Expand` (with `slot`), `Seed`; and `Enriched`, `ValueRead`, `EarlyRead`, `Expanded`, `Snapshot`.
4. **8.3 expansion in every emitted file path** (D4; refines §7.2):
   - A Rename's two paths are two slots of one event.
   - A Launch's expanded image also updates the cached process. Actor references built before that reply arrives keep the short form.
5. **The full-pipeline replay** (D3; completes §12.2) runs the CI recording through the parsers, the intake and the pipeline with fakes. Its checks are scenario assertions and a protobuf-JSON snapshot.
6. **Bounds** (refines "LRU eviction" in §6.2, §7.1, §7.3, §7.4). Every cost is O(1) per insert on average (B2):
   - Bounded maps evict their oldest eighth at once. The early key map evicts oldest-first by insertion (§7.5).
   - The process cache evicts ended processes first, then the least recently looked up, so long-running services stay.
   - Sets that expire by time (address history, failed and confirmed Irps, watchlist coalescing) are queues ordered by time, with a cap (`recent`).
   - Two safety caps clear at once instead:
     - the TCP direction table: an entry is left only by a lost Disconnect, and a cleared one only makes a Close fall back to clarification 8;
     - the fast-path read results: they are pruned every tick anyway, and a cleared one only costs a redone read.
   - Whole-table sweeps (UDP idle, cache retention) run once per second of stream time.
7. **Unknown file handles** (refines §7.1):
   - A SetAttributes on a handle that is never named is dropped and counted (`unknown_file_object`), like Update and Delete: a file event with no path has no detection value.
   - A Rename of a handle whose name is unknown goes out at once with an empty source. It does not wait for the seeder: a snapshot read after the rename could only give the new name. The handle takes the new name once the rename stands.
8. **A TCP Close for a connection opened before the agent watched** takes the end with the lower port to be the server, because its direction is unknown (§7.3).
9. **DNS answers in neither recognised form** are kept, with type 0 and the raw text (§5.4).
10. **Idle (PID 0)** gets the synthetic start key `(BootId << 48) | 0` and the name `Idle` (§5.3 rule 1).
11. **Length limits:** paths, names, value names and user strings are cut to the schema's limits on a character boundary, with no flag, because the schema has none for paths and names. A pathological NT path otherwise makes an invalid event.
12. **Sensor Health** gains `loss.callback_panics`, for events skipped because the ETW callback panicked (1b-2 review m7), and `quality.reg_name_ambiguous`, for value names that could end in more than one place (1b-2 clarification 5).
13. **A Launch seen only through its classic half** (Session B):
    - The start key comes from the live process.
    - Without one there is no uid, so the Launch is dropped and counted (`launch_join_miss`, `actor_dropped`).
14. **`file.op_end` off** (§11.2, the second §13 fallback) emits Create, Delete and Rename at once, without the confirm window.
15. **Watchlist coalescing** keys on (actor uid, logged path, lowercased) (§7.2). The record is made when the Open is pushed, and a failed Create removes it, so a retry after a sharing violation is not suppressed. A late operation failure is recognised for 40 confirm windows (10 s) after its operation stood (`file_op_late_failure`, §5.5).
16. **Key address reuse** (refines §7.4). Key objects are per handle (S7), so a successful CreateKey or OpenKey at an address makes a new object there.
    - Children waiting on that address belonged to the earlier object. They become orphans, which only the seeder naming the child handle itself can name.
    - The same holds for the children of an unknown base that closes, and of a tombstone whose address the seeder names after the tombstone closed.
    - Without this, a reused address would give a Run-key write an unrelated key's path, with `path_unresolved = false` (review R-B3).
17. **A snapshot covers what it was asked** (refines §7.4). A `Snapshot` carries its request's addresses; the start-up pass covers the whole table. A covered address in neither `named` nor `unnamable` was not in the table:
    - it joins the negative cache (owner 0) until the address is used again;
    - the events waiting on it are answered at once: registry events go out unresolved, file events are dropped and counted.
18. **The reuse rule is conservative** (§7.4): a seeded name answers an event only if no Create, Open or Close for the address came after the earlier of the event and the snapshot. For a snapshot applied late, a change after the snapshot also disqualifies it.
19. **Seeding on start and on miss** (§11.2): two settings, both on by default.
    - With on-miss seeding off, an event on an unknown handle waits only for the start-up pass, while that is outstanding. After it, registry events go out unresolved, and file events are dropped and counted.
    - When seeding is unavailable (no `SeDebugPrivilege`), plan 1b-3b turns both off.
20. **Late arrivals** (§3.2): an event older than stream time (the watermark) is late. It is counted and processed at once. Before this, an event between the last released one and the watermark was held, then processed after the windows it belonged to had been applied.
21. **A clean stop** (§11.4) drops what a deadline would have dropped: unnamed file handles, and watchlist Opens that did not match. It no longer emits them with placeholder actors.
22. **A process found through the live lookup** (a cache miss with a start key, §5.3) is cached as started at that event's time. A later lookup by payload PID then finds it, not an earlier process that had the same PID.
23. **A classic half after the join window**, for a process whose Kernel-Process half came (§5.2): that Launch stands, and the cache gains the command line and user. No second Launch is made.
24. **The Value Set floor** (§7.5): an unresolved key means no read was attempted. The event has `data_read_after = false` and `data_unavailable = true`, and counts `value_read_failed`.
25. **Requests and replies:** a pending event's request bookkeeping is freed when the event leaves the completion stage, whether or not its reply came. A reply after that is ignored. Each `Request` gets at most one `Reply`, and a missing one costs only that event's deadline.
26. **Known limitation:** key paths are kept as UTF-8, converted lossily. A key whose name contains unpaired surrogates (a hiding trick) cannot be read back, so its value reads fail and are counted. Value names are kept as UTF-16. Deferred: carrying UTF-16 key names through the key map is a later change.

## Review Focus

These would slip past a plain unit test, so each has a pinned check:
1. **Order before state:** an event that arrives before its process's Launch, but with a later timestamp, still resolves its actor (`events_are_ordered_before_the_state_sees_them`). Windows apply in stream order inside one batch (B3).
2. **PID reuse:** an event resolves to the process live at its time (`actors_resolve_at_the_events_time_across_pid_reuse`).
3. **One Update per written handle,** with the opener as actor even when System cleans up; truncations count as writes; writes after Cleanup are counted (`one_update_per_written_handle_with_the_opener_as_actor`).
4. **Failure confirmation by Irp, within the window:** recycled Irps, an informational status, a late operation, a late failure (`failed_operations_are_dropped_by_irp_within_the_window`). A failed Create removes its handle and its watchlist Open.
5. **The seeder's stream-time rules:**
   - a seeded name never answers an event if the address was reused in between;
   - an unnamable address does not wait;
   - a seeded file event gets its actor from the owner (`an_unknown_base_is_named_by_the_seeder_unless_it_was_reused`, `an_unknown_handle_is_named_by_the_seeder_or_dropped`).
6. **Value reads:** the type and length must match the event; a fast-path read is used only when it names the same key and value; unusual types carry no data (`keys_and_values_with_reads_after_the_event`, `a_fast_path_read_is_used_when_it_names_the_same_key`).
7. **Embedded NULs** survive into the emitted names (`the_agents_own_closes_do_not_forget_keys`, and the replay's `a\0b` and `k\0x`).
8. **8.3 expansion** of every path slot, including a failed expansion (`short_names_in_emitted_paths_are_expanded`). The replay asserts no `RUNNER~1` remains.
9. **Every emitted event is valid,** including 80 KB paths (`pathological_lengths_still_make_valid_events`, and `valid()` in every scenario).
10. **The completion stage keeps order** whatever order results arrive in, and overflow forces out the oldest pending event (property test `order_is_preserved`, `overflow_forces_the_oldest_out`).
11. **The replay is deterministic** (`the_replay_is_deterministic`), so the snapshot is stable.
12. **Key address reuse** never names old children (`a_reused_base_never_names_old_children`, and `keymap::a_reused_or_closed_base_does_not_name_old_children`).
13. **Nothing is kept for events that left:** no reply, absent addresses, failures with no operation (`bookkeeping_is_freed_when_no_reply_comes`, `an_address_absent_from_the_table_is_answered_at_once`). `Pipeline::bookkeeping` (tests only) lists every per-event and per-window structure, and each must be empty after `settle`.

## File Structure

```
Cargo.toml                                    + atlas-etw, atlas-schema, globset in [workspace.dependencies]
crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto   + callback_panics, reg_name_ambiguous
crates/atlas-schema/src/classes/sensor_health.rs               + the two fields
crates/atlas-schema/tests/common/mod.rs                        strategies cover them
crates/atlas-etw/src/parse/reader.rs                           + Sid::from_bytes
crates/atlas-agent/                           NEW crate (library)
  Cargo.toml
  src/lib.rs
  src/config.rs        Config (spec defaults), Ticks
  src/time.rs          Anchor, Clock, filetime_to_unix_ns
  src/input.rs         Session, Header, Incoming
  src/counters.rs      Class, Counters, IntakeCounters
  src/evict.rs         batch eviction
  src/recent.rs        Recent: keys with times, expiring oldest first, with a cap
  src/paths.rs         registry normalization, volume-relative paths, ADS, 8.3 detection, fit
  src/keymap.rs        KeyMap (relative names, pending children, tombstones)
  src/watchlist.rs     DEFAULT patterns, Watchlist (GlobSet)
  src/ordering.rs      Ordering (hold, release, watermark)
  src/completion.rs    Completion, Reason, Wait, Expired
  src/process.rs       start_key, integrity, Identity, ProcInfo, ProcessCache
  src/services.rs      Lookups, Request, Reply, and their payloads
  src/intake.rs        Intake, Queues, EarlyKeys, DNS buckets
  src/fakes.rs         FakeLookups, sequential_ids
  src/pipeline/mod.rs  Pipeline, Setup, IdGen, actor resolution, emission, self-filter
  src/pipeline/proc.rs Launch join, Terminate, Module Load, rundown, enrichment
  src/pipeline/file.rs FileObject map, confirm window, Update coalescing, watchlist Open
  src/pipeline/reg.rs  key and value events, value reads, seeder answers
  src/pipeline/net.rs  TCP, UDP flows, DNS
  src/pipeline/seed.rs seeding questions and snapshots
  src/pipeline/expand.rs  8.3 expansion slots
  src/pipeline/tests.rs   scenarios
  tests/replay.rs      full-pipeline replay of the CI recording
  tests/snapshots/scenario.jsonl   generated in Task 5
docs/…                 Task 6
```

## Interfaces for later plans

- **Plan 1b-3b implements `services`:**
  - `Lookups::dos_path`: the `QueryDosDeviceW` map, refreshed every 60 s and on a miss (at most every 5 s), matching on component boundaries (`FakeLookups` shows the rule).
  - `Lookups::live_process`: `PROCESS_TELEMETRY_ID_INFORMATION`, giving the start key, image path and command line.
  - `Lookups::account_name`: `LookupAccountSidW`, cached.
  - The workers answer `Request::Enrich` with `Reply::Enriched` and cache by file identity and USN; `InvalidateHash` drops entries for a path.
  - The reader lane answers `ReadValue` with `ValueRead`, and the fast path's reads (`intake::FastRead`) with `EarlyRead`.
  - `Expand` is answered with `Expanded` (same `slot`), cached per directory.
  - The seeder answers `Seed` (an empty address list means the start-up pass) with one `Snapshot`, stamped with its QPC and carrying the request's addresses in `asked`. Every covered handle of that kind goes in `named` or `unnamable`, including non-disk files as unnamable; an address in neither was not in the table (`services::Snapshot`).
  - Without `SeDebugPrivilege`, seeding is unavailable: set `Config::seed_on_start` and `seed_on_miss` to false.
  - Each `Request` gets at most one `Reply`. A lost one costs only its event's deadline (clarification 25).
  - `Lookups` are called on the pipeline thread: on cache misses, for the rundown, the join fallback and classic-only Launches. Each must be cheap (cached). `live_process` must describe the process alive now, never a cached PID → start key mapping, because PIDs are reused.
  - It also supplies `Identity` (device uid, boot id, kernel BootId), the anchor pair, the current control set, and the agent's start key for `Setup`.
- **Plan 1b-4 drives it:**
  - It creates one `Intake` per session, `Session::Sensor` with a `FastRead` and `Session::Process` without; the callback is `|rec| intake.on_event(header(rec), gate.parse(rec))`, where `header(rec)` is `Header { session, pid: rec.pid(), tid: rec.tid(), ts: rec.timestamp(), start_key: rec.start_key() }` from `atlas-etw`'s `EventRecord`.
  - It runs the pipeline thread: drain both queues into `push`, feed replies into `reply`, `tick(qpc_now)` every ≤ 50 ms, `take_requests()`. It passes each emitted event to the detection hook, then the buffer writer. `stop()` on a clean stop.
  - Every 60 s it calls `set_anchor`; `add_self_key` adds the canary child.
  - **Sensor Health:**
    - `Pipeline::counters()` gives the pipeline's counters, and `IntakeCounters` the callbacks'.
    - Report the differences between two reads; `Counters` names its Sensor Health field.
    - `callback_panics` comes from `Consumer::panics()`.
- **Payload:** the events are `atlas_schema::Event`. `meta.time` comes from the anchor, and `event_id` from the `IdGen` given to `Pipeline::new` (`EventId::new_v7` in the agent).

---

### Task 1: Schema counters and `Sid::from_bytes`

**Files:**
- Modify: `crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto`, `crates/atlas-schema/src/classes/sensor_health.rs`, `crates/atlas-schema/tests/common/mod.rs`, `crates/atlas-etw/src/parse/reader.rs`

- [ ] **Step 1: The two counters**

`sensor_health.proto`:
```diff
--- a/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
+++ b/crates/atlas-proto/proto/atlas/events/v1/sensor_health.proto
@@ -50,6 +50,8 @@ message SensorLoss {
   repeated ClassCount actor_dropped = 8;
   // Events dropped because the buffer's in-memory backlog was full (disk full or I/O errors).
   optional uint64 buffer_backlog_drops = 9;
+  // Events skipped because the ETW callback panicked on them (caught, never unwound into ETW).
+  optional uint64 callback_panics = 10;
 }
 
 // Events emitted with less than full information, or handled outside the usual path.
@@ -72,6 +74,8 @@ message SensorQuality {
   optional uint64 buffer_invalid_records = 14;
   optional uint64 enrichment_misses = 15;
   optional uint64 enrichment_errors = 16;
+  // Registry value names that could end in more than one place (embedded NULs); the last fit was used.
+  optional uint64 reg_name_ambiguous = 17;
 }
 
 // Evictions from bounded structures, and expected drops.
```

`crates/atlas-schema/src/classes/sensor_health.rs`:
```diff
--- a/crates/atlas-schema/src/classes/sensor_health.rs
+++ b/crates/atlas-schema/src/classes/sensor_health.rs
@@ -140,6 +140,8 @@ counter_group!(
         actor_dropped: Vec<ClassCount>,
         /// Dropped because the buffer's in-memory backlog was full.
         buffer_backlog_drops: Option<u64>,
+        /// Skipped because the ETW callback panicked on them.
+        callback_panics: Option<u64>,
     }
 );
 
@@ -164,6 +166,8 @@ counter_group!(
         buffer_invalid_records: Option<u64>,
         enrichment_misses: Option<u64>,
         enrichment_errors: Option<u64>,
+        /// Registry value names with more than one possible end (embedded NULs).
+        reg_name_ambiguous: Option<u64>,
     }
 );
```

`crates/atlas-schema/tests/common/mod.rs` (the property-test strategies cover the new fields):
```diff
--- a/crates/atlas-schema/tests/common/mod.rs
+++ b/crates/atlas-schema/tests/common/mod.rs
@@ -364,22 +364,33 @@ fn counter() -> impl Strategy<Value = Option<u64>> {
 }
 
 fn arb_health_report() -> BoxedStrategy<HealthReport> {
-    let loss =
-        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), arb_class_counts(), counter())
-            .prop_map(|f| SensorLoss {
-                sensor_session_events_lost: f.0,
-                process_session_events_lost: f.1,
-                sensor_session_buffers_lost: f.2,
-                process_session_buffers_lost: f.3,
-                kernel_queue_drops: f.4,
-                user_queue_drops: f.5,
-                dns_rate_limit_drops: f.6,
-                actor_dropped: f.7,
-                buffer_backlog_drops: f.8,
-            });
+    let loss = (
+        counter(),
+        counter(),
+        counter(),
+        counter(),
+        counter(),
+        counter(),
+        counter(),
+        arb_class_counts(),
+        counter(),
+        counter(),
+    )
+        .prop_map(|f| SensorLoss {
+            sensor_session_events_lost: f.0,
+            process_session_events_lost: f.1,
+            sensor_session_buffers_lost: f.2,
+            process_session_buffers_lost: f.3,
+            kernel_queue_drops: f.4,
+            user_queue_drops: f.5,
+            dns_rate_limit_drops: f.6,
+            actor_dropped: f.7,
+            buffer_backlog_drops: f.8,
+            callback_panics: f.9,
+        });
     let quality = (
         (counter(), counter(), counter(), counter(), arb_class_counts(), counter(), counter(), counter()),
-        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
+        (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
     )
         .prop_map(|(a, b)| SensorQuality {
             late_arrivals: a.0,
@@ -398,6 +409,7 @@ fn arb_health_report() -> BoxedStrategy<HealthReport> {
             buffer_invalid_records: b.5,
             enrichment_misses: b.6,
             enrichment_errors: b.7,
+            reg_name_ambiguous: b.8,
         });
     let housekeeping = (
         (counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter(), counter()),
```

- [ ] **Step 2: `Sid::from_bytes`** (for building SIDs in tests and in 1b-3b)

`crates/atlas-etw/src/parse/reader.rs`:
```diff
--- a/crates/atlas-etw/src/parse/reader.rs
+++ b/crates/atlas-etw/src/parse/reader.rs
@@ -165,6 +165,14 @@ impl Sid {
     /// `SID_MAX_SUB_AUTHORITIES` in winnt.h.
     pub const MAX_SUB_AUTHORITIES: usize = 15;
 
+    /// A SID from its binary form, checked as the parsers check it; `None`
+    /// unless `bytes` is exactly one valid SID.
+    pub fn from_bytes(bytes: &[u8]) -> Option<Sid> {
+        let mut r = Reader::new(bytes);
+        let sid = r.sid().ok()?;
+        r.rest().is_empty().then_some(sid)
+    }
+
     pub fn as_bytes(&self) -> &[u8] {
         &self.0
     }
@@ -294,6 +302,14 @@ mod tests {
         assert_eq!(label.rid(), Some(12288));
     }
 
+    #[test]
+    fn sids_from_bytes() {
+        let b = [1, 1, 0, 0, 0, 0, 0, 16, 0, 0x20, 0, 0];
+        assert_eq!(Sid::from_bytes(&b).map(|s| s.to_string()), Some("S-1-16-8192".into()));
+        assert!(Sid::from_bytes(&b[..11]).is_none());
+        assert!(Sid::from_bytes(&[b.as_slice(), &[0]].concat()).is_none());
+    }
+
     #[test]
     fn bad_sids_are_rejected() {
         // Revision 2.
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-schema -p atlas-proto -p atlas-etw
buf lint crates/atlas-proto/proto
buf breaking crates/atlas-proto/proto --against '.git#branch=main,subdir=crates/atlas-proto/proto'
$env:ATLAS_UPDATE_FIXTURES = '1'; cargo test -p atlas-schema --test golden; Remove-Item Env:ATLAS_UPDATE_FIXTURES
git diff --ignore-cr-at-eol --stat crates/atlas-schema/tests/fixtures
```
Expected: all pass, `buf` reports nothing, and the fixture diff is empty (`git checkout` the line-ending noise).
```powershell
git add crates/atlas-proto crates/atlas-schema crates/atlas-etw
git commit -m "feat(schema): Sensor Health callback_panics and reg_name_ambiguous; atlas-etw Sid::from_bytes"
```

### Task 2: Crate and foundations

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/atlas-agent/Cargo.toml`, `src/lib.rs`, `config.rs`, `time.rs`, `input.rs`, `counters.rs`, `evict.rs`, `recent.rs`, `paths.rs`, `keymap.rs`, `watchlist.rs`, `ordering.rs`, `completion.rs`, `process.rs`, `services.rs`

- [ ] **Step 1: Workspace and crate manifest**

`Cargo.toml`:
```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,9 +9,12 @@ license = "AGPL-3.0-only"
 publish = false
 
 [workspace.dependencies]
+atlas-etw = { path = "crates/atlas-etw" }
 atlas-proto = { path = "crates/atlas-proto" }
+atlas-schema = { path = "crates/atlas-schema" }
 blake3 = "1.8"
 crc32c = "0.6"
+globset = "0.4"
 criterion = "0.8"
 prost = "0.14"
 prost-build = "0.14"
```

`crates/atlas-agent/Cargo.toml`:
```toml
[package]
name = "atlas-agent"
version = "0.1.0"
description = "The Atlas endpoint agent: ETW intake, the event pipeline, enrichment and the buffer writer."
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
atlas-etw.workspace = true
atlas-schema.workspace = true
globset.workspace = true
uuid.workspace = true

[dev-dependencies]
atlas-proto.workspace = true
prost.workspace = true
prost-reflect.workspace = true
proptest.workspace = true
serde_json.workspace = true
```

`crates/atlas-agent/src/lib.rs` (Tasks 3 and 4 add `intake`, `fakes` and `pipeline`):
```rust
//! The Atlas agent (sensor spec §3).

pub mod completion;
pub mod config;
pub mod counters;
pub mod evict;
pub mod input;
pub mod keymap;
pub mod ordering;
pub mod paths;
pub mod process;
pub mod recent;
pub mod services;
pub mod time;
pub mod watchlist;
```

- [ ] **Step 2: Settings, time, input, counters**

`src/config.rs`:
```rust
//! The pipeline's settings, with the sensor spec's defaults (§3.2, §5.5, §7).
//! Reading them from `agent.toml` comes with plan 1b-4; until then the agent
//! uses `Config::default()`.

use std::time::Duration;

/// Pipeline settings. Durations are converted to QPC ticks by [`Ticks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Ordering stage: how long an event is held before release (§3.2).
    pub hold: Duration,
    /// Failure-confirm window for Create, DeletePath and RenamePath (§5.5).
    pub confirm_window: Duration,
    /// Completion deadlines (§3.2).
    pub enrich_deadline: Duration,
    pub join_deadline: Duration,
    pub value_read_deadline: Duration,
    pub expand_deadline: Duration,
    pub seeder_deadline: Duration,
    /// The seeder deadline during the first `startup_period` after start (§3.2).
    pub seeder_startup_deadline: Duration,
    pub startup_period: Duration,
    /// Launch join: the two halves match within this window (§5.2).
    pub join_window: Duration,
    /// Process cache: retention after Terminate, and the entry cap (§6.2).
    pub process_retention: Duration,
    pub process_cap: usize,
    /// FileObject and KeyObject maps: caps (§7.1, §7.4).
    pub file_map_cap: usize,
    pub key_map_cap: usize,
    /// The callback's early key map cap (§7.5).
    pub early_key_map_cap: usize,
    /// UDP flows (§7.3).
    pub network_udp: bool,
    pub udp_idle: Duration,
    pub flow_cap: usize,
    /// DNS-Client per-PID rate limit, events per second (§4.4).
    pub dns_rate_per_pid: u32,
    /// Watchlist (§7.2): `None` uses the built-in list.
    pub watchlist: Option<Vec<String>>,
    /// Extra patterns added to the list in use.
    pub watchlist_extend: Vec<String>,
    /// Repeated opens of one path by one process count once per this period (§7.2).
    pub watchlist_coalesce: Duration,
    /// More pending events than this and the oldest goes out as is (§3.2).
    pub pending_cap: usize,
    /// Registry value reads after the event (§7.5).
    pub registry_value_reads: bool,
    /// Failure confirmation for Create, DeletePath and RenamePath (§5.5). Off
    /// (the second §13 fallback, with the OP_END keyword disabled), they are
    /// emitted at once.
    pub file_op_end: bool,
    /// Seed the key and file maps from the handle table at start (§7.4).
    pub seed_on_start: bool,
    /// Ask the seeder about unknown handles (§7.4). Off, an event on an unknown
    /// handle waits only for the start-up pass, while it is outstanding.
    pub seed_on_miss: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hold: Duration::from_millis(750),
            confirm_window: Duration::from_millis(250),
            enrich_deadline: Duration::from_secs(1),
            join_deadline: Duration::from_secs(1),
            value_read_deadline: Duration::from_secs(1),
            expand_deadline: Duration::from_secs(1),
            seeder_deadline: Duration::from_secs(2),
            seeder_startup_deadline: Duration::from_secs(5),
            startup_period: Duration::from_secs(30),
            join_window: Duration::from_millis(200),
            process_retention: Duration::from_secs(30),
            process_cap: 65_536,
            file_map_cap: 262_144,
            key_map_cap: 262_144,
            early_key_map_cap: 200_000,
            network_udp: true,
            udp_idle: Duration::from_secs(60),
            flow_cap: 65_536,
            dns_rate_per_pid: 100,
            watchlist: None,
            watchlist_extend: Vec::new(),
            watchlist_coalesce: Duration::from_secs(60),
            pending_cap: 100_000,
            registry_value_reads: true,
            file_op_end: true,
            seed_on_start: true,
            seed_on_miss: true,
        }
    }
}

/// Converts durations to QPC ticks (the pipeline's only clock, §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticks {
    /// QPC ticks per second.
    pub frequency: i64,
}

impl Ticks {
    pub fn new(frequency: i64) -> Self {
        assert!(frequency > 0, "QPC frequency must be positive");
        Ticks { frequency }
    }

    pub fn of(&self, d: Duration) -> i64 {
        let ticks = d.as_nanos().saturating_mul(self.frequency as u128) / 1_000_000_000;
        i64::try_from(ticks).unwrap_or(i64::MAX)
    }

    /// Ticks to nanoseconds, saturating.
    pub fn to_nanos(&self, ticks: i64) -> i64 {
        let ns = i128::from(ticks) * 1_000_000_000 / i128::from(self.frequency);
        i64::try_from(ns).unwrap_or(if ns < 0 { i64::MIN } else { i64::MAX })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_round_trip_at_the_usual_frequency() {
        let t = Ticks::new(10_000_000);
        assert_eq!(t.of(Duration::from_millis(750)), 7_500_000);
        assert_eq!(t.to_nanos(7_500_000), 750_000_000);
        assert_eq!(t.of(Duration::MAX), i64::MAX);
    }

    #[test]
    fn defaults_follow_the_spec() {
        let c = Config::default();
        assert_eq!((c.hold, c.confirm_window), (Duration::from_millis(750), Duration::from_millis(250)));
        assert_eq!(c.dns_rate_per_pid, 100);
        assert!(c.network_udp && c.registry_value_reads && c.seed_on_miss);
    }
}
```

`src/time.rs`:
```rust
//! The time base (sensor spec §3.3). Everything inside the pipeline is raw QPC;
//! `meta.time` (Unix ns, UTC) comes from the newest anchor pair.

use crate::config::Ticks;

/// A (QPC, wall clock) pair taken together. Re-taken every 60 s by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub qpc: i64,
    /// Unix time in nanoseconds (from `GetSystemTimePreciseAsFileTime`).
    pub unix_ns: i64,
}

/// Converts QPC timestamps to Unix nanoseconds.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    ticks: Ticks,
    anchor: Anchor,
}

impl Clock {
    pub fn new(ticks: Ticks, anchor: Anchor) -> Self {
        Clock { ticks, anchor }
    }

    pub fn ticks(&self) -> Ticks {
        self.ticks
    }

    /// Replaces the anchor; later conversions use it (§3.3).
    pub fn set_anchor(&mut self, anchor: Anchor) {
        self.anchor = anchor;
    }

    /// `anchor_unix_ns + (qpc − anchor_qpc) × 10⁹ / frequency`, saturating.
    pub fn unix_ns(&self, qpc: i64) -> i64 {
        self.anchor.unix_ns.saturating_add(self.ticks.to_nanos(qpc.saturating_sub(self.anchor.qpc)))
    }
}

/// FILETIME (100 ns since 1601) to Unix nanoseconds, saturating.
pub fn filetime_to_unix_ns(ft: u64) -> i64 {
    const EPOCH_DIFF_100NS: i128 = 116_444_736_000_000_000;
    let ns = (i128::from(ft) - EPOCH_DIFF_100NS) * 100;
    i64::try_from(ns).unwrap_or(if ns < 0 { i64::MIN } else { i64::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_relative_to_the_anchor() {
        let c = Clock::new(Ticks::new(10_000_000), Anchor { qpc: 1_000, unix_ns: 1_700_000_000_000_000_000 });
        assert_eq!(c.unix_ns(1_000), 1_700_000_000_000_000_000);
        assert_eq!(c.unix_ns(11_000), 1_700_000_000_001_000_000); // +10 000 ticks = +1 ms
        assert_eq!(c.unix_ns(0), 1_699_999_999_999_900_000); // before the anchor
    }

    #[test]
    fn a_new_anchor_moves_later_conversions() {
        let mut c = Clock::new(Ticks::new(10_000_000), Anchor { qpc: 0, unix_ns: 0 });
        c.set_anchor(Anchor { qpc: 0, unix_ns: 5 });
        assert_eq!(c.unix_ns(0), 5);
    }

    #[test]
    fn filetime_epoch_and_a_known_date() {
        assert_eq!(filetime_to_unix_ns(116_444_736_000_000_000), 0);
        // 2026-10-02T18:37:27.6102067Z, from spike S8.
        assert_eq!(filetime_to_unix_ns(134_354_398_476_102_067), 1_790_966_247_610_206_700);
    }
}
```

`src/input.rs`:
```rust
//! What the ETW callbacks hand to the pipeline: a parsed event and the parts of
//! its header the pipeline needs (sensor spec §3.2 [1]).

use atlas_etw::parse::RawEvent;

/// Which session delivered the event (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Session {
    /// Session A: the manifest providers.
    Sensor,
    /// Session B: the system logger's classic process events.
    Process,
}

/// The event header, as far as the pipeline uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub session: Session,
    /// The logging process. For network events this is not the owner (§5.3).
    pub pid: u32,
    pub tid: u32,
    /// Raw QPC (§3.3).
    pub ts: i64,
    /// From the extended data, when the provider was enabled with it (Session A).
    pub start_key: Option<u64>,
}

/// One event on its way from a callback to the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    pub header: Header,
    pub event: RawEvent,
}
```

`src/counters.rs`:
```rust
//! The pipeline's counters (sensor spec §9.3). Each one is a Sensor Health
//! field (atlas-schema `SensorLoss`, `SensorQuality`, `SensorHousekeeping`);
//! plan 1b-4 copies them into the periodic report and resets the deltas.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// An OCSF event class, for the per-class counts (`ClassCount`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    File,
    Module,
    Process,
    Network,
    Dns,
    RegistryKey,
    RegistryValue,
}

impl Class {
    /// OCSF 1.9.0 `class_uid`, as atlas-schema derives it.
    pub const fn uid(self) -> u32 {
        match self {
            Class::File => 1001,
            Class::Module => 1005,
            Class::Process => 1007,
            Class::Network => 4001,
            Class::Dns => 4003,
            Class::RegistryKey => 201001,
            Class::RegistryValue => 201002,
        }
    }
}

/// Counters kept by the pipeline thread. Counters count occurrences; the
/// report takes the difference between two snapshots (1b-1, D1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    // Loss.
    pub actor_dropped: BTreeMap<Class, u64>,
    // Quality.
    pub late_arrivals: u64,
    pub launch_join_miss: u64,
    pub actor_unresolved: BTreeMap<Class, u64>,
    pub unknown_file_object: u64,
    pub registry_unresolved: u64,
    pub value_read_failed: u64,
    pub early_read_redone: u64,
    pub reg_type_unusual: u64,
    /// A value name with more than one possible end (plan 1b-2, clarification 5).
    pub reg_name_ambiguous: u64,
    pub file_op_late_failure: u64,
    pub writes_after_cleanup: u64,
    pub file_object_replaced: u64,
    pub enrichment_misses: u64,
    pub enrichment_errors: u64,
    // Housekeeping.
    pub process_cache_evictions: u64,
    pub file_map_evictions: u64,
    pub key_map_evictions: u64,
    pub flow_table_evictions: u64,
    pub file_op_failed: u64,
    pub pending_overflow: u64,
    pub seeder_deferred: u64,
    /// Events dropped at emission because their actor is the agent (§5.5).
    pub self_filtered: u64,
}

impl Counters {
    pub fn add_class(map: &mut BTreeMap<Class, u64>, class: Class) {
        *map.entry(class).or_default() += 1;
    }
}

/// Counters kept by the ETW callbacks, which run on the consumer threads.
#[derive(Debug, Default)]
pub struct IntakeCounters {
    pub kernel_queue_drops: AtomicU64,
    pub user_queue_drops: AtomicU64,
    pub dns_rate_limit_drops: AtomicU64,
    pub parse_errors: AtomicU64,
    pub unknown_version: AtomicU64,
    pub early_key_map_evictions: AtomicU64,
    /// Successful OperationEnds discarded in the callback (§3.2): not a loss.
    pub op_end_discarded: AtomicU64,
    /// Value reads sent on the fast path (§7.5).
    pub fast_reads: AtomicU64,
}

impl IntakeCounters {
    pub fn bump(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Ordering::Relaxed)
    }
}
```

- [ ] **Step 3: Bounded maps, paths, the key map, the watchlist**

`src/evict.rs`:
```rust
//! Bounded maps evict in batches: when a map passes its cap, the oldest eighth
//! goes in one pass. Evicting one entry per insert would scan the whole map on
//! every insert once it is full (O(n) each, at thousands of events a second).

/// The keys of the `len / 8` (at least one) entries with the smallest `age`.
pub fn oldest<K: Copy, A: Ord + Copy>(entries: impl Iterator<Item = (K, A)>, len: usize) -> Vec<K> {
    let mut all: Vec<(A, K)> = entries.map(|(k, a)| (a, k)).collect();
    let n = (len / 8).max(1).min(all.len());
    if n == 0 {
        return Vec::new();
    }
    if n < all.len() {
        all.select_nth_unstable_by_key(n - 1, |(a, _)| *a);
    }
    all.truncate(n);
    all.into_iter().map(|(_, k)| k).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_oldest_eighth() {
        let entries = (0..16u32).map(|k| (k, 100 - k)); // key 15 is the oldest
        let mut got = oldest(entries, 16);
        got.sort_unstable();
        assert_eq!(got, [14, 15]);
        assert_eq!(oldest((0..3u32).map(|k| (k, k)), 3), [0]);
        assert!(oldest(std::iter::empty::<(u32, u32)>(), 0).is_empty());
    }
}
```

`src/recent.rs`:
```rust
//! Keys with the time they were last seen, forgotten oldest first. Inserting
//! and expiring cost O(1) on average, with no scans: the pipeline expires these
//! sets every tick, and a scan of a large map per event or per tick is the
//! trap `evict` describes.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub struct Recent<K> {
    map: HashMap<K, i64>,
    /// Insertion order, with the time inserted. An entry whose time no longer
    /// matches the map (the key was seen again, or removed) is stale and skipped.
    order: VecDeque<(i64, K)>,
    cap: usize,
    evictions: u64,
}

impl<K: Hash + Eq + Clone> Recent<K> {
    /// At most `cap` keys (and queued times) are kept; past it the oldest go.
    pub fn new(cap: usize) -> Self {
        Recent { map: HashMap::new(), order: VecDeque::new(), cap: cap.max(1), evictions: 0 }
    }

    /// Records `key` at `ts`; the latest time wins.
    pub fn insert(&mut self, key: K, ts: i64) {
        match self.map.get_mut(&key) {
            Some(t) if *t >= ts => return,
            Some(t) => *t = ts,
            None => {
                self.map.insert(key.clone(), ts);
            }
        }
        self.order.push_back((ts, key));
        while self.order.len() > self.cap {
            self.pop_front(true);
        }
    }

    pub fn get(&self, key: &K) -> Option<i64> {
        self.map.get(key).copied()
    }

    pub fn remove(&mut self, key: &K) -> Option<i64> {
        self.map.remove(key)
    }

    /// Forgets keys last seen before `before` (in insertion order: a key
    /// inserted out of time order may stay a little longer).
    pub fn expire(&mut self, before: i64) {
        while self.order.front().is_some_and(|(ts, _)| *ts < before) {
            self.pop_front(false);
        }
    }

    fn pop_front(&mut self, evicting: bool) {
        let Some((ts, key)) = self.order.pop_front() else { return };
        if self.map.get(&key) == Some(&ts) {
            self.map.remove(&key);
            if evicting {
                self.evictions += 1;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_time_wins_and_old_keys_expire() {
        let mut r = Recent::new(100);
        r.insert("a", 10);
        r.insert("b", 20);
        r.insert("a", 30); // seen again
        r.insert("a", 5); // older: ignored
        r.expire(25);
        assert_eq!((r.get(&"a"), r.get(&"b")), (Some(30), None));
        assert_eq!(r.remove(&"a"), Some(30));
        assert!(r.is_empty());
    }

    #[test]
    fn the_cap_drops_the_oldest() {
        let mut r = Recent::new(3);
        for (k, ts) in [(1, 1), (2, 2), (3, 3), (4, 4)] {
            r.insert(k, ts);
        }
        assert_eq!((r.len(), r.get(&1), r.evictions()), (3, None, 1));
        // Repeated sightings of one key are bounded too.
        for ts in 5..100 {
            r.insert(9, ts);
        }
        assert!(r.order.len() <= 3);
    }
}
```

`src/paths.rs`:
```rust
//! Path normalization (sensor spec §5.5, §7.2).

use atlas_schema::limits::truncate_utf8;

/// `s` cut to at most `max` UTF-8 bytes on a character boundary. The schema
/// rejects longer strings, and paths and names have no `*_truncated` flag, so a
/// pathological NT path (up to 32,767 UTF-16 units) is cut rather than making
/// the event invalid.
pub fn fit(s: String, max: usize) -> String {
    match truncate_utf8(&s, max) {
        (_, false) => s,
        (t, true) => t.to_string(),
    }
}

/// Registry NT names to the forms Sigma uses (§5.5): `\REGISTRY\MACHINE\…` →
/// `HKLM\…`, `\REGISTRY\USER\…` → `HKU\…`, and `HKLM\SYSTEM\ControlSet00N\…` →
/// `HKLM\SYSTEM\CurrentControlSet\…` when N is the current control set.
/// The prefixes match case-insensitively (the kernel logs both `\REGISTRY\MACHINE`
/// and `\Registry\Machine`); the rest keeps its case.
pub fn registry(nt: &str, current_control_set: u32) -> String {
    let (root, rest) = if let Some(r) = strip_prefix_ci(nt, r"\REGISTRY\MACHINE") {
        ("HKLM", r)
    } else if let Some(r) = strip_prefix_ci(nt, r"\REGISTRY\USER") {
        ("HKU", r)
    } else {
        return nt.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('\\')) {
        return nt.to_string(); // `\REGISTRY\MACHINEX` is not HKLM
    }
    let mut out = String::with_capacity(nt.len());
    out.push_str(root);
    let current = format!(r"\SYSTEM\ControlSet{current_control_set:03}");
    match (root, strip_prefix_ci(rest, &current)) {
        ("HKLM", Some(tail)) if tail.is_empty() || tail.starts_with('\\') => {
            out.push_str(r"\SYSTEM\CurrentControlSet");
            out.push_str(tail);
        }
        _ => out.push_str(rest),
    }
    out
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// The path with its volume removed, for watchlist matching (§7.2):
/// `\Device\HarddiskVolumeShadowCopy3\Windows\x` → `\Windows\x`. A path that
/// is not `\Device\<volume>\…` is returned as is.
pub fn volume_relative(nt: &str) -> &str {
    let Some(rest) = strip_prefix_ci(nt, r"\Device\") else { return nt };
    match rest.find('\\') {
        Some(i) => &rest[i..],
        None => "",
    }
}

/// Removes an alternate data stream from the last component (§7.2):
/// `file.txt:stream` and `file::$DATA` match `file.txt` and `file`.
pub fn strip_stream(path: &str) -> &str {
    let start = path.rfind('\\').map_or(0, |i| i + 1);
    match path[start..].find(':') {
        Some(i) => &path[..start + i],
        None => path,
    }
}

/// Whether a component looks like an 8.3 short name (§7.2):
/// `^[^.~]{1,6}~[0-9]+(\.[^.]{0,3})?$`.
pub fn is_short_name(component: &str) -> bool {
    let (base, ext) = match component.split_once('.') {
        Some((b, e)) => (b, Some(e)),
        None => (component, None),
    };
    if ext.is_some_and(|e| e.len() > 3 || e.contains('.')) {
        return false;
    }
    let Some((stem, digits)) = base.split_once('~') else { return false };
    (1..=6).contains(&stem.chars().count())
        && !stem.contains(['.', '~'])
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Whether any component of the path is an 8.3 short name.
pub fn has_short_name(path: &str) -> bool {
    path.split('\\').any(is_short_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cuts_on_a_character_boundary() {
        assert_eq!(fit("abc".into(), 3), "abc");
        assert_eq!(fit("aé".into(), 2), "a"); // é is two bytes
    }

    #[test]
    fn registry_roots_and_control_set() {
        assert_eq!(registry(r"\REGISTRY\MACHINE\SOFTWARE\x", 1), r"HKLM\SOFTWARE\x");
        assert_eq!(registry(r"\Registry\Machine\Software\x", 1), r"HKLM\Software\x");
        assert_eq!(registry(r"\REGISTRY\USER\S-1-5-21-1-2-3-1001\Software", 1), r"HKU\S-1-5-21-1-2-3-1001\Software");
        assert_eq!(
            registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet001\Services\x", 1),
            r"HKLM\SYSTEM\CurrentControlSet\Services\x"
        );
        assert_eq!(registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet001", 1), r"HKLM\SYSTEM\CurrentControlSet");
        // Not the current set, and not a component boundary.
        assert_eq!(registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet002\x", 1), r"HKLM\SYSTEM\ControlSet002\x");
        assert_eq!(registry(r"\REGISTRY\MACHINE\SYSTEM\ControlSet0011", 1), r"HKLM\SYSTEM\ControlSet0011");
        assert_eq!(registry(r"\REGISTRY\MACHINE", 1), "HKLM");
        assert_eq!(registry(r"\REGISTRY\MACHINEX\a", 1), r"\REGISTRY\MACHINEX\a");
        assert_eq!(registry(r"\REGISTRY\A\{guid}", 1), r"\REGISTRY\A\{guid}");
        assert_eq!(registry(r"Software\Relative", 1), r"Software\Relative");
    }

    #[test]
    fn volume_relative_paths() {
        assert_eq!(
            volume_relative(r"\Device\HarddiskVolume3\Windows\System32\config\SAM"),
            r"\Windows\System32\config\SAM"
        );
        assert_eq!(
            volume_relative(r"\Device\HarddiskVolumeShadowCopy7\Windows\NTDS\ntds.dit"),
            r"\Windows\NTDS\ntds.dit"
        );
        assert_eq!(volume_relative(r"\Device\HarddiskVolume3"), "");
        assert_eq!(volume_relative(r"C:\x"), r"C:\x");
    }

    #[test]
    fn streams_are_stripped_from_the_last_component_only() {
        assert_eq!(strip_stream(r"\a\file.txt:secret"), r"\a\file.txt");
        assert_eq!(strip_stream(r"\a\file::$DATA"), r"\a\file");
        assert_eq!(strip_stream(r"\a\file.txt"), r"\a\file.txt");
    }

    #[test]
    fn short_names() {
        for s in ["ATLASS~1", "LONG-F~1.TXT", "a~12", "PROGRA~1", "x~1.c"] {
            assert!(is_short_name(s), "{s}");
        }
        for s in ["readme.txt", "a~", "~1", "TOOLONGN~1", "a.b~1", "a~1.long", "a~x", "a~1.b.c"] {
            assert!(!is_short_name(s), "{s}");
        }
        assert!(has_short_name(r"\Device\HarddiskVolume3\Users\ATLASS~1\x.txt"));
        assert!(!has_short_name(r"\Device\HarddiskVolume3\Users\jake\x.txt"));
    }
}
```

`src/keymap.rs`:
```rust
//! The registry key map (sensor spec §7.4): `KeyObject → name`.
//!
//! Kernel-Registry names a key relative to a base handle unless the name starts
//! with `\REGISTRY\` (S7). A relative open whose base is named gets its full
//! name at once; otherwise it waits for its base (pending child) and is named,
//! recursively, when the base is. A closed base with pending children stays as
//! a tombstone until they are named or evicted. Names are kept raw (NT form);
//! normalization happens at emission.
//!
//! Key objects are per handle (S7), so a successful open at an address is a new
//! object there. Children waiting on the address belonged to the earlier object:
//! they become orphans, which only the seeder naming them directly can name.
//! The same holds when an unknown base closes, and when the seeder names a
//! tombstone's address after the tombstone closed (plan 1b-3a review B3).

use std::collections::{HashMap, HashSet};

/// What the map knows about a key handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Name {
    /// The full NT name.
    Full(String),
    /// `rel` below `base`, whose name is not known yet.
    Below { base: u64, rel: String },
    /// `rel` below a base object that is gone: only a name for this handle
    /// itself can name it.
    Orphan { rel: String },
}

#[derive(Debug, Clone)]
struct Entry {
    name: Name,
    /// QPC of the event (or snapshot) that set it.
    since: i64,
    /// QPC of its CloseKey: a tombstone kept for its waiting children.
    closed: Option<i64>,
    /// For LRU eviction.
    touched: u64,
}

/// What resolving a handle gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Full(String),
    /// The longest name known: the chain of relative names below the first
    /// unnamed base (`root`), or empty if nothing is known (§7.4 floor).
    Partial {
        known: String,
        root: u64,
    },
}

pub struct KeyMap {
    map: HashMap<u64, Entry>,
    /// base → children waiting for it.
    waiting: HashMap<u64, HashSet<u64>>,
    cap: usize,
    clock: u64,
    evictions: u64,
}

const MAX_DEPTH: usize = 64;

impl KeyMap {
    pub fn new(cap: usize) -> Self {
        KeyMap { map: HashMap::new(), waiting: HashMap::new(), cap, clock: 0, evictions: 0 }
    }

    /// A successful CreateKey or OpenKey (§7.4).
    pub fn open(&mut self, key: u64, base: u64, relative: &str, ts: i64) {
        self.orphan_children(key);
        let name = if starts_with_ci(relative, r"\REGISTRY\") {
            Name::Full(relative.to_string())
        } else {
            match self.map.get(&base).map(|e| &e.name) {
                Some(Name::Full(b)) => Name::Full(join(b, relative)),
                _ => Name::Below { base, rel: relative.to_string() },
            }
        };
        self.set(key, name, ts);
    }

    /// A name from the seeder (§7.4). Never overwrites an entry an ETW event
    /// set after the snapshot was taken.
    pub fn seed(&mut self, key: u64, full: String, taken: i64) -> bool {
        match self.map.get(&key) {
            Some(e) if e.since > taken => return false,
            // Read after the tombstone closed: the name is a later object's.
            Some(e) if e.closed.is_some_and(|c| c <= taken) => self.orphan_children(key),
            _ => {}
        }
        self.set(key, Name::Full(full), taken);
        true
    }

    /// The children waiting on `key` lose their base.
    fn orphan_children(&mut self, key: u64) {
        for child in self.waiting.remove(&key).unwrap_or_default() {
            if let Some(e) = self.map.get_mut(&child)
                && let Name::Below { rel, .. } = &mut e.name
            {
                e.name = Name::Orphan { rel: std::mem::take(rel) };
            }
        }
    }

    fn set(&mut self, key: u64, name: Name, since: i64) {
        self.unlink(key);
        if let Name::Below { base, .. } = &name {
            self.waiting.entry(*base).or_default().insert(key);
        }
        let named = matches!(name, Name::Full(_));
        self.clock += 1;
        self.map.insert(key, Entry { name, since, closed: None, touched: self.clock });
        if named {
            self.name_children(key);
        }
        if self.map.len() > self.cap {
            self.evict_batch();
        }
    }

    /// Names the children waiting for `key`, recursively.
    fn name_children(&mut self, key: u64) {
        let mut stack = vec![key];
        while let Some(base) = stack.pop() {
            let Some(Name::Full(b)) = self.map.get(&base).map(|e| e.name.clone()) else { continue };
            for child in self.waiting.remove(&base).unwrap_or_default() {
                if let Some(e) = self.map.get_mut(&child)
                    && let Name::Below { rel, .. } = &e.name
                {
                    e.name = Name::Full(join(&b, rel));
                    stack.push(child);
                }
            }
            self.drop_if_dead(base);
        }
    }

    /// CloseKey at `ts` (§7.4): removes the entry, or keeps it as a tombstone
    /// while children wait for it. An unknown base that closes orphans its
    /// children: its name can no longer be learned.
    pub fn close(&mut self, key: u64, ts: i64) {
        let has_children = self.waiting.get(&key).is_some_and(|c| !c.is_empty());
        match self.map.get_mut(&key) {
            Some(e) if has_children => e.closed = Some(ts),
            Some(_) => self.remove(key),
            None => self.orphan_children(key),
        }
    }

    /// When the address was last set by an event or snapshot.
    pub fn since(&self, key: u64) -> Option<i64> {
        self.map.get(&key).map(|e| e.since)
    }

    pub fn contains(&self, key: u64) -> bool {
        self.map.contains_key(&key)
    }

    /// The name of a handle (§7.4). `None` if the address is not in the map.
    pub fn resolve(&mut self, key: u64) -> Option<Resolved> {
        self.clock += 1;
        if let Some(e) = self.map.get_mut(&key) {
            e.touched = self.clock;
        }
        let mut rels: Vec<&str> = Vec::new();
        let mut at = key;
        for _ in 0..MAX_DEPTH {
            match &self.map.get(&at)?.name {
                Name::Full(n) => {
                    let mut out = n.clone();
                    for r in rels.iter().rev() {
                        out = join(&out, r);
                    }
                    return Some(Resolved::Full(out));
                }
                Name::Below { base, rel } => {
                    rels.push(rel);
                    if !self.map.contains_key(base) {
                        let known = rels.iter().rev().copied().collect::<Vec<_>>().join("\\");
                        return Some(Resolved::Partial { known, root: *base });
                    }
                    at = *base;
                }
                Name::Orphan { rel } => {
                    rels.push(rel);
                    let known = rels.iter().rev().copied().collect::<Vec<_>>().join("\\");
                    return Some(Resolved::Partial { known, root: at });
                }
            }
        }
        // A cycle or an absurd depth: give what is known, never loop.
        Some(Resolved::Partial { known: rels.iter().rev().copied().collect::<Vec<_>>().join("\\"), root: at })
    }

    fn unlink(&mut self, key: u64) {
        if let Some(Entry { name: Name::Below { base, .. }, .. }) = self.map.get(&key) {
            let base = *base;
            if let Some(c) = self.waiting.get_mut(&base) {
                c.remove(&key);
                if c.is_empty() {
                    self.waiting.remove(&base);
                }
            }
            self.drop_if_dead(base);
        }
    }

    /// A tombstone with no waiting children goes.
    fn drop_if_dead(&mut self, key: u64) {
        let waited_on = self.waiting.get(&key).is_some_and(|c| !c.is_empty());
        if !waited_on && self.map.get(&key).is_some_and(|e| e.closed.is_some()) {
            self.remove(key);
        }
    }

    fn remove(&mut self, key: u64) {
        self.unlink(key);
        self.map.remove(&key);
        // Children of a removed base keep waiting for its address; the seeder may still name it.
    }

    /// Over the cap: the least recently used eighth goes (`crate::evict`).
    fn evict_batch(&mut self) {
        let victims = crate::evict::oldest(self.map.iter().map(|(k, e)| (*k, e.touched)), self.map.len());
        for k in victims {
            self.unlink(k);
            if self.map.remove(&k).is_some() {
                self.evictions += 1;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len()).is_some_and(|h| h.eq_ignore_ascii_case(prefix))
}

fn join(base: &str, rel: &str) -> String {
    if rel.is_empty() {
        base.to_string()
    } else if base.ends_with('\\') {
        format!("{base}{rel}")
    } else {
        format!("{base}\\{rel}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const HKCU: &str = r"\REGISTRY\USER\S-1-5-21-1";

    fn full(s: &str) -> Option<Resolved> {
        Some(Resolved::Full(s.into()))
    }

    #[test]
    fn absolute_and_relative_opens() {
        let mut m = KeyMap::new(100);
        m.open(1, 0, HKCU, 10);
        m.open(2, 1, "Software", 11);
        m.open(3, 2, r"Atlas\A", 12);
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\Software\Atlas\A")));
        // Case of the prefix does not matter.
        m.open(4, 0, r"\Registry\Machine\Software", 13);
        assert_eq!(m.resolve(4), full(r"\Registry\Machine\Software"));
    }

    #[test]
    fn a_child_of_an_unknown_base_is_partial_then_named_by_the_seeder() {
        let mut m = KeyMap::new(100);
        m.open(2, 1, "Software", 10);
        m.open(3, 2, "Run", 11);
        assert_eq!(m.resolve(3), Some(Resolved::Partial { known: r"Software\Run".into(), root: 1 }));
        assert!(m.seed(1, HKCU.into(), 5));
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\Software\Run")));
        assert_eq!(m.resolve(99), None);
    }

    #[test]
    fn a_closed_base_stays_while_children_wait() {
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10); // waits for 1
        m.open(3, 2, "b", 11); // waits for 2
        m.close(2, 11);
        assert!(m.contains(2)); // tombstone: 3 still waits for it
        m.seed(1, HKCU.into(), 12);
        assert!(!m.contains(2)); // named its child, then went
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\a\b")));
        m.close(3, 13);
        assert!(m.is_empty() || !m.contains(3));
    }

    #[test]
    fn a_reused_or_closed_base_does_not_name_old_children() {
        // 0x50 was opened before we watched; "Run" waits for it.
        let mut m = KeyMap::new(100);
        m.open(0x60, 0x50, "Run", 10);
        m.close(0x50, 11); // the old base object is gone
        m.open(0x50, 0, r"\REGISTRY\MACHINE\SOFTWARE\Benign", 12);
        // Not named "Benign\Run": only a name for 0x60 itself helps.
        assert_eq!(m.resolve(0x60), Some(Resolved::Partial { known: "Run".into(), root: 0x60 }));
        m.seed(0x60, format!(r"{HKCU}\Software\Run"), 20);
        assert_eq!(m.resolve(0x60), full(&format!(r"{HKCU}\Software\Run")));

        // A reopen with no close seen (an agent close is ignored) also orphans.
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10);
        m.open(1, 0, r"\REGISTRY\MACHINE\x", 11);
        assert_eq!(m.resolve(2), Some(Resolved::Partial { known: "a".into(), root: 2 }));

        // A tombstone named by a snapshot read after it closed: orphans too;
        // one read before the close names the children.
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10); // 2 waits for 1
        m.open(3, 2, "b", 11); // 3 waits for 2
        m.open(4, 5, "c", 12); // 4 waits for 5
        m.close(2, 13);
        m.seed(2, r"\REGISTRY\MACHINE\new".into(), 14);
        assert!(matches!(m.resolve(3), Some(Resolved::Partial { root: 3, .. })));
        m.close(5, 30);
        assert!(matches!(m.resolve(4), Some(Resolved::Partial { root: 4, .. })));
        let mut m = KeyMap::new(100);
        m.open(2, 1, "a", 10);
        m.open(3, 2, "b", 11);
        m.close(2, 13);
        m.seed(1, HKCU.into(), 12); // read before the close
        assert_eq!(m.resolve(3), full(&format!(r"{HKCU}\a\b")));
    }

    #[test]
    fn a_seed_never_overwrites_a_newer_event() {
        let mut m = KeyMap::new(100);
        m.open(1, 0, r"\REGISTRY\MACHINE\new", 20);
        assert!(!m.seed(1, r"\REGISTRY\MACHINE\old".into(), 10));
        assert_eq!(m.resolve(1), full(r"\REGISTRY\MACHINE\new"));
        assert!(m.seed(1, r"\REGISTRY\MACHINE\newer".into(), 30));
    }

    #[test]
    fn reopening_an_address_replaces_it() {
        let mut m = KeyMap::new(100);
        m.open(1, 0, r"\REGISTRY\MACHINE\a", 1);
        m.open(1, 0, r"\REGISTRY\MACHINE\b", 2);
        assert_eq!(m.resolve(1), full(r"\REGISTRY\MACHINE\b"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn the_cap_evicts_the_least_recently_used() {
        let mut m = KeyMap::new(2);
        m.open(1, 0, r"\REGISTRY\MACHINE\a", 1);
        m.open(2, 0, r"\REGISTRY\MACHINE\b", 2);
        m.resolve(1);
        m.open(3, 0, r"\REGISTRY\MACHINE\c", 3);
        assert_eq!(m.evictions(), 1);
        assert!(m.contains(1) && !m.contains(2) && m.contains(3));
        // Past a larger cap, an eighth goes at once.
        let mut m = KeyMap::new(16);
        for k in 0..17 {
            m.open(k, 0, r"\REGISTRY\MACHINE\x", k as i64);
        }
        assert_eq!((m.len(), m.evictions()), (15, 2));
    }

    #[test]
    fn a_cycle_cannot_loop() {
        let mut m = KeyMap::new(100);
        m.open(1, 2, "a", 1);
        m.open(2, 1, "b", 2);
        assert!(matches!(m.resolve(1), Some(Resolved::Partial { .. })));
    }

    proptest! {
        /// Any chain of relative opens resolves to the base's name plus every
        /// relative part, whether the base is named before or after.
        #[test]
        fn chains_resolve_whatever_the_order(parts in proptest::collection::vec("[a-z]{1,4}", 1..8), seed_first: bool) {
            let mut m = KeyMap::new(1000);
            if seed_first { m.seed(100, HKCU.into(), 0); }
            for (i, p) in parts.iter().enumerate() {
                m.open(101 + i as u64, 100 + i as u64, p, 10 + i as i64);
            }
            if !seed_first { m.seed(100, HKCU.into(), 0); }
            let want = format!("{HKCU}\\{}", parts.join("\\"));
            prop_assert_eq!(m.resolve(100 + parts.len() as u64), full(&want));
        }
    }
}
```

`src/watchlist.rs`:
```rust
//! The sensitive-path watchlist (sensor spec §7.2): a successful Create on a
//! matching path emits File System Activity `Open`.
//!
//! Patterns are volume-relative and case-insensitive, compiled once into one
//! `GlobSet`. They are matched against the path with its volume and any
//! alternate data stream removed, so they match shadow copies too. `*` stays
//! within one component; `**` crosses components.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::paths::{strip_stream, volume_relative};

/// The built-in list (§7.2), replaceable or extendable in config.
pub const DEFAULT: &[&str] = &[
    // Chromium browsers (Chrome, Edge, Brave, …): saved passwords, cookies, the key.
    r"\Users\*\AppData\Local\**\User Data\*\Login Data",
    r"\Users\*\AppData\Local\**\User Data\*\Cookies",
    r"\Users\*\AppData\Local\**\User Data\*\Network\Cookies",
    r"\Users\*\AppData\Local\**\User Data\Local State",
    // Firefox.
    r"\Users\*\AppData\Roaming\Mozilla\Firefox\Profiles\*\logins.json",
    r"\Users\*\AppData\Roaming\Mozilla\Firefox\Profiles\*\key4.db",
    // Registry hives and their copies.
    r"\Windows\System32\config\SAM",
    r"\Windows\System32\config\SECURITY",
    r"\Windows\System32\config\SYSTEM",
    r"\Windows\System32\config\*.sav",
    r"\Windows\System32\config\*.bak",
    // Active Directory.
    r"\Windows\NTDS\ntds.dit",
    // Keys and cloud credentials.
    r"\Users\*\.ssh\*",
    r"\Users\*\.aws\credentials",
    r"\Users\*\.azure\**",
    r"\Users\*\AppData\Roaming\gcloud\**",
    // KeePass databases anywhere.
    r"**\*.kdbx",
];

pub struct Watchlist {
    set: GlobSet,
}

/// A pattern that does not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadPattern {
    pub pattern: String,
    pub error: String,
}

impl Watchlist {
    /// `replace`: use these patterns instead of [`DEFAULT`]; `extend`: add these.
    pub fn new(replace: Option<&[String]>, extend: &[String]) -> Result<Self, BadPattern> {
        let mut b = GlobSetBuilder::new();
        let base: Vec<String> = match replace {
            Some(r) => r.to_vec(),
            None => DEFAULT.iter().map(|s| s.to_string()).collect(),
        };
        for p in base.iter().chain(extend) {
            let glob = GlobBuilder::new(&slashes(p))
                .case_insensitive(true)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map_err(|e| BadPattern { pattern: p.clone(), error: e.to_string() })?;
            b.add(glob);
        }
        let set = b.build().map_err(|e| BadPattern { pattern: String::new(), error: e.to_string() })?;
        Ok(Watchlist { set })
    }

    /// Whether an NT path (`\Device\HarddiskVolume3\…`) is on the list.
    pub fn matches(&self, nt_path: &str) -> bool {
        let p = strip_stream(volume_relative(nt_path));
        !p.is_empty() && self.set.is_match(slashes(p))
    }
}

/// globset separates on `/`; Windows paths use `\`.
fn slashes(s: &str) -> String {
    s.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wl() -> Watchlist {
        Watchlist::new(None, &[]).unwrap()
    }

    #[test]
    fn the_default_list() {
        let w = wl();
        for p in [
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM",
            r"\Device\HarddiskVolume3\windows\system32\CONFIG\sam",
            r"\Device\HarddiskVolumeShadowCopy4\Windows\System32\config\SYSTEM",
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM.sav",
            r"\Device\HarddiskVolume3\Windows\NTDS\ntds.dit",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Default\Login Data",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Microsoft\Edge\User Data\Profile 1\Network\Cookies",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Local\Google\Chrome\User Data\Local State",
            r"\Device\HarddiskVolume3\Users\jake\AppData\Roaming\Mozilla\Firefox\Profiles\ab.default\key4.db",
            r"\Device\HarddiskVolume3\Users\jake\.ssh\id_ed25519",
            r"\Device\HarddiskVolume3\Users\jake\.aws\credentials",
            r"\Device\HarddiskVolume3\Users\jake\.azure\a\b",
            r"\Device\HarddiskVolume3\Users\jake\Documents\vault.KDBX",
            // Alternate data streams are stripped (§7.2).
            r"\Device\HarddiskVolume3\Windows\System32\config\SAM::$DATA",
        ] {
            assert!(w.matches(p), "{p}");
        }
        for p in [
            r"\Device\HarddiskVolume3\Windows\System32\config\SAMx",
            r"\Device\HarddiskVolume3\Windows\System32\cmd.exe",
            r"\Device\HarddiskVolume3\Users\jake\.ssh", // the directory itself
            r"\Device\HarddiskVolume3\Users\a\b\.ssh\id_rsa", // * is one component
            r"\Device\HarddiskVolume3",
        ] {
            assert!(!w.matches(p), "{p}");
        }
    }

    #[test]
    fn replace_and_extend() {
        let w = Watchlist::new(Some(&[r"\secret\*".to_string()]), &[r"**\*.pem".to_string()]).unwrap();
        assert!(w.matches(r"\Device\HarddiskVolume3\secret\x"));
        assert!(w.matches(r"\Device\HarddiskVolume3\a\b\c.pem"));
        assert!(!w.matches(r"\Device\HarddiskVolume3\Windows\System32\config\SAM"));
    }

    #[test]
    fn a_bad_pattern_is_reported() {
        let e = Watchlist::new(None, &["[".to_string()]).err().unwrap();
        assert_eq!(e.pattern, "[");
    }
}
```

- [ ] **Step 4: The ordering and completion stages**

`src/ordering.rs`:
```rust
//! The ordering stage (sensor spec §3.2 [2]): holds each event until
//! `now − event time ≥ hold`, then releases events in timestamp order. An event
//! older than the last one released is late: it passes straight through, out of
//! order, and is counted. Nothing is ever dropped here.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::input::Incoming;

struct Held {
    ts: i64,
    /// Arrival order, so equal timestamps keep their order.
    seq: u64,
    inc: Incoming,
}

impl PartialEq for Held {
    fn eq(&self, o: &Self) -> bool {
        (self.ts, self.seq) == (o.ts, o.seq)
    }
}
impl Eq for Held {}
impl PartialOrd for Held {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Held {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.ts, self.seq).cmp(&(o.ts, o.seq))
    }
}

pub struct Ordering {
    heap: BinaryHeap<Reverse<Held>>,
    seq: u64,
    /// The hold, in QPC ticks.
    hold: i64,
    /// Timestamp of the last event released in order.
    last_released: i64,
}

/// What `push` did with an event.
#[derive(Debug, PartialEq, Eq)]
pub enum Pushed {
    Held,
    /// Older than an event already released: process it now (§3.2).
    Late(Incoming),
}

impl Ordering {
    pub fn new(hold_ticks: i64) -> Self {
        Ordering { heap: BinaryHeap::new(), seq: 0, hold: hold_ticks, last_released: i64::MIN }
    }

    pub fn push(&mut self, inc: Incoming) -> Pushed {
        if inc.header.ts < self.last_released {
            return Pushed::Late(inc);
        }
        self.seq += 1;
        self.heap.push(Reverse(Held { ts: inc.header.ts, seq: self.seq, inc }));
        Pushed::Held
    }

    /// Every held event whose hold has passed at `now`, in timestamp order.
    pub fn release(&mut self, now: i64) -> Vec<Incoming> {
        self.release_until(now.saturating_sub(self.hold))
    }

    /// Everything held, in order (a clean stop, §11.4).
    pub fn drain(&mut self) -> Vec<Incoming> {
        self.release_until(i64::MAX)
    }

    fn release_until(&mut self, limit: i64) -> Vec<Incoming> {
        let mut out = Vec::new();
        while self.heap.peek().is_some_and(|Reverse(h)| h.ts <= limit) {
            let Reverse(h) = self.heap.pop().expect("peeked");
            self.last_released = self.last_released.max(h.ts);
            out.push(h.inc);
        }
        out
    }

    /// Stream time (§3.2): `max(last released, now − hold)`. It advances while
    /// no events arrive.
    pub fn watermark(&self, now: i64) -> i64 {
        self.last_released.max(now.saturating_sub(self.hold))
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Header, Session};
    use atlas_etw::parse::{FileOpEnd, RawEvent};
    use proptest::prelude::*;

    fn inc(ts: i64, tag: u64) -> Incoming {
        Incoming {
            header: Header { session: Session::Sensor, pid: 1, tid: 1, ts, start_key: None },
            event: RawEvent::FileOpEnd(FileOpEnd { irp: tag, extra_information: 0, status: 0 }),
        }
    }

    fn tag(i: &Incoming) -> u64 {
        match &i.event {
            RawEvent::FileOpEnd(o) => o.irp,
            _ => unreachable!(),
        }
    }

    #[test]
    fn holds_then_releases_in_order() {
        let mut o = Ordering::new(100);
        assert_eq!(o.push(inc(50, 1)), Pushed::Held);
        assert_eq!(o.push(inc(10, 2)), Pushed::Held);
        assert!(o.release(105).is_empty()); // 105 − 100 = 5 < 10
        assert_eq!(o.release(150).iter().map(tag).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(o.watermark(150), 50);
        assert_eq!(o.watermark(1000), 900); // advances with no events
    }

    #[test]
    fn a_late_event_passes_through() {
        let mut o = Ordering::new(100);
        o.push(inc(50, 1));
        o.release(150);
        assert!(matches!(o.push(inc(49, 2)), Pushed::Late(i) if tag(&i) == 2));
        assert_eq!(o.push(inc(50, 3)), Pushed::Held); // equal is not late
    }

    #[test]
    fn equal_timestamps_keep_arrival_order() {
        let mut o = Ordering::new(0);
        for t in 1..=5 {
            o.push(inc(7, t));
        }
        assert_eq!(o.drain().iter().map(tag).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
    }

    proptest! {
        /// Whatever the arrival order and release times: every event comes out
        /// exactly once; in-order releases never go back in time; an event is
        /// late exactly when it is older than something already released.
        #[test]
        fn releases_in_order_and_loses_nothing(
            stamps in proptest::collection::vec(0i64..1000, 1..200),
            ticks in proptest::collection::vec(0usize..4, 1..200),
        ) {
            let mut o = Ordering::new(100);
            let mut out = Vec::new();
            let mut late = Vec::new();
            let mut now = 0;
            for (i, ts) in stamps.iter().enumerate() {
                let max_released = out.iter().map(|x: &Incoming| x.header.ts).max();
                match o.push(inc(*ts, i as u64)) {
                    Pushed::Late(x) => {
                        prop_assert!(max_released.is_some_and(|m| x.header.ts < m));
                        late.push(x);
                    }
                    Pushed::Held => prop_assert!(max_released.is_none_or(|m| *ts >= m)),
                }
                now += 37 * ticks[i % ticks.len()] as i64;
                out.extend(o.release(now));
            }
            out.extend(o.drain());
            prop_assert!(out.windows(2).all(|w| w[0].header.ts <= w[1].header.ts));
            let mut seen: Vec<u64> = out.iter().chain(&late).map(tag).collect();
            seen.sort_unstable();
            prop_assert_eq!(seen, (0..stamps.len() as u64).collect::<Vec<_>>());
        }
    }
}
```

`src/completion.rs`:
```rust
//! The completion stage (sensor spec §3.2 [5]): emits events in the order the
//! pipeline produced them. A pending event holds the line until each of its
//! reasons is resolved or past its deadline; the pipeline can also cancel it.
//!
//! Deadlines are QPC times. A reason without one (the failure-confirm window,
//! which is stream time) is resolved or cancelled by the pipeline itself.

use std::collections::{HashMap, VecDeque};

/// Why an event is incomplete (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// Hashes and signature (§6.3).
    Enrich,
    /// The other half of a Launch (§5.2).
    Join,
    /// A registry value read (§7.5).
    ValueRead,
    /// An 8.3 path expansion (§7.2).
    Expand,
    /// A handle name from the seeder (§7.4).
    Seeder,
    /// The failure-confirm window (§5.5); resolved by the pipeline.
    Confirm,
}

/// One reason an event waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    pub reason: Reason,
    /// QPC deadline; `None` means the pipeline resolves it.
    pub deadline: Option<i64>,
    /// If this reason expires unresolved, the event is dropped instead of emitted (§7.1).
    pub drop_at_deadline: bool,
}

/// What happened to a pending event that left the stage incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expired {
    pub reason: Reason,
    pub dropped: bool,
}

pub type PendingId = u64;

enum Slot<E> {
    Ready(E),
    Pending(PendingId),
}

struct Entry<E> {
    event: E,
    waits: Vec<(Wait, bool)>,
    forced: bool,
}

pub struct Completion<E> {
    queue: VecDeque<Slot<E>>,
    pending: HashMap<PendingId, Entry<E>>,
    next: PendingId,
    cap: usize,
    overflow: u64,
    /// Pending events that left (emitted, dropped or cancelled) since the last
    /// `take_exited`: the pipeline frees their bookkeeping.
    exited: Vec<PendingId>,
    /// Pending ids in push order, for the overflow rule: the front is the
    /// oldest that may still be pending. Ids that left are popped as reached.
    order: VecDeque<PendingId>,
}

impl<E> Completion<E> {
    pub fn new(pending_cap: usize) -> Self {
        Completion {
            queue: VecDeque::new(),
            pending: HashMap::new(),
            next: 0,
            cap: pending_cap,
            overflow: 0,
            exited: Vec::new(),
            order: VecDeque::new(),
        }
    }

    /// An event with nothing to wait for.
    pub fn push(&mut self, event: E) {
        self.queue.push_back(Slot::Ready(event));
    }

    /// An event that waits; an empty `waits` makes it ready at the next drain.
    pub fn push_pending(&mut self, event: E, waits: Vec<Wait>) -> PendingId {
        self.next += 1;
        let id = self.next;
        self.pending.insert(id, Entry { event, waits: waits.into_iter().map(|w| (w, false)).collect(), forced: false });
        self.queue.push_back(Slot::Pending(id));
        self.order.push_back(id);
        if self.pending.len() > self.cap {
            // The oldest pending event goes out as is (§3.2). Each id is popped
            // once, so this costs O(1) per push on average, overload included.
            while let Some(oldest) = self.order.pop_front() {
                if let Some(e) = self.pending.get_mut(&oldest) {
                    e.forced = true;
                    self.overflow += 1;
                    break;
                }
            }
        }
        id
    }

    /// Changes a pending event (fills in a result). False if it is gone.
    pub fn update(&mut self, id: PendingId, f: impl FnOnce(&mut E)) -> bool {
        match self.pending.get_mut(&id) {
            Some(e) => {
                f(&mut e.event);
                true
            }
            None => false,
        }
    }

    /// Adds a reason to a pending event that is still waiting.
    pub fn add_wait(&mut self, id: PendingId, wait: Wait) -> bool {
        match self.pending.get_mut(&id) {
            Some(e) => {
                e.waits.push((wait, false));
                true
            }
            None => false,
        }
    }

    /// Marks every wait of this reason as resolved.
    pub fn resolve(&mut self, id: PendingId, reason: Reason) {
        if let Some(e) = self.pending.get_mut(&id) {
            for (w, done) in &mut e.waits {
                if w.reason == reason {
                    *done = true;
                }
            }
        }
    }

    /// Whether the event still waits for this reason.
    pub fn is_waiting(&self, id: PendingId, reason: Reason) -> bool {
        self.pending.get(&id).is_some_and(|e| e.waits.iter().any(|(w, done)| w.reason == reason && !done))
    }

    /// Drops a pending event (for example a delete whose operation failed).
    pub fn cancel(&mut self, id: PendingId) -> bool {
        let gone = self.pending.remove(&id).is_some();
        if gone {
            self.exited.push(id);
        }
        gone
    }

    /// The pending events that left since the last call.
    pub fn take_exited(&mut self) -> Vec<PendingId> {
        std::mem::take(&mut self.exited)
    }

    /// Emits everything that can go at `now`, in order. `on_incomplete` sees
    /// each event that leaves with unresolved reasons, and what happened to
    /// them, before it is emitted or dropped.
    pub fn drain(&mut self, now: i64, mut on_incomplete: impl FnMut(&mut E, &[Expired])) -> Vec<E> {
        let mut out = Vec::new();
        while let Some(front) = self.queue.front() {
            let id = match front {
                Slot::Ready(_) => {
                    let Some(Slot::Ready(e)) = self.queue.pop_front() else { unreachable!() };
                    out.push(e);
                    continue;
                }
                Slot::Pending(id) => *id,
            };
            let Some(entry) = self.pending.get(&id) else {
                self.queue.pop_front(); // cancelled
                continue;
            };
            let open: Vec<&Wait> = entry.waits.iter().filter(|(_, done)| !done).map(|(w, _)| w).collect();
            let blocked = !entry.forced && open.iter().any(|w| w.deadline.is_none_or(|d| now < d));
            if blocked {
                break;
            }
            let expired: Vec<Expired> = open
                .iter()
                .map(|w| Expired { reason: w.reason, dropped: w.drop_at_deadline && !entry.forced })
                .collect();
            self.queue.pop_front();
            let mut entry = self.pending.remove(&id).expect("present");
            self.exited.push(id);
            if !expired.is_empty() {
                on_incomplete(&mut entry.event, &expired);
            }
            if !expired.iter().any(|x| x.dropped) {
                out.push(entry.event);
            }
        }
        while self.order.front().is_some_and(|p| !self.pending.contains_key(p)) {
            self.order.pop_front();
        }
        out
    }

    /// Everything, in order (a clean stop, §11.4): unresolved reasons count as
    /// expired, as if their deadlines had passed, so a reason that drops at its
    /// deadline drops here too.
    pub fn flush(&mut self, mut on_incomplete: impl FnMut(&mut E, &[Expired])) -> Vec<E> {
        let mut out = Vec::new();
        while let Some(slot) = self.queue.pop_front() {
            match slot {
                Slot::Ready(e) => out.push(e),
                Slot::Pending(id) => {
                    if let Some(mut entry) = self.pending.remove(&id) {
                        self.exited.push(id);
                        let expired: Vec<Expired> = entry
                            .waits
                            .iter()
                            .filter(|(_, done)| !done)
                            .map(|(w, _)| Expired { reason: w.reason, dropped: w.drop_at_deadline && !entry.forced })
                            .collect();
                        if !expired.is_empty() {
                            on_incomplete(&mut entry.event, &expired);
                        }
                        if !expired.iter().any(|x| x.dropped) {
                            out.push(entry.event);
                        }
                    }
                }
            }
        }
        self.order.clear();
        out
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Events forced out by the pending cap so far (`pending_overflow`).
    pub fn overflow(&self) -> u64 {
        self.overflow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn w(reason: Reason, deadline: i64) -> Wait {
        Wait { reason, deadline: Some(deadline), drop_at_deadline: false }
    }

    #[test]
    fn a_pending_event_holds_the_line() {
        let mut c = Completion::new(10);
        c.push(1);
        let id = c.push_pending(2, vec![w(Reason::Enrich, 100)]);
        c.push(3);
        assert_eq!(c.drain(0, |_, _| {}), [1]);
        c.update(id, |e| *e = 20);
        c.resolve(id, Reason::Enrich);
        assert_eq!(c.drain(0, |_, _| {}), [20, 3]);
        assert!(c.is_empty());
    }

    #[test]
    fn a_deadline_emits_as_is_and_reports_it() {
        let mut c = Completion::new(10);
        c.push_pending(1, vec![w(Reason::Enrich, 100), w(Reason::Join, 50)]);
        assert!(c.drain(99, |_, _| {}).is_empty());
        let mut seen = Vec::new();
        assert_eq!(c.drain(100, |_, x| seen.extend_from_slice(x)), [1]);
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|x| !x.dropped));
    }

    #[test]
    fn a_clean_stop_drops_what_its_deadline_would() {
        let mut c = Completion::new(10);
        let a = c.push_pending(1, vec![Wait { reason: Reason::Seeder, deadline: Some(10), drop_at_deadline: true }]);
        let b = c.push_pending(2, vec![w(Reason::Enrich, 10)]);
        let x = c.push_pending(3, vec![w(Reason::Enrich, 10)]);
        c.cancel(x);
        assert_eq!(c.flush(|_, _| {}), [2]);
        let mut gone = c.take_exited();
        gone.sort_unstable();
        assert_eq!(gone, [a, b, x]);
    }

    #[test]
    fn drop_at_deadline_drops() {
        let mut c = Completion::new(10);
        c.push_pending(1, vec![Wait { reason: Reason::Seeder, deadline: Some(10), drop_at_deadline: true }]);
        c.push(2);
        let mut seen = Vec::new();
        assert_eq!(c.drain(10, |_, x| seen.extend_from_slice(x)), [2]);
        assert_eq!(seen, [Expired { reason: Reason::Seeder, dropped: true }]);
    }

    #[test]
    fn a_reason_without_deadline_waits_for_the_pipeline() {
        let mut c = Completion::new(10);
        let id = c.push_pending(1, vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: false }]);
        assert!(c.drain(i64::MAX, |_, _| {}).is_empty());
        assert!(c.is_waiting(id, Reason::Confirm));
        c.resolve(id, Reason::Confirm);
        assert_eq!(c.drain(0, |_, _| {}), [1]);
    }

    #[test]
    fn cancel_removes_without_emitting() {
        let mut c = Completion::new(10);
        let id = c.push_pending(1, vec![w(Reason::Confirm, 10)]);
        c.push(2);
        assert!(c.cancel(id));
        assert_eq!(c.drain(0, |_, _| {}), [2]);
        assert!(!c.update(id, |_| {}));
    }

    #[test]
    fn overflow_forces_the_oldest_out() {
        let mut c = Completion::new(2);
        for i in 0..3 {
            c.push_pending(i, vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: true }]);
        }
        assert_eq!(c.overflow(), 1);
        let mut seen = Vec::new();
        // The forced one leaves (as is, not dropped); the next still waits.
        assert_eq!(c.drain(0, |_, x| seen.extend_from_slice(x)), [0]);
        assert_eq!(seen, [Expired { reason: Reason::Confirm, dropped: false }]);
        assert_eq!(c.pending_len(), 2);
    }

    #[test]
    fn flush_emits_everything_in_order() {
        let mut c = Completion::new(10);
        c.push(1);
        c.push_pending(2, vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: false }]);
        c.push(3);
        assert_eq!(c.flush(|_, _| {}), [1, 2, 3]);
    }

    proptest! {
        /// Output order is push order, minus cancelled and dropped events,
        /// whatever order the results arrive in.
        #[test]
        fn order_is_preserved(ops in proptest::collection::vec((any::<bool>(), 0u8..4), 1..100)) {
            let mut c = Completion::new(1000);
            let mut ids = Vec::new();
            let mut expect = Vec::new();
            for (i, (pending, fate)) in ops.iter().enumerate() {
                if *pending {
                    let id = c.push_pending(i, vec![w(Reason::Enrich, 1_000)]);
                    ids.push((id, i, *fate));
                    if *fate != 0 { expect.push(i); }
                } else {
                    c.push(i);
                    expect.push(i);
                }
            }
            let mut out = c.drain(0, |_, _| {});
            for (id, _, fate) in ids.iter().rev() {
                match fate {
                    0 => { c.cancel(*id); }
                    1 => c.resolve(*id, Reason::Enrich),
                    _ => {}
                }
                out.extend(c.drain(0, |_, _| {}));
            }
            out.extend(c.drain(1_000, |_, _| {}));
            prop_assert_eq!(out, expect);
        }
    }
}
```

- [ ] **Step 5: The process cache and the Windows boundary**

`src/process.rs`:
```rust
//! The process cache (sensor spec §6.2) and process identity (§5.2).
//!
//! Entries are keyed by start key, never by PID. A PID index keeps each PID's
//! entries, so "the entry live at the event's timestamp" (§5.3) survives PID
//! reuse. An entry stays `retention` after its Terminate, then is removed.

use std::collections::HashMap;

use atlas_schema::{BootId, DeviceUid, File, Integrity, ProcessRef, ProcessUid, User, process_uid};

/// The start key of a new process (§5.2, S1): `(BootId << 48) | sequence`.
pub fn start_key(boot_id: u16, sequence: u64) -> u64 {
    (u64::from(boot_id) << 48) | (sequence & ((1 << 48) - 1))
}

/// Integrity from the mandatory label's RID (§5.2).
pub fn integrity(rid: u32) -> Option<Integrity> {
    Some(match rid {
        0 => Integrity::Untrusted,
        4096 => Integrity::Low,
        8192 | 8448 => Integrity::Medium,
        12288 => Integrity::High,
        16384 => Integrity::System,
        20480 | 28672 => Integrity::Protected,
        _ => return None,
    })
}

/// The last path component (`file.name`).
pub fn file_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

/// This machine and boot, for computing process uids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub device: DeviceUid,
    pub boot: BootId,
    /// `KUSER_SHARED_DATA.BootId` (§6.1): the start key's high 16 bits.
    pub kernel_boot_id: u16,
}

impl Identity {
    pub fn uid(&self, start_key: u64) -> ProcessUid {
        process_uid(&self.device, &self.boot, start_key)
    }

    /// A reference with only what the uid gives: empty path and name (E11).
    pub fn bare_ref(&self, start_key: u64, pid: u32) -> ProcessRef {
        ProcessRef { uid: self.uid(start_key), pid, file: empty_file(), user: None }
    }
}

pub fn empty_file() -> File {
    File { path: String::new(), name: String::new(), hashes: None, signature: None }
}

/// What the cache knows about one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub start_key: u64,
    pub pid: u32,
    /// Normalized image path (drive form when mappable).
    pub path: String,
    pub user: Option<User>,
    pub cmd_line: Option<String>,
    /// Unix ns.
    pub created_time: Option<i64>,
    pub integrity: Option<Integrity>,
    pub parent: Option<ProcessRef>,
}

impl ProcInfo {
    pub fn new(start_key: u64, pid: u32) -> Self {
        ProcInfo {
            start_key,
            pid,
            path: String::new(),
            user: None,
            cmd_line: None,
            created_time: None,
            integrity: None,
            parent: None,
        }
    }

    pub fn to_ref(&self, id: &Identity) -> ProcessRef {
        ProcessRef {
            uid: id.uid(self.start_key),
            pid: self.pid,
            file: File {
                path: self.path.clone(),
                name: file_name(&self.path).to_string(),
                hashes: None,
                signature: None,
            },
            user: self.user.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    info: ProcInfo,
    /// QPC of the Launch; `i64::MIN` for processes seeded at start (§6.2).
    start: i64,
    /// QPC of the Terminate.
    end: Option<i64>,
    /// For eviction: the last lookup (a long-running service is kept).
    touched: std::cell::Cell<u64>,
}

pub struct ProcessCache {
    by_key: HashMap<u64, Entry>,
    by_pid: HashMap<u32, Vec<u64>>,
    retention: i64,
    cap: usize,
    evictions: u64,
    clock: std::cell::Cell<u64>,
}

impl ProcessCache {
    pub fn new(retention_ticks: i64, cap: usize) -> Self {
        ProcessCache {
            by_key: HashMap::new(),
            by_pid: HashMap::new(),
            retention: retention_ticks,
            cap,
            evictions: 0,
            clock: std::cell::Cell::new(0),
        }
    }

    fn tick(&self) -> u64 {
        self.clock.set(self.clock.get() + 1);
        self.clock.get()
    }

    /// Adds or updates a process. `start` is the Launch's QPC (`i64::MIN` if it
    /// was running when the agent started).
    pub fn insert(&mut self, info: ProcInfo, start: i64) {
        let key = info.start_key;
        let pid = info.pid;
        match self.by_key.get_mut(&key) {
            Some(e) => {
                e.info = info;
                e.start = e.start.min(start);
            }
            None => {
                let touched = std::cell::Cell::new(self.tick());
                self.by_key.insert(key, Entry { info, start, end: None, touched });
                self.by_pid.entry(pid).or_default().push(key);
                if self.by_key.len() > self.cap {
                    self.evict_batch();
                }
            }
        }
    }

    pub fn get(&self, start_key: u64) -> Option<&ProcInfo> {
        self.by_key.get(&start_key).map(|e| &e.info)
    }

    pub fn get_mut(&mut self, start_key: u64) -> Option<&mut ProcInfo> {
        self.by_key.get_mut(&start_key).map(|e| &mut e.info)
    }

    /// Records the Terminate; the entry stays `retention` longer.
    pub fn end(&mut self, start_key: u64, ts: i64) {
        if let Some(e) = self.by_key.get_mut(&start_key) {
            e.end = Some(ts);
        }
    }

    /// The process with this PID live at `ts` (§5.3): the latest one started at
    /// or before `ts`, if it had not ended more than `retention` before.
    pub fn lookup_at(&self, pid: u32, ts: i64) -> Option<&ProcInfo> {
        let keys = self.by_pid.get(&pid)?;
        let e = keys.iter().filter_map(|k| self.by_key.get(k)).filter(|e| e.start <= ts).max_by_key(|e| e.start)?;
        e.touched.set(self.tick());
        e.end.is_none_or(|end| ts <= end.saturating_add(self.retention)).then_some(&e.info)
    }

    /// Removes entries whose retention has passed by stream time `now`.
    pub fn expire(&mut self, now: i64) {
        let gone: Vec<u64> = self
            .by_key
            .iter()
            .filter(|(_, e)| e.end.is_some_and(|end| end.saturating_add(self.retention) < now))
            .map(|(k, _)| *k)
            .collect();
        for k in gone {
            self.remove(k);
        }
    }

    fn remove(&mut self, key: u64) {
        if let Some(e) = self.by_key.remove(&key)
            && let Some(keys) = self.by_pid.get_mut(&e.info.pid)
        {
            keys.retain(|k| *k != key);
            if keys.is_empty() {
                self.by_pid.remove(&e.info.pid);
            }
        }
    }

    /// Over the cap: processes that ended longest ago go first, then the least
    /// recently looked up (a lost Terminate cannot grow memory without limit).
    fn evict_batch(&mut self) {
        // Ended processes first (oldest end first), then the least recently used.
        let ages = self.by_key.iter().map(|(k, e)| (*k, (e.end.is_none(), e.end.unwrap_or(0), e.touched.get())));
        for k in crate::evict::oldest(ages, self.by_key.len()) {
            self.remove(k);
            self.evictions += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(key: u64, pid: u32, path: &str) -> ProcInfo {
        ProcInfo { path: path.into(), ..ProcInfo::new(key, pid) }
    }

    #[test]
    fn the_start_key_formula_matches_s1() {
        // S1: high 16 bits are the kernel BootId, low 48 the sequence number.
        assert_eq!(start_key(7, 1_373_253), 0x0007_0000_0014_F445);
        assert_eq!(start_key(1, u64::MAX), 0x0001_FFFF_FFFF_FFFF);
    }

    #[test]
    fn integrity_levels() {
        assert_eq!(integrity(8448), Some(Integrity::Medium));
        assert_eq!(integrity(28672), Some(Integrity::Protected));
        assert_eq!(integrity(9999), None);
    }

    #[test]
    fn eviction_keeps_processes_in_use() {
        let mut c = ProcessCache::new(10, 8);
        c.insert(ProcInfo::new(1, 100), i64::MIN); // a service running since boot, in use
        for k in 2..=8u64 {
            c.insert(ProcInfo::new(k, 100 + k as u32), k as i64);
        }
        assert!(c.lookup_at(100, 50).is_some());
        c.insert(ProcInfo::new(9, 109), 9); // over the cap: the least recently used goes
        assert!(c.get(1).is_some() && c.get(2).is_none());
        assert_eq!(c.evictions(), 1);
    }

    #[test]
    fn lookup_at_survives_pid_reuse() {
        let mut c = ProcessCache::new(30, 100);
        c.insert(info(1, 500, r"C:\a.exe"), 10);
        c.end(1, 20);
        c.insert(info(2, 500, r"C:\b.exe"), 25);
        assert_eq!(c.lookup_at(500, 15).unwrap().start_key, 1);
        assert_eq!(c.lookup_at(500, 22).unwrap().start_key, 1); // ended, within retention, not yet reused
        assert_eq!(c.lookup_at(500, 26).unwrap().start_key, 2);
        assert!(c.lookup_at(500, 5).is_none()); // before either started
        assert!(c.lookup_at(501, 15).is_none());
    }

    #[test]
    fn retention_then_expiry() {
        let mut c = ProcessCache::new(30, 100);
        c.insert(info(1, 7, ""), i64::MIN);
        c.end(1, 100);
        assert!(c.lookup_at(7, 130).is_some());
        assert!(c.lookup_at(7, 131).is_none());
        c.expire(130);
        assert_eq!(c.len(), 1);
        c.expire(131);
        assert!(c.is_empty());
    }

    #[test]
    fn the_cap_evicts_ended_processes_first() {
        let mut c = ProcessCache::new(1_000, 2);
        c.insert(info(1, 1, ""), 0);
        c.insert(info(2, 2, ""), 1);
        c.end(2, 5);
        c.insert(info(3, 3, ""), 2);
        assert_eq!(c.evictions(), 1);
        assert!(c.get(2).is_none() && c.get(1).is_some() && c.get(3).is_some());
    }

    #[test]
    fn refs_carry_the_uid_path_and_name() {
        let id =
            Identity { device: DeviceUid::from_bytes([1; 16]), boot: BootId::from_bytes([2; 16]), kernel_boot_id: 7 };
        let r = info(42, 9, r"C:\Windows\System32\cmd.exe").to_ref(&id);
        assert_eq!(r.uid, id.uid(42));
        assert_eq!((r.pid, r.file.name.as_str()), (9, "cmd.exe"));
        assert_eq!(id.bare_ref(42, 9).file.path, "");
    }
}
```

`src/services.rs`:
```rust
//! The pipeline's view of Windows (implemented in plan 1b-3b; faked in tests).
//!
//! - [`Lookups`]: cheap, cached queries the pipeline thread makes directly.
//! - [`Request`] / [`Reply`]: slow work done elsewhere, the workers and the
//!   reader lane ([4] in §3.2) and the seeder ([8]). The pipeline only queues
//!   requests; whoever drives it sends them and feeds the replies back.

use atlas_schema::{Hashes, Signature};

use crate::completion::PendingId;

/// A running process as `PROCESS_TELEMETRY_ID_INFORMATION` describes it (§5.2, §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveProcess {
    pub start_key: u64,
    /// NT path of the image.
    pub image_path: String,
    pub command_line: Option<String>,
}

/// Cheap lookups made on the pipeline thread. Implementations cache.
pub trait Lookups {
    /// The drive form of an NT path (`\Device\HarddiskVolume3\x` → `C:\x`),
    /// or `None` to keep the NT path (§5.5).
    fn dos_path(&mut self, nt_path: &str) -> Option<String>;
    /// The live process with this PID, if any (§5.3 rule 2; §5.2's Launch fallback).
    fn live_process(&mut self, pid: u32) -> Option<LiveProcess>;
    /// `DOMAIN\name` for a SID string (`LookupAccountSid`, cached).
    fn account_name(&mut self, sid: &str) -> Option<String>;
}

/// Which file an enrichment result belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrichTarget {
    /// `process.file` of a Launch.
    LaunchImage,
    /// `module.file` of a Module Load.
    Module,
}

/// Seeder questions are about one of the two handle types (§7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandleKind {
    Key,
    File,
}

/// Work the pipeline asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// SHA-256 and signature of a file (§6.3).
    Enrich { id: PendingId, target: EnrichTarget, nt_path: String },
    /// Forget cached results for a path that changed (§6.3).
    InvalidateHash { nt_path: String },
    /// Read a registry value after its SetValueKey (§7.5), on the ordered path.
    ReadValue { id: PendingId, read: ValueRead },
    /// Expand 8.3 components of an NT path (§7.2; plan 1b-3a decision D4). `slot` tells
    /// apart the paths of one event (a Rename has two); the reply repeats it.
    Expand { id: PendingId, slot: u8, nt_path: String },
    /// Read the handle table for these object addresses (§7.4). Empty: the
    /// start-up pass over every handle.
    Seed { kind: HandleKind, addresses: Vec<u64> },
}

/// A registry value read (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueRead {
    /// The raw NT key path as logged (`\REGISTRY\MACHINE\…`, `ControlSet00N`).
    pub key_path: String,
    /// The value name as counted in the event (may contain NULs).
    pub value_name: Vec<u16>,
}

/// What a value read found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueData {
    pub value_type: u32,
    /// The full length of the value.
    pub size: u32,
    /// At most 4 KiB of it.
    pub data: Vec<u8>,
}

/// A seeded name: one handle-table entry the seeder could name (§7.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub address: u64,
    pub owner_pid: u32,
    /// NT name (`\REGISTRY\…` for keys, `\Device\…` for files).
    pub name: String,
}

/// One read of the handle table (§7.4), answering one `Request::Seed`.
///
/// Every handle of `kind` that the read covers appears in `named` or in
/// `unnamable` (a file handle that is not a disk file is unnamable). The read
/// covers the addresses asked about, or the whole table for the start-up pass.
/// A covered address that is in neither list was not in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub kind: HandleKind,
    /// QPC when the table was read (T_snap).
    pub taken: i64,
    /// The request's addresses; empty for the start-up pass.
    pub asked: Vec<u64>,
    pub named: Vec<Named>,
    /// Addresses in the table that could not be named (protected processes,
    /// failed or timed-out queries, non-disk files), with their owner's PID:
    /// the negative cache.
    pub unnamable: Vec<(u64, u32)>,
}

/// Results coming back to the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// `error`: an operational error (§6.3: signature absent, counted).
    Enriched {
        id: PendingId,
        hashes: Option<Hashes>,
        signature: Option<Signature>,
        error: bool,
    },
    ValueRead {
        id: PendingId,
        result: Option<ValueData>,
    },
    /// A read made on the fast path (§7.5), keyed by the event it answers.
    EarlyRead {
        event: EarlyKey,
        read: ValueRead,
        result: Option<ValueData>,
    },
    /// The long NT path, or `None` if it could not be expanded.
    Expanded {
        id: PendingId,
        slot: u8,
        long_path: Option<String>,
    },
    Snapshot(Snapshot),
}

/// Identifies a SetValueKey across the fast path and the ordered path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EarlyKey {
    pub ts: i64,
    pub tid: u32,
    pub key_object: u64,
}
```

- [ ] **Step 6: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy -p atlas-agent --all-targets -- -D warnings
```
Expected: 45 tests pass (property tests included) and clippy is clean.
```powershell
git add Cargo.toml Cargo.lock crates/atlas-agent
git commit -m "feat(agent): atlas-agent foundations: ordering, completion, process cache, key map, watchlist"
```

### Task 3: Intake

**Files:**
- Modify: `crates/atlas-agent/src/lib.rs`
- Create: `crates/atlas-agent/src/intake.rs`

- [ ] **Step 1: Wire the module**

```diff
 pub mod input;
+pub mod intake;
 pub mod keymap;
```

- [ ] **Step 2: The callbacks' logic**

`src/intake.rs`:
```rust
//! What each ETW callback does before an event reaches the pipeline (sensor
//! spec §3.2 [1], §4.4, §7.5). It runs on a consumer thread and must stay fast:
//! a hash-map operation or two per event, never a blocking call.
//!
//! - Parse failures are counted (`parse_errors`, `unknown_version`).
//! - Successful OperationEnds are discarded: only failures matter (§5.5).
//! - Session A keeps the early registry key map and sends fast-path value reads.
//! - DNS-Client events pass a per-PID token bucket, then go to the user-mode
//!   queue; everything else goes to the kernel queue. Full queues drop and count.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, TrySendError};

use atlas_etw::parse::{ParseError, RawEvent};

use crate::config::{Config, Ticks};
use crate::counters::IntakeCounters;
use crate::input::{Header, Incoming, Session};
use crate::services::{EarlyKey, ValueRead};

/// Sends a fast-path read to the reader lane (plan 1b-3b). Must not block.
pub type FastRead = Box<dyn FnMut(EarlyKey, ValueRead) + Send>;

/// The two queues into the pipeline (§3.2): bounded, never blocking.
pub struct Queues {
    /// Kernel providers and Session B (default 65,536 entries).
    pub kernel: SyncSender<Incoming>,
    /// User-mode providers, DNS-Client (default 8,192), so a forger cannot
    /// push kernel events out (§4.4).
    pub user: SyncSender<Incoming>,
}

pub struct Intake {
    kernel: SyncSender<Incoming>,
    user: SyncSender<Incoming>,
    counters: Arc<IntakeCounters>,
    dns: Buckets,
    early: Option<EarlyKeys>,
    fast_read: Option<FastRead>,
    self_keys: HashSet<u64>,
}

impl Intake {
    /// `fast_read` is `Some` for Session A when value reads are on; Session B
    /// passes `None` (it has no registry events).
    pub fn new(
        session: Session,
        cfg: &Config,
        ticks: Ticks,
        queues: Queues,
        counters: Arc<IntakeCounters>,
        fast_read: Option<FastRead>,
        self_keys: &[u64],
    ) -> Self {
        let early = (session == Session::Sensor && fast_read.is_some() && cfg.registry_value_reads)
            .then(|| EarlyKeys::new(cfg.early_key_map_cap));
        Intake {
            kernel: queues.kernel,
            user: queues.user,
            counters,
            dns: Buckets::new(cfg.dns_rate_per_pid, ticks.frequency),
            early,
            fast_read,
            self_keys: self_keys.iter().copied().collect(),
        }
    }

    pub fn on_event(&mut self, header: Header, parsed: Result<RawEvent, ParseError>) {
        let event = match parsed {
            Ok(e) => e,
            Err(ParseError::UnknownEvent) => return,
            Err(ParseError::UnsupportedVersion { .. } | ParseError::NewerVersion { .. }) => {
                IntakeCounters::bump(&self.counters.unknown_version);
                return;
            }
            Err(_) => {
                IntakeCounters::bump(&self.counters.parse_errors);
                return;
            }
        };
        match &event {
            RawEvent::FileOpEnd(o) if !o.failed() => {
                IntakeCounters::bump(&self.counters.op_end_discarded);
                return;
            }
            RawEvent::RegCreateKey(o) | RawEvent::RegOpenKey(o) if o.status == 0 => {
                if let Some(m) = &mut self.early {
                    let evicted = m.open(o.key_object, o.base_object, &o.relative_name.to_string_lossy());
                    if evicted > 0 {
                        self.counters.early_key_map_evictions.fetch_add(evicted, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            RawEvent::RegCloseKey(k) => {
                // The agent's own closes are ignored here too (§7.4).
                if let Some(m) = &mut self.early
                    && !header.start_key.is_some_and(|s| self.self_keys.contains(&s))
                {
                    m.close(k.key_object);
                }
            }
            RawEvent::RegSetValue(v) if v.status == 0 => {
                if let (Some(m), Some(send)) = (&self.early, &mut self.fast_read)
                    && let Some(path) = m.name(v.key_object)
                {
                    IntakeCounters::bump(&self.counters.fast_reads);
                    send(
                        EarlyKey { ts: header.ts, tid: header.tid, key_object: v.key_object },
                        ValueRead { key_path: path.to_string(), value_name: v.value_name.as_units().to_vec() },
                    );
                }
            }
            _ => {}
        }
        let inc = Incoming { header, event };
        if matches!(inc.event, RawEvent::DnsQuery(_)) {
            if !self.dns.take(header.pid, header.ts) {
                IntakeCounters::bump(&self.counters.dns_rate_limit_drops);
                return;
            }
            if let Err(TrySendError::Full(_)) = self.user.try_send(inc) {
                IntakeCounters::bump(&self.counters.user_queue_drops);
            }
        } else if let Err(TrySendError::Full(_)) = self.kernel.try_send(inc) {
            IntakeCounters::bump(&self.counters.kernel_queue_drops);
        }
    }
}

/// The early key map (§7.5): full names only, in arrival order, no seeding.
/// A relative open whose base is unknown is simply not cached; a miss costs
/// only a fall back to the ordered path. Bounded, evicting the oldest.
pub struct EarlyKeys {
    names: HashMap<u64, (String, u64)>,
    order: VecDeque<(u64, u64)>,
    generation: u64,
    cap: usize,
}

impl EarlyKeys {
    pub fn new(cap: usize) -> Self {
        EarlyKeys { names: HashMap::new(), order: VecDeque::new(), generation: 0, cap }
    }

    /// Returns how many entries were evicted to make room.
    pub fn open(&mut self, key: u64, base: u64, relative: &str) -> u64 {
        let full = if relative.get(..10).is_some_and(|p| p.eq_ignore_ascii_case(r"\REGISTRY\")) {
            Some(relative.to_string())
        } else {
            self.names.get(&base).map(|(b, _)| if relative.is_empty() { b.clone() } else { format!("{b}\\{relative}") })
        };
        match full {
            Some(name) => {
                self.generation += 1;
                self.names.insert(key, (name, self.generation));
                self.order.push_back((key, self.generation));
                self.trim()
            }
            None => {
                self.names.remove(&key);
                0
            }
        }
    }

    pub fn close(&mut self, key: u64) {
        self.names.remove(&key);
    }

    pub fn name(&self, key: u64) -> Option<&str> {
        self.names.get(&key).map(|(n, _)| n.as_str())
    }

    fn trim(&mut self) -> u64 {
        let mut evicted = 0;
        while self.names.len() > self.cap {
            let Some((k, g)) = self.order.pop_front() else { break };
            if self.names.get(&k).is_some_and(|(_, gen_)| *gen_ == g) {
                self.names.remove(&k);
                evicted += 1;
            }
        }
        // Stale order entries (closed or replaced keys) are dropped as they surface.
        if self.order.len() > self.cap.saturating_mul(2) {
            let names = &self.names;
            self.order.retain(|(k, g)| names.get(k).is_some_and(|(_, gen_)| gen_ == g));
        }
        evicted
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Per-PID token buckets for DNS-Client (§4.4), refilled by event time.
struct Buckets {
    /// Tokens are kept in millitokens.
    rate: u64,
    frequency: i64,
    by_pid: HashMap<u32, (u64, i64)>,
}

impl Buckets {
    fn new(rate: u32, frequency: i64) -> Self {
        Buckets { rate: u64::from(rate) * 1000, frequency, by_pid: HashMap::new() }
    }

    fn take(&mut self, pid: u32, ts: i64) -> bool {
        if self.rate == 0 {
            return true;
        }
        if self.by_pid.len() > 8192 {
            // Forget PIDs idle for 10 s: a full bucket is the same as no bucket.
            let horizon = ts.saturating_sub(self.frequency.saturating_mul(10));
            self.by_pid.retain(|_, (_, last)| *last >= horizon);
        }
        let (rate, freq) = (self.rate, self.frequency);
        let (tokens, last) = self.by_pid.entry(pid).or_insert((rate, ts));
        let elapsed = u64::try_from(ts.saturating_sub(*last)).unwrap_or(0);
        let refill = u128::from(elapsed) * u128::from(rate) / u128::from(freq.max(1) as u64);
        *tokens = (u128::from(*tokens) + refill).min(u128::from(rate)) as u64;
        *last = (*last).max(ts);
        if *tokens >= 1000 {
            *tokens -= 1000;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_etw::parse::{DnsQuery, FileOpEnd, RegKey, RegOpen, RegSetValue, WStr};
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, sync_channel};

    fn header(pid: u32, ts: i64) -> Header {
        Header { session: Session::Sensor, pid, tid: 9, ts, start_key: Some(77) }
    }

    type Reads = Arc<Mutex<Vec<(EarlyKey, ValueRead)>>>;

    fn intake(kernel_cap: usize) -> (Intake, Receiver<Incoming>, Receiver<Incoming>, Arc<IntakeCounters>, Reads) {
        let (ktx, krx) = sync_channel(kernel_cap);
        let (utx, urx) = sync_channel(4);
        let counters = Arc::new(IntakeCounters::default());
        let reads: Reads = Arc::new(Mutex::new(Vec::new()));
        let r = reads.clone();
        let cfg = Config { dns_rate_per_pid: 2, ..Config::default() };
        let i = Intake::new(
            Session::Sensor,
            &cfg,
            Ticks::new(1000),
            Queues { kernel: ktx, user: utx },
            counters.clone(),
            Some(Box::new(move |k, v| r.lock().unwrap().push((k, v)))),
            &[123],
        );
        (i, krx, urx, counters, reads)
    }

    fn dns() -> RawEvent {
        RawEvent::DnsQuery(DnsQuery {
            query_name: "a".into(),
            query_type: 1,
            query_options: 0,
            query_status: 0,
            query_results: WStr::default(),
        })
    }

    #[test]
    fn successful_op_ends_are_discarded_failures_pass() {
        let (mut i, krx, _, c, _) = intake(8);
        i.on_event(header(1, 0), Ok(RawEvent::FileOpEnd(FileOpEnd { irp: 1, extra_information: 0, status: 0 })));
        i.on_event(header(1, 0), Ok(RawEvent::FileOpEnd(FileOpEnd { irp: 2, extra_information: 0, status: 0x104 })));
        i.on_event(
            header(1, 0),
            Ok(RawEvent::FileOpEnd(FileOpEnd { irp: 3, extra_information: 0, status: 0xC000_0035 })),
        );
        assert_eq!(IntakeCounters::get(&c.op_end_discarded), 2);
        assert!(matches!(krx.try_recv().unwrap().event, RawEvent::FileOpEnd(o) if o.irp == 3));
        assert!(krx.try_recv().is_err());
    }

    #[test]
    fn parse_failures_are_counted_by_kind() {
        let (mut i, _, _, c, _) = intake(8);
        i.on_event(header(1, 0), Err(ParseError::UnknownEvent));
        i.on_event(header(1, 0), Err(ParseError::NewerVersion { version: 9, newest: 4 }));
        i.on_event(header(1, 0), Err(ParseError::UnsupportedVersion { version: 1 }));
        i.on_event(header(1, 0), Err(ParseError::Truncated { field: "x", offset: 0 }));
        assert_eq!((IntakeCounters::get(&c.unknown_version), IntakeCounters::get(&c.parse_errors)), (2, 1));
    }

    #[test]
    fn a_full_queue_drops_and_counts() {
        let (mut i, krx, _, c, _) = intake(1);
        for irp in 0..3 {
            i.on_event(
                header(1, 0),
                Ok(RawEvent::FileOpEnd(FileOpEnd { irp, extra_information: 0, status: 0xC000_0001 })),
            );
        }
        assert_eq!(IntakeCounters::get(&c.kernel_queue_drops), 2);
        assert!(krx.try_recv().is_ok());
    }

    #[test]
    fn dns_is_rate_limited_per_pid_on_its_own_queue() {
        let (mut i, krx, urx, c, _) = intake(8);
        // Rate 2/s at 1000 ticks/s: two pass, the third in the same instant drops.
        for _ in 0..3 {
            i.on_event(header(5, 0), Ok(dns()));
        }
        i.on_event(header(6, 0), Ok(dns())); // another PID has its own bucket
        i.on_event(header(5, 500), Ok(dns())); // half a second refills one token
        assert_eq!(IntakeCounters::get(&c.dns_rate_limit_drops), 1);
        assert_eq!(urx.try_iter().count(), 4);
        assert!(krx.try_recv().is_err());
    }

    fn open(key: u64, base: u64, rel: &str) -> RawEvent {
        RawEvent::RegOpenKey(RegOpen {
            base_object: base,
            key_object: key,
            status: 0,
            disposition: 0,
            base_name: WStr::default(),
            relative_name: rel.into(),
        })
    }

    fn set(key: u64, name: &str) -> RawEvent {
        RawEvent::RegSetValue(RegSetValue {
            key_object: key,
            status: 0,
            value_type: 4,
            data_size: 4,
            key_name: WStr::default(),
            value_name: name.into(),
            value_name_ambiguous: false,
            captured_data: Box::default(),
            previous_data_type: 0,
            previous_data_size: 0,
            previous_data: Box::default(),
        })
    }

    #[test]
    fn the_fast_path_reads_named_keys_only() {
        let (mut i, _, _, c, reads) = intake(64);
        i.on_event(header(1, 1), Ok(open(10, 0, r"\REGISTRY\MACHINE\SOFTWARE")));
        i.on_event(header(1, 2), Ok(open(11, 10, r"Microsoft\Windows\CurrentVersion\Run")));
        i.on_event(header(1, 3), Ok(set(11, "evil")));
        i.on_event(header(1, 4), Ok(open(12, 999, "Unknown")));
        i.on_event(header(1, 5), Ok(set(12, "x"))); // base unknown: no fast read
        let r = reads.lock().unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, EarlyKey { ts: 3, tid: 9, key_object: 11 });
        assert_eq!(r[0].1.key_path, r"\REGISTRY\MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Run");
        assert_eq!(IntakeCounters::get(&c.fast_reads), 1);
    }

    #[test]
    fn the_agents_own_closes_do_not_remove_names() {
        let (mut i, _, _, _, reads) = intake(64);
        i.on_event(header(1, 1), Ok(open(10, 0, r"\REGISTRY\MACHINE\X")));
        let close = RawEvent::RegCloseKey(RegKey { key_object: 10, status: 0, key_name: WStr::default() });
        let mut agent = header(1, 2);
        agent.start_key = Some(123);
        i.on_event(agent, Ok(close.clone()));
        i.on_event(header(1, 3), Ok(set(10, "v")));
        assert_eq!(reads.lock().unwrap().len(), 1);
        i.on_event(header(1, 4), Ok(close));
        i.on_event(header(1, 5), Ok(set(10, "v")));
        assert_eq!(reads.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_early_map_evicts_the_oldest() {
        let mut m = EarlyKeys::new(2);
        m.open(1, 0, r"\REGISTRY\A");
        m.open(2, 0, r"\REGISTRY\B");
        assert_eq!(m.open(3, 0, r"\REGISTRY\C"), 1);
        assert!(m.name(1).is_none() && m.name(3).is_some());
        // Re-opening refreshes an address's place.
        m.open(2, 0, r"\REGISTRY\B2");
        assert_eq!(m.open(4, 0, r"\REGISTRY\D"), 1);
        assert_eq!(m.name(2), Some(r"\REGISTRY\B2"));
        assert!(m.name(3).is_none());
    }
}
```

- [ ] **Step 3: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy -p atlas-agent --all-targets -- -D warnings
```
Expected: 52 tests pass and clippy is clean.
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): intake: op-end filter, early key map, DNS rate limit, bounded queues"
```

### Task 4: The pipeline

**Files:**
- Modify: `crates/atlas-agent/src/lib.rs`
- Create: `crates/atlas-agent/src/fakes.rs`, `src/pipeline/mod.rs`, `proc.rs`, `file.rs`, `reg.rs`, `net.rs`, `seed.rs`, `expand.rs`, `tests.rs`

- [ ] **Step 1: Wire the modules**

```diff
 pub mod evict;
+pub mod fakes;
 pub mod input;
```
```diff
 pub mod paths;
+pub mod pipeline;
 pub mod process;
```

- [ ] **Step 2: The loop**

`src/pipeline/mod.rs`:
```rust
//! The pipeline thread (sensor spec §3.2 [2] + [3] + [5]; plan 1b-3a decision D2):
//! one loop owns the ordering stage, all mutable state and the completion stage.
//!
//! The driver (plan 1b-4) calls, from one thread:
//! - [`Pipeline::push`] for each event from the kernel and user-mode queues;
//! - [`Pipeline::reply`] for each result from the workers, reader lane and seeder;
//! - [`Pipeline::tick`] with the current QPC, at least every ~50 ms, which returns
//!   the events ready to emit;
//! - [`Pipeline::take_requests`] after each call, and hands the requests out.
//!
//! Time is passed in, so every rule is testable without sleeping.

mod expand;
mod file;
mod net;
mod proc;
mod reg;
mod seed;

use std::collections::{HashMap, HashSet};

use atlas_schema::{Device, Event, EventId, EventKind, EventMeta, ProcessRef, ProcessUid, Sensor};

use crate::completion::{Completion, Expired, PendingId, Reason, Wait};
use crate::config::{Config, Ticks};
use crate::counters::{Class, Counters};
use crate::input::{Header, Incoming};
use crate::keymap::KeyMap;
use crate::ordering::{Ordering, Pushed};
use crate::process::{Identity, ProcInfo, ProcessCache, start_key};
use crate::services::{Lookups, Reply, Request};
use crate::time::{Anchor, Clock};
use crate::watchlist::{BadPattern, Watchlist};

pub use expand::Expands;
pub use file::Files;
pub use net::Network;
pub use proc::Launches;
pub use reg::Registry;
pub use seed::Seeding;

/// Produces event ids from the event's Unix time. The agent uses UUIDv7;
/// tests use a deterministic generator.
pub type IdGen = Box<dyn FnMut(i64) -> EventId + Send>;

/// What the pipeline needs to start.
pub struct Setup {
    pub config: Config,
    pub ticks: Ticks,
    pub anchor: Anchor,
    pub identity: Identity,
    /// `HKLM\SYSTEM\Select\Current`, read at start (§5.5).
    pub current_control_set: u32,
    /// The agent's own start key(s): its events are not emitted (§5.5).
    pub self_keys: Vec<u64>,
    /// QPC at start: the seeder deadline is longer at first (§3.2).
    pub started: i64,
}

pub struct Pipeline<L> {
    pub(crate) cfg: Config,
    pub(crate) ticks: Ticks,
    pub(crate) clock: Clock,
    pub(crate) id: Identity,
    pub(crate) ccs: u32,
    pub(crate) self_keys: HashSet<u64>,
    pub(crate) self_uids: HashSet<ProcessUid>,
    pub(crate) started: i64,
    pub(crate) lookups: L,
    pub(crate) ids: IdGen,
    ordering: Ordering,
    pub(crate) completion: Completion<Event>,
    pub(crate) procs: ProcessCache,
    pub(crate) launches: Launches,
    pub(crate) files: Files,
    pub(crate) keys: KeyMap,
    pub(crate) reg: Registry,
    pub(crate) net: Network,
    pub(crate) watch: Watchlist,
    pub(crate) seeding: Seeding,
    pub(crate) expands: Expands,
    pub(crate) requests: Vec<Request>,
    pub(crate) counters: Counters,
    /// Stream time (§3.2): the ordering watermark.
    pub(crate) stream: i64,
    /// The QPC passed to the last tick (wall time for deadlines).
    pub(crate) now: i64,
    /// Stream time of the next sweep over whole tables (once a second).
    next_sweep: i64,
}

impl<L: Lookups> Pipeline<L> {
    pub fn new(setup: Setup, lookups: L, ids: IdGen) -> Result<Self, BadPattern> {
        let Setup { config, ticks, anchor, identity, current_control_set, self_keys, started } = setup;
        let watch = Watchlist::new(config.watchlist.as_deref(), &config.watchlist_extend)?;
        let t = |d| ticks.of(d);
        let mut p = Pipeline {
            ordering: Ordering::new(t(config.hold)),
            completion: Completion::new(config.pending_cap),
            procs: ProcessCache::new(t(config.process_retention), config.process_cap),
            launches: Launches::default(),
            files: Files::new(config.file_map_cap, t(config.confirm_window)),
            keys: KeyMap::new(config.key_map_cap),
            reg: Registry::default(),
            net: Network::new(config.flow_cap, t(config.udp_idle)),
            seeding: Seeding::new(),
            expands: Expands::default(),
            self_uids: self_keys.iter().map(|k| identity.uid(*k)).collect(),
            self_keys: self_keys.into_iter().collect(),
            clock: Clock::new(ticks, anchor),
            cfg: config,
            ticks,
            id: identity,
            ccs: current_control_set,
            started,
            lookups,
            ids,
            watch,
            requests: Vec::new(),
            counters: Counters::default(),
            stream: i64::MIN,
            now: started,
            next_sweep: i64::MIN,
        };
        p.seed_builtins();
        if p.cfg.seed_on_start {
            p.seeding.start(&mut p.requests);
        }
        Ok(p)
    }

    /// One event from a queue (§3.2 [1] → [2]). A late event, one older than
    /// stream time, is processed at once.
    pub fn push(&mut self, inc: Incoming) {
        if inc.header.ts < self.stream {
            self.counters.late_arrivals += 1;
            self.process(inc);
            return;
        }
        if let Pushed::Late(inc) = self.ordering.push(inc) {
            self.counters.late_arrivals += 1;
            self.process(inc);
        }
    }

    /// A result from a worker, the reader lane or the seeder.
    pub fn reply(&mut self, r: Reply) {
        match r {
            Reply::Enriched { id, hashes, signature, error } => self.on_enriched(id, hashes, signature, error),
            Reply::ValueRead { id, result } => self.on_value_read(id, result),
            Reply::EarlyRead { event, read, result } => self.reg.on_early_read(event, read, result),
            Reply::Expanded { id, slot, long_path } => self.on_expanded(id, slot, long_path),
            Reply::Snapshot(s) => self.seeding.queue(s),
        }
    }

    /// Advances to `now` (QPC): releases held events, applies everything that
    /// waits on stream time, and returns the events ready to emit, in order.
    pub fn tick(&mut self, now: i64) -> Vec<Event> {
        self.now = now;
        for inc in self.ordering.release(now) {
            // Stream time reaches each event before it is processed, so a
            // window that closed before it is applied first.
            self.step(inc.header.ts);
            self.process(inc);
        }
        self.advance(self.ordering.watermark(now));
        self.emit(false)
    }

    /// A clean stop (§11.4): everything held is processed and everything
    /// pending goes out as is.
    pub fn stop(&mut self) -> Vec<Event> {
        for inc in self.ordering.drain() {
            self.step(inc.header.ts);
            self.process(inc);
        }
        self.advance(i64::MAX);
        self.net.close_all(&mut self.completion, &mut self.counters, &self.clock, &mut self.ids, &self.id);
        self.emit(true)
    }

    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    pub fn counters(&self) -> Counters {
        let mut c = self.counters.clone();
        c.process_cache_evictions = self.procs.evictions();
        c.key_map_evictions = self.keys.evictions();
        c.file_map_evictions = self.files.evictions();
        c.flow_table_evictions = self.net.evictions();
        c.pending_overflow = self.completion.overflow();
        c
    }

    /// Re-anchors `meta.time` (every 60 s, §3.3).
    pub fn set_anchor(&mut self, anchor: Anchor) {
        self.clock.set_anchor(anchor);
    }

    /// Adds a start key to the self-filter (the canary child, §5.5; plan 1b-4).
    pub fn add_self_key(&mut self, key: u64) {
        self.self_keys.insert(key);
        self.self_uids.insert(self.id.uid(key));
    }

    /// Stream time moves to `stream` between two events: seeder snapshots,
    /// confirm windows and launch halves that are due are applied, in that order.
    fn step(&mut self, stream: i64) {
        if stream <= self.stream {
            return;
        }
        self.stream = stream;
        self.apply_snapshots(stream);
        self.file_confirm(stream);
        self.launch_expiry(stream);
    }

    /// The end of a tick: stream time moves to the watermark, the seeder's
    /// questions go out, and once a second of stream time the sweeps over
    /// whole tables run (UDP idle, cache retention).
    fn advance(&mut self, stream: i64) {
        self.step(stream);
        let s = self.stream;
        self.seeding.flush_asks(self.now, self.ticks, &mut self.requests);
        self.reg.prune_early(s.saturating_sub(self.ticks.of(self.cfg.hold)));
        // Address history is only needed while a snapshot or a waiting event can predate it.
        self.seeding.expire(s.saturating_sub(self.ticks.of(self.cfg.seeder_startup_deadline).saturating_mul(4)));
        if s >= self.next_sweep {
            self.next_sweep = s.saturating_add(self.ticks.frequency);
            self.net.expire(s, &mut self.completion, &mut self.counters, &self.clock, &mut self.ids, &self.id);
            self.procs.expire(s);
        }
    }

    fn process(&mut self, inc: Incoming) {
        use atlas_etw::parse::RawEvent as R;
        let Incoming { header: h, event } = inc;
        match event {
            R::ProcessStart(s) => self.on_process_start(&h, s),
            R::ProcessStop(s) => self.on_process_stop(&h, s),
            R::ImageLoad(i) => self.on_image_load(&h, i),
            R::ClassicProcess(c) => self.on_classic(&h, c),
            R::FileCreate(c) => self.on_file_create(&h, c),
            R::FileCreateNew(c) => self.on_file_create_new(&h, c),
            R::FileCleanup(x) => self.on_file_cleanup(&h, x),
            R::FileClose(x) => self.on_file_close(&h, x),
            R::FileWrite(w) => self.on_file_write(&h, w.file_object),
            R::FileSetInfo(i) => self.on_file_set_info(&h, i),
            R::FileOpEnd(o) => self.on_file_op_end(&h, o),
            R::FileDeletePath(p) => self.on_file_delete_path(&h, p),
            R::FileRenamePath(p) => self.on_file_rename_path(&h, p),
            R::RegCreateKey(o) => self.on_reg_open(&h, o, true),
            R::RegOpenKey(o) => self.on_reg_open(&h, o, false),
            R::RegCloseKey(k) => self.on_reg_close(&h, k),
            R::RegDeleteKey(k) => self.on_reg_delete_key(&h, k),
            R::RegSetValue(v) => self.on_reg_set_value(&h, v),
            R::RegDeleteValue(v) => self.on_reg_delete_value(&h, v),
            R::TcpConnect(n) => self.on_tcp(&h, n, net::Tcp::Connect),
            R::TcpAccept(n) => self.on_tcp(&h, n, net::Tcp::Accept),
            R::TcpDisconnect(n) => self.on_tcp(&h, n, net::Tcp::Disconnect),
            R::UdpSend(n) => self.on_udp(&h, n, true),
            R::UdpRecv(n) => self.on_udp(&h, n, false),
            R::DnsQuery(q) => self.on_dns(&h, q),
        }
    }

    /// Drains the completion stage and applies the self-filter (§5.5).
    fn emit(&mut self, all: bool) -> Vec<Event> {
        let Pipeline { completion, counters, lookups, procs, now, id, .. } = self;
        let on_incomplete = |e: &mut Event, x: &[Expired]| finish_incomplete(e, x, counters, lookups, procs, id);
        let events = if all { completion.flush(on_incomplete) } else { completion.drain(*now, on_incomplete) };
        for id in self.completion.take_exited() {
            self.forget(id);
        }
        let mut out = Vec::with_capacity(events.len());
        for e in events {
            if self.is_self(&e) {
                self.counters.self_filtered += 1;
            } else {
                out.push(e);
            }
        }
        out
    }

    /// What is kept for pending events and recent history, by name (tests check
    /// it all goes once events leave and windows pass).
    #[cfg(test)]
    pub(crate) fn bookkeeping(&self) -> Vec<(&'static str, usize)> {
        let mut v = Vec::new();
        v.extend(self.launches.sizes());
        v.extend(self.expands.sizes());
        v.extend(self.reg.sizes());
        v.extend(self.files.sizes());
        v.extend(self.seeding.sizes());
        v.push(("pending", self.completion.pending_len()));
        v
    }

    /// A pending event left the completion stage: its request bookkeeping goes,
    /// whether or not a reply came.
    fn forget(&mut self, id: PendingId) {
        self.launches.forget(id);
        self.expands.forget(id);
        self.reg.forget(id);
        self.files.forget(id);
    }

    fn is_self(&self, e: &Event) -> bool {
        let uid = match &e.kind {
            EventKind::Process(atlas_schema::classes::process::ProcessActivity::Launch { actor, .. }) => actor.uid,
            EventKind::Process(atlas_schema::classes::process::ProcessActivity::Terminate { process, .. }) => {
                process.uid
            }
            EventKind::Module(a) => a.actor.uid,
            EventKind::Network(a) => a.actor.uid,
            EventKind::File(a) => a.actor.uid,
            EventKind::RegistryKey(a) => a.actor.uid,
            EventKind::RegistryValue(a) => a.actor.uid,
            EventKind::Dns(a) => a.actor.uid,
            EventKind::EventLog(_) | EventKind::SensorHealth(_) => return false,
        };
        self.self_uids.contains(&uid)
    }

    /// A new event at QPC `ts`.
    pub(crate) fn event(&mut self, ts: i64, kind: EventKind) -> Event {
        make_event(&self.clock, &mut self.ids, &self.id, ts, kind)
    }

    pub(crate) fn wait(&self, reason: Reason, deadline: std::time::Duration) -> Wait {
        Wait { reason, deadline: Some(self.now.saturating_add(self.ticks.of(deadline))), drop_at_deadline: false }
    }

    pub(crate) fn seeder_wait(&self, drop_at_deadline: bool) -> Wait {
        let startup = self.now.saturating_sub(self.started) < self.ticks.of(self.cfg.startup_period);
        let d = if startup { self.cfg.seeder_startup_deadline } else { self.cfg.seeder_deadline };
        Wait { drop_at_deadline, ..self.wait(Reason::Seeder, d) }
    }

    /// The actor of a synchronous event: header PID and start key (§5.3).
    pub(crate) fn actor_sync(&mut self, h: &Header, class: Class) -> Option<ProcessRef> {
        if let Some(info) = self.procs.lookup_at(h.pid, h.ts)
            && h.start_key.is_none_or(|k| k == info.start_key)
        {
            return Some(info.to_ref(&self.id));
        }
        let Some(key) = h.start_key else {
            if let Some(r) = self.builtin(h.pid) {
                return Some(r);
            }
            Counters::add_class(&mut self.counters.actor_dropped, class);
            return None;
        };
        // A miss with a known start key: ask Windows, accept only the same process.
        if let Some(live) = self.lookups.live_process(h.pid)
            && live.start_key == key
        {
            let info =
                ProcInfo { path: self.dos(&live.image_path), cmd_line: live.command_line, ..ProcInfo::new(key, h.pid) };
            let r = info.to_ref(&self.id);
            // Started at some time before this event: from here on, the cache
            // answers for this PID with this process, not an earlier one.
            self.procs.insert(info, h.ts);
            return Some(r);
        }
        Counters::add_class(&mut self.counters.actor_unresolved, class);
        Some(self.id.bare_ref(key, h.pid))
    }

    /// The actor named by a payload PID (network, image load; §5.3): the cache
    /// entry live at the event's time, or nothing (no start key, no uid).
    pub(crate) fn actor_payload(&mut self, pid: u32, ts: i64, class: Class) -> Option<ProcessRef> {
        if let Some(info) = self.procs.lookup_at(pid, ts) {
            return Some(info.to_ref(&self.id));
        }
        if let Some(r) = self.builtin(pid) {
            return Some(r);
        }
        Counters::add_class(&mut self.counters.actor_dropped, class);
        None
    }

    /// Idle (PID 0) has no telemetry: a synthetic reference with sequence 0
    /// (§5.3 rule 1). Other built-ins come from the rundown.
    fn builtin(&self, pid: u32) -> Option<ProcessRef> {
        (pid == 0).then(|| {
            let mut r = self.id.bare_ref(start_key(self.id.kernel_boot_id, 0), 0);
            r.file.name = "Idle".into();
            r
        })
    }

    fn seed_builtins(&mut self) {
        let mut idle = ProcInfo::new(start_key(self.id.kernel_boot_id, 0), 0);
        idle.path = String::new();
        self.procs.insert(idle, i64::MIN);
    }

    /// The drive form of an NT file path, when mappable (§5.5).
    pub(crate) fn dos(&mut self, nt: &str) -> String {
        let path = self.lookups.dos_path(nt).unwrap_or_else(|| nt.to_string());
        crate::paths::fit(path, atlas_schema::limits::PATH_MAX)
    }

    pub(crate) fn is_self_key(&self, key: Option<u64>) -> bool {
        key.is_some_and(|k| self.self_keys.contains(&k))
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn make_event(clock: &Clock, ids: &mut IdGen, id: &Identity, ts: i64, kind: EventKind) -> Event {
    let time = clock.unix_ns(ts);
    Event {
        meta: EventMeta { event_id: ids(time), time, sensor: Sensor::Etw },
        device: Device { uid: id.device, boot_id: id.boot },
        kind,
    }
}

/// An event leaves the completion stage with unresolved reasons: count them,
/// and for a Launch whose other half never came, try the live process (§5.2).
fn finish_incomplete<L: Lookups>(
    e: &mut Event,
    expired: &[Expired],
    c: &mut Counters,
    lookups: &mut L,
    procs: &mut ProcessCache,
    id: &Identity,
) {
    for x in expired {
        match x.reason {
            Reason::Enrich => c.enrichment_misses += 1,
            Reason::Join => {
                c.launch_join_miss += 1;
                proc::join_fallback(e, lookups, procs, id);
            }
            Reason::ValueRead => c.value_read_failed += 1,
            Reason::Seeder => match &mut e.kind {
                EventKind::File(_) if x.dropped => c.unknown_file_object += 1,
                EventKind::File(_) => {}
                kind => {
                    c.registry_unresolved += 1;
                    if reg::no_read(kind) {
                        c.value_read_failed += 1;
                    }
                }
            },
            Reason::Expand | Reason::Confirm => {}
        }
    }
}

/// Whether an id is still pending (for replies that arrive after a deadline).
pub(crate) fn alive<E>(c: &Completion<E>, id: PendingId, reason: Reason) -> bool {
    c.is_waiting(id, reason)
}

/// Request bookkeeping shared by the domain modules.
pub(crate) struct Pending<T> {
    pub(crate) by_id: HashMap<PendingId, T>,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Pending { by_id: HashMap::new() }
    }
}

impl<T> Pending<T> {
    pub(crate) fn insert(&mut self, id: PendingId, t: T) {
        self.by_id.insert(id, t);
    }

    pub(crate) fn take(&mut self, id: PendingId) -> Option<T> {
        self.by_id.remove(&id)
    }
}
```

- [ ] **Step 3: Processes**

`src/pipeline/proc.rs`:
```rust
//! Process Launch (the join of Kernel-Process 1 and Session B's classic Start,
//! §5.2), Terminate, Module Load, the rundown that seeds the cache (§6.2), and
//! enrichment results (§6.3).

use std::collections::HashMap;

use atlas_etw::parse::{ClassicKind, ClassicProcess, ImageLoad, ProcessStart, ProcessStop};
use atlas_schema::classes::module::{ModuleAction, ModuleActivity};
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::limits::{CMD_LINE_MAX, USER_NAME_MAX, USER_UID_MAX, truncate_utf8};
use atlas_schema::{Event, EventKind, File, Hashes, Process, ProcessRef, Signature, User};

use super::expand::{Expansion, Slot};
use super::{Pending, Pipeline, alive};
use crate::completion::{PendingId, Reason};
use crate::counters::{Class, Counters};
use crate::input::Header;
use crate::paths::{fit, has_short_name};
use crate::process::{Identity, ProcInfo, ProcessCache, file_name, integrity, start_key};
use crate::services::{EnrichTarget, Lookups, Request};
use crate::time::filetime_to_unix_ns;

/// Launch halves waiting for their partner (§5.2).
#[derive(Default)]
pub struct Launches {
    /// pid → (QPC, the pending Launch, start key): waiting for the classic half.
    kernel: HashMap<u32, (i64, PendingId, u64)>,
    /// pid → the classic half and its header: waiting for the Kernel-Process half.
    classic: HashMap<u32, (i64, ClassicProcess, Header)>,
    enrich: Pending<EnrichTarget>,
}

impl Launches {
    /// The event left the completion stage.
    pub(super) fn forget(&mut self, id: PendingId) {
        self.enrich.take(id);
    }

    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 3] {
        [
            ("enrich", self.enrich.by_id.len()),
            ("kernel halves", self.kernel.len()),
            ("classic halves", self.classic.len()),
        ]
    }
}

impl<L: Lookups> Pipeline<L> {
    pub(super) fn on_process_start(&mut self, h: &Header, s: ProcessStart) {
        let key = start_key(self.id.kernel_boot_id, s.sequence_number);
        let nt = s.image_name.to_string_lossy();
        let parent_key = start_key(self.id.kernel_boot_id, s.parent_sequence_number);
        let parent = match self.procs.get(parent_key) {
            Some(p) => p.to_ref(&self.id),
            None => self.id.bare_ref(parent_key, s.parent_pid),
        };
        let mut info = ProcInfo {
            path: self.dos(&nt),
            created_time: Some(filetime_to_unix_ns(s.create_time)),
            integrity: if s.mandatory_label.is_mandatory_label() {
                s.mandatory_label.rid().and_then(integrity)
            } else {
                None
            },
            parent: Some(parent),
            ..ProcInfo::new(key, s.pid)
        };
        let window = self.ticks.of(self.cfg.join_window);
        let joined = match self.launches.classic.remove(&s.pid) {
            Some((ts, c, _)) if ts.abs_diff(h.ts) <= window.unsigned_abs() => {
                self.apply_classic(&mut info, &c);
                true
            }
            Some(other) => {
                self.launches.classic.insert(s.pid, other);
                false
            }
            None => false,
        };
        self.procs.insert(info.clone(), h.ts);
        let Some(actor) = self.actor_sync(h, Class::Process) else { return };
        self.push_launch(h.ts, actor, &info, &nt, joined, Some((s.pid, key)));
    }

    fn push_launch(
        &mut self,
        ts: i64,
        actor: ProcessRef,
        info: &ProcInfo,
        nt_path: &str,
        joined: bool,
        kernel_half: Option<(u32, u64)>,
    ) {
        let process = launch_process(info, &self.id);
        let ev = self.event(ts, EventKind::Process(ProcessActivity::Launch { actor, process }));
        let mut waits = Vec::new();
        let enrich = !nt_path.is_empty();
        if enrich {
            waits.push(self.wait(Reason::Enrich, self.cfg.enrich_deadline));
        }
        if !joined {
            waits.push(self.wait(Reason::Join, self.cfg.join_deadline));
        }
        let expands = image_expansion(nt_path, Some(info.start_key));
        let Some(id) = self.push_with(ev, waits, expands) else { return };
        if enrich {
            self.launches.enrich.insert(id, EnrichTarget::LaunchImage);
            self.requests.push(Request::Enrich { id, target: EnrichTarget::LaunchImage, nt_path: nt_path.into() });
        }
        if let (false, Some((pid, key))) = (joined, kernel_half) {
            self.launches.kernel.insert(pid, (ts, id, key));
        }
    }

    /// Command line and user from the classic half (§5.2).
    fn apply_classic(&mut self, info: &mut ProcInfo, c: &ClassicProcess) {
        info.cmd_line = Some(c.command_line.to_string_lossy());
        info.user = c.user_sid.as_ref().map(|sid| {
            let uid = sid.to_string();
            let name = self.lookups.account_name(&uid).unwrap_or_default();
            User { uid: fit(uid, USER_UID_MAX), name: fit(name, USER_NAME_MAX) }
        });
    }

    pub(super) fn on_classic(&mut self, h: &Header, c: ClassicProcess) {
        match c.kind {
            ClassicKind::Start => {
                let window = self.ticks.of(self.cfg.join_window);
                match self.launches.kernel.remove(&c.pid) {
                    Some((ts, id, key)) if ts.abs_diff(h.ts) <= window.unsigned_abs() => {
                        let mut info = self.procs.get(key).cloned().unwrap_or_else(|| ProcInfo::new(key, c.pid));
                        self.apply_classic(&mut info, &c);
                        let (cmd, user) = (info.cmd_line.clone(), info.user.clone());
                        self.procs.insert(info, ts);
                        self.completion.update(id, |e| set_launch_details(e, cmd, user));
                        self.completion.resolve(id, Reason::Join);
                    }
                    other => {
                        if let Some(k) = other {
                            self.launches.kernel.insert(c.pid, k);
                        }
                        self.launches.classic.insert(c.pid, (h.ts, c, *h));
                    }
                }
            }
            ClassicKind::DcStart => self.seed_rundown(&c),
            ClassicKind::End | ClassicKind::DcEnd => {}
        }
    }

    /// A process running when Session B started (§6.2): cached, never emitted.
    fn seed_rundown(&mut self, c: &ClassicProcess) {
        if c.pid == 0 {
            return; // Idle is built in
        }
        let Some(live) = self.lookups.live_process(c.pid) else { return };
        let mut info = ProcInfo { path: self.dos(&live.image_path), ..ProcInfo::new(live.start_key, c.pid) };
        self.apply_classic(&mut info, c);
        if info.cmd_line.as_deref().is_none_or(str::is_empty) {
            info.cmd_line = live.command_line;
        }
        self.procs.insert(info, i64::MIN);
    }

    /// Launch halves whose partner did not come within the join window.
    pub(super) fn launch_expiry(&mut self, stream: i64) {
        let window = self.ticks.of(self.cfg.join_window);
        // A Kernel-Process half stays pending in the completion stage until its
        // Join deadline; it just stops accepting a partner.
        self.launches.kernel.retain(|_, (ts, _, _)| ts.saturating_add(window) >= stream);
        let expired: Vec<u32> = self
            .launches
            .classic
            .iter()
            .filter(|(_, (ts, _, _))| ts.saturating_add(window) < stream)
            .map(|(pid, _)| *pid)
            .collect();
        for pid in expired {
            let (ts, c, h) = self.launches.classic.remove(&pid).expect("listed");
            // Only the classic half: the start key must come from the live process.
            let Some(live) = self.lookups.live_process(pid) else {
                self.counters.launch_join_miss += 1;
                Counters::add_class(&mut self.counters.actor_dropped, Class::Process);
                continue;
            };
            if let Some(known) = self.procs.get_mut(live.start_key) {
                // Its Kernel-Process half came, more than the join window away: that
                // Launch stands (its own join miss is counted at its deadline). The
                // cache keeps its integrity and creation time, and gains the details.
                if known.cmd_line.is_none() {
                    known.cmd_line = Some(c.command_line.to_string_lossy());
                }
                if known.user.is_none()
                    && let Some(sid) = &c.user_sid
                {
                    let uid = sid.to_string();
                    let name = self.lookups.account_name(&uid).unwrap_or_default();
                    known.user = Some(User { uid: fit(uid, USER_UID_MAX), name: fit(name, USER_NAME_MAX) });
                }
                continue;
            }
            self.counters.launch_join_miss += 1;
            let mut info = ProcInfo { path: self.dos(&live.image_path), ..ProcInfo::new(live.start_key, pid) };
            self.apply_classic(&mut info, &c);
            info.parent = self.procs.lookup_at(c.parent_pid, ts).map(|p| p.to_ref(&self.id));
            self.procs.insert(info.clone(), ts);
            let Some(actor) = self.actor_sync(&h, Class::Process) else { continue };
            self.push_launch(ts, actor, &info, &live.image_path, true, None);
        }
    }

    pub(super) fn on_process_stop(&mut self, h: &Header, s: ProcessStop) {
        let key = start_key(self.id.kernel_boot_id, s.sequence_number);
        let process = match self.procs.get(key) {
            Some(p) => p.to_ref(&self.id),
            None => {
                let mut r = self.id.bare_ref(key, s.pid);
                r.file.name = String::from_utf8_lossy(&s.image_name).into_owned();
                r
            }
        };
        let ev = self.event(
            h.ts,
            EventKind::Process(ProcessActivity::Terminate { process, exit_code: Some(s.exit_code as i32) }),
        );
        self.completion.push(ev);
        self.procs.end(key, h.ts);
        self.seeding.owner_exited(s.pid);
    }

    pub(super) fn on_image_load(&mut self, h: &Header, i: ImageLoad) {
        // The payload PID is authoritative (§5.3); the header's start key names
        // it when the header is the same process.
        let actor = if h.pid == i.pid {
            self.actor_sync(h, Class::Module)
        } else {
            self.actor_payload(i.pid, h.ts, Class::Module)
        };
        let Some(actor) = actor else { return };
        let nt = i.image_name.to_string_lossy();
        let path = self.dos(&nt);
        let file = File { name: file_name(&path).to_string(), path, hashes: None, signature: None };
        let ev = self.event(
            h.ts,
            EventKind::Module(ModuleActivity {
                actor,
                action: ModuleAction::Load { file, base_address: i.image_base },
            }),
        );
        if nt.is_empty() {
            self.completion.push(ev);
            return;
        }
        let w = self.wait(Reason::Enrich, self.cfg.enrich_deadline);
        let Some(id) = self.push_with(ev, vec![w], image_expansion(&nt, None)) else { return };
        self.launches.enrich.insert(id, EnrichTarget::Module);
        self.requests.push(Request::Enrich { id, target: EnrichTarget::Module, nt_path: nt });
    }

    pub(super) fn on_enriched(&mut self, id: PendingId, hashes: Option<Hashes>, sig: Option<Signature>, error: bool) {
        let Some(target) = self.launches.enrich.take(id) else { return };
        if error {
            self.counters.enrichment_errors += 1;
        }
        // A late result is cached by the workers for the next event (§6.3).
        if !alive(&self.completion, id, Reason::Enrich) {
            return;
        }
        self.completion.update(id, |e| {
            let file = match (&mut e.kind, target) {
                (EventKind::Process(ProcessActivity::Launch { process, .. }), EnrichTarget::LaunchImage) => {
                    &mut process.file
                }
                (EventKind::Module(m), EnrichTarget::Module) => match &mut m.action {
                    ModuleAction::Load { file, .. } => file,
                },
                _ => return,
            };
            file.hashes = hashes;
            file.signature = sig;
        });
        self.completion.resolve(id, Reason::Enrich);
    }
}

/// An image path with 8.3 components is expanded (plan 1b-3a decision D4); for a
/// Launch, the cached process gets the long path too.
fn image_expansion(nt: &str, start_key: Option<u64>) -> Vec<(String, Expansion)> {
    if !has_short_name(nt) {
        return Vec::new();
    }
    vec![(nt.to_string(), Expansion { slot: Slot::Image, image_key: start_key, ..Expansion::file(None) })]
}

fn launch_process(info: &ProcInfo, id: &Identity) -> Process {
    let r = info.to_ref(id);
    let (cmd_line, cmd_line_truncated) = truncate_cmd(info.cmd_line.as_deref().unwrap_or_default());
    Process {
        uid: r.uid,
        pid: r.pid,
        file: r.file,
        user: r.user,
        cmd_line,
        cmd_line_truncated,
        created_time: info.created_time.unwrap_or(0),
        integrity: info.integrity,
        parent_process: info.parent.clone(),
    }
}

fn truncate_cmd(s: &str) -> (String, bool) {
    let (t, cut) = truncate_utf8(s, CMD_LINE_MAX);
    (t.to_string(), cut)
}

fn set_launch_details(e: &mut Event, cmd: Option<String>, user: Option<User>) {
    if let EventKind::Process(ProcessActivity::Launch { process, .. }) = &mut e.kind {
        let (c, cut) = truncate_cmd(cmd.as_deref().unwrap_or_default());
        process.cmd_line = c;
        process.cmd_line_truncated = cut;
        process.user = user;
    }
}

/// A Launch whose classic half never came: the command line from the live
/// process, if it is still running and is the same process (§5.2).
pub(super) fn join_fallback<L: Lookups>(e: &mut Event, lookups: &mut L, procs: &mut ProcessCache, id: &Identity) {
    let EventKind::Process(ProcessActivity::Launch { process, .. }) = &mut e.kind else { return };
    let Some(live) = lookups.live_process(process.pid) else { return };
    if id.uid(live.start_key) != process.uid {
        return;
    }
    if let Some(cmd) = live.command_line {
        let (c, cut) = truncate_cmd(&cmd);
        process.cmd_line = c;
        process.cmd_line_truncated = cut;
        if let Some(info) = procs.get_mut(live.start_key) {
            info.cmd_line = Some(cmd);
        }
    }
}
```

- [ ] **Step 4: Files**

`src/pipeline/file.rs`:
```rust
//! Files (sensor spec §5.5, §7.1, §7.2): the FileObject map, the failure-confirm
//! window, Update coalescing, delete-on-close, renames and watchlist Opens.
//! Every emitted path with 8.3 components is expanded first (`expand`).

use std::collections::{HashMap, VecDeque};

use atlas_etw::parse::{FileCreate, FileHandle, FileOpEnd, FilePath, FileSetInfo};
use atlas_schema::classes::file::{FileAction, FileSystemActivity};
use atlas_schema::{Event, EventKind, File, ProcessRef, ProcessUid};

use super::Pipeline;
use super::expand::{Expansion, Slot};
use crate::completion::{PendingId, Reason, Wait};
use crate::counters::Class;
use crate::input::Header;
use crate::paths::has_short_name;
use crate::process::{empty_file, file_name};
use crate::recent::Recent;
use crate::services::{HandleKind, Lookups, Request};

/// Caps of the Irp and coalescing sets (each also expires by time).
const FAILED_CAP: usize = 1 << 16;
const CONFIRMED_CAP: usize = 1 << 18;
const OPENED_CAP: usize = 1 << 16;

/// A watchlist Open's coalescing key: (actor, logged path lowercased).
type OpenKey = (ProcessUid, String);

/// `FileInformationClass` values (§5.1).
const FILE_BASIC_INFORMATION: u32 = 4;
const FILE_END_OF_FILE_INFORMATION: u32 = 19;

/// One FileObject (§7.1).
#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    /// The NT path as logged, if known (a provisional entry has none until seeded).
    pub(crate) nt: Option<String>,
    /// Its long form, once an expansion of `nt` came back.
    pub(crate) expanded: Option<String>,
    /// Who opened the handle: the actor of its Update and delete-on-close Delete.
    pub(crate) opener: Option<ProcessRef>,
    written: bool,
    delete_on_close: bool,
    cleaned: bool,
    /// QPC of the Create (or snapshot) that set it.
    pub(crate) since: i64,
    touched: u64,
    /// Events waiting for the seeder to name this handle.
    pub(crate) waiting: Vec<(PendingId, i64, Fill)>,
}

/// What a seeded name fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fill {
    /// `file` (SetAttributes: its actor is the event's header).
    File,
    /// `file` and the actor, the handle's owner (an Update or Delete at Cleanup).
    Opened,
}

enum Held {
    /// A Create: on failure the map entry goes, and any watchlist Open with it,
    /// and its coalescing record, so a retry is not suppressed.
    Create {
        fo: u64,
        open: Option<PendingId>,
        coalesce: Option<(OpenKey, i64)>,
    },
    /// The event's id, unless it went out at once (`file.op_end` off).
    Delete {
        id: Option<PendingId>,
    },
    Rename {
        id: Option<PendingId>,
        fo: u64,
        new_nt: String,
    },
}

pub struct Files {
    pub(crate) map: HashMap<u64, FileEntry>,
    cap: usize,
    clock: u64,
    evictions: u64,
    window: i64,
    /// Operations in their failure-confirm window: (QPC, Irp, what), in stream
    /// order; a failed one is taken out (`None`) and skipped.
    held: VecDeque<(i64, u64, Option<Held>)>,
    /// The position of `held`'s front, counted since the start.
    held_base: u64,
    /// Irp → position of the latest held operation with it (Irps are recycled).
    held_by_irp: HashMap<u64, u64>,
    /// Failed OperationEnds no held operation claimed yet (a late operation): Irp → QPC.
    failed: Recent<u64>,
    /// Recently confirmed operations, to recognise a failure that comes too late.
    confirmed: Recent<u64>,
    /// Watchlist Opens → when last emitted, for the 60 s coalescing.
    opened: Recent<OpenKey>,
    /// Pending id → the handle it waits on, to forget it when it leaves.
    waiter_fo: HashMap<PendingId, u64>,
}

impl Files {
    pub fn new(cap: usize, window_ticks: i64) -> Self {
        Files {
            map: HashMap::new(),
            cap,
            clock: 0,
            evictions: 0,
            window: window_ticks,
            held: VecDeque::new(),
            held_base: 0,
            held_by_irp: HashMap::new(),
            failed: Recent::new(FAILED_CAP),
            confirmed: Recent::new(CONFIRMED_CAP),
            opened: Recent::new(OPENED_CAP),
            waiter_fo: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 6] {
        [
            ("file waiters", self.map.values().map(|e| e.waiting.len()).sum()),
            ("file waiter index", self.waiter_fo.len()),
            ("held operations", self.held.len() + self.held_by_irp.len()),
            ("failed irps", self.failed.len()),
            ("confirmed irps", self.confirmed.len()),
            ("watchlist opens", self.opened.len()),
        ]
    }

    /// The event left the completion stage.
    pub(super) fn forget(&mut self, id: PendingId) {
        if let Some(fo) = self.waiter_fo.remove(&id)
            && let Some(e) = self.map.get_mut(&fo)
        {
            e.waiting.retain(|(w, _, _)| *w != id);
        }
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    fn insert(&mut self, fo: u64, e: FileEntry) {
        self.clock += 1;
        self.map.insert(fo, FileEntry { touched: self.clock, ..e });
        if self.map.len() > self.cap {
            for k in crate::evict::oldest(self.map.iter().map(|(k, e)| (*k, e.touched)), self.map.len()) {
                self.map.remove(&k);
                self.evictions += 1;
            }
        }
    }

    fn touch(&mut self, fo: u64) -> Option<&mut FileEntry> {
        self.clock += 1;
        let e = self.map.get_mut(&fo)?;
        e.touched = self.clock;
        Some(e)
    }
}

fn entry(nt: Option<String>, opener: Option<ProcessRef>, since: i64) -> FileEntry {
    FileEntry {
        nt,
        expanded: None,
        opener,
        written: false,
        delete_on_close: false,
        cleaned: false,
        since,
        touched: 0,
        waiting: Vec::new(),
    }
}

/// The expansion to ask for, if `emit_file` found one.
fn expansion(nt: Option<String>, x: Expansion) -> Vec<(String, Expansion)> {
    nt.map(|n| (n, x)).into_iter().collect()
}

impl<L: Lookups> Pipeline<L> {
    pub(crate) fn file_obj(&mut self, nt: &str) -> File {
        let path = self.dos(nt);
        File { name: file_name(&path).to_string(), path, hashes: None, signature: None }
    }

    fn file_event(&mut self, ts: i64, actor: ProcessRef, file: File, action: FileAction) -> Event {
        self.event(ts, EventKind::File(FileSystemActivity { actor, file, action }))
    }

    /// The failure-confirm wait, unless `file.op_end` is off (§5.5).
    fn confirm_waits(&self) -> Vec<Wait> {
        if self.cfg.file_op_end {
            vec![Wait { reason: Reason::Confirm, deadline: None, drop_at_deadline: false }]
        } else {
            Vec::new()
        }
    }

    /// Holds an operation for its confirm window, unless a failure for its Irp
    /// was already seen: the operation arrived late, after its OperationEnd (§5.5).
    fn hold(&mut self, ts: i64, irp: u64, held: Held) {
        if !self.cfg.file_op_end {
            self.confirm(held);
            return;
        }
        let window = self.files.window;
        if let Some(fts) = self.files.failed.get(&irp)
            && fts >= ts
            && fts - ts <= window
        {
            self.files.failed.remove(&irp);
            self.fail(held);
            return;
        }
        let at = self.files.held_base + self.files.held.len() as u64;
        self.files.held_by_irp.insert(irp, at);
        self.files.held.push_back((ts, irp, Some(held)));
    }

    fn fail(&mut self, held: Held) {
        self.counters.file_op_failed += 1;
        match held {
            Held::Create { fo, open, coalesce } => {
                self.files.map.remove(&fo);
                if let Some(id) = open {
                    self.completion.cancel(id);
                }
                if let Some((key, at)) = coalesce
                    && self.files.opened.get(&key) == Some(at)
                {
                    self.files.opened.remove(&key);
                }
            }
            Held::Delete { id } | Held::Rename { id, .. } => {
                if let Some(id) = id {
                    self.completion.cancel(id);
                }
            }
        }
    }

    pub(super) fn on_file_create(&mut self, h: &Header, c: FileCreate) {
        let nt = c.file_name.to_string_lossy();
        let opener = self.actor_sync(h, Class::File);
        if self.files.map.contains_key(&c.file_object) {
            self.counters.file_object_replaced += 1;
        }
        self.seeding.changed(HandleKind::File, c.file_object, h.ts);
        let mut e = entry(Some(nt.clone()), opener.clone(), h.ts);
        e.delete_on_close = c.delete_on_close();
        self.files.insert(c.file_object, e);
        let (open, coalesce) = match opener {
            Some(actor) => self.watch_open(h.ts, actor, &nt, c.file_object),
            None => (None, None),
        };
        self.hold(h.ts, c.irp, Held::Create { fo: c.file_object, open, coalesce });
    }

    /// A Create on a watchlisted path emits `Open` after the confirm window; a
    /// path with 8.3 components is matched again once expanded (§7.2).
    /// Returns the Open's id, if it waits, and its coalescing record.
    fn watch_open(
        &mut self,
        ts: i64,
        actor: ProcessRef,
        nt: &str,
        fo: u64,
    ) -> (Option<PendingId>, Option<(OpenKey, i64)>) {
        let logged = self.watch.matches(nt);
        if !logged && !has_short_name(nt) {
            return (None, None);
        }
        let key = (actor.uid, nt.to_lowercase());
        let period = self.ticks.of(self.cfg.watchlist_coalesce);
        if self.files.opened.get(&key).is_some_and(|last| ts.saturating_sub(last) < period) {
            return (None, None);
        }
        let (file, expand) = self.emit_file(nt, Some(fo));
        let x = Expansion { open_logged: Some(logged), ..Expansion::file(Some(fo)) };
        let ev = self.file_event(ts, actor, file, FileAction::Open);
        let waits = self.confirm_waits();
        let id = self.push_with(ev, waits, expansion(expand, x));
        self.files.opened.insert(key.clone(), ts);
        (id, Some((key, ts)))
    }

    pub(super) fn on_file_create_new(&mut self, h: &Header, c: FileCreate) {
        let nt = c.file_name.to_string_lossy();
        let Some(actor) = self.actor_sync(h, Class::File) else { return };
        if !self.files.map.contains_key(&c.file_object) {
            self.seeding.changed(HandleKind::File, c.file_object, h.ts);
            self.files.insert(c.file_object, entry(Some(nt.clone()), Some(actor.clone()), h.ts));
        }
        let (file, expand) = self.emit_file(&nt, Some(c.file_object));
        let ev = self.file_event(h.ts, actor, file, FileAction::Create);
        self.push_with(ev, vec![], expansion(expand, Expansion::file(Some(c.file_object))));
    }

    /// A Write, or a truncation (the overwrite case, §5.1).
    pub(super) fn on_file_write(&mut self, h: &Header, fo: u64) {
        match self.files.touch(fo) {
            Some(e) => {
                if e.cleaned {
                    self.counters.writes_after_cleanup += 1;
                }
                e.written = true;
            }
            None => {
                let mut e = entry(None, None, h.ts);
                e.written = true;
                self.files.insert(fo, e);
                self.seeding.ask(HandleKind::File, fo, self.cfg.seed_on_miss);
            }
        }
    }

    pub(super) fn on_file_set_info(&mut self, h: &Header, i: FileSetInfo) {
        match i.info_class {
            FILE_END_OF_FILE_INFORMATION => self.on_file_write(h, i.file_object),
            FILE_BASIC_INFORMATION => {
                let Some(actor) = self.actor_sync(h, Class::File) else { return };
                self.file_action_on(h.ts, i.file_object, actor, FileAction::SetAttributes, Fill::File, true);
            }
            _ => {}
        }
    }

    /// Emits an action whose path comes from the FileObject map; an unknown
    /// handle waits for the seeder (§7.1).
    fn file_action_on(
        &mut self,
        ts: i64,
        fo: u64,
        actor: ProcessRef,
        action: FileAction,
        fill: Fill,
        drop_unresolved: bool,
    ) {
        let known = self.files.touch(fo).and_then(|e| e.nt.clone());
        match known {
            Some(nt) => {
                self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
                let (file, expand) = self.emit_file(&nt, Some(fo));
                let ev = self.file_event(ts, actor, file, action);
                self.push_with(ev, vec![], expansion(expand, Expansion::file(Some(fo))));
            }
            None => {
                let can_wait = self.cfg.seed_on_miss || self.seeding.startup_pending(HandleKind::File);
                if !can_wait || self.seeding.is_unnamable(HandleKind::File, fo) {
                    if drop_unresolved {
                        self.counters.unknown_file_object += 1;
                    }
                    return;
                }
                if !self.files.map.contains_key(&fo) {
                    self.files.insert(fo, entry(None, None, ts));
                    self.seeding.ask(HandleKind::File, fo, self.cfg.seed_on_miss);
                }
                // A seeded name is the handle's final (long) path: no expansion needed.
                let ev = self.file_event(ts, actor, empty_file(), action);
                let w = self.seeder_wait(drop_unresolved);
                let id = self.completion.push_pending(ev, vec![w]);
                if let Some(e) = self.files.map.get_mut(&fo) {
                    e.waiting.push((id, ts, fill));
                    self.files.waiter_fo.insert(id, fo);
                }
            }
        }
    }

    pub(super) fn on_file_delete_path(&mut self, h: &Header, p: FilePath) {
        let Some(actor) = self.actor_sync(h, Class::File) else { return };
        let nt = p.file_path.to_string_lossy();
        self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
        let (file, expand) = self.emit_file(&nt, None);
        let ev = self.file_event(h.ts, actor, file, FileAction::Delete);
        let waits = self.confirm_waits();
        let id = self.push_with(ev, waits, expansion(expand, Expansion::file(None)));
        self.hold(h.ts, p.irp, Held::Delete { id });
    }

    pub(super) fn on_file_rename_path(&mut self, h: &Header, p: FilePath) {
        let Some(actor) = self.actor_sync(h, Class::File) else { return };
        let new_nt = p.file_path.to_string_lossy();
        let (file_result, expand_result) = self.emit_file(&new_nt, None);
        let source = self.files.touch(p.file_object).and_then(|e| e.nt.clone());
        let waits = self.confirm_waits();
        let mut expands = expansion(expand_result, Expansion { slot: Slot::RenameResult, ..Expansion::file(None) });
        let file = match &source {
            Some(nt) => {
                self.requests.push(Request::InvalidateHash { nt_path: nt.clone() });
                let (f, x) = self.emit_file(nt, Some(p.file_object));
                expands.extend(expansion(x, Expansion::file(None)));
                f
            }
            None => {
                // A handle opened before we watched. A snapshot read now could only
                // give the new name, so the source goes out empty (clarification 7);
                // the handle takes the new name once the rename stands.
                if !self.files.map.contains_key(&p.file_object) {
                    self.files.insert(p.file_object, entry(None, None, h.ts));
                }
                empty_file()
            }
        };
        let ev = self.file_event(h.ts, actor, file, FileAction::Rename { file_result });
        let id = self.push_with(ev, waits, expands);
        self.hold(h.ts, p.irp, Held::Rename { id, fo: p.file_object, new_nt });
    }

    pub(super) fn on_file_op_end(&mut self, h: &Header, o: FileOpEnd) {
        if !o.failed() {
            return; // the callback discards these (§3.2); replay may not
        }
        // The most recent held operation with this Irp (Irps are recycled).
        let base = self.files.held_base;
        let held = self
            .files
            .held_by_irp
            .remove(&o.irp)
            .and_then(|at| self.files.held.get_mut(usize::try_from(at - base).ok()?))
            .and_then(|(_, _, held)| held.take());
        if let Some(held) = held {
            self.fail(held);
        } else if self.files.confirmed.get(&o.irp).is_some() {
            self.counters.file_op_late_failure += 1;
        } else {
            self.files.failed.insert(o.irp, h.ts);
        }
    }

    pub(super) fn on_file_cleanup(&mut self, h: &Header, x: FileHandle) {
        let Some(e) = self.files.touch(x.file_object) else { return };
        let update = e.written && !e.cleaned;
        let delete = e.delete_on_close;
        e.cleaned = true;
        let opener = e.opener.clone();
        for action in [update.then_some(FileAction::Update), delete.then_some(FileAction::Delete)].into_iter().flatten()
        {
            // The actor is the handle's opener, never the Cleanup's header (§7.1).
            match opener.clone() {
                Some(actor) => self.file_action_on(h.ts, x.file_object, actor, action, Fill::Opened, true),
                None => {
                    // A provisional entry: the seeder may still name it and its owner;
                    // dropped at the deadline otherwise (the §7.1 carve-out from E11).
                    let placeholder = self.id.bare_ref(0, 0);
                    let known = self.files.map.get(&x.file_object).and_then(|e| e.nt.clone());
                    if known.is_some() {
                        // Named but without an owner we can resolve: not attributable.
                        self.counters.unknown_file_object += 1;
                        continue;
                    }
                    self.file_action_on(h.ts, x.file_object, placeholder, action, Fill::Opened, true);
                }
            }
        }
    }

    pub(super) fn on_file_close(&mut self, h: &Header, x: FileHandle) {
        self.files.map.remove(&x.file_object);
        self.seeding.changed(HandleKind::File, x.file_object, h.ts);
    }

    /// An operation stands (§5.5).
    fn confirm(&mut self, held: Held) {
        match held {
            Held::Create { open, .. } | Held::Delete { id: open } => {
                if let Some(id) = open {
                    self.completion.resolve(id, Reason::Confirm);
                }
            }
            Held::Rename { id, fo, new_nt } => {
                if let Some(id) = id {
                    self.completion.resolve(id, Reason::Confirm);
                }
                if let Some(e) = self.files.map.get_mut(&fo) {
                    e.nt = Some(new_nt); // later Updates carry the new name (§7.1)
                    e.expanded = None;
                }
            }
        }
    }

    /// Stream time passed: operations past their window stand (§5.5).
    pub(super) fn file_confirm(&mut self, stream: i64) {
        let window = self.files.window;
        while let Some((ts, _, _)) = self.files.held.front() {
            if ts.saturating_add(window) > stream {
                break;
            }
            let (ts, irp, held) = self.files.held.pop_front().expect("peeked");
            let at = self.files.held_base;
            self.files.held_base += 1;
            if self.files.held_by_irp.get(&irp) == Some(&at) {
                self.files.held_by_irp.remove(&irp);
            }
            if let Some(held) = held {
                self.confirm(held);
                self.files.confirmed.insert(irp, ts);
            }
        }
        self.files.failed.expire(stream.saturating_sub(window));
        self.files.confirmed.expire(stream.saturating_sub(window.saturating_mul(40)));
        self.files.opened.expire(stream.saturating_sub(self.ticks.of(self.cfg.watchlist_coalesce)));
    }
}

/// An entry named by the seeder for a handle opened before we watched (§7.4).
pub(crate) fn seeded_entry(nt: &str, opener: Option<ProcessRef>, taken: i64) -> FileEntry {
    entry(Some(nt.to_string()), opener, taken)
}

/// Fills a file event waiting for a seeded name.
pub(crate) fn fill_file_event(e: &mut Event, fill: Fill, file: File, actor: Option<ProcessRef>) {
    if let EventKind::File(f) = &mut e.kind {
        f.file = file;
        if let (Fill::Opened, Some(a)) = (fill, actor) {
            f.actor = a;
        }
    }
}
```

- [ ] **Step 5: Registry**

`src/pipeline/reg.rs`:
```rust
//! Registry (sensor spec §5.1, §7.4, §7.5): key and value events with paths from
//! the key map, the seeder for unknown handles, the `path_unresolved` floor,
//! and value reads after the event.

use std::collections::HashMap;

use atlas_etw::parse::{RegDeleteValue, RegKey, RegOpen, RegSetValue, WStr};
use atlas_schema::classes::registry::{
    RegType, RegistryKeyAction, RegistryKeyActivity, RegistryValueAction, RegistryValueActivity,
};
use atlas_schema::limits::{PATH_MAX, REG_DATA_MAX};
use atlas_schema::{Event, EventKind, ProcessRef};

use super::{Pending, Pipeline, alive};
use crate::completion::{PendingId, Reason};
use crate::counters::Class;
use crate::input::Header;
use crate::keymap::Resolved;
use crate::paths;
use crate::services::{EarlyKey, HandleKind, Lookups, Request, ValueData, ValueRead};

/// What a value read must match to be accepted (§7.5).
#[derive(Debug, Clone, Copy)]
struct Expected {
    value_type: u32,
    data_size: u32,
}

/// An event waiting for the seeder to name a key.
#[derive(Debug, Clone)]
pub(crate) struct KeyWaiter {
    pub(crate) id: PendingId,
    /// QPC of the event (the seeder's answer must not postdate a reuse, §7.4).
    pub(crate) ts: i64,
    /// The handle whose name the event needs.
    pub(crate) key: u64,
    /// For a Value Set: the read to start once named.
    read: Option<(Vec<u16>, Expected, EarlyKey)>,
}

#[derive(Default)]
pub struct Registry {
    /// Fast-path read results (§7.5), until their event is processed.
    early: HashMap<EarlyKey, (ValueRead, Option<ValueData>)>,
    reads: Pending<Expected>,
    /// Unnamed root address → events waiting for it.
    pub(crate) waiting: HashMap<u64, Vec<KeyWaiter>>,
    /// Pending id → the root it waits on, to forget it when it leaves.
    waiter_root: HashMap<PendingId, u64>,
}

impl Registry {
    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 4] {
        [
            ("value reads", self.reads.by_id.len()),
            ("key waiters", self.waiting.values().map(Vec::len).sum()),
            ("key waiter index", self.waiter_root.len()),
            ("early reads", self.early.len()),
        ]
    }

    /// The event left the completion stage.
    pub(super) fn forget(&mut self, id: PendingId) {
        self.reads.take(id);
        if let Some(root) = self.waiter_root.remove(&id)
            && let Some(ws) = self.waiting.get_mut(&root)
        {
            ws.retain(|w| w.id != id);
            if ws.is_empty() {
                self.waiting.remove(&root);
            }
        }
    }

    pub(super) fn on_early_read(&mut self, event: EarlyKey, read: ValueRead, result: Option<ValueData>) {
        // Results are pruned every tick (`prune_early`); this cap only guards
        // against a flood within one tick, and clearing it only costs redone reads.
        if self.early.len() >= 65_536 {
            self.early.clear();
        }
        self.early.insert(event, (read, result));
    }

    /// Drops fast-path results older than `before` (their events have passed).
    pub(super) fn prune_early(&mut self, before: i64) {
        self.early.retain(|k, _| k.ts >= before);
    }
}

/// What the key map can say about a handle's name, for an event at `ts`.
enum KeyPath {
    Known(String),
    /// Waiting for the seeder to name `root`; `partial` is what is known now.
    Wait {
        partial: String,
        root: u64,
    },
    /// The floor (§7.4): only what ETW logged.
    Unresolved(String),
}

fn units_to_string(w: &WStr) -> String {
    paths::fit(w.to_string_lossy(), PATH_MAX)
}

impl<L: Lookups> Pipeline<L> {
    fn key_path(&mut self, key: u64) -> KeyPath {
        match self.keys.resolve(key) {
            Some(Resolved::Full(nt)) => KeyPath::Known(nt),
            Some(Resolved::Partial { known, root }) => self.key_wait(known, root),
            None => self.key_wait(String::new(), key),
        }
    }

    fn key_wait(&mut self, partial: String, root: u64) -> KeyPath {
        let can_wait = self.cfg.seed_on_miss || self.seeding.startup_pending(HandleKind::Key);
        if !can_wait || self.seeding.is_unnamable(HandleKind::Key, root) {
            return KeyPath::Unresolved(partial);
        }
        self.seeding.ask(HandleKind::Key, root, self.cfg.seed_on_miss);
        KeyPath::Wait { partial, root }
    }

    /// Pushes a registry event whose path is `path`; waits for the seeder when
    /// needed. `set_path` writes a (normalized) path into the event.
    fn push_registry(&mut self, ts: i64, key: u64, mut ev: Event, read: Option<(Vec<u16>, Expected, EarlyKey)>) {
        match self.key_path(key) {
            KeyPath::Known(nt) => {
                set_path(&mut ev, &paths::registry(&nt, self.ccs), false);
                match read {
                    Some((name, exp, early)) => {
                        let id = self.completion.push_pending(ev, vec![]);
                        self.start_read(id, nt, name, exp, early);
                    }
                    None => self.completion.push(ev),
                }
            }
            KeyPath::Unresolved(partial) => {
                set_path(&mut ev, &partial, true);
                self.counters.registry_unresolved += 1;
                if no_read(&mut ev.kind) {
                    self.counters.value_read_failed += 1;
                }
                self.completion.push(ev);
            }
            KeyPath::Wait { partial, root } => {
                set_path(&mut ev, &partial, true);
                let w = self.seeder_wait(false);
                let id = self.completion.push_pending(ev, vec![w]);
                self.reg.waiting.entry(root).or_default().push(KeyWaiter { id, ts, key, read });
                self.reg.waiter_root.insert(id, root);
            }
        }
    }

    /// Starts the value read for a pending Value Set whose key is named (§7.5):
    /// a fast-path result for the same key and name is used; otherwise the
    /// ordered path reads now.
    fn start_read(&mut self, id: PendingId, nt: String, name: Vec<u16>, exp: Expected, early: EarlyKey) {
        if let Some((read, result)) = self.reg.early.remove(&early) {
            if read.key_path == nt && read.value_name == name {
                self.apply_read(id, exp, result);
                return;
            }
            self.counters.early_read_redone += 1;
        }
        let w = self.wait(Reason::ValueRead, self.cfg.value_read_deadline);
        self.completion.add_wait(id, w);
        self.reg.reads.insert(id, exp);
        self.requests.push(Request::ReadValue { id, read: ValueRead { key_path: nt, value_name: name } });
    }

    pub(super) fn on_value_read(&mut self, id: PendingId, result: Option<ValueData>) {
        let Some(exp) = self.reg.reads.take(id) else { return };
        if !alive(&self.completion, id, Reason::ValueRead) {
            return;
        }
        self.apply_read(id, exp, result);
        self.completion.resolve(id, Reason::ValueRead);
    }

    /// Accepts the data only if its type and length match the event (§7.5).
    fn apply_read(&mut self, id: PendingId, exp: Expected, result: Option<ValueData>) {
        let ok = result.filter(|r| r.value_type == exp.value_type && r.size == exp.data_size);
        if ok.is_none() {
            self.counters.value_read_failed += 1;
        }
        self.completion.update(id, |e| {
            if let EventKind::RegistryValue(RegistryValueActivity {
                action: RegistryValueAction::Set { data, data_truncated, data_unavailable, .. },
                ..
            }) = &mut e.kind
                && let Some(r) = ok
            {
                let mut d = r.data;
                d.truncate(REG_DATA_MAX);
                *data = d;
                *data_truncated = exp.data_size as usize > REG_DATA_MAX;
                *data_unavailable = false;
            }
        });
    }

    pub(super) fn on_reg_open(&mut self, h: &Header, o: RegOpen, create: bool) {
        if o.status != 0 {
            return; // includes STATUS_REPARSE's first attempt (§5.5)
        }
        let rel = units_to_string(&o.relative_name);
        self.keys.open(o.key_object, o.base_object, &rel, h.ts);
        self.seeding.changed(HandleKind::Key, o.key_object, h.ts);
        if !(create && o.disposition == 1) {
            return; // an open, or a create that opened an existing key
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryKey) else { return };
        let ev = self.key_event(h.ts, actor, RegistryKeyAction::Create);
        self.push_registry(h.ts, o.key_object, ev, None);
    }

    pub(super) fn on_reg_close(&mut self, h: &Header, k: RegKey) {
        // The agent's own closes (seeding duplicates, value reads) are ignored (§7.4).
        if self.is_self_key(h.start_key) {
            return;
        }
        self.keys.close(k.key_object, h.ts);
        self.seeding.changed(HandleKind::Key, k.key_object, h.ts);
    }

    pub(super) fn on_reg_delete_key(&mut self, h: &Header, k: RegKey) {
        if k.status != 0 {
            return;
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryKey) else { return };
        let ev = self.key_event(h.ts, actor, RegistryKeyAction::Delete);
        self.push_registry(h.ts, k.key_object, ev, None);
    }

    pub(super) fn on_reg_set_value(&mut self, h: &Header, v: RegSetValue) {
        if v.status != 0 {
            return;
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryValue) else { return };
        if v.value_name_ambiguous {
            self.counters.reg_name_ambiguous += 1;
        }
        let value_type = RegType::from_raw(v.value_type);
        let unusual = matches!(value_type, RegType::Raw(_));
        if unusual {
            self.counters.reg_type_unusual += 1;
        }
        let read = (self.cfg.registry_value_reads && !unusual).then(|| {
            let early = EarlyKey { ts: h.ts, tid: h.tid, key_object: v.key_object };
            (v.value_name.as_units().to_vec(), Expected { value_type: v.value_type, data_size: v.data_size }, early)
        });
        let action = RegistryValueAction::Set {
            value_type,
            data: Vec::new(),
            data_truncated: false,
            data_read_after: read.is_some(),
            data_unavailable: true,
        };
        let ev = self.value_event(h.ts, actor, units_to_string(&v.value_name), action);
        self.push_registry(h.ts, v.key_object, ev, read);
    }

    pub(super) fn on_reg_delete_value(&mut self, h: &Header, v: RegDeleteValue) {
        if v.status != 0 {
            return;
        }
        let Some(actor) = self.actor_sync(h, Class::RegistryValue) else { return };
        let ev = self.value_event(h.ts, actor, units_to_string(&v.value_name), RegistryValueAction::Delete);
        self.push_registry(h.ts, v.key_object, ev, None);
    }

    fn key_event(&mut self, ts: i64, actor: ProcessRef, action: RegistryKeyAction) -> Event {
        self.event(
            ts,
            EventKind::RegistryKey(RegistryKeyActivity { actor, path: String::new(), path_unresolved: true, action }),
        )
    }

    fn value_event(&mut self, ts: i64, actor: ProcessRef, name: String, action: RegistryValueAction) -> Event {
        self.event(
            ts,
            EventKind::RegistryValue(RegistryValueActivity {
                actor,
                key_path: String::new(),
                name,
                path_unresolved: true,
                action,
            }),
        )
    }

    /// The seeder named (or could not name) `root`: answer the events waiting
    /// for it whose time allows it (§7.4).
    pub(super) fn answer_key_waiters(&mut self, root: u64, taken: i64, named: bool) {
        let Some(waiters) = self.reg.waiting.remove(&root) else { return };
        let changed = self.seeding.last_change(HandleKind::Key, root);
        for w in waiters {
            if !alive(&self.completion, w.id, Reason::Seeder) {
                continue;
            }
            // The address changed hands since the earlier of the event and the
            // read: the seeded name may belong to the new owner (conservative for
            // a snapshot applied late).
            let reused = changed.is_some_and(|c| c > w.ts.min(taken));
            let full = if named && !reused {
                match self.keys.resolve(w.key) {
                    Some(Resolved::Full(nt)) => Some(nt),
                    _ => None,
                }
            } else {
                None
            };
            match full {
                Some(nt) => {
                    let path = paths::registry(&nt, self.ccs);
                    self.completion.update(w.id, |e| set_path(e, &path, false));
                    self.completion.resolve(w.id, Reason::Seeder);
                    if let Some((name, exp, early)) = w.read {
                        self.start_read(w.id, nt, name, exp, early);
                    }
                }
                None => {
                    // Emitted with the floor already in the event.
                    self.counters.registry_unresolved += 1;
                    let mut no = false;
                    self.completion.update(w.id, |e| no = no_read(&mut e.kind));
                    if no {
                        self.counters.value_read_failed += 1;
                    }
                    self.completion.resolve(w.id, Reason::Seeder);
                }
            }
        }
    }
}

/// A Value Set whose key stayed unresolved: no read was attempted (§7.5).
/// True if it was a Set that expected one.
pub(super) fn no_read(kind: &mut EventKind) -> bool {
    match kind {
        EventKind::RegistryValue(RegistryValueActivity {
            action: RegistryValueAction::Set { data_read_after, .. },
            ..
        }) if *data_read_after => {
            *data_read_after = false;
            true
        }
        _ => false,
    }
}

fn set_path(e: &mut Event, path: &str, unresolved: bool) {
    let path = &paths::fit(path.to_string(), PATH_MAX);
    match &mut e.kind {
        EventKind::RegistryKey(k) => {
            k.path = path.to_string();
            k.path_unresolved = unresolved;
        }
        EventKind::RegistryValue(v) => {
            v.key_path = path.to_string();
            v.path_unresolved = unresolved;
        }
        _ => {}
    }
}
```

- [ ] **Step 6: Network and DNS**

`src/pipeline/net.rs`:
```rust
//! Network and DNS (sensor spec §5.4, §7.3).
//!
//! Which end is which (plan 1b-2, F5): `saddr` is the local end for TCP connect,
//! TCP accept and UDP send; for UDP receive it is the remote sender.
//! OCSF `src_endpoint` is the initiator: the local end for outbound, the remote
//! end for inbound.

use std::collections::HashMap;
use std::net::IpAddr;

use atlas_etw::parse::{DnsAnswer as RawAnswer, DnsQuery, NetEvent, parse_query_results};
use atlas_schema::classes::dns::{DnsAction, DnsActivity, DnsAnswer};
use atlas_schema::classes::network::{NetworkAction, NetworkActivity, NetworkDirection, NetworkProtocol};
use atlas_schema::limits::{DNS_ANSWER_DATA_MAX, DNS_HOSTNAME_MAX, truncate_utf8};
use atlas_schema::{Event, EventKind, NetworkEndpoint, ProcessRef, ProcessUid};

use super::{IdGen, Pipeline, make_event};
use crate::completion::Completion;
use crate::counters::{Class, Counters};
use crate::input::Header;
use crate::process::Identity;
use crate::services::Lookups;
use crate::time::Clock;

pub(super) enum Tcp {
    Connect,
    Accept,
    Disconnect,
}

type Ep = (IpAddr, u16);

#[derive(Debug, Clone)]
struct Flow {
    actor: ProcessRef,
    local: Ep,
    remote: Ep,
    direction: NetworkDirection,
    last: i64,
}

pub struct Network {
    /// TCP connections opened while we watched: their direction, for the Close.
    tcp: HashMap<(u32, Ep, Ep), NetworkDirection>,
    /// UDP flows (§7.3), keyed by (actor, local, remote).
    flows: HashMap<(ProcessUid, Ep, Ep), Flow>,
    cap: usize,
    idle: i64,
    evictions: u64,
}

fn ep((ip, port): Ep) -> NetworkEndpoint {
    NetworkEndpoint { ip, port }
}

fn activity(
    actor: ProcessRef,
    local: Ep,
    remote: Ep,
    protocol: NetworkProtocol,
    direction: NetworkDirection,
    action: NetworkAction,
) -> EventKind {
    let (src, dst) = match direction {
        NetworkDirection::Outbound => (local, remote),
        NetworkDirection::Inbound => (remote, local),
    };
    EventKind::Network(NetworkActivity {
        actor,
        src_endpoint: ep(src),
        dst_endpoint: ep(dst),
        protocol,
        direction,
        action,
    })
}

const CLOSE: NetworkAction = NetworkAction::Close { bytes_in: None, bytes_out: None };

impl Network {
    pub fn new(cap: usize, idle_ticks: i64) -> Self {
        Network { tcp: HashMap::new(), flows: HashMap::new(), cap, idle: idle_ticks, evictions: 0 }
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    fn close_flow(f: Flow, out: &mut Completion<Event>, clock: &Clock, ids: &mut IdGen, id: &Identity) {
        // A UDP Close is timestamped at the flow's last datagram (§3.3).
        let kind = activity(f.actor, f.local, f.remote, NetworkProtocol::Udp, f.direction, CLOSE);
        out.push(make_event(clock, ids, id, f.last, kind));
    }

    /// Closes UDP flows idle for longer than the idle timeout at stream time `now`.
    pub(super) fn expire(
        &mut self,
        now: i64,
        out: &mut Completion<Event>,
        _c: &mut Counters,
        clock: &Clock,
        ids: &mut IdGen,
        id: &Identity,
    ) {
        let idle = self.idle;
        let mut gone: Vec<Flow> = Vec::new();
        self.flows.retain(|_, f| {
            let keep = f.last.saturating_add(idle) >= now;
            if !keep {
                gone.push(f.clone());
            }
            keep
        });
        gone.sort_by_key(|f| f.last);
        for f in gone {
            Self::close_flow(f, out, clock, ids, id);
        }
    }

    /// A clean stop closes every open flow.
    pub(super) fn close_all(
        &mut self,
        out: &mut Completion<Event>,
        c: &mut Counters,
        clock: &Clock,
        ids: &mut IdGen,
        id: &Identity,
    ) {
        self.expire(i64::MAX, out, c, clock, ids, id);
    }
}

impl<L: Lookups> Pipeline<L> {
    pub(super) fn on_tcp(&mut self, h: &Header, n: NetEvent, kind: Tcp) {
        let Some(actor) = self.actor_payload(n.pid, h.ts, Class::Network) else { return };
        let (local, remote) = ((n.saddr, n.sport), (n.daddr, n.dport));
        let key = (n.pid, local, remote);
        let (direction, action) = match kind {
            Tcp::Connect => (NetworkDirection::Outbound, NetworkAction::Open),
            Tcp::Accept => (NetworkDirection::Inbound, NetworkAction::Open),
            Tcp::Disconnect => {
                // A connection opened before we watched has no known direction:
                // the end with the lower port is taken to be the server.
                let d = self.net.tcp.remove(&key).unwrap_or(if local.1 < remote.1 {
                    NetworkDirection::Inbound
                } else {
                    NetworkDirection::Outbound
                });
                (d, CLOSE)
            }
        };
        if matches!(action, NetworkAction::Open) {
            if self.net.tcp.len() >= self.net.cap {
                self.net.tcp.clear();
                self.net.evictions += 1;
            }
            self.net.tcp.insert(key, direction);
        }
        let ev = self.event(h.ts, activity(actor, local, remote, NetworkProtocol::Tcp, direction, action));
        self.completion.push(ev);
    }

    pub(super) fn on_udp(&mut self, h: &Header, n: NetEvent, send: bool) {
        if !self.cfg.network_udp {
            return;
        }
        let Some(actor) = self.actor_payload(n.pid, h.ts, Class::Network) else { return };
        let (local, remote) =
            if send { ((n.saddr, n.sport), (n.daddr, n.dport)) } else { ((n.daddr, n.dport), (n.saddr, n.sport)) };
        let key = (actor.uid, local, remote);
        if let Some(f) = self.net.flows.get_mut(&key) {
            f.last = f.last.max(h.ts);
            return;
        }
        let direction = if send { NetworkDirection::Outbound } else { NetworkDirection::Inbound };
        let ev = self
            .event(h.ts, activity(actor.clone(), local, remote, NetworkProtocol::Udp, direction, NetworkAction::Open));
        self.completion.push(ev);
        if self.net.flows.len() >= self.net.cap {
            // The table is full: the least recently active flows are closed (§7.3).
            let ages = self.net.flows.iter().map(|(k, f)| (*k, f.last));
            let mut victims: Vec<Flow> = crate::evict::oldest(ages, self.net.flows.len())
                .into_iter()
                .filter_map(|k| self.net.flows.remove(&k))
                .collect();
            victims.sort_by_key(|f| f.last);
            for f in victims {
                self.net.evictions += 1;
                Network::close_flow(f, &mut self.completion, &self.clock, &mut self.ids, &self.id);
            }
        }
        self.net.flows.insert(key, Flow { actor, local, remote, direction, last: h.ts });
    }

    pub(super) fn on_dns(&mut self, h: &Header, q: DnsQuery) {
        let Some(actor) = self.actor_sync(h, Class::Dns) else { return };
        let name = q.query_name.to_string_lossy();
        let hostname = truncate_utf8(&name, DNS_HOSTNAME_MAX).0.to_string();
        let parsed = parse_query_results(q.query_results.as_units());
        let answers = parsed
            .answers
            .into_iter()
            .map(|a| {
                let (rr_type, data) = match a {
                    RawAnswer::Address(ip @ IpAddr::V4(_)) => (1, ip.to_string()),
                    RawAnswer::Address(ip @ IpAddr::V6(_)) => (28, ip.to_string()),
                    RawAnswer::Record { rtype, data } => (rtype, data),
                    // Neither form: kept, with type 0 (plan 1b-3a clarification 9).
                    RawAnswer::Unrecognized(s) => (0, s),
                };
                DnsAnswer { rr_type, data: truncate_utf8(&data, DNS_ANSWER_DATA_MAX).0.to_string() }
            })
            .collect();
        let action =
            DnsAction::Response { rcode: rcode(q.query_status), platform_status: Some(q.query_status), answers };
        let query_type = u16::try_from(q.query_type).unwrap_or(u16::MAX);
        let ev = self.event(h.ts, EventKind::Dns(DnsActivity { actor, hostname, query_type, action }));
        self.completion.push(ev);
    }
}

/// DNS response code from the DNS-Client status (§5.4).
pub(crate) fn rcode(status: u32) -> Option<u16> {
    Some(match status {
        0 | 9501 => 0,
        9001 => 1,
        9002 => 2,
        9003 => 3,
        9004 => 4,
        9005 => 5,
        _ => return None,
    })
}
```

- [ ] **Step 7: Seeding and 8.3 expansion**

`src/pipeline/seed.rs`:
```rust
//! Seeding the key and file maps from the handle table (sensor spec §7.4).
//!
//! The pipeline asks for addresses it cannot name, at most once per second per
//! handle type and only for addresses absent from the latest snapshot. Each
//! snapshot is applied once stream time reaches the time it was taken, and:
//! - a seeded name answers an event at time t only if no Create/Open/Close for
//!   that address happened between t and the snapshot (the address may have
//!   changed hands);
//! - it never overwrites an entry an ETW event set after the snapshot;
//! - addresses that cannot be named go into a negative cache until the address
//!   is reused or its owner exits; events on them do not wait.

use std::collections::{HashMap, HashSet};

use atlas_schema::File;

use super::Pipeline;
use super::file::{Fill, fill_file_event};
use crate::completion::Reason;
use crate::config::Ticks;
use crate::process::file_name;
use crate::recent::Recent;
use crate::services::{HandleKind, Lookups, Request, Snapshot};

/// Address history kept for the reuse rule (Creates, Opens, Closes).
const CHANGES_CAP: usize = 1 << 19;
/// The negative cache's cap.
const UNNAMABLE_CAP: usize = 1 << 16;

pub struct Seeding {
    asks: HashMap<HandleKind, HashSet<u64>>,
    last_request: HashMap<HandleKind, i64>,
    latest: HashMap<HandleKind, HashSet<u64>>,
    queued: Vec<Snapshot>,
    /// (kind, address) → (owner PID, insertion order). Owner 0: the address
    /// was not in the table at all.
    unnamable: HashMap<(HandleKind, u64), (u32, u64)>,
    unnamable_seq: u64,
    changes: Recent<(HandleKind, u64)>,
    /// Kinds whose start-up pass has not been applied yet.
    startup: HashSet<HandleKind>,
}

impl Seeding {
    pub(super) fn new() -> Self {
        Seeding {
            asks: HashMap::new(),
            last_request: HashMap::new(),
            latest: HashMap::new(),
            queued: Vec::new(),
            unnamable: HashMap::new(),
            unnamable_seq: 0,
            changes: Recent::new(CHANGES_CAP),
            startup: HashSet::new(),
        }
    }

    /// The start-up pass over every handle, keys first (§7.4).
    pub(super) fn start(&mut self, requests: &mut Vec<Request>) {
        for kind in [HandleKind::Key, HandleKind::File] {
            requests.push(Request::Seed { kind, addresses: Vec::new() });
            self.startup.insert(kind);
        }
    }

    /// Whether the start-up pass for `kind` is still to come: events on unknown
    /// handles may wait for it even with on-miss seeding off.
    pub(super) fn startup_pending(&self, kind: HandleKind) -> bool {
        self.startup.contains(&kind)
    }

    fn mark_unnamable(&mut self, kind: HandleKind, address: u64, owner: u32) {
        if self.unnamable.len() >= UNNAMABLE_CAP {
            let n = self.unnamable.len();
            for k in crate::evict::oldest(self.unnamable.iter().map(|(k, (_, seq))| (*k, *seq)), n) {
                self.unnamable.remove(&k);
            }
        }
        self.unnamable_seq += 1;
        self.unnamable.insert((kind, address), (owner, self.unnamable_seq));
    }

    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 2] {
        [("address history", self.changes.len()), ("queued snapshots", self.queued.len())]
    }

    /// Forgets address history older than `before`.
    pub(super) fn expire(&mut self, before: i64) {
        self.changes.expire(before);
    }

    pub(super) fn ask(&mut self, kind: HandleKind, address: u64, enabled: bool) {
        if !enabled || self.latest.get(&kind).is_some_and(|s| s.contains(&address)) {
            return;
        }
        self.asks.entry(kind).or_default().insert(address);
    }

    /// Sends the batched questions, at most once per second per kind.
    pub(super) fn flush_asks(&mut self, now: i64, ticks: Ticks, requests: &mut Vec<Request>) {
        let second = ticks.of(std::time::Duration::from_secs(1));
        for kind in [HandleKind::Key, HandleKind::File] {
            let due = self.last_request.get(&kind).is_none_or(|t| now.saturating_sub(*t) >= second);
            if !due {
                continue;
            }
            if let Some(set) = self.asks.remove(&kind).filter(|s| !s.is_empty()) {
                let mut addresses: Vec<u64> = set.into_iter().collect();
                addresses.sort_unstable();
                requests.push(Request::Seed { kind, addresses });
                self.last_request.insert(kind, now);
            }
        }
    }

    pub(super) fn queue(&mut self, s: Snapshot) {
        self.queued.push(s);
    }

    /// A Create/Open/Close for this address: it may now belong to someone else.
    pub(super) fn changed(&mut self, kind: HandleKind, address: u64, ts: i64) {
        self.changes.insert((kind, address), ts);
        self.unnamable.remove(&(kind, address));
    }

    pub(super) fn last_change(&self, kind: HandleKind, address: u64) -> Option<i64> {
        self.changes.get(&(kind, address))
    }

    pub(super) fn is_unnamable(&self, kind: HandleKind, address: u64) -> bool {
        self.unnamable.contains_key(&(kind, address))
    }

    pub(super) fn owner_exited(&mut self, pid: u32) {
        self.unnamable.retain(|_, (owner, _)| *owner != pid);
    }

    pub fn negative_cache_len(&self) -> usize {
        self.unnamable.len()
    }
}

impl<L: Lookups> Pipeline<L> {
    /// Applies every queued snapshot taken at or before stream time `stream`.
    pub(super) fn apply_snapshots(&mut self, stream: i64) {
        if self.seeding.queued.is_empty() {
            return;
        }
        let (mut due, later): (Vec<Snapshot>, Vec<Snapshot>) =
            std::mem::take(&mut self.seeding.queued).into_iter().partition(|s| s.taken <= stream);
        self.seeding.queued = later;
        due.sort_by_key(|s| s.taken);
        for s in due {
            self.apply_snapshot(s);
        }
    }

    fn apply_snapshot(&mut self, s: Snapshot) {
        let kind = s.kind;
        let taken = s.taken;
        let in_table: HashSet<u64> = s.named.iter().map(|n| n.address).chain(s.unnamable.iter().map(|u| u.0)).collect();
        // Covered but not in the table: closed before the read. Nothing can name
        // the address until it is used again (the negative cache, owner 0).
        let absent: HashSet<u64> = if s.asked.is_empty() {
            match kind {
                HandleKind::Key => self.reg.waiting.keys().copied().filter(|r| !in_table.contains(r)).collect(),
                HandleKind::File => self
                    .files
                    .map
                    .iter()
                    .filter(|(a, e)| e.nt.is_none() && !in_table.contains(a))
                    .map(|(a, _)| *a)
                    .collect(),
            }
        } else {
            s.asked.iter().copied().filter(|a| !in_table.contains(a)).collect()
        };
        let unchanged = |p: &Self, address: u64| p.seeding.last_change(kind, address).is_none_or(|c| c <= taken);
        for (address, owner) in &s.unnamable {
            if unchanged(self, *address) {
                self.seeding.mark_unnamable(kind, *address, *owner);
            }
        }
        for address in &absent {
            if unchanged(self, *address) {
                self.seeding.mark_unnamable(kind, *address, 0);
            }
        }
        match kind {
            HandleKind::Key => {
                for n in &s.named {
                    self.keys.seed(n.address, n.name.clone(), taken);
                }
                let roots: Vec<u64> =
                    self.reg.waiting.keys().copied().filter(|r| in_table.contains(r) || absent.contains(r)).collect();
                for root in roots {
                    let named = in_table.contains(&root) && !self.seeding.is_unnamable(kind, root);
                    self.answer_key_waiters(root, taken, named);
                }
            }
            HandleKind::File => {
                for n in &s.named {
                    self.seed_file(n.address, n.owner_pid, &n.name, taken);
                }
                for address in s.unnamable.iter().map(|u| u.0).chain(absent.iter().copied()) {
                    self.unnamable_file(address);
                }
            }
        }
        if s.asked.is_empty() {
            self.seeding.startup.remove(&kind);
        }
        self.seeding.latest.insert(kind, in_table);
    }

    fn seed_file(&mut self, address: u64, owner: u32, nt: &str, taken: i64) {
        let opener = self.procs.lookup_at(owner, taken).map(|p| p.to_ref(&self.id));
        match self.files.map.get_mut(&address) {
            // An ETW event after the snapshot set this entry: keep it.
            Some(e) if e.since > taken => return,
            Some(e) if e.nt.is_some() => return,
            Some(e) => {
                e.nt = Some(nt.to_string());
                e.opener = opener.clone();
            }
            None => {
                self.files.map.insert(address, super::file::seeded_entry(nt, opener.clone(), taken));
                return;
            }
        }
        let waiting = self.files.map.get_mut(&address).map(|e| std::mem::take(&mut e.waiting)).unwrap_or_default();
        let changed = self.seeding.last_change(crate::services::HandleKind::File, address);
        let path = self.dos(nt);
        let file = File { name: file_name(&path).to_string(), path, hashes: None, signature: None };
        for (id, ts, fill) in waiting {
            if !self.completion.is_waiting(id, Reason::Seeder) {
                continue;
            }
            // Any Create or Close since the earlier of the event and the read: the
            // address may have changed hands (conservative for a late snapshot).
            let reused = changed.is_some_and(|c| c > ts.min(taken));
            if reused || (fill == Fill::Opened && opener.is_none()) {
                continue; // left to its deadline
            }
            let (f, a) = (file.clone(), opener.clone());
            self.completion.update(id, |e| fill_file_event(e, fill, f, a));
            self.completion.resolve(id, Reason::Seeder);
            self.requests.push(Request::InvalidateHash { nt_path: nt.to_string() });
        }
    }

    /// The seeder saw the handle but could not name it: its waiting events do
    /// not wait any longer (§7.4).
    fn unnamable_file(&mut self, address: u64) {
        let waiting = self.files.map.get_mut(&address).map(|e| std::mem::take(&mut e.waiting)).unwrap_or_default();
        for (id, _, _) in waiting {
            if !self.completion.is_waiting(id, Reason::Seeder) {
                continue;
            }
            self.completion.cancel(id);
            self.counters.unknown_file_object += 1;
        }
    }
}
```

`src/pipeline/expand.rs`:
```rust
//! 8.3 short names in emitted paths (plan 1b-3a decision D4, refining sensor
//! spec §7.2): every emitted file path with a short component waits for its
//! expansion, so paths have one spelling and short names cannot dodge
//! path-based rules. A failed or late expansion leaves the path as logged.
//!
//! One event may need several paths expanded (a Rename's source and result);
//! each is a slot of the same pending event, and the event waits until every
//! slot has answered or the deadline passes.

use std::collections::HashMap;

use atlas_schema::classes::file::FileAction;
use atlas_schema::classes::module::ModuleAction;
use atlas_schema::classes::process::ProcessActivity;
use atlas_schema::{Event, EventKind, File};

use super::{Pipeline, alive};
use crate::completion::{PendingId, Reason, Wait};
use crate::paths::has_short_name;
use crate::process::file_name;
use crate::services::{Lookups, Request};

/// Which path of the event an expansion is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// `file` of a File System Activity.
    File,
    /// `file_result` of a Rename.
    RenameResult,
    /// The image of a Launch (`process.file`) or a Module Load (`module.file`).
    Image,
}

/// One path waiting for its long form.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Expansion {
    pub(crate) slot: Slot,
    /// The FileObject whose map entry keeps the expanded path.
    pub(crate) fo: Option<u64>,
    /// The process whose cached image path gets the expanded path.
    pub(crate) image_key: Option<u64>,
    /// A watchlist Open: whether the logged path already matched (§7.2).
    pub(crate) open_logged: Option<bool>,
}

impl Expansion {
    pub(crate) fn file(fo: Option<u64>) -> Self {
        Expansion { slot: Slot::File, fo, image_key: None, open_logged: None }
    }
}

#[derive(Default)]
pub struct Expands {
    by_slot: HashMap<(PendingId, u8), Expansion>,
    /// Per event: slots still unanswered, and how many it asked for.
    outstanding: HashMap<PendingId, (u8, u8)>,
}

impl Expands {
    /// The event left the completion stage: unanswered slots are forgotten.
    #[cfg(test)]
    pub(super) fn sizes(&self) -> [(&'static str, usize); 2] {
        [("expand slots", self.by_slot.len()), ("expand events", self.outstanding.len())]
    }

    pub(super) fn forget(&mut self, id: PendingId) {
        if let Some((_, total)) = self.outstanding.remove(&id) {
            for slot in 0..total {
                self.by_slot.remove(&(id, slot));
            }
        }
    }
}

impl<L: Lookups> Pipeline<L> {
    /// The `File` for an NT path, and the path to expand if it has 8.3
    /// components. A handle whose path was expanded before reuses it.
    pub(crate) fn emit_file(&mut self, nt: &str, fo: Option<u64>) -> (File, Option<String>) {
        if let Some(long) = fo.and_then(|f| self.files.map.get(&f)).and_then(|e| e.expanded.clone()) {
            return (self.file_obj(&long), None);
        }
        let file = self.file_obj(nt);
        (file, has_short_name(nt).then(|| nt.to_string()))
    }

    /// Pushes an event that may wait: for its `waits`, and for each path in
    /// `expansions`. Returns its id, or `None` if it was ready at once.
    pub(crate) fn push_with(
        &mut self,
        ev: Event,
        mut waits: Vec<Wait>,
        expansions: Vec<(String, Expansion)>,
    ) -> Option<PendingId> {
        if waits.is_empty() && expansions.is_empty() {
            self.completion.push(ev);
            return None;
        }
        if !expansions.is_empty() {
            // An Open whose logged path did not match is dropped if no expansion comes.
            let drop = expansions.iter().any(|(_, x)| x.open_logged == Some(false));
            waits.push(Wait { drop_at_deadline: drop, ..self.wait(Reason::Expand, self.cfg.expand_deadline) });
        }
        let id = self.completion.push_pending(ev, waits);
        if !expansions.is_empty() {
            let n = expansions.len() as u8;
            self.expands.outstanding.insert(id, (n, n));
        }
        for (slot, (nt_path, x)) in expansions.into_iter().enumerate() {
            let slot = slot as u8;
            self.expands.by_slot.insert((id, slot), x);
            self.requests.push(Request::Expand { id, slot, nt_path });
        }
        Some(id)
    }

    pub(super) fn on_expanded(&mut self, id: PendingId, slot: u8, long: Option<String>) {
        let Some(x) = self.expands.by_slot.remove(&(id, slot)) else { return };
        let left = match self.expands.outstanding.get_mut(&id) {
            Some((n, _)) => {
                *n = n.saturating_sub(1);
                *n
            }
            None => 0,
        };
        if left == 0 {
            self.expands.outstanding.remove(&id);
        }
        if !alive(&self.completion, id, Reason::Expand) {
            return;
        }
        if let Some(logged) = x.open_logged {
            let long_matches = long.as_deref().is_some_and(|l| self.watch.matches(l));
            if !logged && !long_matches {
                self.completion.cancel(id);
                return;
            }
        }
        if let Some(l) = long {
            let file = self.file_obj(&l);
            self.completion.update(id, |e| set_path(e, x.slot, file));
            if let Some(fo) = x.fo
                && let Some(e) = self.files.map.get_mut(&fo)
            {
                e.expanded = Some(l.clone());
            }
            if let Some(key) = x.image_key {
                let path = self.dos(&l);
                if let Some(p) = self.procs.get_mut(key) {
                    p.path = path; // later actor references carry the long name
                }
            }
        }
        if left == 0 {
            self.completion.resolve(id, Reason::Expand);
        }
    }
}

/// Writes the expanded path (and name) into the event, keeping any hashes.
fn set_path(e: &mut Event, slot: Slot, file: File) {
    let target = match (&mut e.kind, slot) {
        (EventKind::File(f), Slot::File) => &mut f.file,
        (EventKind::File(f), Slot::RenameResult) => match &mut f.action {
            FileAction::Rename { file_result } => file_result,
            _ => return,
        },
        (EventKind::Process(ProcessActivity::Launch { process, .. }), Slot::Image) => &mut process.file,
        (EventKind::Module(m), Slot::Image) => match &mut m.action {
            ModuleAction::Load { file, .. } => file,
        },
        _ => return,
    };
    target.name = file_name(&file.path).to_string();
    target.path = file.path;
}
```

- [ ] **Step 8: Fakes and scenarios**

`src/fakes.rs`:
```rust
//! Stand-ins for Windows, for tests and the full-pipeline replay: lookups from
//! tables, and event ids that depend only on the event time and order.

use std::collections::HashMap;

use atlas_schema::EventId;

use crate::pipeline::IdGen;
use crate::services::{LiveProcess, Lookups};

/// [`Lookups`] answered from tables.
#[derive(Debug, Clone, Default)]
pub struct FakeLookups {
    /// (NT device prefix, drive) pairs, such as (`\Device\HarddiskVolume3`, `C:`).
    pub devices: Vec<(String, String)>,
    pub live: HashMap<u32, LiveProcess>,
    pub accounts: HashMap<String, String>,
}

impl FakeLookups {
    pub fn with_device(mut self, nt: &str, drive: &str) -> Self {
        self.devices.push((nt.to_string(), drive.to_string()));
        self
    }
}

impl Lookups for FakeLookups {
    /// Prefix match on a component boundary, ignoring case (§5.5):
    /// `HarddiskVolume1` never matches `HarddiskVolume10\…`.
    fn dos_path(&mut self, nt: &str) -> Option<String> {
        self.devices.iter().find_map(|(dev, drive)| {
            let head = nt.get(..dev.len())?;
            let rest = &nt[dev.len()..];
            (head.eq_ignore_ascii_case(dev) && (rest.is_empty() || rest.starts_with('\\')))
                .then(|| format!("{drive}{rest}"))
        })
    }

    fn live_process(&mut self, pid: u32) -> Option<LiveProcess> {
        self.live.get(&pid).cloned()
    }

    fn account_name(&mut self, sid: &str) -> Option<String> {
        self.accounts.get(sid).cloned()
    }
}

/// UUIDv7 ids from the event's time (milliseconds) and a counter, so a replay
/// gives the same ids every run.
pub fn sequential_ids() -> IdGen {
    let mut n: u64 = 0;
    Box::new(move |unix_ns: i64| {
        n += 1;
        let millis = u64::try_from(unix_ns.max(0) / 1_000_000).unwrap_or(0);
        let mut rand = [0u8; 10];
        rand[2..].copy_from_slice(&n.to_be_bytes());
        let uuid = uuid::Builder::from_unix_timestamp_millis(millis, &rand).into_uuid();
        EventId::from_uuid(uuid).expect("a v7 uuid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_prefixes_match_on_component_boundaries() {
        let mut l = FakeLookups::default().with_device(r"\Device\HarddiskVolume1", "C:");
        assert_eq!(l.dos_path(r"\Device\HarddiskVolume1\x"), Some(r"C:\x".into()));
        assert_eq!(l.dos_path(r"\device\harddiskvolume1"), Some("C:".into()));
        assert_eq!(l.dos_path(r"\Device\HarddiskVolume10\x"), None);
    }

    #[test]
    fn ids_are_v7_and_repeatable() {
        let (mut a, mut b) = (sequential_ids(), sequential_ids());
        let x = a(1_790_000_000_123_456_789);
        assert_eq!(x, b(1_790_000_000_123_456_789));
        assert_ne!(x, a(1_790_000_000_123_456_789));
    }
}
```

`src/pipeline/tests.rs`:
```rust
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
```

- [ ] **Step 9: Check and commit**

```powershell
cargo test -p atlas-agent --lib
cargo clippy -p atlas-agent --all-targets -- -D warnings
```
Expected: 93 tests pass and clippy is clean.
```powershell
git add crates/atlas-agent
git commit -m "feat(agent): the pipeline: launches, files, registry, network, DNS, seeding, 8.3 expansion"
```

### Task 5: Full-pipeline replay

**Files:**
- Create: `crates/atlas-agent/tests/replay.rs`, `tests/snapshots/scenario.jsonl` (generated)

The CI recording (1b-2, `crates/atlas-etw/tests/fixtures/scenario.jsonl`) goes through the parsers, the intake and the pipeline. The fake workers answer:
- a fixed hash for every file;
- the scenario's DWORD for the value it sets;
- `runneradmin` for the runner's `RUNNER~1`;
- nothing from the seeder.

The assertions check each live-scenario step:
- the `cmd.exe` Launch (command line, user, hash, actor, parent), its Terminate with exit code 7, and its Module Loads;
- `a.txt` created, updated twice, its attributes set, then renamed to `b.txt`, and `b.txt` deleted;
- delete-on-close `d.txt`;
- `c.txt`: no event for the failed create or the refused delete, then exactly one Delete;
- no `RUNNER~1` left in any path;
- the test key's Create under `HKU\…\Software`, the value `v` read after the event, the embedded-NUL `a\0b` and `k\0x`, and the deletes;
- TCP connect, accept and two closes per IP version, and one UDP flow each way per IP version;
- three DNS answers, the `.invalid` one with rcode 3.

- [ ] **Step 1: The replay**

`tests/replay.rs`:
```rust
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
    assert!(key.starts_with(r"HKU\S-1-5-21-") && key.ends_with(r"\Software\AtlasEtwLive-3268"), "{key}");
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
```

- [ ] **Step 2: Generate and review the snapshot**

```powershell
$env:ATLAS_UPDATE_SNAPSHOT = '1'; cargo test -p atlas-agent --test replay; Remove-Item Env:ATLAS_UPDATE_SNAPSHOT
cargo test -p atlas-agent --test replay
```
Expected: 3 tests pass and the snapshot has 74 lines. Review it against the assertions above before committing it. It holds only the runner's names (`runneradmin`, the runner's SID, its build paths) and the test's DNS names.
```powershell
git add crates/atlas-agent/tests
git commit -m "test(agent): full-pipeline replay of the CI recording, with a golden snapshot"
```

### Task 6: Documentation

- [ ] **Step 1: Spec, schema reference, decision log**

In `docs/specs/2026-10-01-etw-sensor-design.md`:
- Status line: plan 1b-3a done; 1b-3b next.
- §3.2: clarifications 2, 20 and 25.
- §5.2: clarifications 13 and 23. §5.3: clarifications 10 and 22. §5.4: clarification 9.
- §6.2, §7.1, §7.3, §7.4: clarification 6; §7.1: clarification 7; §7.3: clarification 8.
- §7.2: clarifications 4 and 15. §5.5: the late-failure window of clarification 15.
- §7.4: clarifications 16, 17 and 18. §7.5: clarifications 24 and 26.
- §12.2: clarification 5.
- §10.3: clarification 12.
- §11.2: clarifications 14 and 19. §11.4: clarification 21.
- §15.3: a "Plan 1b-3a (2026-10-05)" addendum with B1–B5.
- §17: a "Plan 1b-3a clarifications" paragraph listing 1–26.

In `docs/schema-reference.md`, Sensor Health: `loss` gains "callback panics", and `quality` gains "ambiguous registry value names".

In `docs/architecture-overview.md`:
- Roadmap row 1: "plans 1b-1, 1b-2 and 1b-3a done; plan 1b-3b (Windows services) next", with a link to this plan.
- Decision log: the row for this plan's approval (written when it is approved: D1–D4) and a row for the build.

```powershell
git add docs
git commit -m "docs(1): plan 1b-3a clarifications, findings and schema reference"
```

### Task 7: PR and merge

- [ ] **Step 1:** Push `feat/1b-3a-pipeline` and open the PR. The body summarises the crate, D1–D4, B1–B5 and the test counts, and ends with the attribution line.
- [ ] **Step 2:** CI runs `rust-linux`, `rust-windows`, `etw-live`, `proto` (with `buf breaking`), `powershell` and `audit`. If the Linux job fails on something the Windows-side check could not see (the `atlas-schema` tests), fix it with a test.
- [ ] **Step 3:** When every check on the PR's current head is green, merge, then update the decision log's build row if anything changed on the way.

## Review Log

**Independent review, 2026-10-05** (a separate agent; it read the plan, the spec and the code, and confirmed findings by running probes in a copy of the crate). Every finding was fixed in the code embedded above unless noted. Each fix has a pinned test, and reverting the fix makes that test fail (verification note).

**Blocking**
- **R-B1, an O(n) scan per event.** The address history was pruned with a `retain` over 20 s of addresses on every released event: 24–53 µs per event under high cardinality. Fixed: it is a time-ordered `Recent`, expired per tick in O(1) on average. Re-measured at 0.68–0.91 µs (verification note). The same pattern was removed elsewhere: Irp lookups use maps, whole-table sweeps run once a second, and snapshots are no longer cloned.
- **R-B2, watchlist coalescing records were never pruned.** Every watchlisted or 8.3 Create left one forever. Fixed: `Recent`, expiring after the coalescing period, with a cap.
- **R-B3, a reused base address named old children with the new key's path.** Fixed: clarification 16. Tests: `a_reused_base_never_names_old_children` and the key map's own.
- **R-B4, key waiters were kept forever** when the seeder never named their root, and each held the output line for the whole deadline. Fixed:
  - waiters are freed when their event leaves (clarification 25);
  - an address absent from the snapshot that covered it is answered at once (clarification 17).

**Major**
- **R-M1, request bookkeeping was freed only by a reply.** Fixed: clarification 25.
- **R-M2, seeding a Rename's source could never work,** and the Rename held the line until the deadline. Fixed: clarification 7.
- **R-M3, a failed watchlist Open suppressed the retry for 60 s.** Fixed: clarification 15.
- **R-M4, a process found live was cached as started at `i64::MIN`,** so a payload-PID lookup preferred an earlier process with the same PID. Fixed: clarification 22.
- **R-M5, with on-miss seeding off, file events still waited for the seeder.** Fixed: clarification 19, and `seed_on_start` is now a setting.
- **R-M6, a clean stop emitted placeholder Updates** with a made-up actor. Fixed: clarification 21.
- **R-M7, gaps in the Interfaces section:**
  - the one-reply contract;
  - snapshot coverage;
  - seeding when unavailable;
  - who builds `Header`;
  - what the `Lookups` must guarantee.

  All are now in Interfaces for later plans.

**Minor**
- Fixed:
  - `data_read_after` was set when no read was attempted (clarification 24);
  - a late classic half made a second Launch (clarification 23);
  - events between the last released one and the watermark were not counted as late (clarification 20);
  - process eviction removed long-running processes first (clarification 6);
  - stale comment references;
  - an `abs()` that could overflow (now `abs_diff`).
- Listed instead of changed: the two safety caps that clear at once (clarification 6).
- Deferred: lossy key names (clarification 26).
- Found while fixing these: the pending-cap overflow scanned the completion queue on every push past the cap, quadratic under overload. It is now O(1) on average, through a queue of pending ids.

**Sound, per the review:** the ordering and completion stages, with real property tests; intake (`try_send`, atomics, no locks); schema validity; the test counts per task.
